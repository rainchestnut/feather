//! STL readers for visual mesh payloads.

use std::path::Path;

use crate::document::{LiteDocument, LiteMesh, LiteNode, LitePrimitive};
use crate::importer::ImportError;

const STL_HEADER_LEN: usize = 80;
const STL_COUNT_LEN: usize = 4;
const STL_TRIANGLE_LEN: usize = 50;
const STL_MIN_LEN: usize = STL_HEADER_LEN + STL_COUNT_LEN;
const MAX_STL_TRIANGLES: u32 = 50_000_000;

/// Returns the total byte length of a binary STL payload if the header/count is
/// structurally plausible.
pub fn binary_stl_len(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < STL_MIN_LEN {
        return None;
    }

    let count = u32::from_le_bytes(bytes[STL_HEADER_LEN..STL_MIN_LEN].try_into().ok()?);
    if count == 0 || count > MAX_STL_TRIANGLES {
        return None;
    }

    STL_MIN_LEN.checked_add((count as usize).checked_mul(STL_TRIANGLE_LEN)?)
}

/// Returns true when a valid binary STL payload exactly fills the provided
/// slice.
pub fn is_exact_binary_stl(bytes: &[u8]) -> bool {
    binary_stl_len(bytes)
        .is_some_and(|length| length == bytes.len() && has_finite_binary_stl_vectors(bytes, length))
}

/// Imports a binary STL payload into the visual IR.
pub fn import_binary_stl_document(
    bytes: &[u8],
    source_format: &str,
    mode: &str,
    source_path: Option<&Path>,
) -> Result<LiteDocument, ImportError> {
    let payload_len = binary_stl_len(bytes).ok_or_else(|| {
        ImportError::InvalidData("input is not a structurally valid binary STL".to_string())
    })?;
    if payload_len > bytes.len() {
        return Err(ImportError::InvalidData(
            "binary STL payload is truncated".to_string(),
        ));
    }

    let triangle_count = u32::from_le_bytes(
        bytes[STL_HEADER_LEN..STL_MIN_LEN]
            .try_into()
            .map_err(|_| ImportError::InvalidData("missing STL triangle count".to_string()))?,
    );

    let mut primitive = LitePrimitive::new(None);
    primitive
        .positions
        .reserve((triangle_count as usize).saturating_mul(3));
    primitive
        .normals
        .reserve((triangle_count as usize).saturating_mul(3));
    primitive
        .indices
        .reserve((triangle_count as usize).saturating_mul(3));

    let mut cursor = STL_MIN_LEN;
    for triangle_index in 0..triangle_count {
        let normal = read_vec3(bytes, cursor)?;
        cursor += 12;

        for _ in 0..3 {
            let position = read_vec3(bytes, cursor)?;
            cursor += 12;
            primitive.positions.push(position);
            primitive.normals.push(normal);
            primitive.indices.push(primitive.indices.len() as u32);
        }

        cursor += 2;
        if cursor > payload_len {
            return Err(ImportError::InvalidData(format!(
                "binary STL triangle {triangle_index} overruns payload"
            )));
        }
    }

    let mut mesh = LiteMesh::new("STL_Mesh");
    mesh.primitives.push(primitive);
    mesh.recompute_bbox();

    let mut document = LiteDocument::new(source_format, mode);
    if let Some(path) = source_path {
        document.metadata.source_path = Some(path.display().to_string());
    }
    document.meshes.push(mesh);
    document.nodes.push(LiteNode::new("STL_Mesh", Some(0)));
    document.refresh_metadata();
    Ok(document)
}

/// Returns true when bytes look like an ASCII STL payload.
pub fn is_ascii_stl(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let trimmed = text.trim_start();
    trimmed.len() >= 5
        && trimmed[..5].eq_ignore_ascii_case("solid")
        && contains_ignore_ascii_case(trimmed, "facet")
        && contains_ignore_ascii_case(trimmed, "vertex")
        && contains_ignore_ascii_case(trimmed, "endsolid")
}

/// Imports an ASCII STL payload into the visual IR.
pub fn import_ascii_stl_document(
    bytes: &[u8],
    source_format: &str,
    mode: &str,
    source_path: Option<&Path>,
) -> Result<LiteDocument, ImportError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| ImportError::InvalidData(format!("ASCII STL is not UTF-8: {error}")))?;
    if !is_ascii_stl(bytes) {
        return Err(ImportError::InvalidData(
            "input is not a recognizable ASCII STL".to_string(),
        ));
    }

    let mut primitive = LitePrimitive::new(None);
    let mut current_normal = [0.0, 0.0, 1.0];
    let mut current_vertices = Vec::<[f32; 3]>::new();

    for (line_index, raw_line) in text.lines().enumerate() {
        let tokens = raw_line.split_whitespace().collect::<Vec<_>>();
        if tokens.is_empty() {
            continue;
        }

        if tokens[0].eq_ignore_ascii_case("facet") {
            if tokens.len() == 5 && tokens[1].eq_ignore_ascii_case("normal") {
                current_normal = [
                    parse_ascii_f32(tokens[2], line_index, "normal x")?,
                    parse_ascii_f32(tokens[3], line_index, "normal y")?,
                    parse_ascii_f32(tokens[4], line_index, "normal z")?,
                ];
            }
            current_vertices.clear();
        } else if tokens[0].eq_ignore_ascii_case("vertex") {
            if tokens.len() != 4 {
                return Err(ImportError::InvalidData(format!(
                    "line {}: ASCII STL vertex expects 3 coordinates",
                    line_index + 1
                )));
            }
            current_vertices.push([
                parse_ascii_f32(tokens[1], line_index, "x")?,
                parse_ascii_f32(tokens[2], line_index, "y")?,
                parse_ascii_f32(tokens[3], line_index, "z")?,
            ]);
        } else if tokens[0].eq_ignore_ascii_case("endfacet") {
            if current_vertices.len() != 3 {
                return Err(ImportError::InvalidData(format!(
                    "line {}: ASCII STL facet contains {} vertices",
                    line_index + 1,
                    current_vertices.len()
                )));
            }
            for vertex in &current_vertices {
                primitive.positions.push(*vertex);
                primitive.normals.push(current_normal);
                primitive.indices.push(primitive.indices.len() as u32);
            }
            current_vertices.clear();
        }
    }

    if primitive.indices.is_empty() {
        return Err(ImportError::InvalidData(
            "ASCII STL contains no complete facets".to_string(),
        ));
    }

    let mut mesh = LiteMesh::new("STL_Mesh");
    mesh.primitives.push(primitive);
    mesh.recompute_bbox();

    let mut document = LiteDocument::new(source_format, mode);
    if let Some(path) = source_path {
        document.metadata.source_path = Some(path.display().to_string());
    }
    document.meshes.push(mesh);
    document.nodes.push(LiteNode::new("STL_Mesh", Some(0)));
    document.refresh_metadata();
    Ok(document)
}

fn read_vec3(bytes: &[u8], offset: usize) -> Result<[f32; 3], ImportError> {
    let end = offset + 12;
    if end > bytes.len() {
        return Err(ImportError::InvalidData(
            "binary STL vector is truncated".to_string(),
        ));
    }

    let x = read_f32(bytes, offset)?;
    let y = read_f32(bytes, offset + 4)?;
    let z = read_f32(bytes, offset + 8)?;
    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
        return Err(ImportError::InvalidData(
            "binary STL contains non-finite float".to_string(),
        ));
    }
    Ok([x, y, z])
}

fn has_finite_binary_stl_vectors(bytes: &[u8], payload_len: usize) -> bool {
    let Some(count_bytes) = bytes.get(STL_HEADER_LEN..STL_MIN_LEN) else {
        return false;
    };
    let Ok(count_bytes) = <[u8; STL_COUNT_LEN]>::try_from(count_bytes) else {
        return false;
    };
    let triangle_count = u32::from_le_bytes(count_bytes);
    let mut cursor = STL_MIN_LEN;

    for _ in 0..triangle_count {
        for _ in 0..4 {
            if read_vec3(bytes, cursor).is_err() {
                return false;
            }
            cursor += 12;
        }
        cursor += 2;
        if cursor > payload_len {
            return false;
        }
    }

    cursor == payload_len
}

fn read_f32(bytes: &[u8], offset: usize) -> Result<f32, ImportError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| ImportError::InvalidData("binary STL float is truncated".to_string()))?;
    Ok(f32::from_le_bytes(value.try_into().map_err(|_| {
        ImportError::InvalidData("binary STL float has invalid width".to_string())
    })?))
}

fn parse_ascii_f32(token: &str, line_index: usize, field: &str) -> Result<f32, ImportError> {
    let value = token.parse::<f32>().map_err(|error| {
        ImportError::InvalidData(format!(
            "line {}: invalid ASCII STL {field}: {error}",
            line_index + 1
        ))
    })?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ImportError::InvalidData(format!(
            "line {}: ASCII STL {field} is not finite",
            line_index + 1
        )))
    }
}

fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}
