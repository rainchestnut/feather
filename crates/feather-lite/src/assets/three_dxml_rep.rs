//! Readable Dassault 3DXML representation asset importer.
//!
//! 3DXML packages can contain XML `.3DRep` representation files that already
//! hold polygonal preview data. This module imports that open tessellated subset
//! into Feather Lite IR. It intentionally does not attempt to decode binary or
//! encrypted proprietary 3DRep streams.

use std::path::Path;

use crate::document::{LiteDocument, LiteMesh, LiteNode, LitePrimitive};
use crate::importer::ImportError;

/// Returns true when the payload looks like readable XML 3DRep polygon data.
pub fn is_3dxml_rep(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    if !text.trim_start().starts_with('<') {
        return false;
    }

    xml_elements_by_local_names(text, &["rep"])
        .into_iter()
        .any(|rep| {
            xml_child_text(&rep.body, &["positions"])
                .filter(|value| !value.trim().is_empty())
                .is_some()
                && !xml_elements_by_local_names(&rep.body, &["face"]).is_empty()
        })
}

/// Imports a readable XML `.3DRep` payload as a visual-only document.
pub fn import_3dxml_rep_document(
    bytes: &[u8],
    source_format: &str,
    mode: &str,
    source_path: Option<&Path>,
) -> Result<LiteDocument, ImportError> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        ImportError::InvalidData(format!("3DXML 3DRep payload is not UTF-8 XML: {error}"))
    })?;
    let reps = xml_elements_by_local_names(text, &["rep"]);
    if reps.is_empty() {
        return Err(ImportError::InvalidData(
            "3DXML 3DRep payload does not contain readable Rep elements".to_string(),
        ));
    }

    let mut mesh = LiteMesh::new("3DXML_Rep");
    for (rep_index, rep) in reps.iter().enumerate() {
        if !rep_contains_geometry(rep) {
            continue;
        }
        let primitive = import_rep_primitive(rep, rep_index)?;
        mesh.primitives.push(primitive);
    }

    if mesh.primitives.is_empty() {
        return Err(ImportError::InvalidData(
            "3DXML 3DRep payload contains no readable triangle faces".to_string(),
        ));
    }

    mesh.recompute_bbox();

    let mut document = LiteDocument::new(source_format, mode);
    if let Some(path) = source_path {
        document.metadata.source_path = Some(path.display().to_string());
    }
    document.metadata.warnings.push(
        "imported readable XML 3DRep polygonal tessellation; binary/surface 3DRep data was not decoded"
            .to_string(),
    );
    document.meshes.push(mesh);
    document.nodes.push(LiteNode::new("3DXML_Rep", Some(0)));
    document.refresh_metadata();
    Ok(document)
}

fn rep_contains_geometry(rep: &XmlElement) -> bool {
    xml_child_text(&rep.body, &["positions"])
        .filter(|value| !value.trim().is_empty())
        .is_some()
        && !xml_elements_by_local_names(&rep.body, &["face"]).is_empty()
}

fn import_rep_primitive(rep: &XmlElement, rep_index: usize) -> Result<LitePrimitive, ImportError> {
    let positions_text = xml_child_text(&rep.body, &["positions"]).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "3DXML Rep #{rep_index} is missing VertexBuffer Positions"
        ))
    })?;
    let positions = parse_vec3_list(&positions_text, "3DXML Positions")?;
    if positions.is_empty() {
        return Err(ImportError::InvalidData(format!(
            "3DXML Rep #{rep_index} has no vertex positions"
        )));
    }

    let normals = match xml_child_text(&rep.body, &["normals"]) {
        Some(normals_text) if !normals_text.trim().is_empty() => {
            let normals = parse_vec3_list(&normals_text, "3DXML Normals")?;
            if normals.len() != positions.len() {
                return Err(ImportError::InvalidData(format!(
                    "3DXML Rep #{rep_index} has {} normals for {} positions",
                    normals.len(),
                    positions.len()
                )));
            }
            normals
        }
        _ => Vec::new(),
    };

    let faces = xml_elements_by_local_names(&rep.body, &["face"]);
    if faces.is_empty() {
        return Err(ImportError::InvalidData(format!(
            "3DXML Rep #{rep_index} has no Face elements"
        )));
    }

    let mut indices = Vec::new();
    for (face_index, face) in faces.iter().enumerate() {
        append_face_indices(&mut indices, face, rep_index, face_index)?;
    }
    if indices.is_empty() {
        return Err(ImportError::InvalidData(format!(
            "3DXML Rep #{rep_index} has no triangle indices"
        )));
    }
    validate_indices(&indices, positions.len(), rep_index)?;

    let mut primitive = LitePrimitive::new(None);
    primitive.positions = positions;
    primitive.normals = normals;
    primitive.indices = indices;
    Ok(primitive)
}

fn append_face_indices(
    indices: &mut Vec<u32>,
    face: &XmlElement,
    rep_index: usize,
    face_index: usize,
) -> Result<(), ImportError> {
    if let Some(value) = face_value(face, &["triangles"]) {
        let triangles = parse_index_list(&value, "3DXML Face triangles")?;
        if triangles.len() % 3 != 0 {
            return Err(ImportError::InvalidData(format!(
                "3DXML Rep #{rep_index} Face #{face_index} triangles expects a multiple of 3 indices, found {}",
                triangles.len()
            )));
        }
        for chunk in triangles.chunks_exact(3) {
            push_triangle(indices, chunk[0], chunk[1], chunk[2]);
        }
    }

    if let Some(value) = face_value(face, &["strips"]) {
        for group in parse_index_groups(&value, "3DXML Face strips")? {
            append_strip_indices(indices, &group);
        }
    }

    if let Some(value) = face_value(face, &["fans"]) {
        for group in parse_index_groups(&value, "3DXML Face fans")? {
            append_fan_indices(indices, &group);
        }
    }

    Ok(())
}

fn face_value(face: &XmlElement, names: &[&str]) -> Option<String> {
    find_attribute(&face.attributes, names)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| xml_child_text(&face.body, names))
}

fn append_strip_indices(indices: &mut Vec<u32>, strip: &[u32]) {
    if strip.len() < 3 {
        return;
    }
    for index in 0..strip.len() - 2 {
        let a = strip[index];
        let b = strip[index + 1];
        let c = strip[index + 2];
        if index % 2 == 0 {
            push_triangle(indices, a, b, c);
        } else {
            push_triangle(indices, b, a, c);
        }
    }
}

fn append_fan_indices(indices: &mut Vec<u32>, fan: &[u32]) {
    if fan.len() < 3 {
        return;
    }
    let pivot = fan[0];
    for index in 1..fan.len() - 1 {
        push_triangle(indices, pivot, fan[index], fan[index + 1]);
    }
}

fn push_triangle(indices: &mut Vec<u32>, a: u32, b: u32, c: u32) {
    if a == b || b == c || a == c {
        return;
    }
    indices.extend([a, b, c]);
}

fn validate_indices(
    indices: &[u32],
    position_count: usize,
    rep_index: usize,
) -> Result<(), ImportError> {
    let Some(max_index) = indices.iter().max().copied() else {
        return Ok(());
    };
    if max_index as usize >= position_count {
        return Err(ImportError::InvalidData(format!(
            "3DXML Rep #{rep_index} references vertex index {max_index}, but only {position_count} positions are available"
        )));
    }
    Ok(())
}

fn parse_vec3_list(value: &str, context: &str) -> Result<Vec<[f32; 3]>, ImportError> {
    let values = parse_float_list(value, context)?;
    if values.len() % 3 != 0 {
        return Err(ImportError::InvalidData(format!(
            "{context} expects triples, found {} scalar values",
            values.len()
        )));
    }
    Ok(values
        .chunks_exact(3)
        .map(|chunk| [chunk[0], chunk[1], chunk[2]])
        .collect())
}

fn parse_float_list(value: &str, context: &str) -> Result<Vec<f32>, ImportError> {
    value
        .split(float_separator)
        .filter(|token| !token.is_empty())
        .map(|token| {
            let parsed = token.parse::<f32>().map_err(|error| {
                ImportError::InvalidData(format!(
                    "{context} contains invalid float `{token}`: {error}"
                ))
            })?;
            if !parsed.is_finite() {
                return Err(ImportError::InvalidData(format!(
                    "{context} contains non-finite float `{token}`"
                )));
            }
            Ok(parsed)
        })
        .collect()
}

fn parse_index_groups(value: &str, context: &str) -> Result<Vec<Vec<u32>>, ImportError> {
    let groups = value
        .split([';', '|', '\n'])
        .filter(|group| !group.trim().is_empty())
        .flat_map(split_comma_groups)
        .map(|group| parse_index_list(group, context))
        .collect::<Result<Vec<_>, _>>()?;

    if groups.len() > 1 && groups.iter().all(|group| group.len() == 1) {
        Ok(vec![
            groups
                .into_iter()
                .filter_map(|group| group.into_iter().next())
                .collect(),
        ])
    } else {
        Ok(groups)
    }
}

fn split_comma_groups(group: &str) -> Vec<&str> {
    let comma_groups = group
        .split(',')
        .filter(|candidate| !candidate.trim().is_empty())
        .collect::<Vec<_>>();
    if comma_groups.len() <= 1 {
        return vec![group];
    }

    let parsed_lengths = comma_groups
        .iter()
        .map(|candidate| parse_index_list(candidate, "3DXML comma grouping").map(|list| list.len()))
        .collect::<Result<Vec<_>, _>>();
    if parsed_lengths
        .as_ref()
        .map(|lengths| lengths.iter().all(|length| *length == 1))
        .unwrap_or(false)
    {
        vec![group]
    } else {
        comma_groups
    }
}

fn parse_index_list(value: &str, context: &str) -> Result<Vec<u32>, ImportError> {
    value
        .split(index_separator)
        .filter(|token| !token.is_empty())
        .map(|token| {
            let parsed = token.parse::<i64>().map_err(|error| {
                ImportError::InvalidData(format!(
                    "{context} contains invalid index `{token}`: {error}"
                ))
            })?;
            if parsed < 0 || parsed > u32::MAX as i64 {
                return Err(ImportError::InvalidData(format!(
                    "{context} contains out-of-range index `{token}`"
                )));
            }
            Ok(parsed as u32)
        })
        .collect()
}

fn float_separator(character: char) -> bool {
    character.is_whitespace() || matches!(character, ',' | ';' | '[' | ']' | '(' | ')')
}

fn index_separator(character: char) -> bool {
    character.is_whitespace() || matches!(character, ',' | '[' | ']' | '(' | ')')
}

#[derive(Debug)]
struct XmlStartTag {
    name: String,
    attributes: Vec<(String, String)>,
    self_closing: bool,
}

#[derive(Debug)]
struct XmlElement {
    attributes: Vec<(String, String)>,
    body: String,
}

fn xml_elements_by_local_names(text: &str, names: &[&str]) -> Vec<XmlElement> {
    let mut elements = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = text[cursor..].find('<') {
        let start = cursor + relative_start;
        let Some(relative_end) = text[start..].find('>') else {
            break;
        };
        let end = start + relative_end;
        let content = text[start + 1..end].trim();
        cursor = end + 1;

        if content.is_empty()
            || content.starts_with('/')
            || content.starts_with('?')
            || content.starts_with('!')
            || content.starts_with("!--")
        {
            continue;
        }

        let Some(tag) = parse_xml_start_tag(content) else {
            continue;
        };
        let tag_name = xml_local_name(&tag.name);
        if !names.iter().any(|name| tag_name.eq_ignore_ascii_case(name)) {
            continue;
        }

        let body = if tag.self_closing {
            String::new()
        } else if let Some((body_end, after_end)) = find_xml_element_end(text, cursor, &tag_name) {
            let body_start = end + 1;
            let body = text[body_start..body_end].to_string();
            cursor = after_end;
            body
        } else {
            String::new()
        };
        elements.push(XmlElement {
            attributes: tag.attributes,
            body,
        });
    }

    elements
}

fn find_xml_element_end(text: &str, cursor: usize, local_name: &str) -> Option<(usize, usize)> {
    let mut scan = cursor;
    let mut depth = 1_usize;

    while let Some(relative_start) = text[scan..].find('<') {
        let start = scan + relative_start;
        let Some(relative_end) = text[start..].find('>') else {
            break;
        };
        let end = start + relative_end;
        let content = text[start + 1..end].trim();
        scan = end + 1;

        if content.is_empty()
            || content.starts_with('?')
            || content.starts_with('!')
            || content.starts_with("!--")
        {
            continue;
        }

        if let Some(close_name) = content.strip_prefix('/') {
            if xml_local_name(close_name.trim()).eq_ignore_ascii_case(local_name) {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((start, scan));
                }
            }
            continue;
        }

        let Some(tag) = parse_xml_start_tag(content) else {
            continue;
        };
        if !tag.self_closing && xml_local_name(&tag.name).eq_ignore_ascii_case(local_name) {
            depth += 1;
        }
    }

    None
}

fn xml_child_text(body: &str, names: &[&str]) -> Option<String> {
    let mut cursor = 0;
    while let Some(relative_start) = body[cursor..].find('<') {
        let start = cursor + relative_start;
        let Some(relative_end) = body[start..].find('>') else {
            break;
        };
        let end = start + relative_end;
        let content = body[start + 1..end].trim();
        cursor = end + 1;

        if content.is_empty()
            || content.starts_with('/')
            || content.starts_with('?')
            || content.starts_with('!')
            || content.starts_with("!--")
        {
            continue;
        }

        let Some(tag) = parse_xml_start_tag(content) else {
            continue;
        };
        let tag_name = xml_local_name(&tag.name);
        if !names.iter().any(|name| tag_name.eq_ignore_ascii_case(name)) {
            continue;
        }
        if tag.self_closing {
            return Some(String::new());
        }
        let (body_end, _after_end) = find_xml_element_end(body, cursor, &tag_name)?;
        return Some(decode_xml_entities(body[cursor..body_end].trim()));
    }
    None
}

fn parse_xml_start_tag(content: &str) -> Option<XmlStartTag> {
    let content = content.trim();
    let self_closing = content.ends_with('/');
    let content = content.trim_end_matches('/').trim_end();
    let name_end = content
        .find(|character: char| character.is_whitespace())
        .unwrap_or(content.len());
    if name_end == 0 {
        return None;
    }

    let name = content[..name_end].to_string();
    let attributes = parse_xml_attributes(&content[name_end..]);
    Some(XmlStartTag {
        name,
        attributes,
        self_closing,
    })
}

fn parse_xml_attributes(input: &str) -> Vec<(String, String)> {
    let bytes = input.as_bytes();
    let mut attributes = Vec::new();
    let mut cursor = 0;

    while cursor < bytes.len() {
        while bytes
            .get(cursor)
            .map(|byte| byte.is_ascii_whitespace())
            .unwrap_or(false)
        {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }

        let key_start = cursor;
        while cursor < bytes.len()
            && !bytes[cursor].is_ascii_whitespace()
            && bytes[cursor] != b'='
            && bytes[cursor] != b'/'
        {
            cursor += 1;
        }
        if key_start == cursor {
            cursor += 1;
            continue;
        }
        let key = &input[key_start..cursor];

        while bytes
            .get(cursor)
            .map(|byte| byte.is_ascii_whitespace())
            .unwrap_or(false)
        {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'=') {
            attributes.push((key.to_string(), String::new()));
            continue;
        }
        cursor += 1;
        while bytes
            .get(cursor)
            .map(|byte| byte.is_ascii_whitespace())
            .unwrap_or(false)
        {
            cursor += 1;
        }

        let Some(quote) = bytes.get(cursor).copied() else {
            attributes.push((key.to_string(), String::new()));
            break;
        };
        let value = if quote == b'"' || quote == b'\'' {
            cursor += 1;
            let value_start = cursor;
            while cursor < bytes.len() && bytes[cursor] != quote {
                cursor += 1;
            }
            let value = decode_xml_entities(&input[value_start..cursor]);
            if cursor < bytes.len() {
                cursor += 1;
            }
            value
        } else {
            let value_start = cursor;
            while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            decode_xml_entities(&input[value_start..cursor])
        };
        attributes.push((key.to_string(), value));
    }

    attributes
}

fn find_attribute<'a>(attributes: &'a [(String, String)], names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|candidate| {
        attributes.iter().find_map(|(name, value)| {
            let local = xml_local_name(name).to_ascii_lowercase();
            (local == *candidate).then_some(value.as_str())
        })
    })
}

fn decode_xml_entities(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn xml_local_name(name: &str) -> String {
    name.rsplit_once(':')
        .map(|(_, local)| local)
        .unwrap_or(name)
        .trim()
        .to_string()
}
