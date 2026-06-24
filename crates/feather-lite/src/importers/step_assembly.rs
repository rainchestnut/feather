//! STEP shape-representation assembly hierarchy and instance transforms.
//!
//! Geometry remains owned by the B-Rep importer. This module maps reusable
//! solid meshes into a validated representation DAG and expands only scene
//! nodes, preserving mesh reuse for repeated part and subassembly instances.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use crate::document::{LiteDocument, LiteNode, Transform, identity_transform};
use crate::importer::{ImportError, ImportOptions};

use super::step_brep::resolve_axis2_transform;
use super::step_part21::{
    StepRecord, parse_reference, parse_references, parse_step_string, split_top_level_args,
};

/// One reusable STEP shape representation and its directly owned solid meshes.
struct ShapeRepresentation {
    name: String,
    meshes: Vec<usize>,
}

/// One transformed parent-to-child shape-representation occurrence.
struct RepresentationEdge {
    child: usize,
    name: String,
    transform: Transform,
}

/// Applies a STEP representation assembly to already tessellated solid meshes.
///
/// Returns `true` when an assembly relationship graph was found and emitted.
pub(super) fn apply_step_assembly(
    document: &mut LiteDocument,
    records: &[StepRecord],
    solid_meshes: &HashMap<usize, usize>,
    options: &ImportOptions,
) -> Result<bool, ImportError> {
    if !options.load_assembly || solid_meshes.is_empty() {
        return Ok(false);
    }
    let record_map = records
        .iter()
        .map(|record| (record.id, record))
        .collect::<HashMap<_, _>>();
    let representations = collect_shape_representations(records, solid_meshes)?;
    let edges = collect_representation_edges(records, &record_map, &representations)?;
    if edges.is_empty() {
        return Ok(false);
    }

    let (roots, active_representations) = validate_representation_dag(&representations, &edges)?;
    document.nodes.clear();
    let mut queue = VecDeque::<(usize, usize)>::new();
    for representation_id in roots {
        let representation = representations.get(&representation_id).ok_or_else(|| {
            ImportError::InvalidData(format!(
                "STEP assembly references missing representation #{representation_id}"
            ))
        })?;
        let node_index = append_representation_node(
            document,
            representation_id,
            &representation.name,
            identity_transform(),
            representation,
            options.limits.max_step_assembly_nodes,
        )?;
        queue.push_back((representation_id, node_index));
    }

    while let Some((representation_id, parent_node)) = queue.pop_front() {
        for edge in edges.get(&representation_id).into_iter().flatten() {
            let child = representations.get(&edge.child).ok_or_else(|| {
                ImportError::InvalidData(format!(
                    "STEP assembly references missing child representation #{}",
                    edge.child
                ))
            })?;
            let name = if edge.name.is_empty() {
                child.name.as_str()
            } else {
                edge.name.as_str()
            };
            let child_node = append_representation_node(
                document,
                edge.child,
                name,
                edge.transform,
                child,
                options.limits.max_step_assembly_nodes,
            )?;
            document.nodes[parent_node].children.push(child_node);
            queue.push_back((edge.child, child_node));
        }
    }

    if active_representations.is_empty() || document.nodes.is_empty() {
        return Err(ImportError::InvalidData(
            "STEP assembly contains no reachable representations".to_string(),
        ));
    }
    append_unreferenced_mesh_nodes(document, options.limits.max_step_assembly_nodes)?;
    Ok(true)
}

/// Returns whether the STEP data declares transformed representation relationships.
pub(super) fn has_step_assembly_relationships(records: &[StepRecord]) -> bool {
    records.iter().any(|record| {
        record.kind == "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION"
            || record
                .component("REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION")
                .is_some()
    })
}

/// Collects reusable geometric representations and their directly owned meshes.
fn collect_shape_representations(
    records: &[StepRecord],
    solid_meshes: &HashMap<usize, usize>,
) -> Result<BTreeMap<usize, ShapeRepresentation>, ImportError> {
    let mut representations = BTreeMap::new();
    for record in records {
        if !is_shape_representation(&record.kind) {
            continue;
        }
        let args = split_top_level_args(&record.args);
        if args.len() < 3 {
            return Err(ImportError::InvalidData(format!(
                "#{} {} expects at least three arguments",
                record.id, record.kind
            )));
        }
        let name = parse_step_string(args[0])
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("STEP_Representation_{}", record.id));
        let meshes = parse_references(args[1])
            .into_iter()
            .filter_map(|item_id| solid_meshes.get(&item_id).copied())
            .collect();
        representations.insert(record.id, ShapeRepresentation { name, meshes });
    }
    apply_product_names(records, &mut representations);
    Ok(representations)
}

/// Replaces generated representation labels with names from product definitions.
fn apply_product_names(
    records: &[StepRecord],
    representations: &mut BTreeMap<usize, ShapeRepresentation>,
) {
    let record_map = records
        .iter()
        .map(|record| (record.id, record))
        .collect::<HashMap<_, _>>();
    for record in records {
        if record.kind != "SHAPE_DEFINITION_REPRESENTATION" {
            continue;
        }
        let args = split_top_level_args(&record.args);
        if args.len() < 2 {
            continue;
        }
        let (Some(definition_id), Some(representation_id)) =
            (parse_reference(args[0]), parse_reference(args[1]))
        else {
            continue;
        };
        let Some(representation) = representations.get_mut(&representation_id) else {
            continue;
        };
        if !representation.name.starts_with("STEP_Representation_") {
            continue;
        }
        if let Some(name) = resolve_product_name(definition_id, &record_map) {
            representation.name = name;
        }
    }
}

/// Follows the standard product-definition chain to a displayable product name.
fn resolve_product_name(
    definition_id: usize,
    records: &HashMap<usize, &StepRecord>,
) -> Option<String> {
    let definition = records.get(&definition_id)?;
    if definition.kind != "PRODUCT_DEFINITION_SHAPE" {
        return None;
    }
    let definition_args = split_top_level_args(&definition.args);
    let product_definition_id = parse_reference(definition_args.get(2)?)?;
    let product_definition = records.get(&product_definition_id)?;
    if product_definition.kind != "PRODUCT_DEFINITION" {
        return None;
    }
    let product_definition_args = split_top_level_args(&product_definition.args);
    let formation_id = parse_reference(product_definition_args.get(2)?)?;
    let formation = records.get(&formation_id)?;
    if !formation.kind.starts_with("PRODUCT_DEFINITION_FORMATION") {
        return None;
    }
    let formation_args = split_top_level_args(&formation.args);
    let product_id = parse_reference(formation_args.get(2)?)?;
    let product = records.get(&product_id)?;
    if product.kind != "PRODUCT" {
        return None;
    }
    let product_args = split_top_level_args(&product.args);
    [product_args.get(1), product_args.first()]
        .into_iter()
        .flatten()
        .find_map(|value| parse_step_string(value).filter(|value| !value.is_empty()))
}

/// Returns whether an entity can own supported B-Rep or tessellated geometry.
fn is_shape_representation(kind: &str) -> bool {
    matches!(
        kind,
        "SHAPE_REPRESENTATION"
            | "ADVANCED_BREP_SHAPE_REPRESENTATION"
            | "TESSELLATED_SHAPE_REPRESENTATION"
            | "MANIFOLD_SURFACE_SHAPE_REPRESENTATION"
            | "GEOMETRICALLY_BOUNDED_SURFACE_SHAPE_REPRESENTATION"
    )
}

/// Resolves transformed parent-child occurrences between shape representations.
fn collect_representation_edges(
    records: &[StepRecord],
    record_map: &HashMap<usize, &StepRecord>,
    representations: &BTreeMap<usize, ShapeRepresentation>,
) -> Result<BTreeMap<usize, Vec<RepresentationEdge>>, ImportError> {
    let mut edges = BTreeMap::<usize, Vec<RepresentationEdge>>::new();
    let occurrence_names = collect_occurrence_names(records, record_map);
    for record in records {
        let Some((mut name, parent, child, transform_id)) = representation_relationship(record)?
        else {
            continue;
        };
        if name.is_empty()
            && let Some(occurrence_name) = occurrence_names.get(&record.id)
        {
            name = occurrence_name.clone();
        }
        if !representations.contains_key(&parent) && !representations.contains_key(&child) {
            continue;
        }
        if !representations.contains_key(&parent) || !representations.contains_key(&child) {
            return Err(ImportError::InvalidData(format!(
                "#{} STEP assembly relationship references an unknown shape representation",
                record.id
            )));
        }
        let transform_record = record_map.get(&transform_id).copied().ok_or_else(|| {
            ImportError::InvalidData(format!(
                "STEP assembly relationship references missing entity #{transform_id}"
            ))
        })?;
        let transform = resolve_representation_transform(transform_record, record_map)?;
        edges.entry(parent).or_default().push(RepresentationEdge {
            child,
            name,
            transform,
        });
    }
    Ok(edges)
}

/// Maps representation relationships to NAUO occurrence names when available.
fn collect_occurrence_names(
    records: &[StepRecord],
    record_map: &HashMap<usize, &StepRecord>,
) -> HashMap<usize, String> {
    let mut names = HashMap::new();
    for record in records {
        if record.kind != "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION" {
            continue;
        }
        let args = split_top_level_args(&record.args);
        if args.len() < 2 {
            continue;
        }
        let (Some(relationship_id), Some(shape_id)) =
            (parse_reference(args[0]), parse_reference(args[1]))
        else {
            continue;
        };
        let Some(shape) = record_map.get(&shape_id) else {
            continue;
        };
        let shape_args = split_top_level_args(&shape.args);
        let Some(occurrence_id) = shape_args.get(2).and_then(|value| parse_reference(value)) else {
            continue;
        };
        let Some(occurrence) = record_map.get(&occurrence_id) else {
            continue;
        };
        if occurrence.kind != "NEXT_ASSEMBLY_USAGE_OCCURRENCE" {
            continue;
        }
        let occurrence_args = split_top_level_args(&occurrence.args);
        let name = [
            occurrence_args.get(1),
            occurrence_args.get(5),
            occurrence_args.first(),
        ]
        .into_iter()
        .flatten()
        .find_map(|value| parse_step_string(value).filter(|value| !value.is_empty()));
        if let Some(name) = name {
            names.insert(relationship_id, name);
        }
    }
    names
}

/// Parses simple or complex transformed representation relationship syntax.
fn representation_relationship(
    record: &StepRecord,
) -> Result<Option<(String, usize, usize, usize)>, ImportError> {
    if let (Some(base), Some(transformation)) = (
        record.component("REPRESENTATION_RELATIONSHIP"),
        record.component("REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION"),
    ) {
        let base_args = split_top_level_args(&base.args);
        let transform_args = split_top_level_args(&transformation.args);
        if base_args.len() < 4 || transform_args.is_empty() {
            return Err(ImportError::InvalidData(format!(
                "#{} STEP representation relationship is incomplete",
                record.id
            )));
        }
        return Ok(Some((
            parse_step_string(base_args[0]).unwrap_or_default(),
            require_reference(base_args[2], record.id, "parent representation")?,
            require_reference(base_args[3], record.id, "child representation")?,
            require_reference(transform_args[0], record.id, "transformation operator")?,
        )));
    }
    if record.kind != "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION" {
        return Ok(None);
    }
    let args = split_top_level_args(&record.args);
    if args.len() < 5 {
        return Err(ImportError::InvalidData(format!(
            "#{} REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION expects five arguments",
            record.id
        )));
    }
    Ok(Some((
        parse_step_string(args[0]).unwrap_or_default(),
        require_reference(args[2], record.id, "parent representation")?,
        require_reference(args[3], record.id, "child representation")?,
        require_reference(args[4], record.id, "transformation operator")?,
    )))
}

/// Parses a required assembly reference with relationship-specific diagnostics.
fn require_reference(value: &str, record_id: usize, relation: &str) -> Result<usize, ImportError> {
    parse_reference(value).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{record_id} STEP assembly has no {relation} reference"
        ))
    })
}

/// Resolves a rigid item-defined transform from source into target placement.
fn resolve_representation_transform(
    record: &StepRecord,
    records: &HashMap<usize, &StepRecord>,
) -> Result<Transform, ImportError> {
    if record.kind != "ITEM_DEFINED_TRANSFORMATION" {
        return Err(ImportError::TessellationUnsupported {
            format: "STEP".to_string(),
            reason: format!(
                "#{} uses unsupported assembly transform {}; ITEM_DEFINED_TRANSFORMATION is required",
                record.id, record.kind
            ),
        });
    }
    let args = split_top_level_args(&record.args);
    if args.len() < 4 {
        return Err(ImportError::InvalidData(format!(
            "#{} ITEM_DEFINED_TRANSFORMATION expects four arguments",
            record.id
        )));
    }
    let target_id = require_reference(args[2], record.id, "target placement")?;
    let source_id = require_reference(args[3], record.id, "source placement")?;
    let target = resolve_axis2_transform(target_id, records)?;
    let source = resolve_axis2_transform(source_id, records)?;
    Ok(multiply_transforms(target, invert_rigid_transform(source)))
}

/// Validates the representation graph and returns deterministic root order.
fn validate_representation_dag(
    representations: &BTreeMap<usize, ShapeRepresentation>,
    edges: &BTreeMap<usize, Vec<RepresentationEdge>>,
) -> Result<(Vec<usize>, BTreeSet<usize>), ImportError> {
    let mut active = representations
        .iter()
        .filter_map(|(id, representation)| (!representation.meshes.is_empty()).then_some(*id))
        .collect::<BTreeSet<_>>();
    for (parent, children) in edges {
        active.insert(*parent);
        active.extend(children.iter().map(|edge| edge.child));
    }
    let mut indegree = active
        .iter()
        .map(|id| (*id, 0_usize))
        .collect::<BTreeMap<_, _>>();
    for children in edges.values() {
        for edge in children {
            *indegree.get_mut(&edge.child).ok_or_else(|| {
                ImportError::InvalidData(format!(
                    "STEP assembly references missing representation #{}",
                    edge.child
                ))
            })? += 1;
        }
    }
    let roots = indegree
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
        .collect::<Vec<_>>();
    let mut queue = VecDeque::from(roots.clone());
    let mut visited = 0_usize;
    while let Some(parent) = queue.pop_front() {
        visited += 1;
        for edge in edges.get(&parent).into_iter().flatten() {
            let degree = indegree.get_mut(&edge.child).ok_or_else(|| {
                ImportError::InvalidData(format!(
                    "STEP assembly references missing representation #{}",
                    edge.child
                ))
            })?;
            *degree -= 1;
            if *degree == 0 {
                queue.push_back(edge.child);
            }
        }
    }
    if visited != active.len() {
        return Err(ImportError::InvalidData(
            "STEP assembly relationship cycle detected".to_string(),
        ));
    }
    Ok((roots, active))
}

/// Appends one representation instance while preserving direct mesh reuse.
fn append_representation_node(
    document: &mut LiteDocument,
    representation_id: usize,
    name: &str,
    transform: Transform,
    representation: &ShapeRepresentation,
    node_limit: usize,
) -> Result<usize, ImportError> {
    let node_index = append_node(
        document,
        name,
        representation.meshes.first().copied(),
        node_limit,
    )?;
    document.nodes[node_index].transform = transform;
    document.nodes[node_index].source_id = Some(format!("#{representation_id}"));
    for (mesh_offset, mesh) in representation.meshes.iter().copied().enumerate().skip(1) {
        let child = append_node(
            document,
            &format!("{name}_Mesh_{}", mesh_offset + 1),
            Some(mesh),
            node_limit,
        )?;
        document.nodes[node_index].children.push(child);
    }
    Ok(node_index)
}

/// Appends one bounded scene node and reports the attempted count on overflow.
fn append_node(
    document: &mut LiteDocument,
    name: &str,
    mesh: Option<usize>,
    limit: usize,
) -> Result<usize, ImportError> {
    let actual = document.nodes.len().saturating_add(1);
    if actual > limit {
        return Err(ImportError::ResourceLimitExceeded {
            resource: "STEP assembly nodes",
            limit,
            actual,
        });
    }
    let index = document.nodes.len();
    document.nodes.push(LiteNode::new(name, mesh));
    Ok(index)
}

/// Keeps valid geometry outside the relationship graph reachable and bounded.
fn append_unreferenced_mesh_nodes(
    document: &mut LiteDocument,
    node_limit: usize,
) -> Result<(), ImportError> {
    let mut referenced = vec![false; document.meshes.len()];
    for node in &document.nodes {
        if let Some(mesh) = node.mesh
            && mesh < referenced.len()
        {
            referenced[mesh] = true;
        }
    }
    let unreferenced = referenced
        .iter()
        .enumerate()
        .filter_map(|(mesh, referenced)| (!referenced).then_some(mesh))
        .collect::<Vec<_>>();
    for mesh in unreferenced {
        let name = document.meshes[mesh].name.clone();
        append_node(document, &name, Some(mesh), node_limit)?;
    }
    Ok(())
}

/// Inverts an orthonormal affine transform without a general matrix solver.
fn invert_rigid_transform(transform: Transform) -> Transform {
    let mut inverse = identity_transform();
    for column in 0..3 {
        for row in 0..3 {
            inverse[column][row] = transform[row][column];
        }
    }
    let inverse_translation = [
        -(0..3)
            .map(|column| inverse[column][0] * transform[3][column])
            .sum::<f32>(),
        -(0..3)
            .map(|column| inverse[column][1] * transform[3][column])
            .sum::<f32>(),
        -(0..3)
            .map(|column| inverse[column][2] * transform[3][column])
            .sum::<f32>(),
    ];
    inverse[3][0..3].copy_from_slice(&inverse_translation);
    inverse
}

/// Multiplies column-major transforms in scene traversal order.
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
