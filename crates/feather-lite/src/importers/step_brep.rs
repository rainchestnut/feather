//! Native tessellation for the supported STEP B-Rep topology subset.
//!
//! The importer resolves `ADVANCED_FACE` boundaries through `EDGE_LOOP` or
//! `POLY_LOOP`, validates supported analytic curve geometry, and triangulates
//! planar, cylindrical, conical, spherical, and ring-toroidal surfaces.
//! Outer and inner bounds are triangulated in a validated parameter domain;
//! unsupported or invalid topology is rejected before any geometry is emitted.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;

use crate::document::{LiteDocument, LiteMesh, LiteNode, LitePrimitive, Transform};
use crate::importer::{ImportError, ImportOptions};

use super::step_assembly::apply_step_assembly;
use super::step_part21::{
    StepRecord, parse_float_list, parse_integer_list, parse_reference, parse_references,
    parse_step_string, split_top_level_args,
};
use super::step_style::{StepColorKey, collect_step_materials, collect_styled_item_colors};
use super::step_units::StepResolvedUnit;

const GEOMETRY_EPSILON: f32 = 1.0e-6;

/// Orthonormal basis resolved from one STEP `AXIS2_PLACEMENT_3D`.
#[derive(Clone, Copy)]
struct AxisPlacement {
    origin: [f32; 3],
    axis: [f32; 3],
    x_axis: [f32; 3],
    y_axis: [f32; 3],
}

/// Analytic ellipse parameters resolved from one STEP `ELLIPSE`.
#[derive(Clone, Copy)]
struct EllipseGeometry {
    placement: AxisPlacement,
    semi_axis_1: f32,
    semi_axis_2: f32,
}

/// Analytic circle parameters resolved from one STEP `CIRCLE`.
#[derive(Clone, Copy)]
struct CircleGeometry {
    placement: AxisPlacement,
    radius: f32,
}

/// STEP B-Spline curve data normalized for bounded de Boor evaluation.
struct BsplineCurveGeometry {
    record_id: usize,
    degree: usize,
    control_points: Vec<[f64; 4]>,
    knots: Vec<f64>,
    parameter_domain: Range<f64>,
    start_parameter: f64,
    end_parameter: f64,
    characteristic_length: f32,
}

/// Analytic sphere parameters resolved from one STEP `SPHERICAL_SURFACE`.
#[derive(Clone, Copy)]
struct SphereGeometry {
    placement: AxisPlacement,
    radius: f32,
}

/// Analytic ring-torus parameters resolved from one STEP `TOROIDAL_SURFACE`.
#[derive(Clone, Copy)]
struct TorusGeometry {
    placement: AxisPlacement,
    major_radius: f32,
    minor_radius: f32,
}

/// One resolved STEP face boundary and its range in the flattened vertex array.
struct ResolvedFaceLoop {
    loop_id: usize,
    range: Range<usize>,
}

/// Outer boundary followed by zero or more inner boundaries for one STEP face.
struct ResolvedFaceBoundaries {
    positions: Vec<[f32; 3]>,
    loops: Vec<ResolvedFaceLoop>,
}

impl ResolvedFaceBoundaries {
    fn hole_indices(&self) -> Vec<usize> {
        self.loops
            .iter()
            .skip(1)
            .map(|boundary| boundary.range.start)
            .collect()
    }
}

/// Supported analytic surface-of-revolution families.
#[derive(Clone, Copy)]
enum RevolvedSurfaceKind {
    Cylinder,
    Cone,
}

impl RevolvedSurfaceKind {
    fn step_name(self) -> &'static str {
        match self {
            Self::Cylinder => "CYLINDRICAL_SURFACE",
            Self::Cone => "CONICAL_SURFACE",
        }
    }
}

/// Analytic surface-of-revolution parameters used for validation and UV projection.
#[derive(Clone, Copy)]
struct RevolvedSurfaceGeometry {
    placement: AxisPlacement,
    reference_radius: f32,
    radial_slope: f32,
    kind: RevolvedSurfaceKind,
}

impl RevolvedSurfaceGeometry {
    fn radius_at_height(self, height: f32) -> f32 {
        self.reference_radius + height * self.radial_slope
    }
}

/// Faces owned by one reusable STEP solid, or one fallback ungrouped face set.
struct BrepFaceGroup<'a> {
    solid_id: Option<usize>,
    name: String,
    faces: Vec<&'a StepRecord>,
}

/// Imports supported STEP `ADVANCED_FACE` B-Rep geometry when present.
pub fn import_brep_step(
    records: &[StepRecord],
    source_path: Option<&std::path::Path>,
    options: &ImportOptions,
    plane_angle_unit: Option<&StepResolvedUnit>,
) -> Result<Option<LiteDocument>, ImportError> {
    let record_map = build_record_map(records)?;
    let face_groups = select_brep_face_groups(records, &record_map)?;
    if face_groups.is_empty() {
        return Ok(None);
    }

    let styled_face_colors = collect_styled_item_colors(records)?;
    let mut grouped_primitives = Vec::with_capacity(face_groups.len());
    for group in face_groups {
        let mut primitives = BTreeMap::<Option<StepColorKey>, LitePrimitive>::new();
        for face in group.faces {
            let color = styled_face_colors.get(&face.id).copied();
            append_brep_face(
                primitives
                    .entry(color)
                    .or_insert_with(|| LitePrimitive::new(None)),
                face,
                &record_map,
                options,
                plane_angle_unit,
            )?;
        }
        grouped_primitives.push((group.solid_id, group.name, primitives));
    }
    if grouped_primitives.iter().all(|(_, _, primitives)| {
        primitives
            .values()
            .all(|primitive| primitive.indices.is_empty())
    }) {
        return Err(ImportError::InvalidData(
            "STEP B-Rep contains no tessellatable faces".to_string(),
        ));
    }

    let mut document = LiteDocument::new("STEP", "step-brep-tessellated");
    document.metadata.has_brep = true;
    document.metadata.brep_preserved = false;
    document.metadata.warnings.push(
        "tessellated supported STEP ADVANCED_FACE topology; source B-Rep was not preserved"
            .to_string(),
    );
    if let Some(path) = source_path {
        document.metadata.source_path = Some(path.display().to_string());
    }
    document.materials = collect_step_materials(
        grouped_primitives
            .iter()
            .flat_map(|(_, _, primitives)| primitives.keys().filter_map(|color| *color)),
    );
    let mut solid_meshes = HashMap::new();
    for (solid_id, name, primitives) in grouped_primitives {
        let mut mesh = LiteMesh::new(name);
        for (color, mut primitive) in primitives {
            primitive.material = color.and_then(|color| {
                document
                    .materials
                    .iter()
                    .position(|material| color.matches(material.base_color))
            });
            mesh.primitives.push(primitive);
        }
        mesh.recompute_bbox();
        let mesh_index = document.meshes.len();
        document.meshes.push(mesh);
        if let Some(solid_id) = solid_id {
            solid_meshes.insert(solid_id, mesh_index);
        }
    }
    if apply_step_assembly(&mut document, records, &solid_meshes, options)? {
        document.metadata.mode = "step-brep-assembly-tessellated".to_string();
        document
            .metadata
            .warnings
            .push("preserved STEP shape-representation assembly instances".to_string());
    } else {
        for (mesh_index, mesh) in document.meshes.iter().enumerate() {
            document
                .nodes
                .push(LiteNode::new(mesh.name.clone(), Some(mesh_index)));
        }
    }
    document.refresh_metadata();
    Ok(Some(document))
}

fn select_brep_face_groups<'a>(
    records: &'a [StepRecord],
    record_map: &'a HashMap<usize, &'a StepRecord>,
) -> Result<Vec<BrepFaceGroup<'a>>, ImportError> {
    let solids = records
        .iter()
        .filter(|record| record.kind == "MANIFOLD_SOLID_BREP")
        .collect::<Vec<_>>();
    if solids.is_empty() {
        let faces = records
            .iter()
            .filter(|record| record.kind == "ADVANCED_FACE")
            .collect::<Vec<_>>();
        return Ok((!faces.is_empty())
            .then_some(BrepFaceGroup {
                solid_id: None,
                name: "STEP_BRep_Tessellation".to_string(),
                faces,
            })
            .into_iter()
            .collect());
    }

    let mut groups = Vec::with_capacity(solids.len());
    for solid in solids {
        let solid_args = require_args(solid, 2)?;
        let name = parse_step_string(solid_args[0])
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("STEP_Solid_{}", solid.id));
        let shell_id = parse_reference(solid_args[1]).ok_or_else(|| {
            ImportError::InvalidData(format!(
                "#{} MANIFOLD_SOLID_BREP has no shell reference",
                solid.id
            ))
        })?;
        let shell = require_record(record_map, shell_id, "MANIFOLD_SOLID_BREP shell")?;
        if shell.kind != "CLOSED_SHELL" {
            return Err(ImportError::InvalidData(format!(
                "#{shell_id} is {}, expected CLOSED_SHELL",
                shell.kind
            )));
        }
        let shell_args = require_args(shell, 2)?;
        let shell_face_ids = parse_references(shell_args[1]);
        if shell_face_ids.is_empty() {
            return Err(ImportError::InvalidData(format!(
                "#{shell_id} CLOSED_SHELL contains no faces"
            )));
        }
        let mut face_ids = BTreeSet::new();
        for face_id in shell_face_ids {
            let face = require_record(record_map, face_id, "CLOSED_SHELL face")?;
            if face.kind != "ADVANCED_FACE" {
                return Err(unsupported(format!(
                    "#{shell_id} CLOSED_SHELL contains {} face #{face_id}; only ADVANCED_FACE is supported",
                    face.kind
                )));
            }
            face_ids.insert(face_id);
        }
        let faces = face_ids
            .into_iter()
            .map(|face_id| require_record(record_map, face_id, "selected B-Rep face"))
            .collect::<Result<Vec<_>, _>>()?;
        groups.push(BrepFaceGroup {
            solid_id: Some(solid.id),
            name,
            faces,
        });
    }
    Ok(groups)
}

fn build_record_map(records: &[StepRecord]) -> Result<HashMap<usize, &StepRecord>, ImportError> {
    let mut record_map = HashMap::with_capacity(records.len());
    for record in records {
        if record_map.insert(record.id, record).is_some() {
            return Err(ImportError::InvalidData(format!(
                "STEP entity id #{} is duplicated",
                record.id
            )));
        }
    }
    Ok(record_map)
}

fn append_brep_face(
    primitive: &mut LitePrimitive,
    face: &StepRecord,
    records: &HashMap<usize, &StepRecord>,
    options: &ImportOptions,
    plane_angle_unit: Option<&StepResolvedUnit>,
) -> Result<(), ImportError> {
    let args = require_args(face, 4)?;
    let bound_ids = parse_references(args[1]);
    if bound_ids.is_empty() {
        return Err(ImportError::InvalidData(format!(
            "#{} ADVANCED_FACE has no face bounds",
            face.id
        )));
    }
    let boundaries = resolve_face_boundaries(&bound_ids, records, options, face.id)?;

    let surface_id = parse_reference(args[2]).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{} ADVANCED_FACE has no surface reference",
            face.id
        ))
    })?;
    let same_sense = parse_step_bool(args[3], face.id, "ADVANCED_FACE same_sense")?;

    let surface = require_record(records, surface_id, "ADVANCED_FACE surface")?;
    let (mut local_indices, normals) = match surface.kind.as_str() {
        "PLANE" => tessellate_planar_face(&boundaries, surface, same_sense, records, face.id)?,
        "CYLINDRICAL_SURFACE" | "CONICAL_SURFACE" => tessellate_revolved_face(
            &boundaries,
            surface,
            same_sense,
            records,
            plane_angle_unit,
            face.id,
        )?,
        "SPHERICAL_SURFACE" => {
            tessellate_spherical_face(&boundaries, surface, same_sense, records, face.id)?
        }
        "TOROIDAL_SURFACE" => {
            tessellate_toroidal_face(&boundaries, surface, same_sense, records, face.id)?
        }
        kind => {
            return Err(unsupported(format!(
                "#{} ADVANCED_FACE uses {kind} surface; supported surfaces are PLANE, CYLINDRICAL_SURFACE, CONICAL_SURFACE, SPHERICAL_SURFACE, and TOROIDAL_SURFACE",
                face.id
            )));
        }
    };
    let base_index = u32::try_from(primitive.positions.len())
        .map_err(|_| ImportError::InvalidData("STEP B-Rep vertex count exceeds u32".to_string()))?;
    for index in &mut local_indices {
        *index = index
            .checked_add(base_index)
            .ok_or_else(|| ImportError::InvalidData("STEP B-Rep index exceeds u32".to_string()))?;
    }

    primitive.normals.extend(normals);
    primitive.positions.extend(boundaries.positions);
    primitive.indices.extend(local_indices);
    Ok(())
}

/// Resolves one outer bound and all inner bounds under per-face resource limits.
fn resolve_face_boundaries(
    bound_ids: &[usize],
    records: &HashMap<usize, &StepRecord>,
    options: &ImportOptions,
    face_id: usize,
) -> Result<ResolvedFaceBoundaries, ImportError> {
    if bound_ids.len() > options.limits.max_step_face_loops {
        return Err(ImportError::ResourceLimitExceeded {
            resource: "STEP face loops",
            limit: options.limits.max_step_face_loops,
            actual: bound_ids.len(),
        });
    }

    let mut outer_bound = None::<&StepRecord>;
    let mut inner_bounds = Vec::<&StepRecord>::new();
    for bound_id in bound_ids {
        let bound = require_record(records, *bound_id, "ADVANCED_FACE bound")?;
        match bound.kind.as_str() {
            "FACE_OUTER_BOUND" => {
                if outer_bound.replace(bound).is_some() {
                    return Err(ImportError::InvalidData(format!(
                        "#{face_id} ADVANCED_FACE has multiple outer bounds"
                    )));
                }
            }
            "FACE_BOUND" => inner_bounds.push(bound),
            kind => {
                return Err(ImportError::InvalidData(format!(
                    "#{face_id} ADVANCED_FACE references unsupported bound entity {kind}"
                )));
            }
        }
    }
    let outer_bound = outer_bound.ok_or_else(|| {
        ImportError::InvalidData(format!("#{face_id} ADVANCED_FACE has no FACE_OUTER_BOUND"))
    })?;

    let mut positions = Vec::new();
    let mut loops = Vec::with_capacity(bound_ids.len());
    for bound in std::iter::once(outer_bound).chain(inner_bounds) {
        let bound_args = require_args(bound, 3)?;
        let loop_id = parse_reference(bound_args[1]).ok_or_else(|| {
            ImportError::InvalidData(format!(
                "#{} {} has no loop reference",
                bound.id, bound.kind
            ))
        })?;
        let orientation = parse_step_bool(
            bound_args[2],
            bound.id,
            &format!("{} orientation", bound.kind),
        )?;
        let start = positions.len();
        let mut loop_positions =
            resolve_loop_positions(loop_id, records, options, positions.len())?;
        if !orientation {
            loop_positions.reverse();
        }
        positions.extend(loop_positions);
        loops.push(ResolvedFaceLoop {
            loop_id,
            range: start..positions.len(),
        });
    }

    Ok(ResolvedFaceBoundaries { positions, loops })
}

fn tessellate_planar_face(
    boundaries: &ResolvedFaceBoundaries,
    surface: &StepRecord,
    same_sense: bool,
    records: &HashMap<usize, &StepRecord>,
    face_id: usize,
) -> Result<(Vec<u32>, Vec<[f32; 3]>), ImportError> {
    let (plane_origin, mut plane_normal) = resolve_plane(surface, records)?;
    if !same_sense {
        plane_normal = scale3(plane_normal, -1.0);
    }
    validate_planarity(&boundaries.positions, plane_origin, plane_normal, face_id)?;
    let projected = project_polygon(&boundaries.positions, plane_normal);
    let mut indices = triangulate_projected_face(&projected, &boundaries.hole_indices(), face_id)?;
    orient_triangles(&mut indices, &boundaries.positions, plane_normal, face_id)?;
    Ok((indices, vec![plane_normal; boundaries.positions.len()]))
}

fn tessellate_revolved_face(
    boundaries: &ResolvedFaceBoundaries,
    surface: &StepRecord,
    same_sense: bool,
    records: &HashMap<usize, &StepRecord>,
    plane_angle_unit: Option<&StepResolvedUnit>,
    face_id: usize,
) -> Result<(Vec<u32>, Vec<[f32; 3]>), ImportError> {
    let geometry = resolve_revolved_surface(surface, records, plane_angle_unit)?;
    let AxisPlacement {
        origin,
        axis,
        x_axis,
        y_axis,
    } = geometry.placement;
    let mut projected = Vec::with_capacity(boundaries.positions.len());
    let mut normals = Vec::with_capacity(boundaries.positions.len());
    let tolerance = geometry_tolerance(&boundaries.positions);

    for (loop_index, boundary) in boundaries.loops.iter().enumerate() {
        validate_revolved_loop_geometry(boundary.loop_id, geometry, records, face_id)?;
        let mut loop_projected = Vec::with_capacity(boundary.range.len());
        let mut previous_angle = None::<f32>;
        for position in &boundaries.positions[boundary.range.clone()] {
            let offset = sub3(*position, origin);
            let height = dot3(offset, axis);
            let radial = sub3(offset, scale3(axis, height));
            let radial_length = length3(radial);
            let expected_radius = geometry.radius_at_height(height);
            if !expected_radius.is_finite() || expected_radius <= tolerance {
                return Err(unsupported(format!(
                    "#{face_id} {} boundary reaches or crosses the cone apex",
                    geometry.kind.step_name()
                )));
            }
            if !radial_length.is_finite() || (radial_length - expected_radius).abs() > tolerance {
                return Err(ImportError::InvalidData(format!(
                    "#{face_id} ADVANCED_FACE boundary does not lie on {} #{}",
                    geometry.kind.step_name(),
                    surface.id
                )));
            }
            let radial_direction = scale3(radial, 1.0 / radial_length);
            let raw_angle = dot3(radial, y_axis).atan2(dot3(radial, x_axis));
            let angle = unwrap_angle(previous_angle, raw_angle);
            previous_angle = Some(angle);
            loop_projected.push([angle * geometry.reference_radius, height]);
            let surface_normal =
                normalize3(sub3(radial_direction, scale3(axis, geometry.radial_slope)))
                    .ok_or_else(|| {
                        ImportError::InvalidData(format!(
                            "#{face_id} {} has an invalid surface normal",
                            geometry.kind.step_name()
                        ))
                    })?;
            normals.push(if same_sense {
                surface_normal
            } else {
                scale3(surface_normal, -1.0)
            });
        }
        if loop_index > 0 {
            align_periodic_loop(
                &mut loop_projected,
                &projected[boundaries.loops[0].range.clone()],
                [
                    Some(std::f32::consts::TAU * geometry.reference_radius),
                    None,
                ],
            );
        }
        projected.extend(loop_projected);
    }

    let mut indices = triangulate_projected_face(&projected, &boundaries.hole_indices(), face_id)?;
    orient_revolved_triangles(
        &mut indices,
        &boundaries.positions,
        geometry,
        same_sense,
        face_id,
    )?;
    Ok((indices, normals))
}

fn tessellate_spherical_face(
    boundaries: &ResolvedFaceBoundaries,
    surface: &StepRecord,
    same_sense: bool,
    records: &HashMap<usize, &StepRecord>,
    face_id: usize,
) -> Result<(Vec<u32>, Vec<[f32; 3]>), ImportError> {
    let sphere = resolve_sphere(surface, records)?;
    let mut projected = Vec::with_capacity(boundaries.positions.len());
    let mut normals = Vec::with_capacity(boundaries.positions.len());
    let tolerance = geometry_tolerance(&boundaries.positions);

    for (loop_index, boundary) in boundaries.loops.iter().enumerate() {
        validate_spherical_loop_geometry(boundary.loop_id, sphere, records, face_id)?;
        let mut loop_projected = Vec::with_capacity(boundary.range.len());
        let mut previous_longitude = None::<f32>;
        for position in &boundaries.positions[boundary.range.clone()] {
            let offset = sub3(*position, sphere.placement.origin);
            let radial_length = length3(offset);
            if !radial_length.is_finite() || (radial_length - sphere.radius).abs() > tolerance {
                return Err(ImportError::InvalidData(format!(
                    "#{face_id} ADVANCED_FACE boundary does not lie on SPHERICAL_SURFACE #{}",
                    surface.id
                )));
            }
            let x = dot3(offset, sphere.placement.x_axis);
            let y = dot3(offset, sphere.placement.y_axis);
            let z = dot3(offset, sphere.placement.axis);
            let horizontal = x.hypot(y);
            if horizontal <= tolerance {
                return Err(unsupported(format!(
                    "#{face_id} SPHERICAL_SURFACE boundary touches a parameterization pole"
                )));
            }
            let raw_longitude = y.atan2(x);
            let longitude = unwrap_angle(previous_longitude, raw_longitude);
            previous_longitude = Some(longitude);
            let latitude = (z / sphere.radius).clamp(-1.0, 1.0).asin();
            loop_projected.push([longitude * sphere.radius, latitude * sphere.radius]);
            let normal = scale3(offset, 1.0 / radial_length);
            normals.push(if same_sense {
                normal
            } else {
                scale3(normal, -1.0)
            });
        }
        if loop_index > 0 {
            align_periodic_loop(
                &mut loop_projected,
                &projected[boundaries.loops[0].range.clone()],
                [Some(std::f32::consts::TAU * sphere.radius), None],
            );
        }
        projected.extend(loop_projected);
    }

    let mut indices = triangulate_projected_face(&projected, &boundaries.hole_indices(), face_id)?;
    orient_spherical_triangles(
        &mut indices,
        &boundaries.positions,
        sphere,
        same_sense,
        face_id,
    )?;
    Ok((indices, normals))
}

/// Tessellates a regular ring-torus face in its unwrapped two-angle parameter domain.
fn tessellate_toroidal_face(
    boundaries: &ResolvedFaceBoundaries,
    surface: &StepRecord,
    same_sense: bool,
    records: &HashMap<usize, &StepRecord>,
    face_id: usize,
) -> Result<(Vec<u32>, Vec<[f32; 3]>), ImportError> {
    let torus = resolve_torus(surface, records)?;
    let mut projected = Vec::with_capacity(boundaries.positions.len());
    let mut normals = Vec::with_capacity(boundaries.positions.len());
    let tolerance = geometry_tolerance(&boundaries.positions);

    for (loop_index, boundary) in boundaries.loops.iter().enumerate() {
        validate_toroidal_loop_geometry(boundary.loop_id, torus, records, face_id)?;
        let mut loop_projected = Vec::with_capacity(boundary.range.len());
        let mut previous_major_angle = None::<f32>;
        let mut previous_minor_angle = None::<f32>;
        for position in &boundaries.positions[boundary.range.clone()] {
            let offset = sub3(*position, torus.placement.origin);
            let x = dot3(offset, torus.placement.x_axis);
            let y = dot3(offset, torus.placement.y_axis);
            let z = dot3(offset, torus.placement.axis);
            let radial_length = x.hypot(y);
            if !radial_length.is_finite() || radial_length <= tolerance {
                return Err(ImportError::InvalidData(format!(
                    "#{face_id} TOROIDAL_SURFACE boundary has an invalid radial direction"
                )));
            }
            let tube_radial = radial_length - torus.major_radius;
            let tube_length = tube_radial.hypot(z);
            if !tube_length.is_finite() || (tube_length - torus.minor_radius).abs() > tolerance {
                return Err(ImportError::InvalidData(format!(
                    "#{face_id} ADVANCED_FACE boundary does not lie on TOROIDAL_SURFACE #{}",
                    surface.id
                )));
            }

            let raw_major_angle = y.atan2(x);
            let major_angle = unwrap_angle(previous_major_angle, raw_major_angle);
            previous_major_angle = Some(major_angle);
            let raw_minor_angle = z.atan2(tube_radial);
            let minor_angle = unwrap_angle(previous_minor_angle, raw_minor_angle);
            previous_minor_angle = Some(minor_angle);
            loop_projected.push([
                major_angle * torus.major_radius,
                minor_angle * torus.minor_radius,
            ]);

            let radial_direction = normalize3(add3(
                scale3(torus.placement.x_axis, x),
                scale3(torus.placement.y_axis, y),
            ))
            .ok_or_else(|| {
                ImportError::InvalidData(format!(
                    "#{face_id} TOROIDAL_SURFACE has an invalid radial direction"
                ))
            })?;
            let normal = normalize3(add3(
                scale3(radial_direction, tube_radial),
                scale3(torus.placement.axis, z),
            ))
            .ok_or_else(|| {
                ImportError::InvalidData(format!(
                    "#{face_id} TOROIDAL_SURFACE has an invalid surface normal"
                ))
            })?;
            normals.push(if same_sense {
                normal
            } else {
                scale3(normal, -1.0)
            });
        }
        if loop_index > 0 {
            align_periodic_loop(
                &mut loop_projected,
                &projected[boundaries.loops[0].range.clone()],
                [
                    Some(std::f32::consts::TAU * torus.major_radius),
                    Some(std::f32::consts::TAU * torus.minor_radius),
                ],
            );
        }
        projected.extend(loop_projected);
    }

    let mut indices = triangulate_projected_face(&projected, &boundaries.hole_indices(), face_id)?;
    orient_toroidal_triangles(
        &mut indices,
        &boundaries.positions,
        torus,
        same_sense,
        face_id,
    )?;
    Ok((indices, normals))
}

fn resolve_loop_positions(
    loop_id: usize,
    records: &HashMap<usize, &StepRecord>,
    options: &ImportOptions,
    existing_face_vertices: usize,
) -> Result<Vec<[f32; 3]>, ImportError> {
    let loop_record = require_record(records, loop_id, "face loop")?;
    let args = require_args(loop_record, 2)?;
    match loop_record.kind.as_str() {
        "POLY_LOOP" => {
            let point_ids = parse_references(args[1]);
            if point_ids.len() < 3 {
                return Err(ImportError::InvalidData(format!(
                    "#{} POLY_LOOP has fewer than three points",
                    loop_record.id
                )));
            }
            enforce_step_face_vertex_limit(
                existing_face_vertices,
                point_ids.len(),
                options.limits.max_step_face_vertices,
            )?;
            point_ids
                .into_iter()
                .map(|point_id| resolve_cartesian_point(point_id, records))
                .collect()
        }
        "EDGE_LOOP" => resolve_edge_loop(
            loop_record,
            args[1],
            records,
            options,
            existing_face_vertices,
        ),
        kind => Err(ImportError::InvalidData(format!(
            "#{} face bound references unsupported loop entity {kind}",
            loop_record.id
        ))),
    }
}

fn resolve_edge_loop(
    loop_record: &StepRecord,
    edge_list: &str,
    records: &HashMap<usize, &StepRecord>,
    options: &ImportOptions,
    existing_face_vertices: usize,
) -> Result<Vec<[f32; 3]>, ImportError> {
    let oriented_edge_ids = parse_references(edge_list);
    if oriented_edge_ids.is_empty() {
        return Err(ImportError::InvalidData(format!(
            "#{} EDGE_LOOP contains no edges",
            loop_record.id
        )));
    }

    let mut positions = Vec::new();
    let mut expected_start = None::<usize>;
    let mut first_start = None::<usize>;
    for oriented_edge_id in oriented_edge_ids {
        let path = resolve_oriented_edge_path(oriented_edge_id, records, options)?;
        if let Some(expected_start) = expected_start
            && path.start_vertex != expected_start
        {
            return Err(ImportError::InvalidData(format!(
                "#{} EDGE_LOOP is discontinuous at oriented edge #{oriented_edge_id}",
                loop_record.id
            )));
        }
        first_start.get_or_insert(path.start_vertex);
        let resolved_loop_vertices = positions.len().saturating_add(path.point_count - 1);
        enforce_step_face_vertex_limit(
            existing_face_vertices,
            resolved_loop_vertices,
            options.limits.max_step_face_vertices,
        )?;
        positions.extend(path.positions.into_iter().take(path.point_count - 1));
        expected_start = Some(path.end_vertex);
    }
    if expected_start != first_start {
        return Err(ImportError::InvalidData(format!(
            "#{} EDGE_LOOP is not closed",
            loop_record.id
        )));
    }
    if positions.len() < 3 {
        return Err(ImportError::InvalidData(format!(
            "#{} EDGE_LOOP resolves to fewer than three boundary points",
            loop_record.id
        )));
    }

    Ok(positions)
}

/// Enforces the cumulative tessellated boundary-vertex limit for one face.
fn enforce_step_face_vertex_limit(
    existing: usize,
    additional: usize,
    limit: usize,
) -> Result<(), ImportError> {
    let actual = existing
        .checked_add(additional)
        .ok_or(ImportError::ResourceLimitExceeded {
            resource: "STEP face vertices",
            limit,
            actual: usize::MAX,
        })?;
    if actual > limit {
        return Err(ImportError::ResourceLimitExceeded {
            resource: "STEP face vertices",
            limit,
            actual,
        });
    }
    Ok(())
}

struct OrientedEdgePath {
    start_vertex: usize,
    end_vertex: usize,
    point_count: usize,
    positions: Vec<[f32; 3]>,
}

/// Topological edge and its resolved analytic geometry within an `EDGE_LOOP`.
struct LoopEdgeGeometry<'a> {
    edge: &'a StepRecord,
    geometry: &'a StepRecord,
}

/// One STEP `TRIMMED_CURVE` normalized to parameter trim values.
struct TrimmedCurveGeometry<'a> {
    basis: &'a StepRecord,
    trim_1: f64,
    trim_2: f64,
    sense_agreement: bool,
}

fn resolve_loop_edge_geometries<'a>(
    loop_record: &StepRecord,
    records: &'a HashMap<usize, &'a StepRecord>,
    relation: &str,
) -> Result<Vec<LoopEdgeGeometry<'a>>, ImportError> {
    let loop_args = require_args(loop_record, 2)?;
    let mut resolved = Vec::new();
    for oriented_edge_id in parse_references(loop_args[1]) {
        let oriented_edge = require_record(records, oriented_edge_id, relation)?;
        if oriented_edge.kind != "ORIENTED_EDGE" {
            return Err(ImportError::InvalidData(format!(
                "#{oriented_edge_id} is {}, expected ORIENTED_EDGE",
                oriented_edge.kind
            )));
        }
        let oriented_args = require_args(oriented_edge, 5)?;
        let edge_id = parse_reference(oriented_args[3]).ok_or_else(|| {
            ImportError::InvalidData(format!(
                "#{oriented_edge_id} ORIENTED_EDGE has no edge element"
            ))
        })?;
        let edge = require_record(records, edge_id, relation)?;
        if edge.kind != "EDGE_CURVE" {
            return Err(ImportError::InvalidData(format!(
                "#{edge_id} is {}, expected EDGE_CURVE",
                edge.kind
            )));
        }
        let edge_args = require_args(edge, 5)?;
        let geometry_id = parse_reference(edge_args[3]).ok_or_else(|| {
            ImportError::InvalidData(format!("#{edge_id} EDGE_CURVE has no geometry reference"))
        })?;
        let geometry = require_record(records, geometry_id, relation)?;
        resolved.push(LoopEdgeGeometry {
            edge,
            geometry: resolve_effective_curve_geometry(geometry, records, relation)?,
        });
    }
    Ok(resolved)
}

fn resolve_effective_curve_geometry<'a>(
    geometry: &'a StepRecord,
    records: &'a HashMap<usize, &'a StepRecord>,
    relation: &str,
) -> Result<&'a StepRecord, ImportError> {
    if geometry.kind != "TRIMMED_CURVE" {
        return Ok(geometry);
    }
    Ok(resolve_trimmed_curve(geometry, records, relation)?.basis)
}

fn resolve_oriented_edge_path(
    oriented_edge_id: usize,
    records: &HashMap<usize, &StepRecord>,
    options: &ImportOptions,
) -> Result<OrientedEdgePath, ImportError> {
    let oriented_edge = require_record(records, oriented_edge_id, "EDGE_LOOP oriented edge")?;
    if oriented_edge.kind != "ORIENTED_EDGE" {
        return Err(ImportError::InvalidData(format!(
            "#{oriented_edge_id} is {}, expected ORIENTED_EDGE",
            oriented_edge.kind
        )));
    }
    let args = require_args(oriented_edge, 5)?;
    let edge_id = parse_reference(args[3]).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{oriented_edge_id} ORIENTED_EDGE has no edge element"
        ))
    })?;
    let orientation = parse_step_bool(args[4], oriented_edge_id, "ORIENTED_EDGE orientation")?;

    let edge = require_record(records, edge_id, "ORIENTED_EDGE edge element")?;
    if edge.kind != "EDGE_CURVE" {
        return Err(ImportError::InvalidData(format!(
            "#{edge_id} is {}, expected EDGE_CURVE",
            edge.kind
        )));
    }
    let edge_args = require_args(edge, 5)?;
    let start = parse_reference(edge_args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{edge_id} EDGE_CURVE has no start vertex"))
    })?;
    let end = parse_reference(edge_args[2]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{edge_id} EDGE_CURVE has no end vertex"))
    })?;
    let geometry_id = parse_reference(edge_args[3]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{edge_id} EDGE_CURVE has no geometry reference"))
    })?;
    let edge_same_sense = parse_step_bool(edge_args[4], edge_id, "EDGE_CURVE same_sense")?;
    let (start_vertex, end_vertex) = if orientation {
        (start, end)
    } else {
        (end, start)
    };
    let geometry = require_record(records, geometry_id, "EDGE_CURVE geometry")?;
    let parameter_increases = edge_same_sense == orientation;
    let positions = match geometry.kind.as_str() {
        "TRIMMED_CURVE" => tessellate_trimmed_curve_edge(
            geometry,
            start_vertex,
            end_vertex,
            parameter_increases,
            records,
            options,
            edge_id,
        )?,
        "LINE" => {
            validate_line_geometry(geometry, start, end, records, edge_id)?;
            vec![
                resolve_vertex_point(start_vertex, records)?,
                resolve_vertex_point(end_vertex, records)?,
            ]
        }
        "CIRCLE" => tessellate_circle_edge(
            geometry,
            start_vertex,
            end_vertex,
            parameter_increases,
            records,
            options,
            edge_id,
        )?,
        "ELLIPSE" => tessellate_ellipse_edge(
            geometry,
            start_vertex,
            end_vertex,
            parameter_increases,
            records,
            options,
            edge_id,
        )?,
        _ if is_bspline_curve_with_knots(geometry) => tessellate_bspline_edge(
            geometry,
            start_vertex,
            end_vertex,
            parameter_increases,
            records,
            options,
            edge_id,
        )?,
        kind => {
            return Err(unsupported(format!(
                "#{edge_id} uses unsupported edge geometry {kind}; supported curves are LINE, CIRCLE, ELLIPSE, B_SPLINE_CURVE_WITH_KNOTS, and parameter TRIMMED_CURVE over those curve bases"
            )));
        }
    };
    let point_count = positions.len();
    Ok(OrientedEdgePath {
        start_vertex,
        end_vertex,
        point_count,
        positions,
    })
}

fn tessellate_circle_edge(
    circle: &StepRecord,
    start_vertex_id: usize,
    end_vertex_id: usize,
    parameter_increases: bool,
    records: &HashMap<usize, &StepRecord>,
    options: &ImportOptions,
    edge_id: usize,
) -> Result<Vec<[f32; 3]>, ImportError> {
    let circle_geometry = resolve_circle(circle, records)?;
    let AxisPlacement {
        origin,
        axis,
        x_axis,
        y_axis,
    } = circle_geometry.placement;
    let radius = circle_geometry.radius;
    let start = resolve_vertex_point(start_vertex_id, records)?;
    let end = resolve_vertex_point(end_vertex_id, records)?;
    validate_circle_point(start, origin, axis, radius, edge_id, circle.id)?;
    validate_circle_point(end, origin, axis, radius, edge_id, circle.id)?;

    let start_offset = sub3(start, origin);
    let end_offset = sub3(end, origin);
    let start_angle = dot3(start_offset, y_axis).atan2(dot3(start_offset, x_axis));
    let end_angle = dot3(end_offset, y_axis).atan2(dot3(end_offset, x_axis));
    let mut delta = directed_angle_delta(start_angle, end_angle, parameter_increases);
    if delta.abs() <= GEOMETRY_EPSILON {
        if start_vertex_id != end_vertex_id {
            return Err(ImportError::InvalidData(format!(
                "#{edge_id} closed CIRCLE geometry uses different topological vertices"
            )));
        }
        delta = if parameter_increases {
            std::f32::consts::TAU
        } else {
            -std::f32::consts::TAU
        };
    }

    let chord_error = resolve_curve_chord_error(options, radius)?;
    let ratio = (f64::from(chord_error) / f64::from(radius)).min(1.0);
    let max_angle = 2.0 * (1.0 - ratio).acos();
    if !max_angle.is_finite() || max_angle <= f64::EPSILON {
        return Err(ImportError::ResourceLimitExceeded {
            resource: "STEP curve segments",
            limit: options.limits.max_step_curve_segments,
            actual: usize::MAX,
        });
    }
    let minimum_segments = if start_vertex_id == end_vertex_id {
        3.0
    } else {
        1.0
    };
    let required_segments = (f64::from(delta.abs()) / max_angle)
        .ceil()
        .max(minimum_segments);
    if required_segments > options.limits.max_step_curve_segments as f64 {
        return Err(ImportError::ResourceLimitExceeded {
            resource: "STEP curve segments",
            limit: options.limits.max_step_curve_segments,
            actual: required_segments.min(usize::MAX as f64) as usize,
        });
    }
    let segments = required_segments as usize;

    let mut positions = Vec::with_capacity(segments + 1);
    for index in 0..=segments {
        if index == segments {
            positions.push(end);
            continue;
        }
        let angle = start_angle + delta * index as f32 / segments as f32;
        let (sin, cos) = stable_sin_cos(angle);
        positions.push(add3(
            origin,
            add3(scale3(x_axis, radius * cos), scale3(y_axis, radius * sin)),
        ));
    }
    Ok(positions)
}

fn tessellate_ellipse_edge(
    ellipse_record: &StepRecord,
    start_vertex_id: usize,
    end_vertex_id: usize,
    parameter_increases: bool,
    records: &HashMap<usize, &StepRecord>,
    options: &ImportOptions,
    edge_id: usize,
) -> Result<Vec<[f32; 3]>, ImportError> {
    let ellipse = resolve_ellipse(ellipse_record, records)?;
    let start = resolve_vertex_point(start_vertex_id, records)?;
    let end = resolve_vertex_point(end_vertex_id, records)?;
    let start_angle = resolve_ellipse_angle(start, ellipse, edge_id)?;
    let end_angle = resolve_ellipse_angle(end, ellipse, edge_id)?;
    let mut delta = directed_angle_delta(start_angle, end_angle, parameter_increases);
    if delta.abs() <= GEOMETRY_EPSILON {
        if start_vertex_id != end_vertex_id {
            return Err(ImportError::InvalidData(format!(
                "#{edge_id} closed ELLIPSE geometry uses different topological vertices"
            )));
        }
        delta = if parameter_increases {
            std::f32::consts::TAU
        } else {
            -std::f32::consts::TAU
        };
    }
    let chord_error =
        resolve_curve_chord_error(options, ellipse.semi_axis_1.max(ellipse.semi_axis_2))?;
    tessellate_ellipse_arc(
        ellipse,
        start_angle,
        delta,
        start,
        end,
        chord_error,
        options.limits.max_step_curve_segments,
    )
}

fn is_bspline_curve_with_knots(record: &StepRecord) -> bool {
    record.kind == "B_SPLINE_CURVE_WITH_KNOTS"
        || record.component("B_SPLINE_CURVE_WITH_KNOTS").is_some()
}

fn tessellate_trimmed_curve_edge(
    trimmed_record: &StepRecord,
    start_vertex_id: usize,
    end_vertex_id: usize,
    edge_follows_trimmed_sense: bool,
    records: &HashMap<usize, &StepRecord>,
    options: &ImportOptions,
    edge_id: usize,
) -> Result<Vec<[f32; 3]>, ImportError> {
    let trimmed = resolve_trimmed_curve(trimmed_record, records, "TRIMMED_CURVE basis curve")?;
    if is_bspline_curve_with_knots(trimmed.basis) {
        return tessellate_bspline_edge(
            trimmed_record,
            start_vertex_id,
            end_vertex_id,
            edge_follows_trimmed_sense,
            records,
            options,
            edge_id,
        );
    }

    let (start_parameter, end_parameter) =
        trimmed_edge_parameters(&trimmed, edge_follows_trimmed_sense);
    let basis_parameter_increases =
        trimmed_basis_parameter_increases(&trimmed, edge_follows_trimmed_sense);
    match trimmed.basis.kind.as_str() {
        "LINE" => tessellate_trimmed_line_edge(
            trimmed.basis,
            start_vertex_id,
            end_vertex_id,
            start_parameter,
            end_parameter,
            records,
            edge_id,
        ),
        "CIRCLE" => {
            validate_trimmed_circle_edge(
                trimmed.basis,
                start_vertex_id,
                end_vertex_id,
                start_parameter,
                end_parameter,
                records,
                edge_id,
            )?;
            tessellate_circle_edge(
                trimmed.basis,
                start_vertex_id,
                end_vertex_id,
                basis_parameter_increases,
                records,
                options,
                edge_id,
            )
        }
        "ELLIPSE" => {
            validate_trimmed_ellipse_edge(
                trimmed.basis,
                start_vertex_id,
                end_vertex_id,
                start_parameter,
                end_parameter,
                records,
                edge_id,
            )?;
            tessellate_ellipse_edge(
                trimmed.basis,
                start_vertex_id,
                end_vertex_id,
                basis_parameter_increases,
                records,
                options,
                edge_id,
            )
        }
        kind => Err(unsupported(format!(
            "#{} TRIMMED_CURVE uses {kind} basis; supported bases are LINE, CIRCLE, ELLIPSE, and B_SPLINE_CURVE_WITH_KNOTS",
            trimmed_record.id
        ))),
    }
}

fn trimmed_edge_parameters(
    trimmed: &TrimmedCurveGeometry<'_>,
    edge_follows_trimmed_sense: bool,
) -> (f64, f64) {
    let trimmed_sense = if trimmed.sense_agreement {
        (trimmed.trim_1, trimmed.trim_2)
    } else {
        (trimmed.trim_2, trimmed.trim_1)
    };
    if edge_follows_trimmed_sense {
        trimmed_sense
    } else {
        (trimmed_sense.1, trimmed_sense.0)
    }
}

fn trimmed_basis_parameter_increases(
    trimmed: &TrimmedCurveGeometry<'_>,
    edge_follows_trimmed_sense: bool,
) -> bool {
    edge_follows_trimmed_sense == trimmed.sense_agreement
}

fn tessellate_trimmed_line_edge(
    line: &StepRecord,
    start_vertex_id: usize,
    end_vertex_id: usize,
    start_parameter: f64,
    end_parameter: f64,
    records: &HashMap<usize, &StepRecord>,
    edge_id: usize,
) -> Result<Vec<[f32; 3]>, ImportError> {
    let start = resolve_vertex_point(start_vertex_id, records)?;
    let end = resolve_vertex_point(end_vertex_id, records)?;
    let expected_start = line_point_at_parameter(line, start_parameter, records)?;
    let expected_end = line_point_at_parameter(line, end_parameter, records)?;
    validate_trimmed_endpoint(start, expected_start, edge_id, "start")?;
    validate_trimmed_endpoint(end, expected_end, edge_id, "end")?;
    if points_equal(start, end) {
        return Err(ImportError::InvalidData(format!(
            "#{edge_id} EDGE_CURVE has identical start and end points"
        )));
    }
    Ok(vec![start, end])
}

fn validate_trimmed_circle_edge(
    circle: &StepRecord,
    start_vertex_id: usize,
    end_vertex_id: usize,
    start_parameter: f64,
    end_parameter: f64,
    records: &HashMap<usize, &StepRecord>,
    edge_id: usize,
) -> Result<(), ImportError> {
    let circle = resolve_circle(circle, records)?;
    let start = resolve_vertex_point(start_vertex_id, records)?;
    let end = resolve_vertex_point(end_vertex_id, records)?;
    validate_trimmed_endpoint(
        start,
        circle_point(circle, start_parameter as f32),
        edge_id,
        "start",
    )?;
    validate_trimmed_endpoint(
        end,
        circle_point(circle, end_parameter as f32),
        edge_id,
        "end",
    )
}

fn validate_trimmed_ellipse_edge(
    ellipse_record: &StepRecord,
    start_vertex_id: usize,
    end_vertex_id: usize,
    start_parameter: f64,
    end_parameter: f64,
    records: &HashMap<usize, &StepRecord>,
    edge_id: usize,
) -> Result<(), ImportError> {
    let ellipse = resolve_ellipse(ellipse_record, records)?;
    let start = resolve_vertex_point(start_vertex_id, records)?;
    let end = resolve_vertex_point(end_vertex_id, records)?;
    validate_trimmed_endpoint(
        start,
        ellipse_point(ellipse, start_parameter as f32),
        edge_id,
        "start",
    )?;
    validate_trimmed_endpoint(
        end,
        ellipse_point(ellipse, end_parameter as f32),
        edge_id,
        "end",
    )
}

fn validate_trimmed_endpoint(
    actual: [f32; 3],
    expected: [f32; 3],
    edge_id: usize,
    endpoint: &str,
) -> Result<(), ImportError> {
    let tolerance = [actual, expected]
        .iter()
        .flat_map(|position| position.iter())
        .fold(1.0_f32, |scale, value| scale.max(value.abs()))
        * 1.0e-5;
    if length3(sub3(actual, expected)) > tolerance {
        return Err(ImportError::InvalidData(format!(
            "#{edge_id} TRIMMED_CURVE {endpoint} vertex does not match trim parameter"
        )));
    }
    Ok(())
}

fn tessellate_bspline_edge(
    curve_record: &StepRecord,
    start_vertex_id: usize,
    end_vertex_id: usize,
    parameter_increases: bool,
    records: &HashMap<usize, &StepRecord>,
    options: &ImportOptions,
    edge_id: usize,
) -> Result<Vec<[f32; 3]>, ImportError> {
    let curve = resolve_bspline_curve_geometry(curve_record, records, options)?;
    let start = resolve_vertex_point(start_vertex_id, records)?;
    let end = resolve_vertex_point(end_vertex_id, records)?;
    let curve_start = evaluate_bspline_curve(&curve, curve.start_parameter)?;
    let curve_end = evaluate_bspline_curve(&curve, curve.end_parameter)?;
    let (expected_start, expected_end) = if parameter_increases {
        (curve_start, curve_end)
    } else {
        (curve_end, curve_start)
    };
    validate_bspline_endpoint(start, expected_start, &curve, edge_id, "start")?;
    validate_bspline_endpoint(end, expected_end, &curve, edge_id, "end")?;

    let chord_error = resolve_curve_chord_error(options, curve.characteristic_length)?;
    let (start_parameter, end_parameter) = if parameter_increases {
        (curve.start_parameter, curve.end_parameter)
    } else {
        (curve.end_parameter, curve.start_parameter)
    };
    tessellate_bspline_interval(
        &curve,
        start_parameter,
        start,
        end_parameter,
        end,
        chord_error,
        options.limits.max_step_curve_segments,
    )
}

fn resolve_bspline_curve_geometry(
    curve: &StepRecord,
    records: &HashMap<usize, &StepRecord>,
    options: &ImportOptions,
) -> Result<BsplineCurveGeometry, ImportError> {
    if curve.kind == "TRIMMED_CURVE" {
        return resolve_trimmed_bspline_curve(curve, records, options);
    }
    resolve_bspline_curve(curve, records, options)
}

fn resolve_trimmed_bspline_curve(
    trimmed: &StepRecord,
    records: &HashMap<usize, &StepRecord>,
    options: &ImportOptions,
) -> Result<BsplineCurveGeometry, ImportError> {
    let trimmed = resolve_trimmed_curve(trimmed, records, "TRIMMED_CURVE basis curve")?;
    if !is_bspline_curve_with_knots(trimmed.basis) {
        return Err(unsupported(format!(
            "#{} TRIMMED_CURVE uses {} basis; only B_SPLINE_CURVE_WITH_KNOTS basis is supported",
            trimmed.basis.id, trimmed.basis.kind
        )));
    }

    let mut curve = resolve_bspline_curve(trimmed.basis, records, options)?;
    validate_trimmed_bspline_parameter(trimmed.trim_1, &curve, trimmed.basis.id, "trim_1")?;
    validate_trimmed_bspline_parameter(trimmed.trim_2, &curve, trimmed.basis.id, "trim_2")?;
    let (start_parameter, end_parameter) = trimmed_edge_parameters(&trimmed, true);
    curve.start_parameter = start_parameter;
    curve.end_parameter = end_parameter;
    Ok(curve)
}

fn resolve_trimmed_curve<'a>(
    trimmed: &StepRecord,
    records: &'a HashMap<usize, &StepRecord>,
    relation: &str,
) -> Result<TrimmedCurveGeometry<'a>, ImportError> {
    let args = require_args(trimmed, 6)?;
    let basis_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{} TRIMMED_CURVE has no basis curve reference",
            trimmed.id
        ))
    })?;
    let trim_1 = parse_trimmed_curve_parameter(args[2], trimmed.id, "trim_1")?;
    let trim_2 = parse_trimmed_curve_parameter(args[3], trimmed.id, "trim_2")?;
    if (trim_1 - trim_2).abs() <= f64::EPSILON {
        return Err(ImportError::InvalidData(format!(
            "#{} TRIMMED_CURVE trim parameters must be distinct",
            trimmed.id
        )));
    }
    Ok(TrimmedCurveGeometry {
        basis: require_record(records, basis_id, relation)?,
        trim_1,
        trim_2,
        sense_agreement: parse_step_bool(args[4], trimmed.id, "TRIMMED_CURVE sense_agreement")?,
    })
}

fn resolve_bspline_curve(
    curve: &StepRecord,
    records: &HashMap<usize, &StepRecord>,
    options: &ImportOptions,
) -> Result<BsplineCurveGeometry, ImportError> {
    let (degree_arg, control_points_arg, multiplicities_arg, knots_arg) =
        if curve.kind == "B_SPLINE_CURVE_WITH_KNOTS" {
            let args = require_args(curve, 9)?;
            (args[1], args[2], args[6], args[7])
        } else {
            let base = curve.component("B_SPLINE_CURVE").ok_or_else(|| {
                ImportError::InvalidData(format!(
                    "#{} B_SPLINE_CURVE_WITH_KNOTS has no B_SPLINE_CURVE component",
                    curve.id
                ))
            })?;
            let knots = curve
                .component("B_SPLINE_CURVE_WITH_KNOTS")
                .ok_or_else(|| {
                    ImportError::InvalidData(format!(
                        "#{} has no B_SPLINE_CURVE_WITH_KNOTS component",
                        curve.id
                    ))
                })?;
            let base_args = require_component_args(curve.id, &base.kind, &base.args, 3)?;
            let knot_args = require_component_args(curve.id, &knots.kind, &knots.args, 3)?;
            let (degree_arg, control_points_arg) = if base_args.len() >= 6 {
                (base_args[1], base_args[2])
            } else {
                (base_args[0], base_args[1])
            };
            if knot_args.len() >= 8 {
                (degree_arg, control_points_arg, knot_args[6], knot_args[7])
            } else {
                (degree_arg, control_points_arg, knot_args[0], knot_args[1])
            }
        };

    let degree = parse_step_usize_scalar(degree_arg, curve.id, "B_SPLINE_CURVE degree")?;
    if degree == 0 {
        return Err(ImportError::InvalidData(format!(
            "#{} B_SPLINE_CURVE degree must be positive",
            curve.id
        )));
    }
    if degree > options.limits.max_step_spline_degree {
        return Err(ImportError::ResourceLimitExceeded {
            resource: "STEP spline degree",
            limit: options.limits.max_step_spline_degree,
            actual: degree,
        });
    }

    let control_point_ids = parse_references(control_points_arg);
    if control_point_ids.len() <= degree {
        return Err(ImportError::InvalidData(format!(
            "#{} B_SPLINE_CURVE control point count must exceed its degree",
            curve.id
        )));
    }
    if control_point_ids.len() > options.limits.max_step_spline_control_points {
        return Err(ImportError::ResourceLimitExceeded {
            resource: "STEP spline control points",
            limit: options.limits.max_step_spline_control_points,
            actual: control_point_ids.len(),
        });
    }

    let control_positions = control_point_ids
        .iter()
        .map(|point_id| resolve_cartesian_point(*point_id, records))
        .collect::<Result<Vec<_>, _>>()?;
    let characteristic_length = bspline_characteristic_length(&control_positions, curve.id)?;
    let weights = resolve_bspline_weights(curve, control_positions.len())?;
    let control_points = control_positions
        .iter()
        .zip(weights)
        .map(|(point, weight)| {
            [
                f64::from(point[0]) * weight,
                f64::from(point[1]) * weight,
                f64::from(point[2]) * weight,
                weight,
            ]
        })
        .collect::<Vec<_>>();

    let knots = expand_bspline_knots(
        curve.id,
        multiplicities_arg,
        knots_arg,
        control_points.len() + degree + 1,
        degree,
    )?;
    let domain_start = knots[degree];
    let domain_end = knots[control_points.len()];
    if !domain_start.is_finite() || !domain_end.is_finite() || domain_end <= domain_start {
        return Err(ImportError::InvalidData(format!(
            "#{} B_SPLINE_CURVE_WITH_KNOTS has an invalid parameter domain",
            curve.id
        )));
    }

    Ok(BsplineCurveGeometry {
        record_id: curve.id,
        degree,
        control_points,
        knots,
        parameter_domain: domain_start..domain_end,
        start_parameter: domain_start,
        end_parameter: domain_end,
        characteristic_length,
    })
}

fn parse_trimmed_curve_parameter(
    value: &str,
    curve_id: usize,
    label: &str,
) -> Result<f64, ImportError> {
    let trimmed = strip_outer_step_parentheses(value.trim());
    let mut parameters = Vec::new();
    for selection in split_top_level_args(trimmed) {
        let selection = selection.trim();
        if selection.starts_with("PARAMETER_VALUE") {
            let values = parse_float_list(selection);
            if values.len() == 1 {
                parameters.push(f64::from(values[0]));
            }
        } else if !selection.contains('#')
            && !selection.contains('(')
            && let Ok(parameter) = selection.parse::<f64>()
        {
            parameters.push(parameter);
        }
    }

    match parameters.as_slice() {
        [parameter] if parameter.is_finite() => Ok(*parameter),
        [] => Err(unsupported(format!(
            "#{curve_id} TRIMMED_CURVE {label} requires a PARAMETER_VALUE trim"
        ))),
        _ => Err(ImportError::InvalidData(format!(
            "#{curve_id} TRIMMED_CURVE {label} must contain exactly one finite PARAMETER_VALUE"
        ))),
    }
}

fn validate_trimmed_bspline_parameter(
    parameter: f64,
    curve: &BsplineCurveGeometry,
    trimmed_id: usize,
    label: &str,
) -> Result<(), ImportError> {
    let tolerance = f64::from(GEOMETRY_EPSILON);
    if parameter < curve.parameter_domain.start - tolerance
        || parameter > curve.parameter_domain.end + tolerance
    {
        return Err(ImportError::InvalidData(format!(
            "#{trimmed_id} TRIMMED_CURVE {label} parameter is outside the B-Spline domain"
        )));
    }
    Ok(())
}

fn resolve_bspline_weights(
    curve: &StepRecord,
    control_point_count: usize,
) -> Result<Vec<f64>, ImportError> {
    let Some(rational) = curve.component("RATIONAL_B_SPLINE_CURVE") else {
        return Ok(vec![1.0; control_point_count]);
    };
    let args = require_component_args(curve.id, &rational.kind, &rational.args, 1)?;
    let weights = parse_float_list(args[0]);
    if weights.len() != control_point_count {
        return Err(ImportError::InvalidData(format!(
            "#{} RATIONAL_B_SPLINE_CURVE weight count does not match control points",
            curve.id
        )));
    }
    weights
        .into_iter()
        .map(|weight| {
            if !weight.is_finite() || weight <= 0.0 {
                return Err(ImportError::InvalidData(format!(
                    "#{} RATIONAL_B_SPLINE_CURVE weights must be finite and positive",
                    curve.id
                )));
            }
            Ok(f64::from(weight))
        })
        .collect()
}

fn expand_bspline_knots(
    curve_id: usize,
    multiplicities_arg: &str,
    knots_arg: &str,
    expected_count: usize,
    degree: usize,
) -> Result<Vec<f64>, ImportError> {
    let multiplicities = parse_integer_list(multiplicities_arg);
    let knot_values = parse_float_list(knots_arg);
    if multiplicities.is_empty() || multiplicities.len() != knot_values.len() {
        return Err(ImportError::InvalidData(format!(
            "#{curve_id} B_SPLINE_CURVE_WITH_KNOTS requires matching knot values and multiplicities"
        )));
    }

    let mut expanded = Vec::with_capacity(expected_count);
    let mut previous = None::<f32>;
    let knot_value_count = knot_values.len();
    for (knot_index, (multiplicity, knot)) in
        multiplicities.into_iter().zip(knot_values).enumerate()
    {
        if !knot.is_finite() || previous.is_some_and(|previous| knot < previous) {
            return Err(ImportError::InvalidData(format!(
                "#{curve_id} B_SPLINE_CURVE_WITH_KNOTS knots must be finite and non-decreasing"
            )));
        }
        let multiplicity = usize::try_from(multiplicity)
            .ok()
            .filter(|multiplicity| *multiplicity > 0)
            .ok_or_else(|| {
                ImportError::InvalidData(format!(
                    "#{curve_id} B_SPLINE_CURVE_WITH_KNOTS multiplicities must be positive"
                ))
            })?;
        if knot_index > 0 && knot_index + 1 < knot_value_count && multiplicity > degree {
            return Err(unsupported(format!(
                "#{curve_id} B_SPLINE_CURVE_WITH_KNOTS has an internal knot multiplicity greater than its degree"
            )));
        }
        if expanded.len().saturating_add(multiplicity) > expected_count {
            return Err(ImportError::InvalidData(format!(
                "#{curve_id} B_SPLINE_CURVE_WITH_KNOTS expanded knot count exceeds the degree/control-point contract"
            )));
        }
        expanded.extend(std::iter::repeat_n(f64::from(knot), multiplicity));
        previous = Some(knot);
    }

    if expanded.len() != expected_count {
        return Err(ImportError::InvalidData(format!(
            "#{curve_id} B_SPLINE_CURVE_WITH_KNOTS expanded knot count does not match the degree/control-point contract"
        )));
    }
    Ok(expanded)
}

fn bspline_characteristic_length(
    control_positions: &[[f32; 3]],
    curve_id: usize,
) -> Result<f32, ImportError> {
    let mut min = control_positions[0];
    let mut max = control_positions[0];
    for position in control_positions.iter().skip(1) {
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    let diagonal = length3(sub3(max, min));
    if !diagonal.is_finite() || diagonal <= GEOMETRY_EPSILON {
        return Err(ImportError::InvalidData(format!(
            "#{curve_id} B_SPLINE_CURVE control points are degenerate"
        )));
    }
    Ok(diagonal)
}

fn validate_bspline_endpoint(
    actual: [f32; 3],
    expected: [f32; 3],
    curve: &BsplineCurveGeometry,
    edge_id: usize,
    endpoint: &str,
) -> Result<(), ImportError> {
    let tolerance = curve
        .characteristic_length
        .max(length3(actual))
        .max(length3(expected))
        .max(1.0)
        * 1.0e-5;
    if length3(sub3(actual, expected)) > tolerance {
        return Err(ImportError::InvalidData(format!(
            "#{edge_id} EDGE_CURVE {endpoint} vertex does not match B_SPLINE_CURVE_WITH_KNOTS #{} endpoint",
            curve.record_id
        )));
    }
    Ok(())
}

fn tessellate_bspline_interval(
    curve: &BsplineCurveGeometry,
    start_parameter: f64,
    start: [f32; 3],
    end_parameter: f64,
    end: [f32; 3],
    chord_error: f32,
    segment_limit: usize,
) -> Result<Vec<[f32; 3]>, ImportError> {
    let mut pending = vec![(start_parameter, start, end_parameter, end, 0_u8)];
    let mut positions = vec![start];
    let mut accepted_segments = 0_usize;

    while let Some((parameter_a, point_a, parameter_b, point_b, depth)) = pending.pop() {
        let mut deviation = 0.0_f32;
        for fraction in [0.25_f64, 0.5, 0.75] {
            let parameter = parameter_a + (parameter_b - parameter_a) * fraction;
            let curve_point = evaluate_bspline_curve(curve, parameter)?;
            let chord_point = add3(
                scale3(point_a, (1.0 - fraction) as f32),
                scale3(point_b, fraction as f32),
            );
            deviation = deviation.max(length3(sub3(curve_point, chord_point)));
        }

        if deviation.is_finite() && deviation <= chord_error {
            accepted_segments += 1;
            if accepted_segments > segment_limit {
                return Err(step_curve_segment_limit(segment_limit, accepted_segments));
            }
            positions.push(point_b);
            continue;
        }

        if depth == u8::MAX
            || accepted_segments
                .saturating_add(pending.len())
                .saturating_add(2)
                > segment_limit
        {
            return Err(step_curve_segment_limit(
                segment_limit,
                segment_limit.saturating_add(1),
            ));
        }
        let middle_parameter = (parameter_a + parameter_b) * 0.5;
        let middle = evaluate_bspline_curve(curve, middle_parameter)?;
        pending.push((middle_parameter, middle, parameter_b, point_b, depth + 1));
        pending.push((parameter_a, point_a, middle_parameter, middle, depth + 1));
    }

    Ok(positions)
}

fn evaluate_bspline_curve(
    curve: &BsplineCurveGeometry,
    parameter: f64,
) -> Result<[f32; 3], ImportError> {
    let parameter = parameter.clamp(curve.parameter_domain.start, curve.parameter_domain.end);
    let span = find_bspline_span(
        curve.degree,
        curve.control_points.len(),
        &curve.knots,
        parameter,
    );
    let mut points = (0..=curve.degree)
        .map(|index| curve.control_points[span - curve.degree + index])
        .collect::<Vec<_>>();

    for order in 1..=curve.degree {
        for index in (order..=curve.degree).rev() {
            let knot_index = span - curve.degree + index;
            let denominator =
                curve.knots[knot_index + curve.degree + 1 - order] - curve.knots[knot_index];
            let alpha = if denominator.abs() <= f64::EPSILON {
                0.0
            } else {
                (parameter - curve.knots[knot_index]) / denominator
            };
            let previous = points[index - 1];
            for (component_index, value) in points[index].iter_mut().enumerate() {
                *value = (1.0 - alpha) * previous[component_index] + alpha * *value;
            }
        }
    }

    let weight = points[curve.degree][3];
    if !weight.is_finite() || weight <= f64::EPSILON {
        return Err(ImportError::InvalidData(format!(
            "#{} B_SPLINE_CURVE evaluates to an invalid rational weight",
            curve.record_id
        )));
    }
    let position = [
        (points[curve.degree][0] / weight) as f32,
        (points[curve.degree][1] / weight) as f32,
        (points[curve.degree][2] / weight) as f32,
    ];
    if position.iter().any(|component| !component.is_finite()) {
        return Err(ImportError::InvalidData(format!(
            "#{} B_SPLINE_CURVE evaluates to a non-finite point",
            curve.record_id
        )));
    }
    Ok(position)
}

fn find_bspline_span(
    degree: usize,
    control_point_count: usize,
    knots: &[f64],
    parameter: f64,
) -> usize {
    let last_control = control_point_count - 1;
    if parameter >= knots[last_control + 1] {
        return last_control;
    }
    if parameter <= knots[degree] {
        return degree;
    }

    let mut low = degree;
    let mut high = last_control + 1;
    let mut middle = (low + high) / 2;
    while parameter < knots[middle] || parameter >= knots[middle + 1] {
        if parameter < knots[middle] {
            high = middle;
        } else {
            low = middle;
        }
        middle = (low + high) / 2;
    }
    middle
}

fn resolve_ellipse(
    ellipse: &StepRecord,
    records: &HashMap<usize, &StepRecord>,
) -> Result<EllipseGeometry, ImportError> {
    let args = require_args(ellipse, 4)?;
    let placement_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{} ELLIPSE has no placement reference",
            ellipse.id
        ))
    })?;
    let semi_axis_1 = parse_positive_scalar(args[2], ellipse.id, "ELLIPSE semi_axis_1")?;
    let semi_axis_2 = parse_positive_scalar(args[3], ellipse.id, "ELLIPSE semi_axis_2")?;
    if semi_axis_1 < semi_axis_2 {
        return Err(ImportError::InvalidData(format!(
            "#{} ELLIPSE semi_axis_1 must not be smaller than semi_axis_2",
            ellipse.id
        )));
    }
    Ok(EllipseGeometry {
        placement: resolve_axis2_placement(placement_id, records)?,
        semi_axis_1,
        semi_axis_2,
    })
}

fn resolve_circle(
    circle: &StepRecord,
    records: &HashMap<usize, &StepRecord>,
) -> Result<CircleGeometry, ImportError> {
    let args = require_args(circle, 3)?;
    let placement_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{} CIRCLE has no placement reference", circle.id))
    })?;
    Ok(CircleGeometry {
        placement: resolve_axis2_placement(placement_id, records)?,
        radius: parse_positive_scalar(args[2], circle.id, "CIRCLE radius")?,
    })
}

fn resolve_ellipse_angle(
    point: [f32; 3],
    ellipse: EllipseGeometry,
    edge_id: usize,
) -> Result<f32, ImportError> {
    let offset = sub3(point, ellipse.placement.origin);
    let axial = dot3(offset, ellipse.placement.axis);
    let x = dot3(offset, ellipse.placement.x_axis);
    let y = dot3(offset, ellipse.placement.y_axis);
    let scale = ellipse.semi_axis_1.max(ellipse.semi_axis_2).max(1.0);
    let tolerance = scale * 1.0e-5;
    let equation = (x / ellipse.semi_axis_1).powi(2) + (y / ellipse.semi_axis_2).powi(2);
    if axial.abs() > tolerance || !equation.is_finite() || (equation - 1.0).abs() > 1.0e-4 {
        return Err(ImportError::InvalidData(format!(
            "#{edge_id} EDGE_CURVE vertex does not lie on ELLIPSE"
        )));
    }
    Ok((y / ellipse.semi_axis_2).atan2(x / ellipse.semi_axis_1))
}

fn tessellate_ellipse_arc(
    ellipse: EllipseGeometry,
    start_angle: f32,
    delta: f32,
    start: [f32; 3],
    end: [f32; 3],
    chord_error: f32,
    segment_limit: usize,
) -> Result<Vec<[f32; 3]>, ImportError> {
    let end_angle = start_angle + delta;
    let mut pending = vec![(start_angle, start, end_angle, end, 0_u8)];
    let mut positions = vec![start];
    let mut accepted_segments = 0_usize;

    while let Some((angle_a, point_a, angle_b, point_b, depth)) = pending.pop() {
        let middle_angle = (angle_a + angle_b) * 0.5;
        let middle = ellipse_point(ellipse, middle_angle);
        let mut deviation = 0.0_f32;
        for fraction in [0.25_f32, 0.5, 0.75] {
            let curve_point = ellipse_point(ellipse, angle_a + (angle_b - angle_a) * fraction);
            let chord_point = add3(scale3(point_a, 1.0 - fraction), scale3(point_b, fraction));
            deviation = deviation.max(length3(sub3(curve_point, chord_point)));
        }
        if deviation.is_finite() && deviation <= chord_error {
            accepted_segments += 1;
            if accepted_segments > segment_limit {
                return Err(step_curve_segment_limit(segment_limit, accepted_segments));
            }
            positions.push(point_b);
            continue;
        }
        if depth == u8::MAX
            || accepted_segments
                .saturating_add(pending.len())
                .saturating_add(2)
                > segment_limit
        {
            return Err(step_curve_segment_limit(
                segment_limit,
                segment_limit.saturating_add(1),
            ));
        }
        pending.push((middle_angle, middle, angle_b, point_b, depth + 1));
        pending.push((angle_a, point_a, middle_angle, middle, depth + 1));
    }
    Ok(positions)
}

fn ellipse_point(ellipse: EllipseGeometry, angle: f32) -> [f32; 3] {
    let (sin, cos) = stable_sin_cos(angle);
    add3(
        ellipse.placement.origin,
        add3(
            scale3(ellipse.placement.x_axis, ellipse.semi_axis_1 * cos),
            scale3(ellipse.placement.y_axis, ellipse.semi_axis_2 * sin),
        ),
    )
}

fn circle_point(circle: CircleGeometry, angle: f32) -> [f32; 3] {
    let (sin, cos) = stable_sin_cos(angle);
    add3(
        circle.placement.origin,
        add3(
            scale3(circle.placement.x_axis, circle.radius * cos),
            scale3(circle.placement.y_axis, circle.radius * sin),
        ),
    )
}

fn stable_sin_cos(angle: f32) -> (f32, f32) {
    let (sin, cos) = angle.sin_cos();
    (canonical_trig_value(sin), canonical_trig_value(cos))
}

fn canonical_trig_value(value: f32) -> f32 {
    if value.abs() <= GEOMETRY_EPSILON {
        0.0
    } else if (value.abs() - 1.0).abs() <= GEOMETRY_EPSILON {
        value.signum()
    } else {
        value
    }
}

fn resolve_curve_chord_error(
    options: &ImportOptions,
    characteristic_length: f32,
) -> Result<f32, ImportError> {
    if options.max_lod_error > 0.0 && options.max_lod_error.is_finite() {
        Ok(options.max_lod_error)
    } else if options.max_lod_error == 0.0 {
        Ok(characteristic_length * 1.0e-3)
    } else {
        Err(ImportError::InvalidData(
            "STEP max_lod_error must be finite and non-negative".to_string(),
        ))
    }
}

fn step_curve_segment_limit(limit: usize, actual: usize) -> ImportError {
    ImportError::ResourceLimitExceeded {
        resource: "STEP curve segments",
        limit,
        actual,
    }
}

fn validate_line_geometry(
    line: &StepRecord,
    start_vertex_id: usize,
    end_vertex_id: usize,
    records: &HashMap<usize, &StepRecord>,
    edge_id: usize,
) -> Result<(), ImportError> {
    let args = require_args(line, 3)?;
    let line_origin_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{} LINE has no point reference", line.id))
    })?;
    let vector_id = parse_reference(args[2]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{} LINE has no vector reference", line.id))
    })?;
    let line_origin = resolve_cartesian_point(line_origin_id, records)?;
    let line_direction = resolve_vector_direction(vector_id, records)?;
    let start = resolve_vertex_point(start_vertex_id, records)?;
    let end = resolve_vertex_point(end_vertex_id, records)?;
    if points_equal(start, end) {
        return Err(ImportError::InvalidData(format!(
            "#{edge_id} EDGE_CURVE has identical start and end points"
        )));
    }

    let scale = [line_origin, start, end]
        .iter()
        .flat_map(|position| position.iter())
        .fold(1.0_f32, |scale, value| scale.max(value.abs()));
    let tolerance = scale * 1.0e-5;
    let start_distance = length3(cross3(sub3(start, line_origin), line_direction));
    let end_distance = length3(cross3(sub3(end, line_origin), line_direction));
    if !start_distance.is_finite()
        || !end_distance.is_finite()
        || start_distance > tolerance
        || end_distance > tolerance
    {
        return Err(ImportError::InvalidData(format!(
            "#{edge_id} EDGE_CURVE vertices do not lie on LINE #{} within tolerance {tolerance}",
            line.id
        )));
    }
    Ok(())
}

fn line_point_at_parameter(
    line: &StepRecord,
    parameter: f64,
    records: &HashMap<usize, &StepRecord>,
) -> Result<[f32; 3], ImportError> {
    let args = require_args(line, 3)?;
    let line_origin_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{} LINE has no point reference", line.id))
    })?;
    let vector_id = parse_reference(args[2]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{} LINE has no vector reference", line.id))
    })?;
    let origin = resolve_cartesian_point(line_origin_id, records)?;
    let vector = resolve_vector(vector_id, records)?;
    Ok(add3(origin, scale3(vector, parameter as f32)))
}

fn resolve_vector_direction(
    vector_id: usize,
    records: &HashMap<usize, &StepRecord>,
) -> Result<[f32; 3], ImportError> {
    normalize3(resolve_vector(vector_id, records)?)
        .ok_or_else(|| ImportError::InvalidData(format!("#{vector_id} VECTOR has zero length")))
}

fn resolve_vector(
    vector_id: usize,
    records: &HashMap<usize, &StepRecord>,
) -> Result<[f32; 3], ImportError> {
    let vector = require_record(records, vector_id, "LINE vector")?;
    if vector.kind != "VECTOR" {
        return Err(ImportError::InvalidData(format!(
            "#{vector_id} is {}, expected VECTOR",
            vector.kind
        )));
    }
    let args = require_args(vector, 3)?;
    let direction_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{vector_id} VECTOR has no direction reference"))
    })?;
    let magnitudes = parse_float_list(args[2]);
    if magnitudes.len() != 1 || !magnitudes[0].is_finite() || magnitudes[0] <= 0.0 {
        return Err(ImportError::InvalidData(format!(
            "#{vector_id} VECTOR magnitude must be finite and positive"
        )));
    }
    Ok(scale3(
        resolve_direction(direction_id, records)?,
        magnitudes[0],
    ))
}

fn resolve_vertex_point(
    vertex_id: usize,
    records: &HashMap<usize, &StepRecord>,
) -> Result<[f32; 3], ImportError> {
    let vertex = require_record(records, vertex_id, "EDGE_CURVE vertex")?;
    if vertex.kind != "VERTEX_POINT" {
        return Err(ImportError::InvalidData(format!(
            "#{vertex_id} is {}, expected VERTEX_POINT",
            vertex.kind
        )));
    }
    let args = require_args(vertex, 2)?;
    let point_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{vertex_id} VERTEX_POINT has no point reference"))
    })?;
    resolve_cartesian_point(point_id, records)
}

fn resolve_cartesian_point(
    point_id: usize,
    records: &HashMap<usize, &StepRecord>,
) -> Result<[f32; 3], ImportError> {
    let point = require_record(records, point_id, "point geometry")?;
    if point.kind != "CARTESIAN_POINT" {
        return Err(ImportError::InvalidData(format!(
            "#{point_id} is {}, expected CARTESIAN_POINT",
            point.kind
        )));
    }
    let args = require_args(point, 2)?;
    parse_vec3(args[1], point.id, "CARTESIAN_POINT coordinates")
}

fn resolve_plane(
    surface: &StepRecord,
    records: &HashMap<usize, &StepRecord>,
) -> Result<([f32; 3], [f32; 3]), ImportError> {
    let surface_args = require_args(surface, 2)?;
    let placement_id = parse_reference(surface_args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{} PLANE has no placement reference", surface.id))
    })?;
    let placement = resolve_axis2_placement(placement_id, records)?;
    Ok((placement.origin, placement.axis))
}

fn resolve_sphere(
    surface: &StepRecord,
    records: &HashMap<usize, &StepRecord>,
) -> Result<SphereGeometry, ImportError> {
    let args = require_args(surface, 3)?;
    let placement_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{} SPHERICAL_SURFACE has no placement reference",
            surface.id
        ))
    })?;
    Ok(SphereGeometry {
        placement: resolve_axis2_placement(placement_id, records)?,
        radius: parse_positive_scalar(args[2], surface.id, "SPHERICAL_SURFACE radius")?,
    })
}

fn validate_spherical_loop_geometry(
    loop_id: usize,
    sphere: SphereGeometry,
    records: &HashMap<usize, &StepRecord>,
    face_id: usize,
) -> Result<(), ImportError> {
    let loop_record = require_record(records, loop_id, "spherical face loop")?;
    if loop_record.kind != "EDGE_LOOP" {
        return Err(unsupported(format!(
            "#{face_id} SPHERICAL_SURFACE requires EDGE_LOOP topology with CIRCLE edges"
        )));
    }
    for resolved in resolve_loop_edge_geometries(loop_record, records, "spherical edge")? {
        if resolved.geometry.kind != "CIRCLE" {
            return Err(unsupported(format!(
                "#{} SPHERICAL_SURFACE boundary uses {}; only CIRCLE edges are supported",
                resolved.edge.id, resolved.geometry.kind
            )));
        }
        validate_spherical_circle_edge(resolved.edge.id, resolved.geometry, sphere, records)?;
    }
    Ok(())
}

fn validate_spherical_circle_edge(
    edge_id: usize,
    circle: &StepRecord,
    sphere: SphereGeometry,
    records: &HashMap<usize, &StepRecord>,
) -> Result<(), ImportError> {
    let args = require_args(circle, 3)?;
    let placement_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{} CIRCLE has no placement reference", circle.id))
    })?;
    let placement = resolve_axis2_placement(placement_id, records)?;
    let circle_radius = parse_positive_scalar(args[2], circle.id, "CIRCLE radius")?;
    let center_offset = sub3(placement.origin, sphere.placement.origin);
    let center_distance = dot3(center_offset, placement.axis);
    let center_off_axis = sub3(center_offset, scale3(placement.axis, center_distance));
    let enclosing_radius = center_distance.hypot(circle_radius);
    let tolerance = sphere.radius.max(circle_radius).max(1.0) * 1.0e-5;
    if length3(center_off_axis) > tolerance || (enclosing_radius - sphere.radius).abs() > tolerance
    {
        return Err(ImportError::InvalidData(format!(
            "#{edge_id} CIRCLE boundary does not lie on SPHERICAL_SURFACE"
        )));
    }
    Ok(())
}

/// Resolves and validates the radii of a non-singular ring torus.
fn resolve_torus(
    surface: &StepRecord,
    records: &HashMap<usize, &StepRecord>,
) -> Result<TorusGeometry, ImportError> {
    let args = require_args(surface, 4)?;
    let placement_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{} TOROIDAL_SURFACE has no placement reference",
            surface.id
        ))
    })?;
    let major_radius = parse_positive_scalar(args[2], surface.id, "TOROIDAL_SURFACE major radius")?;
    let minor_radius = parse_positive_scalar(args[3], surface.id, "TOROIDAL_SURFACE minor radius")?;
    if major_radius <= minor_radius {
        return Err(unsupported(format!(
            "#{} TOROIDAL_SURFACE is not a regular ring torus; horn and spindle tori are unsupported",
            surface.id
        )));
    }
    Ok(TorusGeometry {
        placement: resolve_axis2_placement(placement_id, records)?,
        major_radius,
        minor_radius,
    })
}

/// Restricts torus boundaries to canonical parallels and meridians.
fn validate_toroidal_loop_geometry(
    loop_id: usize,
    torus: TorusGeometry,
    records: &HashMap<usize, &StepRecord>,
    face_id: usize,
) -> Result<(), ImportError> {
    let loop_record = require_record(records, loop_id, "toroidal face loop")?;
    if loop_record.kind != "EDGE_LOOP" {
        return Err(unsupported(format!(
            "#{face_id} TOROIDAL_SURFACE requires EDGE_LOOP topology with CIRCLE edges"
        )));
    }
    for resolved in resolve_loop_edge_geometries(loop_record, records, "toroidal edge")? {
        if resolved.geometry.kind != "CIRCLE" {
            return Err(unsupported(format!(
                "#{} TOROIDAL_SURFACE boundary uses {}; only meridian and parallel CIRCLE edges are supported",
                resolved.edge.id, resolved.geometry.kind
            )));
        }
        validate_toroidal_circle_edge(resolved.edge.id, resolved.geometry, torus, records)?;
    }
    Ok(())
}

/// Validates that a circle is a constant-major-angle meridian or constant-minor-angle parallel.
fn validate_toroidal_circle_edge(
    edge_id: usize,
    circle: &StepRecord,
    torus: TorusGeometry,
    records: &HashMap<usize, &StepRecord>,
) -> Result<(), ImportError> {
    let args = require_args(circle, 3)?;
    let placement_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{} CIRCLE has no placement reference", circle.id))
    })?;
    let placement = resolve_axis2_placement(placement_id, records)?;
    let circle_radius = parse_positive_scalar(args[2], circle.id, "CIRCLE radius")?;
    let center_offset = sub3(placement.origin, torus.placement.origin);
    let center_height = dot3(center_offset, torus.placement.axis);
    let center_radial = sub3(center_offset, scale3(torus.placement.axis, center_height));
    let center_radial_length = length3(center_radial);
    let tolerance = (torus.major_radius + torus.minor_radius)
        .max(circle_radius)
        .max(1.0)
        * 1.0e-5;

    let is_parallel = length3(cross3(placement.axis, torus.placement.axis)) <= 1.0e-5
        && center_radial_length <= tolerance
        && ((circle_radius - torus.major_radius).hypot(center_height) - torus.minor_radius).abs()
            <= tolerance;

    let is_meridian = if center_height.abs() <= tolerance
        && (center_radial_length - torus.major_radius).abs() <= tolerance
        && (circle_radius - torus.minor_radius).abs() <= tolerance
    {
        let radial_direction = normalize3(center_radial).ok_or_else(|| {
            ImportError::InvalidData(format!(
                "#{edge_id} CIRCLE boundary has no torus meridian direction"
            ))
        })?;
        let tangent =
            normalize3(cross3(torus.placement.axis, radial_direction)).ok_or_else(|| {
                ImportError::InvalidData(format!(
                    "#{edge_id} CIRCLE boundary has an invalid torus meridian plane"
                ))
            })?;
        dot3(placement.axis, tangent).abs() >= 1.0 - 1.0e-5
    } else {
        false
    };

    if !is_parallel && !is_meridian {
        return Err(ImportError::InvalidData(format!(
            "#{edge_id} CIRCLE boundary is neither a meridian nor a parallel of TOROIDAL_SURFACE"
        )));
    }
    Ok(())
}

fn resolve_revolved_surface(
    surface: &StepRecord,
    records: &HashMap<usize, &StepRecord>,
    plane_angle_unit: Option<&StepResolvedUnit>,
) -> Result<RevolvedSurfaceGeometry, ImportError> {
    let minimum_args = if surface.kind == "CONICAL_SURFACE" {
        4
    } else {
        3
    };
    let args = require_args(surface, minimum_args)?;
    let placement_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{} {} has no placement reference",
            surface.id, surface.kind
        ))
    })?;
    let placement = resolve_axis2_placement(placement_id, records)?;
    let reference_radius =
        parse_positive_scalar(args[2], surface.id, &format!("{} radius", surface.kind))?;
    let (radial_slope, kind) = match surface.kind.as_str() {
        "CYLINDRICAL_SURFACE" => (0.0, RevolvedSurfaceKind::Cylinder),
        "CONICAL_SURFACE" => {
            let unit = plane_angle_unit.ok_or_else(|| {
                ImportError::InvalidData(format!(
                    "#{} CONICAL_SURFACE requires an explicit STEP plane-angle unit",
                    surface.id
                ))
            })?;
            let source_angle =
                parse_positive_scalar(args[3], surface.id, "CONICAL_SURFACE semi-angle")?;
            let angle = source_angle * unit.scale_to_si;
            if !angle.is_finite()
                || angle <= GEOMETRY_EPSILON
                || angle >= std::f32::consts::FRAC_PI_2 - GEOMETRY_EPSILON
            {
                return Err(ImportError::InvalidData(format!(
                    "#{} CONICAL_SURFACE semi-angle must resolve between 0 and pi/2 radians",
                    surface.id
                )));
            }
            (angle.tan(), RevolvedSurfaceKind::Cone)
        }
        _ => {
            return Err(ImportError::InvalidData(format!(
                "#{} is not a supported surface of revolution",
                surface.id
            )));
        }
    };
    Ok(RevolvedSurfaceGeometry {
        placement,
        reference_radius,
        radial_slope,
        kind,
    })
}

fn validate_revolved_loop_geometry(
    loop_id: usize,
    surface: RevolvedSurfaceGeometry,
    records: &HashMap<usize, &StepRecord>,
    face_id: usize,
) -> Result<(), ImportError> {
    let loop_record = require_record(records, loop_id, "surface-of-revolution face loop")?;
    if loop_record.kind != "EDGE_LOOP" {
        return Err(unsupported(format!(
            "#{face_id} {} requires EDGE_LOOP topology with supported curve edges",
            surface.kind.step_name()
        )));
    }
    for resolved in
        resolve_loop_edge_geometries(loop_record, records, "surface-of-revolution edge")?
    {
        match resolved.geometry.kind.as_str() {
            "LINE" => {
                validate_revolved_line_edge(resolved.edge, resolved.geometry, surface, records)?
            }
            "CIRCLE" => validate_revolved_circle_edge(
                resolved.edge.id,
                resolved.geometry,
                surface,
                records,
            )?,
            _ if is_bspline_curve_with_knots(resolved.geometry) => {
                // B-Spline samples are validated against the surface before
                // triangulation, so this topology pass only needs to admit the
                // curve family after endpoint and resource checks have run.
            }
            kind => {
                return Err(unsupported(format!(
                    "#{} uses unsupported {} edge geometry {kind}",
                    resolved.edge.id,
                    surface.kind.step_name()
                )));
            }
        }
    }
    Ok(())
}

fn validate_revolved_line_edge(
    edge: &StepRecord,
    line: &StepRecord,
    surface: RevolvedSurfaceGeometry,
    records: &HashMap<usize, &StepRecord>,
) -> Result<(), ImportError> {
    let edge_args = require_args(edge, 5)?;
    let start_id = parse_reference(edge_args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{} EDGE_CURVE has no start vertex", edge.id))
    })?;
    let start = resolve_vertex_point(start_id, records)?;
    let offset = sub3(start, surface.placement.origin);
    let height = dot3(offset, surface.placement.axis);
    let radial = sub3(offset, scale3(surface.placement.axis, height));
    let radial_direction = normalize3(radial).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{} LINE boundary starts at the surface axis",
            edge.id
        ))
    })?;
    let generator = normalize3(add3(
        surface.placement.axis,
        scale3(radial_direction, surface.radial_slope),
    ))
    .ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{} LINE boundary has an invalid generator direction",
            edge.id
        ))
    })?;

    let line_args = require_args(line, 3)?;
    let vector_id = parse_reference(line_args[2]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{} LINE has no vector reference", line.id))
    })?;
    let direction = resolve_vector_direction(vector_id, records)?;
    if length3(cross3(direction, generator)) > 1.0e-5 {
        let relation = match surface.kind {
            RevolvedSurfaceKind::Cylinder => "parallel to",
            RevolvedSurfaceKind::Cone => "a generator of",
        };
        return Err(ImportError::InvalidData(format!(
            "#{} LINE boundary is not {relation} {}",
            edge.id,
            surface.kind.step_name()
        )));
    }
    Ok(())
}

fn validate_revolved_circle_edge(
    edge_id: usize,
    circle: &StepRecord,
    surface: RevolvedSurfaceGeometry,
    records: &HashMap<usize, &StepRecord>,
) -> Result<(), ImportError> {
    let circle_args = require_args(circle, 3)?;
    let placement_id = parse_reference(circle_args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!("#{} CIRCLE has no placement reference", circle.id))
    })?;
    let placement = resolve_axis2_placement(placement_id, records)?;
    let circle_radius = parse_positive_scalar(circle_args[2], circle.id, "CIRCLE radius")?;
    let center_offset = sub3(placement.origin, surface.placement.origin);
    let center_height = dot3(center_offset, surface.placement.axis);
    let center_radial = sub3(center_offset, scale3(surface.placement.axis, center_height));
    let expected_radius = surface.radius_at_height(center_height);
    let tolerance = surface.reference_radius.max(circle_radius).max(1.0) * 1.0e-5;
    if expected_radius <= tolerance
        || length3(cross3(placement.axis, surface.placement.axis)) > 1.0e-5
        || length3(center_radial) > tolerance
        || (circle_radius - expected_radius).abs() > tolerance
    {
        return Err(ImportError::InvalidData(format!(
            "#{edge_id} CIRCLE boundary does not lie on {}",
            surface.kind.step_name()
        )));
    }
    Ok(())
}

fn resolve_axis2_placement(
    placement_id: usize,
    records: &HashMap<usize, &StepRecord>,
) -> Result<AxisPlacement, ImportError> {
    let placement = require_record(records, placement_id, "axis placement")?;
    if placement.kind != "AXIS2_PLACEMENT_3D" {
        return Err(ImportError::InvalidData(format!(
            "#{placement_id} is {}, expected AXIS2_PLACEMENT_3D",
            placement.kind
        )));
    }
    let args = require_args(placement, 3)?;
    let origin_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{placement_id} AXIS2_PLACEMENT_3D has no location"
        ))
    })?;
    let origin = resolve_cartesian_point(origin_id, records)?;
    let axis = if args[2].trim() == "$" {
        [0.0, 0.0, 1.0]
    } else {
        let direction_id = parse_reference(args[2]).ok_or_else(|| {
            ImportError::InvalidData(format!(
                "#{placement_id} AXIS2_PLACEMENT_3D has invalid axis"
            ))
        })?;
        resolve_direction(direction_id, records)?
    };
    let reference = if args.get(3).map(|value| value.trim()) == Some("$") || args.len() < 4 {
        default_reference_direction(axis)
    } else {
        let direction_id = parse_reference(args[3]).ok_or_else(|| {
            ImportError::InvalidData(format!(
                "#{placement_id} AXIS2_PLACEMENT_3D has invalid reference direction"
            ))
        })?;
        resolve_direction(direction_id, records)?
    };
    let projected_reference = sub3(reference, scale3(axis, dot3(reference, axis)));
    let x_axis = normalize3(projected_reference).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{placement_id} AXIS2_PLACEMENT_3D reference direction is parallel to its axis"
        ))
    })?;
    let y_axis = normalize3(cross3(axis, x_axis)).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{placement_id} AXIS2_PLACEMENT_3D has an invalid basis"
        ))
    })?;
    Ok(AxisPlacement {
        origin,
        axis,
        x_axis,
        y_axis,
    })
}

/// Resolves one STEP axis placement as a glTF-compatible local transform.
pub(super) fn resolve_axis2_transform(
    placement_id: usize,
    records: &HashMap<usize, &StepRecord>,
) -> Result<Transform, ImportError> {
    let placement = resolve_axis2_placement(placement_id, records)?;
    Ok([
        [
            placement.x_axis[0],
            placement.x_axis[1],
            placement.x_axis[2],
            0.0,
        ],
        [
            placement.y_axis[0],
            placement.y_axis[1],
            placement.y_axis[2],
            0.0,
        ],
        [placement.axis[0], placement.axis[1], placement.axis[2], 0.0],
        [
            placement.origin[0],
            placement.origin[1],
            placement.origin[2],
            1.0,
        ],
    ])
}

fn resolve_direction(
    direction_id: usize,
    records: &HashMap<usize, &StepRecord>,
) -> Result<[f32; 3], ImportError> {
    let direction = require_record(records, direction_id, "axis direction")?;
    if direction.kind != "DIRECTION" {
        return Err(ImportError::InvalidData(format!(
            "#{direction_id} is {}, expected DIRECTION",
            direction.kind
        )));
    }
    let args = require_args(direction, 2)?;
    let vector = parse_vec3(args[1], direction.id, "DIRECTION ratios")?;
    normalize3(vector).ok_or_else(|| {
        ImportError::InvalidData(format!("#{direction_id} DIRECTION has zero length"))
    })
}

fn validate_planarity(
    positions: &[[f32; 3]],
    origin: [f32; 3],
    normal: [f32; 3],
    face_id: usize,
) -> Result<(), ImportError> {
    let scale = positions
        .iter()
        .flat_map(|position| position.iter())
        .fold(1.0_f32, |scale, value| scale.max(value.abs()));
    let tolerance = scale * 1.0e-5;
    for position in positions {
        let distance = dot3(sub3(*position, origin), normal).abs();
        if !distance.is_finite() || distance > tolerance {
            return Err(ImportError::InvalidData(format!(
                "#{face_id} ADVANCED_FACE boundary is not planar within tolerance {tolerance}"
            )));
        }
    }
    Ok(())
}

/// Validates and triangulates one projected outer loop plus its inner loops.
fn triangulate_projected_face(
    projected: &[[f32; 2]],
    hole_indices: &[usize],
    face_id: usize,
) -> Result<Vec<u32>, ImportError> {
    let ranges = projected_loop_ranges(projected.len(), hole_indices, face_id)?;
    let scale = projected
        .iter()
        .flat_map(|point| point.iter())
        .fold(1.0_f32, |scale, value| scale.max(value.abs()));
    let linear_tolerance = scale * GEOMETRY_EPSILON;
    let cross_tolerance = scale * linear_tolerance;
    let area_tolerance = linear_tolerance * linear_tolerance;

    for (loop_index, range) in ranges.iter().enumerate() {
        let points = &projected[range.clone()];
        for (index, point) in points.iter().enumerate() {
            if !point[0].is_finite() || !point[1].is_finite() {
                return Err(ImportError::InvalidData(format!(
                    "#{face_id} ADVANCED_FACE contains a non-finite projected vertex"
                )));
            }
            if points.iter().enumerate().any(|(other_index, other)| {
                index != other_index && points2_equal(*point, *other, linear_tolerance)
            }) {
                return Err(ImportError::InvalidData(format!(
                    "#{face_id} ADVANCED_FACE boundary {} contains duplicate vertices",
                    loop_index + 1
                )));
            }
        }
        validate_simple_loop(
            points,
            linear_tolerance,
            cross_tolerance,
            face_id,
            loop_index,
        )?;
        let area = signed_area(points);
        if !area.is_finite() || area.abs() <= area_tolerance {
            return Err(ImportError::InvalidData(format!(
                "#{face_id} ADVANCED_FACE boundary {} is degenerate",
                loop_index + 1
            )));
        }
    }

    for first in 0..ranges.len() {
        for second in first + 1..ranges.len() {
            if loops_intersect(
                &projected[ranges[first].clone()],
                &projected[ranges[second].clone()],
                linear_tolerance,
                cross_tolerance,
            ) {
                return Err(ImportError::InvalidData(format!(
                    "#{face_id} ADVANCED_FACE boundaries intersect or touch"
                )));
            }
        }
    }

    let outer = &projected[ranges[0].clone()];
    for hole_index in 1..ranges.len() {
        let hole = &projected[ranges[hole_index].clone()];
        if point_in_polygon(hole[0], outer, linear_tolerance, cross_tolerance) != 1 {
            return Err(ImportError::InvalidData(format!(
                "#{face_id} ADVANCED_FACE inner boundary {} lies outside the outer boundary or touches it",
                hole_index
            )));
        }
        for other_index in 1..hole_index {
            let other = &projected[ranges[other_index].clone()];
            if point_in_polygon(hole[0], other, linear_tolerance, cross_tolerance) != 0
                || point_in_polygon(other[0], hole, linear_tolerance, cross_tolerance) != 0
            {
                return Err(ImportError::InvalidData(format!(
                    "#{face_id} ADVANCED_FACE inner boundaries overlap or are nested"
                )));
            }
        }
    }

    let flat = projected
        .iter()
        .flat_map(|point| [f64::from(point[0]), f64::from(point[1])])
        .collect::<Vec<_>>();
    let raw_indices = earcutr::earcut(&flat, hole_indices, 2).map_err(|error| {
        ImportError::InvalidData(format!(
            "#{face_id} ADVANCED_FACE constrained triangulation failed: {error}"
        ))
    })?;
    if raw_indices.is_empty() || raw_indices.len() % 3 != 0 {
        return Err(ImportError::InvalidData(format!(
            "#{face_id} ADVANCED_FACE constrained triangulation produced invalid indices"
        )));
    }
    validate_triangulated_area(projected, &ranges, &raw_indices, area_tolerance, face_id)?;
    raw_indices
        .into_iter()
        .map(|index| {
            u32::try_from(index).map_err(|_| {
                ImportError::InvalidData("STEP face vertex count exceeds u32".to_string())
            })
        })
        .collect()
}

/// Proves that generated triangles cover exactly the outer area minus all holes.
fn validate_triangulated_area(
    projected: &[[f32; 2]],
    ranges: &[Range<usize>],
    indices: &[usize],
    area_tolerance: f32,
    face_id: usize,
) -> Result<(), ImportError> {
    let expected_area = signed_area(&projected[ranges[0].clone()]).abs()
        - ranges
            .iter()
            .skip(1)
            .map(|range| signed_area(&projected[range.clone()]).abs())
            .sum::<f32>();
    if !expected_area.is_finite() || expected_area <= area_tolerance {
        return Err(ImportError::InvalidData(format!(
            "#{face_id} ADVANCED_FACE inner boundaries consume the outer boundary"
        )));
    }

    let mut actual_area = 0.0_f32;
    for triangle in indices.chunks_exact(3) {
        if triangle.iter().any(|index| *index >= projected.len()) {
            return Err(ImportError::InvalidData(format!(
                "#{face_id} ADVANCED_FACE constrained triangulation produced an out-of-range index"
            )));
        }
        let area = cross2(
            projected[triangle[0]],
            projected[triangle[1]],
            projected[triangle[2]],
        )
        .abs()
            * 0.5;
        if !area.is_finite() || area <= area_tolerance {
            return Err(ImportError::InvalidData(format!(
                "#{face_id} ADVANCED_FACE constrained triangulation produced a degenerate triangle"
            )));
        }
        actual_area += area;
    }
    let coverage_tolerance =
        expected_area * 1.0e-4 + area_tolerance * (projected.len() + indices.len()) as f32;
    if !actual_area.is_finite() || (actual_area - expected_area).abs() > coverage_tolerance {
        return Err(ImportError::InvalidData(format!(
            "#{face_id} ADVANCED_FACE constrained triangulation does not cover the bounded face"
        )));
    }
    Ok(())
}

/// Builds validated outer/inner ranges into the flattened projected vertex list.
fn projected_loop_ranges(
    vertex_count: usize,
    hole_indices: &[usize],
    face_id: usize,
) -> Result<Vec<Range<usize>>, ImportError> {
    let mut starts = Vec::with_capacity(hole_indices.len() + 1);
    starts.push(0);
    starts.extend_from_slice(hole_indices);
    let mut ranges = Vec::with_capacity(starts.len());
    for (index, start) in starts.iter().copied().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(vertex_count);
        if start >= end || end > vertex_count || end - start < 3 {
            return Err(ImportError::InvalidData(format!(
                "#{face_id} ADVANCED_FACE has an invalid boundary vertex range"
            )));
        }
        ranges.push(start..end);
    }
    Ok(ranges)
}

/// Rejects self-intersecting or self-touching polygon loops.
fn validate_simple_loop(
    points: &[[f32; 2]],
    linear_tolerance: f32,
    cross_tolerance: f32,
    face_id: usize,
    loop_index: usize,
) -> Result<(), ImportError> {
    for first in 0..points.len() {
        let first_next = (first + 1) % points.len();
        for second in first + 1..points.len() {
            let second_next = (second + 1) % points.len();
            if first == second
                || first_next == second
                || second_next == first
                || (first == 0 && second_next == 0)
            {
                continue;
            }
            if segments_intersect_or_touch(
                points[first],
                points[first_next],
                points[second],
                points[second_next],
                linear_tolerance,
                cross_tolerance,
            ) {
                return Err(ImportError::InvalidData(format!(
                    "#{face_id} ADVANCED_FACE boundary {} is self-intersecting",
                    loop_index + 1
                )));
            }
        }
    }
    Ok(())
}

/// Reports whether any segments from two distinct loops intersect or touch.
fn loops_intersect(
    first: &[[f32; 2]],
    second: &[[f32; 2]],
    linear_tolerance: f32,
    cross_tolerance: f32,
) -> bool {
    (0..first.len()).any(|first_index| {
        let first_next = (first_index + 1) % first.len();
        (0..second.len()).any(|second_index| {
            let second_next = (second_index + 1) % second.len();
            segments_intersect_or_touch(
                first[first_index],
                first[first_next],
                second[second_index],
                second[second_next],
                linear_tolerance,
                cross_tolerance,
            )
        })
    })
}

/// Tests two closed line segments with separate linear and cross-product tolerances.
fn segments_intersect_or_touch(
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    d: [f32; 2],
    linear_tolerance: f32,
    cross_tolerance: f32,
) -> bool {
    let ab_c = cross2(a, b, c);
    let ab_d = cross2(a, b, d);
    let cd_a = cross2(c, d, a);
    let cd_b = cross2(c, d, b);
    if ((ab_c > cross_tolerance && ab_d < -cross_tolerance)
        || (ab_c < -cross_tolerance && ab_d > cross_tolerance))
        && ((cd_a > cross_tolerance && cd_b < -cross_tolerance)
            || (cd_a < -cross_tolerance && cd_b > cross_tolerance))
    {
        return true;
    }
    (ab_c.abs() <= cross_tolerance && point_on_segment(c, a, b, linear_tolerance))
        || (ab_d.abs() <= cross_tolerance && point_on_segment(d, a, b, linear_tolerance))
        || (cd_a.abs() <= cross_tolerance && point_on_segment(a, c, d, linear_tolerance))
        || (cd_b.abs() <= cross_tolerance && point_on_segment(b, c, d, linear_tolerance))
}

/// Tests whether a collinear point lies within a closed segment's bounds.
fn point_on_segment(point: [f32; 2], start: [f32; 2], end: [f32; 2], tolerance: f32) -> bool {
    point[0] >= start[0].min(end[0]) - tolerance
        && point[0] <= start[0].max(end[0]) + tolerance
        && point[1] >= start[1].min(end[1]) - tolerance
        && point[1] <= start[1].max(end[1]) + tolerance
}

/// Returns `1` inside, `0` outside, and `-1` on the polygon boundary.
fn point_in_polygon(
    point: [f32; 2],
    polygon: &[[f32; 2]],
    linear_tolerance: f32,
    cross_tolerance: f32,
) -> i8 {
    let mut inside = false;
    for index in 0..polygon.len() {
        let next = (index + 1) % polygon.len();
        let a = polygon[index];
        let b = polygon[next];
        if cross2(a, b, point).abs() <= cross_tolerance
            && point_on_segment(point, a, b, linear_tolerance)
        {
            return -1;
        }
        if (a[1] > point[1]) != (b[1] > point[1])
            && point[0] < (b[0] - a[0]) * (point[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
    }
    i8::from(inside)
}

/// Moves an independently unwrapped inner loop to the nearest outer-loop period.
fn align_periodic_loop(points: &mut [[f32; 2]], outer: &[[f32; 2]], periods: [Option<f32>; 2]) {
    let outer_center = polygon_center(outer);
    let center = polygon_center(points);
    for dimension in 0..2 {
        let Some(period) = periods[dimension] else {
            continue;
        };
        let shift = ((outer_center[dimension] - center[dimension]) / period).round() * period;
        for point in &mut *points {
            point[dimension] += shift;
        }
    }
}

/// Computes the arithmetic center used only for periodic-domain alignment.
fn polygon_center(points: &[[f32; 2]]) -> [f32; 2] {
    let sum = points.iter().copied().fold([0.0, 0.0], |sum, point| {
        [sum[0] + point[0], sum[1] + point[1]]
    });
    [sum[0] / points.len() as f32, sum[1] / points.len() as f32]
}

fn orient_triangles(
    indices: &mut [u32],
    positions: &[[f32; 3]],
    normal: [f32; 3],
    face_id: usize,
) -> Result<(), ImportError> {
    for triangle in indices.chunks_exact_mut(3) {
        let a = positions[triangle[0] as usize];
        let b = positions[triangle[1] as usize];
        let c = positions[triangle[2] as usize];
        let triangle_normal = cross3(sub3(b, a), sub3(c, a));
        let alignment = dot3(triangle_normal, normal);
        if !alignment.is_finite() || alignment.abs() <= GEOMETRY_EPSILON {
            return Err(ImportError::InvalidData(format!(
                "#{face_id} ADVANCED_FACE produced a degenerate triangle"
            )));
        }
        if alignment < 0.0 {
            triangle.swap(1, 2);
        }
    }
    Ok(())
}

fn orient_revolved_triangles(
    indices: &mut [u32],
    positions: &[[f32; 3]],
    surface: RevolvedSurfaceGeometry,
    same_sense: bool,
    face_id: usize,
) -> Result<(), ImportError> {
    for triangle in indices.chunks_exact_mut(3) {
        let a = positions[triangle[0] as usize];
        let b = positions[triangle[1] as usize];
        let c = positions[triangle[2] as usize];
        let triangle_normal = cross3(sub3(b, a), sub3(c, a));
        let center = scale3(add3(add3(a, b), c), 1.0 / 3.0);
        let offset = sub3(center, surface.placement.origin);
        let radial = sub3(
            offset,
            scale3(surface.placement.axis, dot3(offset, surface.placement.axis)),
        );
        let radial_direction = normalize3(radial).ok_or_else(|| {
            ImportError::InvalidData(format!(
                "#{face_id} {} triangle center lies on the surface axis",
                surface.kind.step_name()
            ))
        })?;
        let mut desired = sub3(
            radial_direction,
            scale3(surface.placement.axis, surface.radial_slope),
        );
        if !same_sense {
            desired = scale3(desired, -1.0);
        }
        let alignment = dot3(triangle_normal, desired);
        if !alignment.is_finite() || alignment.abs() <= GEOMETRY_EPSILON {
            return Err(ImportError::InvalidData(format!(
                "#{face_id} {} produced a degenerate triangle",
                surface.kind.step_name()
            )));
        }
        if alignment < 0.0 {
            triangle.swap(1, 2);
        }
    }
    Ok(())
}

fn orient_spherical_triangles(
    indices: &mut [u32],
    positions: &[[f32; 3]],
    sphere: SphereGeometry,
    same_sense: bool,
    face_id: usize,
) -> Result<(), ImportError> {
    for triangle in indices.chunks_exact_mut(3) {
        let a = positions[triangle[0] as usize];
        let b = positions[triangle[1] as usize];
        let c = positions[triangle[2] as usize];
        let triangle_normal = cross3(sub3(b, a), sub3(c, a));
        let center = scale3(add3(add3(a, b), c), 1.0 / 3.0);
        let mut desired = sub3(center, sphere.placement.origin);
        if !same_sense {
            desired = scale3(desired, -1.0);
        }
        let alignment = dot3(triangle_normal, desired);
        if !alignment.is_finite() || alignment.abs() <= GEOMETRY_EPSILON {
            return Err(ImportError::InvalidData(format!(
                "#{face_id} SPHERICAL_SURFACE produced a degenerate triangle"
            )));
        }
        if alignment < 0.0 {
            triangle.swap(1, 2);
        }
    }
    Ok(())
}

/// Orients torus triangles against the outward tube normal at each triangle center.
fn orient_toroidal_triangles(
    indices: &mut [u32],
    positions: &[[f32; 3]],
    torus: TorusGeometry,
    same_sense: bool,
    face_id: usize,
) -> Result<(), ImportError> {
    for triangle in indices.chunks_exact_mut(3) {
        let a = positions[triangle[0] as usize];
        let b = positions[triangle[1] as usize];
        let c = positions[triangle[2] as usize];
        let triangle_normal = cross3(sub3(b, a), sub3(c, a));
        let center = scale3(add3(add3(a, b), c), 1.0 / 3.0);
        let offset = sub3(center, torus.placement.origin);
        let height = dot3(offset, torus.placement.axis);
        let radial = sub3(offset, scale3(torus.placement.axis, height));
        let radial_direction = normalize3(radial).ok_or_else(|| {
            ImportError::InvalidData(format!(
                "#{face_id} TOROIDAL_SURFACE triangle center lies on the surface axis"
            ))
        })?;
        let centerline = add3(
            torus.placement.origin,
            scale3(radial_direction, torus.major_radius),
        );
        let mut desired = sub3(center, centerline);
        if !same_sense {
            desired = scale3(desired, -1.0);
        }
        let alignment = dot3(triangle_normal, desired);
        if !alignment.is_finite() || alignment.abs() <= GEOMETRY_EPSILON {
            return Err(ImportError::InvalidData(format!(
                "#{face_id} TOROIDAL_SURFACE produced a degenerate triangle"
            )));
        }
        if alignment < 0.0 {
            triangle.swap(1, 2);
        }
    }
    Ok(())
}

fn project_polygon(positions: &[[f32; 3]], normal: [f32; 3]) -> Vec<[f32; 2]> {
    let axis = if normal[0].abs() >= normal[1].abs() && normal[0].abs() >= normal[2].abs() {
        0
    } else if normal[1].abs() >= normal[2].abs() {
        1
    } else {
        2
    };
    positions
        .iter()
        .map(|position| match axis {
            0 => [position[1], position[2]],
            1 => [position[0], position[2]],
            _ => [position[0], position[1]],
        })
        .collect()
}

fn signed_area(points: &[[f32; 2]]) -> f32 {
    let mut area = 0.0;
    for index in 0..points.len() {
        let next = (index + 1) % points.len();
        area += points[index][0] * points[next][1] - points[next][0] * points[index][1];
    }
    area * 0.5
}

fn cross2(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn points2_equal(a: [f32; 2], b: [f32; 2], tolerance: f32) -> bool {
    (a[0] - b[0]).abs() <= tolerance && (a[1] - b[1]).abs() <= tolerance
}

fn geometry_tolerance(positions: &[[f32; 3]]) -> f32 {
    positions
        .iter()
        .flat_map(|position| position.iter())
        .fold(1.0_f32, |scale, value| scale.max(value.abs()))
        * 1.0e-5
}

fn parse_positive_scalar(value: &str, record_id: usize, label: &str) -> Result<f32, ImportError> {
    let values = parse_float_list(value);
    if values.len() != 1 || !values[0].is_finite() || values[0] <= 0.0 {
        return Err(ImportError::InvalidData(format!(
            "#{record_id} {label} must be finite and positive"
        )));
    }
    Ok(values[0])
}

fn parse_step_usize_scalar(
    value: &str,
    record_id: usize,
    label: &str,
) -> Result<usize, ImportError> {
    value.trim().parse::<usize>().map_err(|error| {
        ImportError::InvalidData(format!("#{record_id} {label} must be an integer: {error}"))
    })
}

fn strip_outer_step_parentheses(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('(') && value.ends_with(')') {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn validate_circle_point(
    point: [f32; 3],
    origin: [f32; 3],
    axis: [f32; 3],
    radius: f32,
    edge_id: usize,
    circle_id: usize,
) -> Result<(), ImportError> {
    let offset = sub3(point, origin);
    let axial_distance = dot3(offset, axis).abs();
    let radial = sub3(offset, scale3(axis, dot3(offset, axis)));
    let scale = radius.max(length3(offset)).max(1.0);
    let tolerance = scale * 1.0e-5;
    if axial_distance > tolerance || (length3(radial) - radius).abs() > tolerance {
        return Err(ImportError::InvalidData(format!(
            "#{edge_id} EDGE_CURVE vertex does not lie on CIRCLE #{circle_id}"
        )));
    }
    Ok(())
}

fn directed_angle_delta(start: f32, end: f32, increases: bool) -> f32 {
    let full_turn = std::f32::consts::TAU;
    if increases {
        (end - start).rem_euclid(full_turn)
    } else {
        -(start - end).rem_euclid(full_turn)
    }
}

fn unwrap_angle(previous: Option<f32>, angle: f32) -> f32 {
    let Some(previous) = previous else {
        return angle;
    };
    let full_turn = std::f32::consts::TAU;
    let turns = ((previous - angle) / full_turn).round();
    angle + turns * full_turn
}

fn default_reference_direction(axis: [f32; 3]) -> [f32; 3] {
    if axis[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn parse_vec3(value: &str, record_id: usize, label: &str) -> Result<[f32; 3], ImportError> {
    let values = parse_float_list(value);
    if values.len() != 3 || values.iter().any(|value| !value.is_finite()) {
        return Err(ImportError::InvalidData(format!(
            "#{record_id} {label} must contain exactly three finite values"
        )));
    }
    Ok([values[0], values[1], values[2]])
}

fn parse_step_bool(value: &str, record_id: usize, label: &str) -> Result<bool, ImportError> {
    match value.trim().to_ascii_uppercase().as_str() {
        ".T." => Ok(true),
        ".F." => Ok(false),
        _ => Err(ImportError::InvalidData(format!(
            "#{record_id} {label} must be .T. or .F."
        ))),
    }
}

fn require_args(record: &StepRecord, minimum: usize) -> Result<Vec<&str>, ImportError> {
    let args = split_top_level_args(&record.args);
    if args.len() < minimum {
        return Err(ImportError::InvalidData(format!(
            "#{} {} expects at least {minimum} arguments",
            record.id, record.kind
        )));
    }
    Ok(args)
}

fn require_component_args<'a>(
    record_id: usize,
    kind: &str,
    args: &'a str,
    minimum: usize,
) -> Result<Vec<&'a str>, ImportError> {
    let args = split_top_level_args(args);
    if args.len() < minimum {
        return Err(ImportError::InvalidData(format!(
            "#{record_id} {kind} expects at least {minimum} arguments"
        )));
    }
    Ok(args)
}

fn require_record<'a>(
    records: &'a HashMap<usize, &StepRecord>,
    id: usize,
    relation: &str,
) -> Result<&'a StepRecord, ImportError> {
    records.get(&id).copied().ok_or_else(|| {
        ImportError::InvalidData(format!("STEP {relation} references missing entity #{id}"))
    })
}

fn unsupported(reason: String) -> ImportError {
    ImportError::TessellationUnsupported {
        format: "STEP".to_string(),
        reason,
    }
}

fn points_equal(a: [f32; 3], b: [f32; 3]) -> bool {
    (a[0] - b[0]).abs() <= GEOMETRY_EPSILON
        && (a[1] - b[1]).abs() <= GEOMETRY_EPSILON
        && (a[2] - b[2]).abs() <= GEOMETRY_EPSILON
}

fn normalize3(value: [f32; 3]) -> Option<[f32; 3]> {
    let length = dot3(value, value).sqrt();
    if !length.is_finite() || length <= GEOMETRY_EPSILON {
        return None;
    }
    Some(scale3(value, 1.0 / length))
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale3(value: [f32; 3], scale: f32) -> [f32; 3] {
    [value[0] * scale, value[1] * scale, value[2] * scale]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length3(value: [f32; 3]) -> f32 {
    dot3(value, value).sqrt()
}
