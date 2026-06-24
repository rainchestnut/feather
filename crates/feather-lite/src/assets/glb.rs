//! glTF 2.0 / GLB reader for static preview meshes.
//!
//! The importer supports the mesh subset Feather exports and most preview glTF
//! assets use: bufferViews/accessors, triangle primitives, POSITION/NORMAL
//! attributes, scalar indices, simple materials, and node hierarchy. It
//! deliberately rejects unsupported binary layouts instead of guessing geometry.

use std::path::Path;

use crate::document::{LiteDocument, LiteMaterial, LiteMesh, LiteNode, LitePrimitive};
use crate::importer::ImportError;

const GLB_MAGIC: u32 = 0x4654_6C67;
const GLB_VERSION: u32 = 2;
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;

/// Returns the declared byte length of a GLB payload at the start of `bytes`.
pub fn glb_len(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 12 {
        return None;
    }
    if read_u32(bytes, 0)? != GLB_MAGIC || read_u32(bytes, 4)? != GLB_VERSION {
        return None;
    }
    let length = read_u32(bytes, 8)? as usize;
    (length >= 20 && length <= bytes.len()).then_some(length)
}

/// Returns true when bytes are exactly one GLB payload.
pub fn is_exact_glb(bytes: &[u8]) -> bool {
    glb_len(bytes).is_some_and(|length| length == bytes.len())
}

/// Imports a GLB payload into the visual IR.
pub fn import_glb_document(
    bytes: &[u8],
    source_format: &str,
    mode: &str,
    source_path: Option<&Path>,
) -> Result<LiteDocument, ImportError> {
    let payload_len = glb_len(bytes).ok_or_else(|| {
        ImportError::InvalidData("input is not a structurally valid GLB 2.0 payload".to_string())
    })?;
    let chunks = read_glb_chunks(&bytes[..payload_len])?;
    let json_text = std::str::from_utf8(chunks.json)
        .map_err(|error| ImportError::InvalidData(format!("GLB JSON is not UTF-8: {error}")))?;
    let root = JsonParser::new(json_text).parse()?;
    let root = root
        .as_object()
        .ok_or_else(|| ImportError::InvalidData("GLB JSON root must be an object".to_string()))?;

    import_gltf_root(root, &[chunks.bin], source_format, mode, source_path)
}

/// External binary buffer made available to a `.gltf` JSON payload.
#[derive(Debug, Clone, Copy)]
pub struct GltfExternalBuffer<'a> {
    pub uri: &'a str,
    pub bytes: &'a [u8],
}

/// Returns true when bytes look like a JSON glTF mesh document.
pub fn is_gltf_json(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let Ok(root) = JsonParser::new(text).parse() else {
        return false;
    };
    let Some(root) = root.as_object() else {
        return false;
    };

    object_get(root, "asset").is_some()
        && object_get(root, "meshes").is_some()
        && object_get(root, "buffers").is_some()
}

/// Imports a JSON glTF payload using caller-provided external buffers.
pub fn import_gltf_document(
    json_bytes: &[u8],
    external_buffers: &[GltfExternalBuffer<'_>],
    source_format: &str,
    mode: &str,
    source_path: Option<&Path>,
) -> Result<LiteDocument, ImportError> {
    let json_text = std::str::from_utf8(json_bytes)
        .map_err(|error| ImportError::InvalidData(format!("glTF JSON is not UTF-8: {error}")))?;
    let root = JsonParser::new(json_text).parse()?;
    let root = root
        .as_object()
        .ok_or_else(|| ImportError::InvalidData("glTF JSON root must be an object".to_string()))?;
    let buffers = parse_gltf_buffers(root, external_buffers)?;
    let buffer_refs = buffers.iter().map(Vec::as_slice).collect::<Vec<_>>();

    import_gltf_root(root, &buffer_refs, source_format, mode, source_path)
}

fn import_gltf_root(
    root: &[(String, JsonValue)],
    buffers: &[&[u8]],
    source_format: &str,
    mode: &str,
    source_path: Option<&Path>,
) -> Result<LiteDocument, ImportError> {
    let buffer_views = parse_buffer_views(root)?;
    let accessors = parse_accessors(root)?;
    let materials = parse_materials(root);
    let meshes = parse_meshes(root, buffers, &buffer_views, &accessors)?;
    if meshes.is_empty() {
        return Err(ImportError::InvalidData(
            "glTF contains no importable triangle meshes".to_string(),
        ));
    }

    let mut document = LiteDocument::new(source_format, mode);
    if let Some(path) = source_path {
        document.metadata.source_path = Some(path.display().to_string());
    }
    document.materials = materials;
    document.meshes = meshes;
    document.nodes = parse_nodes(root);
    document.add_default_nodes_for_unreferenced_meshes();
    document.refresh_metadata();
    Ok(document)
}

fn parse_gltf_buffers(
    root: &[(String, JsonValue)],
    external_buffers: &[GltfExternalBuffer<'_>],
) -> Result<Vec<Vec<u8>>, ImportError> {
    let buffers = object_get(root, "buffers")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| ImportError::InvalidData("glTF JSON has no buffers".to_string()))?;
    let mut parsed = Vec::with_capacity(buffers.len());

    for (index, buffer) in buffers.iter().enumerate() {
        let buffer = buffer.as_object().ok_or_else(|| {
            ImportError::InvalidData(format!("glTF buffer {index} must be an object"))
        })?;
        let uri = object_get(buffer, "uri")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| {
                ImportError::InvalidData(format!("glTF buffer {index} missing external uri"))
            })?;
        let byte_length = json_usize(object_get(buffer, "byteLength")).ok_or_else(|| {
            ImportError::InvalidData(format!("glTF buffer {index} missing byteLength"))
        })?;
        let payload = if uri.starts_with("data:") {
            decode_data_uri_buffer(uri, index)?
        } else {
            let external = external_buffers
                .iter()
                .find(|candidate| candidate.uri == uri)
                .ok_or_else(|| {
                    ImportError::InvalidData(format!("glTF buffer `{uri}` was not provided"))
                })?;
            external.bytes.to_vec()
        };
        let payload = payload.get(..byte_length).ok_or_else(|| {
            ImportError::InvalidData(format!(
                "glTF buffer `{uri}` is shorter than declared byteLength"
            ))
        })?;
        parsed.push(payload.to_vec());
    }

    Ok(parsed)
}

fn decode_data_uri_buffer(uri: &str, buffer_index: usize) -> Result<Vec<u8>, ImportError> {
    let (metadata, payload) = uri.split_once(',').ok_or_else(|| {
        ImportError::InvalidData(format!("glTF buffer {buffer_index} has malformed data URI"))
    })?;
    if !metadata
        .split(';')
        .any(|parameter| parameter.eq_ignore_ascii_case("base64"))
    {
        return Err(ImportError::InvalidData(format!(
            "glTF buffer {buffer_index} uses non-base64 data URI"
        )));
    }
    decode_base64(payload).map_err(|message| {
        ImportError::InvalidData(format!(
            "glTF buffer {buffer_index} has invalid base64 data URI: {message}"
        ))
    })
}

fn decode_base64(payload: &str) -> Result<Vec<u8>, &'static str> {
    let compact = payload
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    if compact.is_empty() || compact.len() % 4 != 0 {
        return Err("length is not a multiple of 4");
    }

    let mut decoded = Vec::with_capacity(compact.len() / 4 * 3);
    for (chunk_index, chunk) in compact.chunks_exact(4).enumerate() {
        let is_last = chunk_index + 1 == compact.len() / 4;
        let first = base64_value(chunk[0]).ok_or("invalid character")?;
        let second = base64_value(chunk[1]).ok_or("invalid character")?;
        let third = if chunk[2] == b'=' {
            None
        } else {
            Some(base64_value(chunk[2]).ok_or("invalid character")?)
        };
        let fourth = if chunk[3] == b'=' {
            None
        } else {
            Some(base64_value(chunk[3]).ok_or("invalid character")?)
        };

        if !is_last && (third.is_none() || fourth.is_none()) {
            return Err("padding before final quantum");
        }
        if third.is_none() && fourth.is_some() {
            return Err("invalid padding");
        }

        decoded.push((first << 2) | (second >> 4));
        if let Some(third) = third {
            decoded.push(((second & 0x0F) << 4) | (third >> 2));
            if let Some(fourth) = fourth {
                decoded.push(((third & 0x03) << 6) | fourth);
            }
        }
    }

    Ok(decoded)
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

struct GlbChunks<'a> {
    json: &'a [u8],
    bin: &'a [u8],
}

fn read_glb_chunks(bytes: &[u8]) -> Result<GlbChunks<'_>, ImportError> {
    let mut cursor = 12;
    let mut json = None;
    let mut bin = None;

    while cursor + 8 <= bytes.len() {
        let chunk_len = read_u32(bytes, cursor)
            .ok_or_else(|| ImportError::InvalidData("GLB chunk length is truncated".to_string()))?
            as usize;
        let chunk_type = read_u32(bytes, cursor + 4)
            .ok_or_else(|| ImportError::InvalidData("GLB chunk type is truncated".to_string()))?;
        let data_start = cursor + 8;
        let data_end = data_start.checked_add(chunk_len).ok_or_else(|| {
            ImportError::InvalidData("GLB chunk length overflows usize".to_string())
        })?;
        if data_end > bytes.len() {
            return Err(ImportError::InvalidData(
                "GLB chunk extends past payload length".to_string(),
            ));
        }

        match chunk_type {
            CHUNK_JSON => json = Some(&bytes[data_start..data_end]),
            CHUNK_BIN => bin = Some(&bytes[data_start..data_end]),
            _ => {}
        }
        cursor = data_end;
    }

    Ok(GlbChunks {
        json: json.ok_or_else(|| ImportError::InvalidData("GLB has no JSON chunk".to_string()))?,
        bin: bin.unwrap_or(&[]),
    })
}

#[derive(Debug, Clone)]
struct BufferView {
    buffer: usize,
    byte_offset: usize,
    byte_length: usize,
    byte_stride: Option<usize>,
}

#[derive(Debug, Clone)]
struct Accessor {
    buffer_view: usize,
    byte_offset: usize,
    component_type: u32,
    count: usize,
    type_name: String,
}

fn parse_buffer_views(root: &[(String, JsonValue)]) -> Result<Vec<BufferView>, ImportError> {
    let Some(views) = object_get(root, "bufferViews").and_then(JsonValue::as_array) else {
        return Ok(Vec::new());
    };

    let mut parsed = Vec::with_capacity(views.len());
    for (index, view) in views.iter().enumerate() {
        let view = view.as_object().ok_or_else(|| {
            ImportError::InvalidData(format!("bufferView {index} must be an object"))
        })?;
        parsed.push(BufferView {
            buffer: json_usize(object_get(view, "buffer")).unwrap_or(0),
            byte_offset: json_usize(object_get(view, "byteOffset")).unwrap_or(0),
            byte_length: json_usize(object_get(view, "byteLength")).ok_or_else(|| {
                ImportError::InvalidData(format!("bufferView {index} missing byteLength"))
            })?,
            byte_stride: json_usize(object_get(view, "byteStride")),
        });
    }
    Ok(parsed)
}

fn parse_accessors(root: &[(String, JsonValue)]) -> Result<Vec<Accessor>, ImportError> {
    let accessors = object_get(root, "accessors")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| ImportError::InvalidData("GLB JSON has no accessors".to_string()))?;

    let mut parsed = Vec::with_capacity(accessors.len());
    for (index, accessor) in accessors.iter().enumerate() {
        let accessor = accessor.as_object().ok_or_else(|| {
            ImportError::InvalidData(format!("accessor {index} must be an object"))
        })?;
        parsed.push(Accessor {
            buffer_view: json_usize(object_get(accessor, "bufferView")).ok_or_else(|| {
                ImportError::InvalidData(format!("accessor {index} missing bufferView"))
            })?,
            byte_offset: json_usize(object_get(accessor, "byteOffset")).unwrap_or(0),
            component_type: json_u32(object_get(accessor, "componentType")).ok_or_else(|| {
                ImportError::InvalidData(format!("accessor {index} missing componentType"))
            })?,
            count: json_usize(object_get(accessor, "count")).ok_or_else(|| {
                ImportError::InvalidData(format!("accessor {index} missing count"))
            })?,
            type_name: object_get(accessor, "type")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| ImportError::InvalidData(format!("accessor {index} missing type")))?
                .to_string(),
        });
    }
    Ok(parsed)
}

fn parse_materials(root: &[(String, JsonValue)]) -> Vec<LiteMaterial> {
    let Some(materials) = object_get(root, "materials").and_then(JsonValue::as_array) else {
        return Vec::new();
    };

    materials
        .iter()
        .enumerate()
        .map(|(index, material)| {
            let object = material.as_object();
            let name = object
                .and_then(|object| object_get(object, "name"))
                .and_then(JsonValue::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("Material_{index}"));
            let color = object
                .and_then(|object| object_get(object, "pbrMetallicRoughness"))
                .and_then(JsonValue::as_object)
                .and_then(|pbr| object_get(pbr, "baseColorFactor"))
                .and_then(json_f32_array4)
                .unwrap_or([0.8, 0.8, 0.82, 1.0]);
            LiteMaterial::new(name, color)
        })
        .collect()
}

fn parse_meshes(
    root: &[(String, JsonValue)],
    buffers: &[&[u8]],
    buffer_views: &[BufferView],
    accessors: &[Accessor],
) -> Result<Vec<LiteMesh>, ImportError> {
    let meshes = object_get(root, "meshes")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| ImportError::InvalidData("GLB JSON has no meshes".to_string()))?;
    let mut parsed = Vec::new();

    for (mesh_index, mesh_value) in meshes.iter().enumerate() {
        let mesh_object = mesh_value.as_object().ok_or_else(|| {
            ImportError::InvalidData(format!("mesh {mesh_index} must be an object"))
        })?;
        let mut mesh = LiteMesh::new(
            object_get(mesh_object, "name")
                .and_then(JsonValue::as_str)
                .unwrap_or("GLB_Mesh"),
        );
        let primitives = object_get(mesh_object, "primitives")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| {
                ImportError::InvalidData(format!("mesh {mesh_index} has no primitives"))
            })?;

        for (primitive_index, primitive_value) in primitives.iter().enumerate() {
            let primitive_object = primitive_value.as_object().ok_or_else(|| {
                ImportError::InvalidData(format!(
                    "mesh {mesh_index} primitive {primitive_index} must be an object"
                ))
            })?;
            let mode = json_u32(object_get(primitive_object, "mode")).unwrap_or(4);
            if mode != 4 {
                continue;
            }
            let attributes = object_get(primitive_object, "attributes")
                .and_then(JsonValue::as_object)
                .ok_or_else(|| {
                    ImportError::InvalidData(format!(
                        "mesh {mesh_index} primitive {primitive_index} has no attributes"
                    ))
                })?;
            let position_accessor =
                json_usize(object_get(attributes, "POSITION")).ok_or_else(|| {
                    ImportError::InvalidData(format!(
                        "mesh {mesh_index} primitive {primitive_index} missing POSITION"
                    ))
                })?;
            let positions =
                read_vec3_accessor(position_accessor, buffers, buffer_views, accessors)?;
            let normals =
                if let Some(normal_accessor) = json_usize(object_get(attributes, "NORMAL")) {
                    read_vec3_accessor(normal_accessor, buffers, buffer_views, accessors)?
                } else {
                    Vec::new()
                };
            let indices =
                if let Some(index_accessor) = json_usize(object_get(primitive_object, "indices")) {
                    read_index_accessor(index_accessor, buffers, buffer_views, accessors)?
                } else {
                    (0..positions.len() as u32).collect()
                };
            let material = json_usize(object_get(primitive_object, "material"));

            mesh.primitives.push(LitePrimitive {
                positions,
                normals,
                indices,
                material,
            });
        }

        mesh.recompute_bbox();
        if !mesh.primitives.is_empty() {
            parsed.push(mesh);
        }
    }

    Ok(parsed)
}

fn parse_nodes(root: &[(String, JsonValue)]) -> Vec<LiteNode> {
    let Some(nodes) = object_get(root, "nodes").and_then(JsonValue::as_array) else {
        return Vec::new();
    };

    nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            let object = node.as_object()?;
            let mut lite_node = LiteNode::new(
                object_get(object, "name")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("Node_{index}")),
                json_usize(object_get(object, "mesh")),
            );
            if let Some(children) = object_get(object, "children").and_then(JsonValue::as_array) {
                lite_node.children = children
                    .iter()
                    .filter_map(|value| json_usize(Some(value)))
                    .collect();
            }
            lite_node.transform =
                if let Some(matrix) = object_get(object, "matrix").and_then(json_f32_array16) {
                    gltf_matrix_to_transform(matrix)
                } else {
                    gltf_trs_to_transform(object)
                };
            Some(lite_node)
        })
        .collect()
}

fn gltf_matrix_to_transform(matrix: [f32; 16]) -> crate::document::Transform {
    let mut transform = crate::document::identity_transform();
    for (matrix_index, value) in matrix.iter().enumerate() {
        transform[matrix_index / 4][matrix_index % 4] = *value;
    }
    transform
}

fn gltf_trs_to_transform(object: &[(String, JsonValue)]) -> crate::document::Transform {
    let translation = object_get(object, "translation")
        .and_then(json_f32_array3)
        .unwrap_or([0.0, 0.0, 0.0]);
    let rotation = object_get(object, "rotation")
        .and_then(json_f32_array4)
        .unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let scale = object_get(object, "scale")
        .and_then(json_f32_array3)
        .unwrap_or([1.0, 1.0, 1.0]);

    let [x, y, z, w] = normalize_quaternion(rotation);
    let x2 = x + x;
    let y2 = y + y;
    let z2 = z + z;
    let xx = x * x2;
    let xy = x * y2;
    let xz = x * z2;
    let yy = y * y2;
    let yz = y * z2;
    let zz = z * z2;
    let wx = w * x2;
    let wy = w * y2;
    let wz = w * z2;

    [
        [
            (1.0 - (yy + zz)) * scale[0],
            (xy + wz) * scale[0],
            (xz - wy) * scale[0],
            0.0,
        ],
        [
            (xy - wz) * scale[1],
            (1.0 - (xx + zz)) * scale[1],
            (yz + wx) * scale[1],
            0.0,
        ],
        [
            (xz + wy) * scale[2],
            (yz - wx) * scale[2],
            (1.0 - (xx + yy)) * scale[2],
            0.0,
        ],
        [translation[0], translation[1], translation[2], 1.0],
    ]
}

fn normalize_quaternion(rotation: [f32; 4]) -> [f32; 4] {
    let length = (rotation[0] * rotation[0]
        + rotation[1] * rotation[1]
        + rotation[2] * rotation[2]
        + rotation[3] * rotation[3])
        .sqrt();
    if length <= f32::EPSILON {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        [
            rotation[0] / length,
            rotation[1] / length,
            rotation[2] / length,
            rotation[3] / length,
        ]
    }
}

fn read_vec3_accessor(
    accessor_index: usize,
    buffers: &[&[u8]],
    buffer_views: &[BufferView],
    accessors: &[Accessor],
) -> Result<Vec<[f32; 3]>, ImportError> {
    let accessor = accessors.get(accessor_index).ok_or_else(|| {
        ImportError::InvalidData(format!("accessor {accessor_index} is out of range"))
    })?;
    if accessor.component_type != 5126 || accessor.type_name != "VEC3" {
        return Err(ImportError::InvalidData(format!(
            "accessor {accessor_index} must be FLOAT VEC3"
        )));
    }
    let view = buffer_views.get(accessor.buffer_view).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "accessor {accessor_index} references missing bufferView"
        ))
    })?;
    let bin = buffer_for_view(view, buffers, accessor_index)?;
    let stride = view.byte_stride.unwrap_or(12);
    if stride < 12 {
        return Err(ImportError::InvalidData(format!(
            "accessor {accessor_index} has invalid VEC3 stride {stride}"
        )));
    }
    let base = view.byte_offset + accessor.byte_offset;
    validate_view_range(
        view,
        accessor.byte_offset,
        accessor.count,
        stride,
        12,
        accessor_index,
    )?;
    let mut values = Vec::with_capacity(accessor.count);
    for item_index in 0..accessor.count {
        let offset = base + item_index * stride;
        values.push([
            read_f32(bin, offset)?,
            read_f32(bin, offset + 4)?,
            read_f32(bin, offset + 8)?,
        ]);
    }
    Ok(values)
}

fn read_index_accessor(
    accessor_index: usize,
    buffers: &[&[u8]],
    buffer_views: &[BufferView],
    accessors: &[Accessor],
) -> Result<Vec<u32>, ImportError> {
    let accessor = accessors.get(accessor_index).ok_or_else(|| {
        ImportError::InvalidData(format!("accessor {accessor_index} is out of range"))
    })?;
    if accessor.type_name != "SCALAR" {
        return Err(ImportError::InvalidData(format!(
            "index accessor {accessor_index} must be SCALAR"
        )));
    }
    let view = buffer_views.get(accessor.buffer_view).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "accessor {accessor_index} references missing bufferView"
        ))
    })?;
    let bin = buffer_for_view(view, buffers, accessor_index)?;
    let component_size = match accessor.component_type {
        5121 => 1,
        5123 => 2,
        5125 => 4,
        other => {
            return Err(ImportError::InvalidData(format!(
                "unsupported index component type {other}"
            )));
        }
    };
    let stride = view.byte_stride.unwrap_or(component_size);
    let base = view.byte_offset + accessor.byte_offset;
    validate_view_range(
        view,
        accessor.byte_offset,
        accessor.count,
        stride,
        component_size,
        accessor_index,
    )?;
    let mut values = Vec::with_capacity(accessor.count);
    for item_index in 0..accessor.count {
        let offset = base + item_index * stride;
        values.push(match accessor.component_type {
            5121 => *bin
                .get(offset)
                .ok_or_else(|| ImportError::InvalidData("GLB u8 index is truncated".to_string()))?
                as u32,
            5123 => read_u16(bin, offset)? as u32,
            5125 => read_u32(bin, offset).ok_or_else(|| {
                ImportError::InvalidData("GLB u32 index is truncated".to_string())
            })?,
            _ => unreachable!(),
        });
    }
    Ok(values)
}

fn buffer_for_view<'a>(
    view: &BufferView,
    buffers: &'a [&'a [u8]],
    accessor_index: usize,
) -> Result<&'a [u8], ImportError> {
    let buffer = buffers.get(view.buffer).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "accessor {accessor_index} references missing buffer {}",
            view.buffer
        ))
    })?;
    let view_end = view
        .byte_offset
        .checked_add(view.byte_length)
        .ok_or_else(|| {
            ImportError::InvalidData(format!(
                "accessor {accessor_index} bufferView range overflows"
            ))
        })?;
    if view_end > buffer.len() {
        return Err(ImportError::InvalidData(format!(
            "accessor {accessor_index} bufferView reads past its buffer"
        )));
    }
    Ok(buffer)
}

fn validate_view_range(
    view: &BufferView,
    accessor_offset: usize,
    count: usize,
    stride: usize,
    item_size: usize,
    accessor_index: usize,
) -> Result<(), ImportError> {
    if count == 0 {
        return Ok(());
    }
    let last_start = accessor_offset
        .checked_add((count - 1).checked_mul(stride).ok_or_else(|| {
            ImportError::InvalidData(format!("accessor {accessor_index} byte range overflows"))
        })?)
        .ok_or_else(|| {
            ImportError::InvalidData(format!("accessor {accessor_index} byte range overflows"))
        })?;
    let required = last_start.checked_add(item_size).ok_or_else(|| {
        ImportError::InvalidData(format!("accessor {accessor_index} byte range overflows"))
    })?;
    if required > view.byte_length {
        return Err(ImportError::InvalidData(format!(
            "accessor {accessor_index} reads past its bufferView"
        )));
    }
    Ok(())
}

fn read_f32(bytes: &[u8], offset: usize) -> Result<f32, ImportError> {
    let value = f32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| ImportError::InvalidData("GLB float is truncated".to_string()))?
            .try_into()
            .map_err(|_| ImportError::InvalidData("GLB float has invalid width".to_string()))?,
    );
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ImportError::InvalidData(
            "GLB contains non-finite float".to_string(),
        ))
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ImportError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or_else(|| ImportError::InvalidData("GLB u16 is truncated".to_string()))?
            .try_into()
            .map_err(|_| ImportError::InvalidData("GLB u16 has invalid width".to_string()))?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn json_usize(value: Option<&JsonValue>) -> Option<usize> {
    value.and_then(JsonValue::as_number).and_then(|value| {
        (value.is_finite() && value >= 0.0 && value.fract() == 0.0).then_some(value as usize)
    })
}

fn json_u32(value: Option<&JsonValue>) -> Option<u32> {
    json_usize(value).and_then(|value| u32::try_from(value).ok())
}

fn json_f32_array3(value: &JsonValue) -> Option<[f32; 3]> {
    let array = value.as_array()?;
    (array.len() == 3).then(|| {
        [
            array[0].as_number().unwrap_or(0.0) as f32,
            array[1].as_number().unwrap_or(0.0) as f32,
            array[2].as_number().unwrap_or(0.0) as f32,
        ]
    })
}

fn json_f32_array4(value: &JsonValue) -> Option<[f32; 4]> {
    let array = value.as_array()?;
    (array.len() == 4).then(|| {
        [
            array[0].as_number().unwrap_or(0.0) as f32,
            array[1].as_number().unwrap_or(0.0) as f32,
            array[2].as_number().unwrap_or(0.0) as f32,
            array[3].as_number().unwrap_or(1.0) as f32,
        ]
    })
}

fn json_f32_array16(value: &JsonValue) -> Option<[f32; 16]> {
    let array = value.as_array()?;
    if array.len() != 16 {
        return None;
    }
    let mut values = [0.0_f32; 16];
    for (index, value) in array.iter().enumerate() {
        values[index] = value.as_number()? as f32;
    }
    Some(values)
}

fn object_get<'a>(object: &'a [(String, JsonValue)], key: &str) -> Option<&'a JsonValue> {
    object
        .iter()
        .find_map(|(candidate, value)| (candidate == key).then_some(value))
}

#[derive(Debug, Clone)]
enum JsonValue {
    Null,
    Bool,
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

impl JsonValue {
    fn as_object(&self) -> Option<&[(String, JsonValue)]> {
        match self {
            Self::Object(value) => Some(value),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }

    fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> JsonParser<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            cursor: 0,
        }
    }

    fn parse(mut self) -> Result<JsonValue, ImportError> {
        let value = self.parse_value()?;
        self.skip_ws();
        Ok(value)
    }

    fn parse_value(&mut self) -> Result<JsonValue, ImportError> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => self.parse_string().map(JsonValue::String),
            Some(b't') => {
                self.expect_literal(b"true")?;
                Ok(JsonValue::Bool)
            }
            Some(b'f') => {
                self.expect_literal(b"false")?;
                Ok(JsonValue::Bool)
            }
            Some(b'n') => {
                self.expect_literal(b"null")?;
                Ok(JsonValue::Null)
            }
            Some(b'-' | b'0'..=b'9') => self.parse_number().map(JsonValue::Number),
            _ => Err(ImportError::InvalidData("invalid JSON value".to_string())),
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, ImportError> {
        self.expect_byte(b'{')?;
        let mut entries = Vec::new();
        self.skip_ws();
        if self.consume_byte(b'}') {
            return Ok(JsonValue::Object(entries));
        }
        loop {
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect_byte(b':')?;
            let value = self.parse_value()?;
            entries.push((key, value));
            self.skip_ws();
            if self.consume_byte(b'}') {
                break;
            }
            self.expect_byte(b',')?;
        }
        Ok(JsonValue::Object(entries))
    }

    fn parse_array(&mut self) -> Result<JsonValue, ImportError> {
        self.expect_byte(b'[')?;
        let mut values = Vec::new();
        self.skip_ws();
        if self.consume_byte(b']') {
            return Ok(JsonValue::Array(values));
        }
        loop {
            values.push(self.parse_value()?);
            self.skip_ws();
            if self.consume_byte(b']') {
                break;
            }
            self.expect_byte(b',')?;
        }
        Ok(JsonValue::Array(values))
    }

    fn parse_string(&mut self) -> Result<String, ImportError> {
        self.expect_byte(b'"')?;
        let mut output = String::new();
        while let Some(byte) = self.next() {
            match byte {
                b'"' => return Ok(output),
                b'\\' => {
                    let escaped = self.next().ok_or_else(|| {
                        ImportError::InvalidData("unterminated JSON escape".to_string())
                    })?;
                    match escaped {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{0008}'),
                        b'f' => output.push('\u{000C}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => output.push(self.parse_unicode_escape()?),
                        _ => {
                            return Err(ImportError::InvalidData(
                                "invalid JSON escape".to_string(),
                            ));
                        }
                    }
                }
                value if value < 0x20 => {
                    return Err(ImportError::InvalidData(
                        "control character in JSON string".to_string(),
                    ));
                }
                value => output.push(value as char),
            }
        }
        Err(ImportError::InvalidData(
            "unterminated JSON string".to_string(),
        ))
    }

    fn parse_unicode_escape(&mut self) -> Result<char, ImportError> {
        let slice = self
            .bytes
            .get(self.cursor..self.cursor + 4)
            .ok_or_else(|| ImportError::InvalidData("short JSON unicode escape".to_string()))?;
        self.cursor += 4;
        let text = std::str::from_utf8(slice)
            .map_err(|_| ImportError::InvalidData("invalid JSON unicode escape".to_string()))?;
        let value = u16::from_str_radix(text, 16)
            .map_err(|_| ImportError::InvalidData("invalid JSON unicode escape".to_string()))?;
        char::from_u32(value as u32)
            .ok_or_else(|| ImportError::InvalidData("invalid JSON unicode scalar".to_string()))
    }

    fn parse_number(&mut self) -> Result<f64, ImportError> {
        let start = self.cursor;
        if self.peek() == Some(b'-') {
            self.cursor += 1;
        }
        self.consume_digits();
        if self.peek() == Some(b'.') {
            self.cursor += 1;
            self.consume_digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.cursor += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.cursor += 1;
            }
            self.consume_digits();
        }
        let text = std::str::from_utf8(&self.bytes[start..self.cursor])
            .map_err(|_| ImportError::InvalidData("invalid JSON number".to_string()))?;
        text.parse::<f64>()
            .map_err(|error| ImportError::InvalidData(format!("invalid JSON number: {error}")))
    }

    fn consume_digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.cursor += 1;
        }
    }

    fn expect_literal(&mut self, literal: &[u8]) -> Result<(), ImportError> {
        if self.bytes.get(self.cursor..self.cursor + literal.len()) == Some(literal) {
            self.cursor += literal.len();
            Ok(())
        } else {
            Err(ImportError::InvalidData("invalid JSON literal".to_string()))
        }
    }

    fn expect_byte(&mut self, expected: u8) -> Result<(), ImportError> {
        if self.consume_byte(expected) {
            Ok(())
        } else {
            Err(ImportError::InvalidData(format!(
                "expected JSON byte `{}`",
                expected as char
            )))
        }
    }

    fn consume_byte(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.cursor += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.cursor).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.cursor += 1;
        Some(byte)
    }
}
