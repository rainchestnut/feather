//! IR validation before and after mesh processing.

use crate::document::{LiteDocument, LitePrimitive};
use crate::importer::ImportError;

/// Validates cross references and primitive array invariants.
pub fn validate_document(document: &LiteDocument) -> Result<(), ImportError> {
    for (node_index, node) in document.nodes.iter().enumerate() {
        if let Some(mesh_index) = node.mesh
            && mesh_index >= document.meshes.len()
        {
            return Err(ImportError::InvalidData(format!(
                "node {node_index} references missing mesh {mesh_index}"
            )));
        }

        for child_index in &node.children {
            if *child_index >= document.nodes.len() {
                return Err(ImportError::InvalidData(format!(
                    "node {node_index} references missing child {child_index}"
                )));
            }
        }
    }

    for (mesh_index, mesh) in document.meshes.iter().enumerate() {
        for (primitive_index, primitive) in mesh.primitives.iter().enumerate() {
            if !primitive.normals.is_empty() && primitive.normals.len() != primitive.positions.len()
            {
                return Err(ImportError::InvalidData(format!(
                    "mesh {mesh_index} primitive {primitive_index} has mismatched normals"
                )));
            }

            if primitive.indices.len() % 3 != 0 {
                return Err(ImportError::InvalidData(format!(
                    "mesh {mesh_index} primitive {primitive_index} index count is not triangular"
                )));
            }

            if let Some(material_index) = primitive.material
                && material_index >= document.materials.len()
            {
                return Err(ImportError::InvalidData(format!(
                    "mesh {mesh_index} primitive {primitive_index} references missing material {material_index}"
                )));
            }

            for index in &primitive.indices {
                if (*index as usize) >= primitive.positions.len() {
                    return Err(ImportError::InvalidData(format!(
                        "mesh {mesh_index} primitive {primitive_index} index {index} is out of range"
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Counts degenerate indexed triangles in one primitive.
pub(crate) fn primitive_degenerate_triangle_count(primitive: &LitePrimitive) -> u64 {
    primitive
        .indices
        .chunks_exact(3)
        .filter(|triangle| is_degenerate_triangle(triangle, &primitive.positions))
        .count() as u64
}

/// Returns true when an indexed triangle has duplicate vertices or no stable area.
pub(crate) fn is_degenerate_triangle(triangle: &[u32], positions: &[[f32; 3]]) -> bool {
    let [a, b, c] = triangle else {
        return true;
    };
    if a == b || b == c || a == c {
        return true;
    }

    let Some(a) = positions.get(*a as usize).copied() else {
        return true;
    };
    let Some(b) = positions.get(*b as usize).copied() else {
        return true;
    };
    let Some(c) = positions.get(*c as usize).copied() else {
        return true;
    };
    a == b || b == c || a == c || has_degenerate_area(a, b, c)
}

fn has_degenerate_area(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> bool {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let bc = [c[0] - b[0], c[1] - b[1], c[2] - b[2]];
    let edge_scale = length_squared(ab)
        .max(length_squared(ac))
        .max(length_squared(bc));
    let cross = cross(ab, ac);
    let area_scale = length_squared(cross);
    area_scale <= f32::EPSILON * f32::EPSILON * edge_scale * edge_scale
}

fn length_squared(value: [f32; 3]) -> f32 {
    value[0] * value[0] + value[1] * value[1] + value[2] * value[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
