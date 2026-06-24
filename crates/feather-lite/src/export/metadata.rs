//! Metadata JSON writer for conversion sidecars.

use crate::document::{Aabb, LiteDocument, Transform, identity_transform};
use crate::json::escape_json;

/// Serializes document metadata as stable JSON.
pub fn export_metadata_json(document: &LiteDocument) -> String {
    let metadata = &document.metadata;
    let mut json = String::new();
    json.push_str("{\n");
    push_json_string_field(&mut json, "source_format", &metadata.source_format, true);
    push_json_string_field(&mut json, "mode", &metadata.mode, true);
    push_json_string_field(&mut json, "precision", &metadata.precision, true);
    push_json_number_field(&mut json, "mesh_count", metadata.mesh_count as u64, true);
    push_json_number_field(&mut json, "triangle_count", metadata.triangle_count, true);
    push_json_bool_field(&mut json, "has_brep", metadata.has_brep, true);
    push_json_bool_field(&mut json, "brep_preserved", metadata.brep_preserved, true);
    push_json_bbox_field(&mut json, "bbox", scene_bbox(document), true);

    json.push_str("  \"source_path\": ");
    if let Some(source_path) = &metadata.source_path {
        json.push('"');
        json.push_str(&escape_json(source_path));
        json.push('"');
    } else {
        json.push_str("null");
    }
    json.push_str(",\n");

    json.push_str("  \"warnings\": [");
    for (index, warning) in metadata.warnings.iter().enumerate() {
        if index > 0 {
            json.push_str(", ");
        }
        json.push('"');
        json.push_str(&escape_json(warning));
        json.push('"');
    }
    json.push_str("]\n");
    json.push_str("}\n");
    json
}

fn push_json_bbox_field(json: &mut String, key: &str, bbox: Option<Aabb>, comma: bool) {
    json.push_str("  \"");
    json.push_str(key);
    json.push_str("\": ");
    if let Some(bbox) = bbox {
        json.push_str("{\"min\": ");
        push_json_f32_array(json, &bbox.min);
        json.push_str(", \"max\": ");
        push_json_f32_array(json, &bbox.max);
        json.push('}');
    } else {
        json.push_str("null");
    }
    if comma {
        json.push(',');
    }
    json.push('\n');
}

fn push_json_f32_array(json: &mut String, values: &[f32; 3]) {
    json.push('[');
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            json.push_str(", ");
        }
        json.push_str(&format_f32(*value));
    }
    json.push(']');
}

fn push_json_string_field(json: &mut String, key: &str, value: &str, comma: bool) {
    json.push_str("  \"");
    json.push_str(key);
    json.push_str("\": \"");
    json.push_str(&escape_json(value));
    json.push('"');
    if comma {
        json.push(',');
    }
    json.push('\n');
}

fn push_json_number_field(json: &mut String, key: &str, value: u64, comma: bool) {
    json.push_str("  \"");
    json.push_str(key);
    json.push_str("\": ");
    json.push_str(&value.to_string());
    if comma {
        json.push(',');
    }
    json.push('\n');
}

fn push_json_bool_field(json: &mut String, key: &str, value: bool, comma: bool) {
    json.push_str("  \"");
    json.push_str(key);
    json.push_str("\": ");
    json.push_str(if value { "true" } else { "false" });
    if comma {
        json.push(',');
    }
    json.push('\n');
}

fn scene_bbox(document: &LiteDocument) -> Option<Aabb> {
    let mut bbox = Aabb::empty();
    let mut referenced_meshes = vec![false; document.meshes.len()];

    let roots = root_node_indices(document);
    for node_index in roots {
        include_node_bbox(
            document,
            node_index,
            identity_transform(),
            document.nodes.len().saturating_add(1),
            &mut referenced_meshes,
            &mut bbox,
        );
    }

    for (mesh_index, mesh) in document.meshes.iter().enumerate() {
        if referenced_meshes.get(mesh_index).copied().unwrap_or(false) {
            continue;
        }
        for primitive in &mesh.primitives {
            for position in &primitive.positions {
                bbox.include_point(*position);
            }
        }
    }

    (!bbox.is_empty()).then_some(bbox.normalized())
}

fn include_node_bbox(
    document: &LiteDocument,
    node_index: usize,
    parent_transform: Transform,
    remaining_depth: usize,
    referenced_meshes: &mut [bool],
    bbox: &mut Aabb,
) {
    if remaining_depth == 0 {
        return;
    }
    let Some(node) = document.nodes.get(node_index) else {
        return;
    };
    let world_transform = multiply_transforms(parent_transform, node.transform);

    if let Some(mesh_index) = node.mesh
        && let Some(mesh) = document.meshes.get(mesh_index)
    {
        if let Some(referenced) = referenced_meshes.get_mut(mesh_index) {
            *referenced = true;
        }
        for primitive in &mesh.primitives {
            for position in &primitive.positions {
                bbox.include_point(transform_point(world_transform, *position));
            }
        }
    }

    for child_index in &node.children {
        include_node_bbox(
            document,
            *child_index,
            world_transform,
            remaining_depth - 1,
            referenced_meshes,
            bbox,
        );
    }
}

fn root_node_indices(document: &LiteDocument) -> Vec<usize> {
    let mut referenced = vec![false; document.nodes.len()];
    for node in &document.nodes {
        for child_index in &node.children {
            if *child_index < referenced.len() {
                referenced[*child_index] = true;
            }
        }
    }

    referenced
        .iter()
        .enumerate()
        .filter_map(|(index, is_child)| (!is_child).then_some(index))
        .collect()
}

fn multiply_transforms(left: Transform, right: Transform) -> Transform {
    let mut result = [[0.0_f32; 4]; 4];
    for column in 0..4 {
        for row in 0..4 {
            result[column][row] = (0..4)
                .map(|index| left[index][row] * right[column][index])
                .sum();
        }
    }
    result
}

fn transform_point(transform: Transform, point: [f32; 3]) -> [f32; 3] {
    let x = point[0];
    let y = point[1];
    let z = point[2];
    let transformed = [
        transform[0][0] * x + transform[1][0] * y + transform[2][0] * z + transform[3][0],
        transform[0][1] * x + transform[1][1] * y + transform[2][1] * z + transform[3][1],
        transform[0][2] * x + transform[1][2] * y + transform[2][2] * z + transform[3][2],
    ];
    let w = transform[0][3] * x + transform[1][3] * y + transform[2][3] * z + transform[3][3];
    if w.is_finite() && w != 0.0 && w != 1.0 {
        [transformed[0] / w, transformed[1] / w, transformed[2] / w]
    } else {
        transformed
    }
}

fn format_f32(value: f32) -> String {
    if value == -0.0 {
        "0".to_string()
    } else {
        value.to_string()
    }
}
