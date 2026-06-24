//! IR validation before and after mesh processing.

use crate::document::LiteDocument;
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
