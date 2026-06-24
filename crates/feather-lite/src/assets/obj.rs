//! Wavefront OBJ reader for visual mesh payloads.

use std::path::Path;

use crate::document::{LiteDocument, LiteMaterial, LiteMesh, LiteNode, LitePrimitive};
use crate::importer::ImportError;

const DEFAULT_OBJ_MATERIAL_COLOR: [f32; 4] = [0.8, 0.8, 0.82, 1.0];

/// Material parsed from a Wavefront MTL sidecar.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjMaterial {
    pub name: String,
    pub base_color: [f32; 4],
}

/// Returns true when bytes look like a Wavefront OBJ mesh.
pub fn is_obj(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };

    let mut has_vertex = false;
    let mut has_face = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("v ") {
            has_vertex = true;
        } else if trimmed.starts_with("f ") {
            has_face = true;
        }

        if has_vertex && has_face {
            return true;
        }
    }

    false
}

/// Imports a Wavefront OBJ payload into the visual IR.
pub fn import_obj_document(
    bytes: &[u8],
    source_format: &str,
    mode: &str,
    source_path: Option<&Path>,
) -> Result<LiteDocument, ImportError> {
    import_obj_document_with_materials(bytes, &[], source_format, mode, source_path)
}

/// Imports a Wavefront OBJ payload and applies pre-parsed MTL materials.
pub fn import_obj_document_with_materials(
    bytes: &[u8],
    materials: &[ObjMaterial],
    source_format: &str,
    mode: &str,
    source_path: Option<&Path>,
) -> Result<LiteDocument, ImportError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| ImportError::InvalidData(format!("OBJ input is not UTF-8: {error}")))?;
    if !is_obj(bytes) {
        return Err(ImportError::InvalidData(
            "input is not a recognizable OBJ mesh".to_string(),
        ));
    }

    let mut source_positions = Vec::<[f32; 3]>::new();
    let mut source_normals = Vec::<[f32; 3]>::new();
    let mut document_materials = materials
        .iter()
        .map(|material| LiteMaterial::new(&material.name, material.base_color))
        .collect::<Vec<_>>();
    let mut primitives = Vec::<LitePrimitive>::new();
    let mut current_material = None::<usize>;

    for (line_index, raw_line) in text.lines().enumerate() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        let tokens = line.split_whitespace().collect::<Vec<_>>();
        match tokens[0] {
            "v" => source_positions.push(parse_vec3(&tokens, line_index, "vertex")?),
            "vn" => source_normals.push(parse_vec3(&tokens, line_index, "normal")?),
            "usemtl" if tokens.len() >= 2 => {
                current_material =
                    Some(material_index_for_name(&mut document_materials, tokens[1]));
            }
            "f" => {
                let primitive = primitive_for_material(&mut primitives, current_material);
                triangulate_face(
                    &tokens[1..],
                    line_index,
                    &source_positions,
                    &source_normals,
                    primitive,
                )?;
            }
            _ => {}
        }
    }

    primitives.retain(|primitive| !primitive.indices.is_empty());
    if primitives.is_empty() {
        return Err(ImportError::InvalidData(
            "OBJ contains no polygon faces".to_string(),
        ));
    }

    let mut mesh = LiteMesh::new("OBJ_Mesh");
    mesh.primitives = primitives;
    mesh.recompute_bbox();

    let mut document = LiteDocument::new(source_format, mode);
    if let Some(path) = source_path {
        document.metadata.source_path = Some(path.display().to_string());
    }
    document.materials = document_materials;
    document.meshes.push(mesh);
    document.nodes.push(LiteNode::new("OBJ_Mesh", Some(0)));
    document.refresh_metadata();
    Ok(document)
}

/// Parses a Wavefront MTL payload into simple CAD preview materials.
pub fn parse_mtl_materials(bytes: &[u8]) -> Result<Vec<ObjMaterial>, ImportError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| ImportError::InvalidData(format!("MTL input is not UTF-8: {error}")))?;
    let mut materials = Vec::<ObjMaterial>::new();
    let mut current = None::<ObjMaterial>;

    for (line_index, raw_line) in text.lines().enumerate() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        match tokens[0] {
            "newmtl" => {
                if let Some(material) = current.take() {
                    materials.push(material);
                }
                if tokens.len() < 2 {
                    return Err(ImportError::InvalidData(format!(
                        "line {}: MTL newmtl expects a material name",
                        line_index + 1
                    )));
                }
                current = Some(ObjMaterial {
                    name: tokens[1].to_string(),
                    base_color: DEFAULT_OBJ_MATERIAL_COLOR,
                });
            }
            "Kd" => {
                if tokens.len() < 4 {
                    return Err(ImportError::InvalidData(format!(
                        "line {}: MTL Kd expects 3 color components",
                        line_index + 1
                    )));
                }
                if let Some(material) = current.as_mut() {
                    material.base_color[0] = parse_f32(tokens[1], line_index, "Kd r")?;
                    material.base_color[1] = parse_f32(tokens[2], line_index, "Kd g")?;
                    material.base_color[2] = parse_f32(tokens[3], line_index, "Kd b")?;
                }
            }
            "d" => {
                if tokens.len() < 2 {
                    return Err(ImportError::InvalidData(format!(
                        "line {}: MTL d expects an alpha value",
                        line_index + 1
                    )));
                }
                if let Some(material) = current.as_mut() {
                    material.base_color[3] = parse_f32(tokens[1], line_index, "d")?;
                }
            }
            "Tr" => {
                if tokens.len() < 2 {
                    return Err(ImportError::InvalidData(format!(
                        "line {}: MTL Tr expects a transparency value",
                        line_index + 1
                    )));
                }
                if let Some(material) = current.as_mut() {
                    material.base_color[3] = 1.0 - parse_f32(tokens[1], line_index, "Tr")?;
                }
            }
            _ => {}
        }
    }

    if let Some(material) = current {
        materials.push(material);
    }
    Ok(materials)
}

fn triangulate_face(
    face_tokens: &[&str],
    line_index: usize,
    source_positions: &[[f32; 3]],
    source_normals: &[[f32; 3]],
    primitive: &mut LitePrimitive,
) -> Result<(), ImportError> {
    if face_tokens.len() < 3 {
        return Err(ImportError::InvalidData(format!(
            "line {}: OBJ face needs at least 3 vertices",
            line_index + 1
        )));
    }

    let mut face_vertices = Vec::with_capacity(face_tokens.len());
    for token in face_tokens {
        face_vertices.push(parse_face_vertex(
            token,
            line_index,
            source_positions,
            source_normals,
        )?);
    }

    for offset in 1..face_vertices.len() - 1 {
        push_face_vertex(primitive, face_vertices[0]);
        push_face_vertex(primitive, face_vertices[offset]);
        push_face_vertex(primitive, face_vertices[offset + 1]);
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct ObjFaceVertex {
    position: [f32; 3],
    normal: Option<[f32; 3]>,
}

fn parse_face_vertex(
    token: &str,
    line_index: usize,
    source_positions: &[[f32; 3]],
    source_normals: &[[f32; 3]],
) -> Result<ObjFaceVertex, ImportError> {
    let parts = token.split('/').collect::<Vec<_>>();
    let position_index = parse_obj_index(parts[0], source_positions.len(), line_index, "position")?;
    let normal = if parts.len() >= 3 && !parts[2].is_empty() {
        let normal_index = parse_obj_index(parts[2], source_normals.len(), line_index, "normal")?;
        Some(source_normals[normal_index])
    } else {
        None
    };

    Ok(ObjFaceVertex {
        position: source_positions[position_index],
        normal,
    })
}

fn parse_obj_index(
    token: &str,
    len: usize,
    line_index: usize,
    kind: &str,
) -> Result<usize, ImportError> {
    if token.is_empty() {
        return Err(ImportError::InvalidData(format!(
            "line {}: OBJ face missing {kind} index",
            line_index + 1
        )));
    }

    let raw = token.parse::<isize>().map_err(|error| {
        ImportError::InvalidData(format!(
            "line {}: invalid OBJ {kind} index `{token}`: {error}",
            line_index + 1
        ))
    })?;

    let index = if raw > 0 {
        raw - 1
    } else if raw < 0 {
        len as isize + raw
    } else {
        return Err(ImportError::InvalidData(format!(
            "line {}: OBJ {kind} index cannot be zero",
            line_index + 1
        )));
    };

    if index < 0 || index as usize >= len {
        return Err(ImportError::InvalidData(format!(
            "line {}: OBJ {kind} index {raw} is out of range",
            line_index + 1
        )));
    }

    Ok(index as usize)
}

fn push_face_vertex(primitive: &mut LitePrimitive, vertex: ObjFaceVertex) {
    primitive.positions.push(vertex.position);
    if let Some(normal) = vertex.normal {
        primitive.normals.push(normal);
    }
    primitive.indices.push(primitive.indices.len() as u32);
}

fn primitive_for_material(
    primitives: &mut Vec<LitePrimitive>,
    material: Option<usize>,
) -> &mut LitePrimitive {
    if let Some(index) = primitives
        .iter()
        .position(|primitive| primitive.material == material)
    {
        &mut primitives[index]
    } else {
        primitives.push(LitePrimitive::new(material));
        primitives.last_mut().expect("primitive was just pushed")
    }
}

fn material_index_for_name(materials: &mut Vec<LiteMaterial>, name: &str) -> usize {
    if let Some(index) = materials.iter().position(|material| material.name == name) {
        index
    } else {
        let index = materials.len();
        materials.push(LiteMaterial::new(name, DEFAULT_OBJ_MATERIAL_COLOR));
        index
    }
}

fn parse_vec3(tokens: &[&str], line_index: usize, kind: &str) -> Result<[f32; 3], ImportError> {
    if tokens.len() < 4 {
        return Err(ImportError::InvalidData(format!(
            "line {}: OBJ {kind} expects at least 3 coordinates",
            line_index + 1
        )));
    }

    Ok([
        parse_f32(tokens[1], line_index, kind)?,
        parse_f32(tokens[2], line_index, kind)?,
        parse_f32(tokens[3], line_index, kind)?,
    ])
}

fn parse_f32(token: &str, line_index: usize, kind: &str) -> Result<f32, ImportError> {
    let value = token.parse::<f32>().map_err(|error| {
        ImportError::InvalidData(format!(
            "line {}: invalid OBJ {kind} coordinate `{token}`: {error}",
            line_index + 1
        ))
    })?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ImportError::InvalidData(format!(
            "line {}: OBJ {kind} coordinate is not finite",
            line_index + 1
        )))
    }
}
