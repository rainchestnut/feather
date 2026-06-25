//! Mesh cleaning for visualization data.

use std::collections::HashMap;

use crate::document::{LiteDocument, LitePrimitive};
use crate::mesh::validate::is_degenerate_triangle;

/// Options for deterministic mesh cleanup.
#[derive(Debug, Clone)]
pub struct MeshOptions {
    pub weld_vertices: bool,
    pub position_epsilon: f32,
    pub rebuild_missing_normals: bool,
    pub position_quantization_step: Option<f32>,
    pub max_triangles: Option<u64>,
}

impl Default for MeshOptions {
    fn default() -> Self {
        Self {
            weld_vertices: true,
            position_epsilon: 0.0001,
            rebuild_missing_normals: true,
            position_quantization_step: None,
            max_triangles: None,
        }
    }
}

/// Optimizes every mesh primitive in a visual document.
pub fn optimize_document(document: &mut LiteDocument, options: &MeshOptions) {
    let mut removed_degenerate_triangles = 0_u64;
    for mesh in &mut document.meshes {
        for primitive in &mut mesh.primitives {
            ensure_indices(primitive);

            if let Some(step) = options.position_quantization_step {
                quantize_primitive_positions(primitive, step.max(f32::EPSILON));
            }

            if options.rebuild_missing_normals
                && primitive.normals.len() != primitive.positions.len()
            {
                rebuild_normals(primitive);
            }

            if options.weld_vertices {
                weld_primitive_vertices(primitive, options.position_epsilon.max(f32::EPSILON));
            }

            removed_degenerate_triangles += remove_degenerate_triangles(primitive);
        }
    }

    if let Some(step) = options.position_quantization_step {
        document.metadata.warnings.push(format!(
            "quantized mesh positions to grid step {}",
            format_step(step)
        ));
    }
    if removed_degenerate_triangles > 0 {
        document.metadata.warnings.push(format!(
            "removed {removed_degenerate_triangles} degenerate triangles after mesh cleanup"
        ));
        prune_empty_geometry(document);
    }
    apply_triangle_budget(document, options.max_triangles);
    for mesh in &mut document.meshes {
        mesh.recompute_bbox();
    }
    document.refresh_metadata();
}

fn ensure_indices(primitive: &mut LitePrimitive) {
    if primitive.indices.is_empty() {
        primitive.indices = (0..primitive.positions.len() as u32).collect();
    }
}

fn rebuild_normals(primitive: &mut LitePrimitive) {
    primitive.normals = vec![[0.0, 0.0, 0.0]; primitive.positions.len()];

    for triangle in primitive.indices.chunks_exact(3) {
        let a = triangle[0] as usize;
        let b = triangle[1] as usize;
        let c = triangle[2] as usize;
        if a >= primitive.positions.len()
            || b >= primitive.positions.len()
            || c >= primitive.positions.len()
        {
            continue;
        }

        let normal = face_normal(
            primitive.positions[a],
            primitive.positions[b],
            primitive.positions[c],
        );
        add_assign(&mut primitive.normals[a], normal);
        add_assign(&mut primitive.normals[b], normal);
        add_assign(&mut primitive.normals[c], normal);
    }

    for normal in &mut primitive.normals {
        *normal = normalize(*normal);
    }
}

fn weld_primitive_vertices(primitive: &mut LitePrimitive, epsilon: f32) {
    let mut remap = vec![0_u32; primitive.positions.len()];
    let mut unique_positions = Vec::new();
    let mut unique_normals = Vec::new();
    let mut seen = HashMap::<VertexKey, u32>::new();

    for (old_index, position) in primitive.positions.iter().enumerate() {
        let normal = primitive
            .normals
            .get(old_index)
            .copied()
            .unwrap_or([0.0; 3]);
        let key = VertexKey::from_position_normal(*position, normal, epsilon);
        let new_index = if let Some(existing) = seen.get(&key) {
            *existing
        } else {
            let index = unique_positions.len() as u32;
            unique_positions.push(*position);
            if !primitive.normals.is_empty() {
                unique_normals.push(normal);
            }
            seen.insert(key, index);
            index
        };
        remap[old_index] = new_index;
    }

    for index in &mut primitive.indices {
        if let Some(new_index) = remap.get(*index as usize) {
            *index = *new_index;
        }
    }

    primitive.positions = unique_positions;
    primitive.normals = unique_normals;
}

// Drops triangles that became invalid during cleanup, then compacts the vertex
// buffers so downstream GLB export only sees referenced geometry.
fn remove_degenerate_triangles(primitive: &mut LitePrimitive) -> u64 {
    if primitive.indices.is_empty() {
        return 0;
    }

    let old_positions = primitive.positions.clone();
    let old_normals = primitive.normals.clone();
    let has_normals = old_normals.len() == old_positions.len();
    let mut remap = HashMap::<u32, u32>::new();
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::with_capacity(primitive.indices.len());
    let mut removed = 0_u64;

    for triangle in primitive.indices.chunks_exact(3) {
        if is_degenerate_triangle(triangle, &old_positions) {
            removed += 1;
            continue;
        }

        for old_index in triangle {
            let new_index = if let Some(existing) = remap.get(old_index) {
                *existing
            } else {
                let old_position_index = *old_index as usize;
                let new_index = positions.len() as u32;
                positions.push(old_positions[old_position_index]);
                if has_normals {
                    normals.push(old_normals[old_position_index]);
                }
                remap.insert(*old_index, new_index);
                new_index
            };
            indices.push(new_index);
        }
    }

    if removed > 0 {
        primitive.positions = positions;
        primitive.normals = normals;
        primitive.indices = indices;
    }
    removed
}

fn apply_triangle_budget(document: &mut LiteDocument, max_triangles: Option<u64>) {
    let Some(max_triangles) = max_triangles else {
        return;
    };
    let current_triangles = document.triangle_count();
    if max_triangles == 0 || current_triangles <= max_triangles {
        return;
    }

    let paths = primitive_triangle_paths(document);
    if paths.is_empty() {
        return;
    }

    let budgets = allocate_triangle_budget(&paths, current_triangles, max_triangles);
    for (path, budget) in paths.into_iter().zip(budgets) {
        let primitive = &mut document.meshes[path.mesh_index].primitives[path.primitive_index];
        simplify_primitive_to_triangle_count(primitive, budget as usize);
    }

    prune_empty_geometry(document);
    let reduced_triangles = document.triangle_count();
    document.metadata.warnings.push(format!(
        "applied triangle budget LOD: reduced triangles from {current_triangles} to {reduced_triangles}"
    ));
}

#[derive(Debug, Clone, Copy)]
struct PrimitiveTrianglePath {
    mesh_index: usize,
    primitive_index: usize,
    triangles: u64,
}

fn primitive_triangle_paths(document: &LiteDocument) -> Vec<PrimitiveTrianglePath> {
    let mut paths = Vec::new();
    for (mesh_index, mesh) in document.meshes.iter().enumerate() {
        for (primitive_index, primitive) in mesh.primitives.iter().enumerate() {
            let triangles = primitive.triangle_count();
            if triangles > 0 {
                paths.push(PrimitiveTrianglePath {
                    mesh_index,
                    primitive_index,
                    triangles,
                });
            }
        }
    }
    paths
}

fn allocate_triangle_budget(
    paths: &[PrimitiveTrianglePath],
    total_triangles: u64,
    max_triangles: u64,
) -> Vec<u64> {
    let target = max_triangles.min(total_triangles);
    let mut budgets = Vec::with_capacity(paths.len());
    let mut remainders = Vec::with_capacity(paths.len());
    let mut allocated = 0_u64;

    for (index, path) in paths.iter().enumerate() {
        let scaled = u128::from(path.triangles) * u128::from(target);
        let budget = (scaled / u128::from(total_triangles)) as u64;
        let remainder = (scaled % u128::from(total_triangles)) as u64;
        budgets.push(budget);
        remainders.push((index, remainder, path.triangles));
        allocated += budget;
    }

    remainders.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| left.0.cmp(&right.0))
    });

    for (index, _, _) in remainders {
        if allocated >= target {
            break;
        }
        budgets[index] += 1;
        allocated += 1;
    }

    budgets
}

fn simplify_primitive_to_triangle_count(primitive: &mut LitePrimitive, target_triangles: usize) {
    let total_triangles = primitive.triangle_count() as usize;
    if target_triangles >= total_triangles {
        return;
    }
    if target_triangles == 0 {
        primitive.positions.clear();
        primitive.normals.clear();
        primitive.indices.clear();
        return;
    }

    let old_positions = primitive.positions.clone();
    let old_normals = primitive.normals.clone();
    let old_indices = primitive.indices.clone();
    let selected_triangles = evenly_spaced_triangle_indices(total_triangles, target_triangles);
    let has_normals = old_normals.len() == old_positions.len();
    let mut remap = HashMap::<u32, u32>::new();
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::with_capacity(target_triangles * 3);

    for triangle_index in selected_triangles {
        let start = triangle_index * 3;
        for old_index in &old_indices[start..start + 3] {
            let new_index = if let Some(existing) = remap.get(old_index) {
                *existing
            } else {
                let old_position_index = *old_index as usize;
                let new_index = positions.len() as u32;
                positions.push(old_positions[old_position_index]);
                if has_normals {
                    normals.push(old_normals[old_position_index]);
                }
                remap.insert(*old_index, new_index);
                new_index
            };
            indices.push(new_index);
        }
    }

    primitive.positions = positions;
    primitive.normals = normals;
    primitive.indices = indices;
}

fn quantize_primitive_positions(primitive: &mut LitePrimitive, step: f32) {
    for position in &mut primitive.positions {
        *position = [
            quantize_f32(position[0], step),
            quantize_f32(position[1], step),
            quantize_f32(position[2], step),
        ];
    }
}

fn quantize_f32(value: f32, step: f32) -> f32 {
    if value.is_finite() {
        (value / step).round() * step
    } else {
        0.0
    }
}

fn format_step(step: f32) -> String {
    if step.is_finite() {
        format!("{step:.7}")
    } else {
        "0.0".to_string()
    }
}

fn evenly_spaced_triangle_indices(total_triangles: usize, target_triangles: usize) -> Vec<usize> {
    let mut selected = Vec::with_capacity(target_triangles);
    let mut last = None;
    for index in 0..target_triangles {
        let candidate = index * total_triangles / target_triangles;
        if last == Some(candidate) {
            continue;
        }
        selected.push(candidate);
        last = Some(candidate);
    }

    let mut candidate = 0;
    while selected.len() < target_triangles {
        if !selected.contains(&candidate) {
            selected.push(candidate);
        }
        candidate += 1;
    }
    selected.sort_unstable();
    selected
}

fn prune_empty_geometry(document: &mut LiteDocument) {
    let mut mesh_remap = vec![None; document.meshes.len()];
    let mut meshes = Vec::new();

    for (mesh_index, mut mesh) in document.meshes.drain(..).enumerate() {
        mesh.primitives
            .retain(|primitive| primitive.triangle_count() > 0 && !primitive.positions.is_empty());
        if mesh.primitives.is_empty() {
            continue;
        }
        mesh_remap[mesh_index] = Some(meshes.len());
        meshes.push(mesh);
    }

    for node in &mut document.nodes {
        if let Some(mesh_index) = node.mesh {
            node.mesh = mesh_remap.get(mesh_index).copied().flatten();
        }
    }

    document.meshes = meshes;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct VertexKey {
    position: [i64; 3],
    normal: [i64; 3],
}

impl VertexKey {
    fn from_position_normal(position: [f32; 3], normal: [f32; 3], epsilon: f32) -> Self {
        Self {
            position: quantize_vec3(position, epsilon),
            normal: quantize_vec3(normal, 0.0001),
        }
    }
}

fn quantize_vec3(value: [f32; 3], epsilon: f32) -> [i64; 3] {
    [
        (value[0] / epsilon).round() as i64,
        (value[1] / epsilon).round() as i64,
        (value[2] / epsilon).round() as i64,
    ]
}

fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    normalize(cross(ab, ac))
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn add_assign(target: &mut [f32; 3], value: [f32; 3]) {
    target[0] += value[0];
    target[1] += value[1];
    target[2] += value[2];
}

fn normalize(value: [f32; 3]) -> [f32; 3] {
    let length = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt();
    if length <= f32::EPSILON {
        [0.0, 0.0, 1.0]
    } else {
        [value[0] / length, value[1] / length, value[2] / length]
    }
}
