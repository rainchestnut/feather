//! Binary glTF 2.0 exporter.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;

use crate::assets::glb::{import_glb_document_preserving_scene, is_exact_glb};
use crate::document::{Aabb, LiteDocument, LitePrimitive, Transform};
use crate::importer::ImportError;
use crate::json::escape_json;
use crate::mesh::validate::{primitive_degenerate_triangle_count, validate_document};

/// Options for writing GLB payloads.
#[derive(Debug, Clone)]
pub struct GlbExportOptions {
    pub include_normals: bool,
}

impl Default for GlbExportOptions {
    fn default() -> Self {
        Self {
            include_normals: true,
        }
    }
}

/// Error type for GLB export.
#[derive(Debug)]
pub enum ExportError {
    InvalidDocument(ImportError),
    InvalidOutput(ImportError),
    TooLarge(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDocument(error) => write!(formatter, "invalid document: {error}"),
            Self::InvalidOutput(error) => write!(formatter, "invalid GLB output: {error}"),
            Self::TooLarge(message) => write!(formatter, "GLB is too large: {message}"),
        }
    }
}

impl Error for ExportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidDocument(error) | Self::InvalidOutput(error) => Some(error),
            _ => None,
        }
    }
}

/// Summary returned after validating a generated GLB payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlbValidationSummary {
    pub node_count: usize,
    pub mesh_count: usize,
    pub primitive_count: usize,
    pub vertex_count: usize,
    pub triangle_count: u64,
}

/// Exports a Feather Lite document as a GLB byte vector.
pub fn export_glb(
    document: &LiteDocument,
    options: &GlbExportOptions,
) -> Result<Vec<u8>, ExportError> {
    validate_document(document).map_err(ExportError::InvalidDocument)?;

    let mut builder = GlbBuilder::default();
    let json = builder.build_json(document, options);
    builder.finish(json)
}

/// Re-imports and validates a GLB payload before it is accepted as an output artifact.
pub fn validate_glb_payload(bytes: &[u8]) -> Result<GlbValidationSummary, ExportError> {
    if !is_exact_glb(bytes) {
        return Err(ExportError::InvalidOutput(ImportError::InvalidData(
            "GLB payload length does not match its header".to_string(),
        )));
    }

    let document =
        import_glb_document_preserving_scene(bytes, "GLB", "glb-output-validation", None)
            .map_err(ExportError::InvalidOutput)?;
    validate_document(&document).map_err(ExportError::InvalidOutput)?;
    validate_glb_output_document(&document).map_err(ExportError::InvalidOutput)?;

    Ok(GlbValidationSummary {
        node_count: document.nodes.len(),
        mesh_count: document.metadata.mesh_count,
        primitive_count: document.primitive_count(),
        vertex_count: document.vertex_count(),
        triangle_count: document.metadata.triangle_count,
    })
}

fn validate_glb_output_document(document: &LiteDocument) -> Result<(), ImportError> {
    if document.nodes.is_empty() {
        return Err(ImportError::InvalidData(
            "GLB payload contains no scene nodes".to_string(),
        ));
    }
    if document.meshes.is_empty() {
        return Err(ImportError::InvalidData(
            "GLB payload contains no meshes".to_string(),
        ));
    }
    if document.metadata.triangle_count == 0 {
        return Err(ImportError::InvalidData(
            "GLB payload contains no triangles".to_string(),
        ));
    }

    let roots = root_nodes(document);
    if roots.is_empty() {
        return Err(ImportError::InvalidData(
            "GLB payload contains no root scene nodes".to_string(),
        ));
    }

    let mut visit_state = vec![NodeVisitState::Unvisited; document.nodes.len()];
    let mut reachable_meshes = vec![false; document.meshes.len()];
    for root in roots {
        validate_scene_node(document, root, &mut visit_state, &mut reachable_meshes)?;
    }
    if !reachable_meshes.iter().any(|reachable| *reachable) {
        return Err(ImportError::InvalidData(
            "GLB payload scene references no mesh geometry".to_string(),
        ));
    }
    if let Some(mesh_index) = reachable_meshes.iter().position(|reachable| !*reachable) {
        return Err(ImportError::InvalidData(format!(
            "GLB payload mesh {mesh_index} is not reachable from scene roots"
        )));
    }

    let degenerate_triangles: u64 = document
        .meshes
        .iter()
        .flat_map(|mesh| &mesh.primitives)
        .map(primitive_degenerate_triangle_count)
        .sum();
    if degenerate_triangles > 0 {
        return Err(ImportError::InvalidData(format!(
            "GLB payload contains {degenerate_triangles} degenerate triangles"
        )));
    }

    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeVisitState {
    Unvisited,
    Visiting,
    Visited,
}

fn validate_scene_node(
    document: &LiteDocument,
    node_index: usize,
    visit_state: &mut [NodeVisitState],
    reachable_meshes: &mut [bool],
) -> Result<(), ImportError> {
    match visit_state[node_index] {
        NodeVisitState::Visited => return Ok(()),
        NodeVisitState::Visiting => {
            return Err(ImportError::InvalidData(format!(
                "GLB payload node graph contains a cycle at node {node_index}"
            )));
        }
        NodeVisitState::Unvisited => {}
    }

    visit_state[node_index] = NodeVisitState::Visiting;
    let node = &document.nodes[node_index];
    if let Some(mesh_index) = node.mesh
        && let Some(reachable) = reachable_meshes.get_mut(mesh_index)
    {
        *reachable = true;
    }
    for child_index in &node.children {
        validate_scene_node(document, *child_index, visit_state, reachable_meshes)?;
    }
    visit_state[node_index] = NodeVisitState::Visited;
    Ok(())
}

#[derive(Default)]
struct GlbBuilder {
    binary: Vec<u8>,
    buffer_views: Vec<BufferView>,
    accessors: Vec<Accessor>,
    mesh_json: Vec<String>,
}

impl GlbBuilder {
    fn build_json(&mut self, document: &LiteDocument, options: &GlbExportOptions) -> String {
        self.write_meshes(document, options);

        let mut json = String::new();
        json.push('{');
        json.push_str("\"asset\":{\"version\":\"2.0\",\"generator\":\"feather-lite\"},");
        json.push_str("\"scene\":0,");
        json.push_str("\"scenes\":[{\"nodes\":");
        push_usize_array(&mut json, &root_nodes(document));
        json.push_str("}],");
        self.push_nodes(&mut json, document);
        json.push(',');
        json.push_str("\"meshes\":[");
        json.push_str(&self.mesh_json.join(","));
        json.push_str("],");
        self.push_materials(&mut json, document);
        json.push(',');
        self.push_buffers(&mut json);
        json.push(',');
        self.push_buffer_views(&mut json);
        json.push(',');
        self.push_accessors(&mut json);
        json.push('}');
        json
    }

    fn write_meshes(&mut self, document: &LiteDocument, options: &GlbExportOptions) {
        for mesh in &document.meshes {
            let mut primitives = Vec::new();
            for primitive in &mesh.primitives {
                let position_accessor = self.write_vec3_accessor(
                    &primitive.positions,
                    34962,
                    Some(Aabb::from_positions(&primitive.positions).normalized()),
                );

                let normal_accessor = if options.include_normals
                    && primitive.normals.len() == primitive.positions.len()
                {
                    Some(self.write_vec3_accessor(&primitive.normals, 34962, None))
                } else {
                    None
                };

                let indices = indices_or_sequential(primitive);
                let index_accessor = self.write_index_accessor(&indices, 34963);

                let mut primitive_json = String::new();
                primitive_json.push_str("{\"attributes\":{\"POSITION\":");
                primitive_json.push_str(&position_accessor.to_string());
                if let Some(normal_accessor) = normal_accessor {
                    primitive_json.push_str(",\"NORMAL\":");
                    primitive_json.push_str(&normal_accessor.to_string());
                }
                primitive_json.push_str("},\"indices\":");
                primitive_json.push_str(&index_accessor.to_string());
                primitive_json.push_str(",\"mode\":4");
                if let Some(material_index) = primitive.material {
                    primitive_json.push_str(",\"material\":");
                    primitive_json.push_str(&material_index.to_string());
                }
                primitive_json.push('}');
                primitives.push(primitive_json);
            }

            self.mesh_json.push(format!(
                "{{\"name\":\"{}\",\"primitives\":[{}]}}",
                escape_json(&mesh.name),
                primitives.join(",")
            ));
        }
    }

    fn write_vec3_accessor(
        &mut self,
        values: &[[f32; 3]],
        target: u32,
        bounds: Option<Aabb>,
    ) -> usize {
        self.align_binary();
        let offset = self.binary.len();
        for value in values {
            for component in value {
                self.binary.extend_from_slice(&component.to_le_bytes());
            }
        }
        let length = self.binary.len() - offset;
        let view = self.push_buffer_view(offset, length, target);
        let accessor = self.accessors.len();
        self.accessors.push(Accessor {
            buffer_view: view,
            component_type: 5126,
            count: values.len(),
            type_name: "VEC3",
            bounds,
        });
        accessor
    }

    fn write_index_accessor(&mut self, values: &[u32], target: u32) -> usize {
        self.align_binary();
        let offset = self.binary.len();
        let component_type = if values.iter().all(|value| u16::try_from(*value).is_ok()) {
            for value in values {
                self.binary
                    .extend_from_slice(&(*value as u16).to_le_bytes());
            }
            5123
        } else {
            for value in values {
                self.binary.extend_from_slice(&value.to_le_bytes());
            }
            5125
        };
        let length = self.binary.len() - offset;
        let view = self.push_buffer_view(offset, length, target);
        let accessor = self.accessors.len();
        self.accessors.push(Accessor {
            buffer_view: view,
            component_type,
            count: values.len(),
            type_name: "SCALAR",
            bounds: None,
        });
        accessor
    }

    fn push_buffer_view(&mut self, offset: usize, length: usize, target: u32) -> usize {
        let index = self.buffer_views.len();
        self.buffer_views.push(BufferView {
            byte_offset: offset,
            byte_length: length,
            target,
        });
        index
    }

    fn align_binary(&mut self) {
        while !self.binary.len().is_multiple_of(4) {
            self.binary.push(0);
        }
    }

    fn push_nodes(&self, json: &mut String, document: &LiteDocument) {
        json.push_str("\"nodes\":[");
        for (index, node) in document.nodes.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            json.push_str("{\"name\":\"");
            json.push_str(&escape_json(&node.name));
            json.push('"');
            if let Some(mesh) = node.mesh {
                json.push_str(",\"mesh\":");
                json.push_str(&mesh.to_string());
            }
            if !node.children.is_empty() {
                json.push_str(",\"children\":");
                push_usize_array(json, &node.children);
            }
            if !is_identity(&node.transform) {
                json.push_str(",\"matrix\":");
                push_matrix(json, node.transform);
            }
            json.push('}');
        }
        json.push(']');
    }

    fn push_materials(&self, json: &mut String, document: &LiteDocument) {
        json.push_str("\"materials\":[");
        for (index, material) in document.materials.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            json.push_str("{\"name\":\"");
            json.push_str(&escape_json(&material.name));
            json.push_str("\",\"pbrMetallicRoughness\":{\"baseColorFactor\":");
            push_f32_array(json, &material.base_color);
            json.push_str(",\"metallicFactor\":0.0,\"roughnessFactor\":0.82}}");
        }
        json.push(']');
    }

    fn push_buffers(&self, json: &mut String) {
        json.push_str("\"buffers\":[{\"byteLength\":");
        json.push_str(&self.binary.len().to_string());
        json.push_str("}]");
    }

    fn push_buffer_views(&self, json: &mut String) {
        json.push_str("\"bufferViews\":[");
        for (index, view) in self.buffer_views.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            json.push_str("{\"buffer\":0,\"byteOffset\":");
            json.push_str(&view.byte_offset.to_string());
            json.push_str(",\"byteLength\":");
            json.push_str(&view.byte_length.to_string());
            json.push_str(",\"target\":");
            json.push_str(&view.target.to_string());
            json.push('}');
        }
        json.push(']');
    }

    fn push_accessors(&self, json: &mut String) {
        json.push_str("\"accessors\":[");
        for (index, accessor) in self.accessors.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            json.push_str("{\"bufferView\":");
            json.push_str(&accessor.buffer_view.to_string());
            json.push_str(",\"componentType\":");
            json.push_str(&accessor.component_type.to_string());
            json.push_str(",\"count\":");
            json.push_str(&accessor.count.to_string());
            json.push_str(",\"type\":\"");
            json.push_str(accessor.type_name);
            json.push('"');
            if let Some(bounds) = accessor.bounds {
                json.push_str(",\"min\":");
                push_f32_array(json, &bounds.min);
                json.push_str(",\"max\":");
                push_f32_array(json, &bounds.max);
            }
            json.push('}');
        }
        json.push(']');
    }

    fn finish(mut self, json: String) -> Result<Vec<u8>, ExportError> {
        let mut json_bytes = json.into_bytes();
        while !json_bytes.len().is_multiple_of(4) {
            json_bytes.push(b' ');
        }
        while !self.binary.len().is_multiple_of(4) {
            self.binary.push(0);
        }

        let total_len = 12_usize + 8 + json_bytes.len() + 8 + self.binary.len();
        let total_len = u32::try_from(total_len)
            .map_err(|_| ExportError::TooLarge("GLB length exceeds u32".to_string()))?;
        let json_len = u32::try_from(json_bytes.len())
            .map_err(|_| ExportError::TooLarge("JSON chunk exceeds u32".to_string()))?;
        let bin_len = u32::try_from(self.binary.len())
            .map_err(|_| ExportError::TooLarge("BIN chunk exceeds u32".to_string()))?;

        let mut glb = Vec::with_capacity(total_len as usize);
        glb.extend_from_slice(&0x4654_6C67_u32.to_le_bytes());
        glb.extend_from_slice(&2_u32.to_le_bytes());
        glb.extend_from_slice(&total_len.to_le_bytes());
        glb.extend_from_slice(&json_len.to_le_bytes());
        glb.extend_from_slice(&0x4E4F_534A_u32.to_le_bytes());
        glb.extend_from_slice(&json_bytes);
        glb.extend_from_slice(&bin_len.to_le_bytes());
        glb.extend_from_slice(&0x004E_4942_u32.to_le_bytes());
        glb.extend_from_slice(&self.binary);
        Ok(glb)
    }
}

#[derive(Debug)]
struct BufferView {
    byte_offset: usize,
    byte_length: usize,
    target: u32,
}

#[derive(Debug)]
struct Accessor {
    buffer_view: usize,
    component_type: u32,
    count: usize,
    type_name: &'static str,
    bounds: Option<Aabb>,
}

fn indices_or_sequential(primitive: &LitePrimitive) -> Cow<'_, [u32]> {
    if primitive.indices.is_empty() {
        Cow::Owned((0..primitive.positions.len() as u32).collect())
    } else {
        Cow::Borrowed(&primitive.indices)
    }
}

fn root_nodes(document: &LiteDocument) -> Vec<usize> {
    let mut referenced = vec![false; document.nodes.len()];
    for node in &document.nodes {
        for child in &node.children {
            if *child < referenced.len() {
                referenced[*child] = true;
            }
        }
    }

    referenced
        .iter()
        .enumerate()
        .filter_map(|(index, is_child)| (!is_child).then_some(index))
        .collect()
}

fn is_identity(matrix: &Transform) -> bool {
    let identity = crate::document::identity_transform();
    matrix == &identity
}

fn push_usize_array(json: &mut String, values: &[usize]) {
    json.push('[');
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str(&value.to_string());
    }
    json.push(']');
}

fn push_matrix(json: &mut String, matrix: Transform) {
    json.push('[');
    for (column_index, column) in matrix.iter().enumerate() {
        for (row_index, value) in column.iter().enumerate() {
            if column_index != 0 || row_index != 0 {
                json.push(',');
            }
            push_f32(json, *value);
        }
    }
    json.push(']');
}

fn push_f32_array<const N: usize>(json: &mut String, values: &[f32; N]) {
    json.push('[');
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        push_f32(json, *value);
    }
    json.push(']');
}

fn push_f32(json: &mut String, value: f32) {
    if value.is_finite() {
        json.push_str(&format!("{value:.7}"));
    } else {
        json.push_str("0.0");
    }
}
