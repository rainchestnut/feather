//! Native parser for AP242 tessellated STEP entities.
//!
//! This module handles visual STEP data that is already represented as
//! coordinates and triangle indices. It does not attempt to tessellate B-Rep
//! surfaces, which belongs in a real geometry-kernel implementation.

use std::collections::{BTreeMap, HashMap};

use crate::document::{LiteDocument, LiteMesh, LiteNode, LitePrimitive};
use crate::importer::{ImportError, ImportOptions};

use super::step_assembly::{apply_step_assembly, has_step_assembly_relationships};
use super::step_part21::{
    StepRecord, parse_integer_list, parse_nested_integer_lists, parse_optional_usize,
    parse_reference, parse_vec3_list, split_top_level_args,
};
use super::step_style::{StepColorKey, collect_step_materials, collect_styled_item_colors};

/// Imports AP242 tessellated faces when present in a STEP Part 21 file.
pub fn import_tessellated_step(
    records: &[StepRecord],
    source_path: Option<&std::path::Path>,
    options: &ImportOptions,
) -> Result<Option<LiteDocument>, ImportError> {
    let coordinate_lists = collect_coordinate_lists(records)?;
    if coordinate_lists.is_empty() {
        return Ok(None);
    }

    let styled_face_colors = collect_styled_item_colors(records)?;
    if options.load_assembly && has_step_assembly_relationships(records) {
        return import_tessellated_assembly(
            records,
            source_path,
            options,
            &coordinate_lists,
            &styled_face_colors,
        );
    }
    let mut primitive_indices = BTreeMap::<PrimitiveKey, Vec<u32>>::new();
    for record in records {
        match record.kind.as_str() {
            "TRIANGULATED_FACE" => collect_triangulated_face(
                record,
                &coordinate_lists,
                &styled_face_colors,
                &mut primitive_indices,
            )?,
            "COMPLEX_TRIANGULATED_FACE" => collect_complex_triangulated_face(
                record,
                &coordinate_lists,
                &styled_face_colors,
                &mut primitive_indices,
            )?,
            _ => {}
        }
    }

    if primitive_indices.is_empty() {
        return Ok(None);
    }

    let mut document = LiteDocument::new("STEP", "step-ap242-tessellated");
    document.materials =
        collect_step_materials(primitive_indices.keys().filter_map(|key| key.color));

    let mut mesh = LiteMesh::new("STEP_Tessellation");
    for (primitive_key, indices) in primitive_indices {
        let Some(positions) = coordinate_lists.get(&primitive_key.coordinates_id) else {
            return Err(ImportError::InvalidData(format!(
                "triangulated face references missing coordinates list #{}",
                primitive_key.coordinates_id
            )));
        };
        let material = primitive_key.color.and_then(|color| {
            document
                .materials
                .iter()
                .position(|material| color.matches(material.base_color))
        });
        let mut primitive = LitePrimitive::new(material);
        primitive.positions = positions.clone();
        primitive.indices = indices;
        mesh.primitives.push(primitive);
    }
    mesh.recompute_bbox();

    document.metadata.has_brep = true;
    document.metadata.brep_preserved = false;
    if let Some(path) = source_path {
        document.metadata.source_path = Some(path.display().to_string());
    }
    document.meshes.push(mesh);
    document
        .nodes
        .push(LiteNode::new("STEP_Tessellation", Some(0)));
    document.refresh_metadata();
    Ok(Some(document))
}

fn import_tessellated_assembly(
    records: &[StepRecord],
    source_path: Option<&std::path::Path>,
    options: &ImportOptions,
    coordinate_lists: &HashMap<usize, Vec<[f32; 3]>>,
    styled_face_colors: &HashMap<usize, StepColorKey>,
) -> Result<Option<LiteDocument>, ImportError> {
    let mut face_primitives = Vec::<(usize, BTreeMap<PrimitiveKey, Vec<u32>>)>::new();
    for record in records {
        let mut primitives = BTreeMap::new();
        match record.kind.as_str() {
            "TRIANGULATED_FACE" => collect_triangulated_face(
                record,
                coordinate_lists,
                styled_face_colors,
                &mut primitives,
            )?,
            "COMPLEX_TRIANGULATED_FACE" => collect_complex_triangulated_face(
                record,
                coordinate_lists,
                styled_face_colors,
                &mut primitives,
            )?,
            _ => continue,
        }
        if !primitives.is_empty() {
            face_primitives.push((record.id, primitives));
        }
    }
    if face_primitives.is_empty() {
        return Ok(None);
    }

    let mut document = LiteDocument::new("STEP", "step-ap242-assembly-tessellated");
    document.materials = collect_step_materials(
        face_primitives
            .iter()
            .flat_map(|(_, primitives)| primitives.keys().filter_map(|key| key.color)),
    );
    let mut face_meshes = HashMap::new();
    for (face_id, primitives) in face_primitives {
        let mut mesh = LiteMesh::new(format!("STEP_Tessellated_{face_id}"));
        for (primitive_key, indices) in primitives {
            let positions = coordinate_lists
                .get(&primitive_key.coordinates_id)
                .ok_or_else(|| {
                    ImportError::InvalidData(format!(
                        "tessellated face references missing coordinates list #{}",
                        primitive_key.coordinates_id
                    ))
                })?;
            let material = primitive_key.color.and_then(|color| {
                document
                    .materials
                    .iter()
                    .position(|material| color.matches(material.base_color))
            });
            let mut primitive = LitePrimitive::new(material);
            primitive.positions = positions.clone();
            primitive.indices = indices;
            mesh.primitives.push(primitive);
        }
        mesh.recompute_bbox();
        let mesh_index = document.meshes.len();
        document.meshes.push(mesh);
        face_meshes.insert(face_id, mesh_index);
    }
    if !apply_step_assembly(&mut document, records, &face_meshes, options)? {
        return Err(ImportError::InvalidData(
            "STEP tessellated assembly has no usable representation relationships".to_string(),
        ));
    }
    document.metadata.has_brep = true;
    document.metadata.brep_preserved = false;
    document
        .metadata
        .warnings
        .push("preserved STEP shape-representation assembly instances".to_string());
    if let Some(path) = source_path {
        document.metadata.source_path = Some(path.display().to_string());
    }
    document.refresh_metadata();
    Ok(Some(document))
}

fn collect_coordinate_lists(
    records: &[StepRecord],
) -> Result<HashMap<usize, Vec<[f32; 3]>>, ImportError> {
    let mut lists = HashMap::new();
    for record in records {
        if !matches!(
            record.kind.as_str(),
            "COORDINATES_LIST" | "CARTESIAN_POINT_LIST_3D"
        ) {
            continue;
        }

        let args = split_top_level_args(&record.args);
        if args.len() < 3 {
            return Err(ImportError::InvalidData(format!(
                "#{id} {kind} expects at least 3 arguments",
                id = record.id,
                kind = record.kind
            )));
        }

        let expected_count = parse_optional_usize(args[1]);
        let positions = parse_vec3_list(args[2]).map_err(|message| {
            ImportError::InvalidData(format!(
                "#{id} {kind} has invalid coordinate list: {message}",
                id = record.id,
                kind = record.kind
            ))
        })?;

        if let Some(expected_count) = expected_count
            && positions.len() != expected_count
        {
            return Err(ImportError::InvalidData(format!(
                "#{id} declares {expected_count} points but contains {actual}",
                id = record.id,
                actual = positions.len()
            )));
        }

        lists.insert(record.id, positions);
    }
    Ok(lists)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PrimitiveKey {
    coordinates_id: usize,
    color: Option<StepColorKey>,
}

fn collect_triangulated_face(
    record: &StepRecord,
    coordinate_lists: &HashMap<usize, Vec<[f32; 3]>>,
    styled_face_colors: &HashMap<usize, StepColorKey>,
    primitive_indices: &mut BTreeMap<PrimitiveKey, Vec<u32>>,
) -> Result<(), ImportError> {
    let args = split_top_level_args(&record.args);
    if args.len() < 7 {
        return Err(ImportError::InvalidData(format!(
            "#{} TRIANGULATED_FACE expects 7 arguments",
            record.id
        )));
    }

    let coordinates_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{} TRIANGULATED_FACE has no coordinates reference",
            record.id
        ))
    })?;
    let positions = coordinate_lists.get(&coordinates_id).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{} references missing coordinates list #{coordinates_id}",
            record.id
        ))
    })?;
    let pnindex = parse_integer_list(args[5]);
    let triangles = parse_integer_list(args[6]);

    if !triangles.len().is_multiple_of(3) {
        return Err(ImportError::InvalidData(format!(
            "#{} TRIANGULATED_FACE triangle index count is not divisible by 3",
            record.id
        )));
    }

    let target = primitive_indices
        .entry(PrimitiveKey {
            coordinates_id,
            color: styled_face_colors.get(&record.id).copied(),
        })
        .or_default();
    for triangle in triangles.chunks_exact(3) {
        for index in triangle {
            target.push(resolve_point_index(
                *index,
                &pnindex,
                positions.len(),
                record.id,
            )?);
        }
    }
    Ok(())
}

fn collect_complex_triangulated_face(
    record: &StepRecord,
    coordinate_lists: &HashMap<usize, Vec<[f32; 3]>>,
    styled_face_colors: &HashMap<usize, StepColorKey>,
    primitive_indices: &mut BTreeMap<PrimitiveKey, Vec<u32>>,
) -> Result<(), ImportError> {
    let args = split_top_level_args(&record.args);
    if args.len() < 8 {
        return Err(ImportError::InvalidData(format!(
            "#{} COMPLEX_TRIANGULATED_FACE expects 8 arguments",
            record.id
        )));
    }

    let coordinates_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{} COMPLEX_TRIANGULATED_FACE has no coordinates reference",
            record.id
        ))
    })?;
    let positions = coordinate_lists.get(&coordinates_id).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{} references missing coordinates list #{coordinates_id}",
            record.id
        ))
    })?;
    let pnindex = parse_integer_list(args[5]);
    let target = primitive_indices
        .entry(PrimitiveKey {
            coordinates_id,
            color: styled_face_colors.get(&record.id).copied(),
        })
        .or_default();

    for strip in parse_nested_integer_lists(args[6]) {
        if strip.len() < 3 {
            continue;
        }
        for offset in 0..strip.len() - 2 {
            let triangle = if offset % 2 == 0 {
                [strip[offset], strip[offset + 1], strip[offset + 2]]
            } else {
                [strip[offset + 1], strip[offset], strip[offset + 2]]
            };
            push_resolved_triangle(target, triangle, &pnindex, positions.len(), record.id)?;
        }
    }

    for fan in parse_nested_integer_lists(args[7]) {
        if fan.len() < 3 {
            continue;
        }
        let root = fan[0];
        for offset in 1..fan.len() - 1 {
            push_resolved_triangle(
                target,
                [root, fan[offset], fan[offset + 1]],
                &pnindex,
                positions.len(),
                record.id,
            )?;
        }
    }

    Ok(())
}

fn push_resolved_triangle(
    target: &mut Vec<u32>,
    triangle: [i64; 3],
    pnindex: &[i64],
    point_count: usize,
    record_id: usize,
) -> Result<(), ImportError> {
    for index in triangle {
        target.push(resolve_point_index(index, pnindex, point_count, record_id)?);
    }
    Ok(())
}

fn resolve_point_index(
    raw_index: i64,
    pnindex: &[i64],
    point_count: usize,
    record_id: usize,
) -> Result<u32, ImportError> {
    if raw_index <= 0 {
        return Err(ImportError::InvalidData(format!(
            "#{record_id} contains non-positive STEP point index {raw_index}"
        )));
    }

    let point_number = if pnindex.is_empty() {
        raw_index
    } else {
        let lookup = (raw_index - 1) as usize;
        *pnindex.get(lookup).ok_or_else(|| {
            ImportError::InvalidData(format!(
                "#{record_id} point index {raw_index} is outside pnindex"
            ))
        })?
    };

    if point_number <= 0 || point_number as usize > point_count {
        return Err(ImportError::InvalidData(format!(
            "#{record_id} resolved point index {point_number} outside 1..={point_count}"
        )));
    }

    Ok((point_number - 1) as u32)
}
