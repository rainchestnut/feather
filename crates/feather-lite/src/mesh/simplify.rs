//! Deterministic triangle-budget simplification for lightweight previews.
//!
//! The primary path uses meshoptimizer's topology-aware edge-collapse
//! simplifier. A topology-relaxed pass is reserved for primitives where the
//! hard delivery budget cannot otherwise be met; callers surface that choice
//! in document warnings instead of silently dropping unrelated triangles.

use meshopt::{DecodePosition, SimplifyOptions, simplify_decoder, simplify_sloppy_decoder};

use crate::document::{LiteDocument, LitePrimitive};

const MAX_RELATIVE_SIMPLIFICATION_ERROR: f32 = 1.0;

/// Summary of one document-level triangle-budget operation.
pub(super) struct TriangleBudgetSummary {
    pub original_triangles: u64,
    pub dropped_primitives: usize,
    pub topology_relaxed_primitives: usize,
}

/// Applies a hard triangle budget while keeping non-empty primitives visible
/// whenever the total budget can reserve at least one triangle for each one.
pub(super) fn apply_triangle_budget(
    document: &mut LiteDocument,
    max_triangles: Option<u64>,
) -> Option<TriangleBudgetSummary> {
    let max_triangles = max_triangles?;
    let original_triangles = document.triangle_count();
    if max_triangles == 0 || original_triangles <= max_triangles {
        return None;
    }

    let paths = primitive_triangle_paths(document);
    if paths.is_empty() {
        return None;
    }

    let budgets = allocate_triangle_budget(&paths, original_triangles, max_triangles);
    let dropped_primitives = budgets.iter().filter(|budget| **budget == 0).count();
    let mut topology_relaxed_primitives = 0;
    for (path, budget) in paths.into_iter().zip(budgets) {
        let primitive = &mut document.meshes[path.mesh_index].primitives[path.primitive_index];
        topology_relaxed_primitives += usize::from(simplify_primitive_to_triangle_count(
            primitive,
            budget as usize,
        ));
    }

    Some(TriangleBudgetSummary {
        original_triangles,
        dropped_primitives,
        topology_relaxed_primitives,
    })
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
    let primitive_count = paths.len() as u64;
    if target < primitive_count {
        return allocate_scarce_triangle_budget(paths, target);
    }

    let mut budgets = vec![1; paths.len()];
    let remaining_target = target - primitive_count;
    let reducible_total = total_triangles - primitive_count;
    if remaining_target == 0 || reducible_total == 0 {
        return budgets;
    }

    let mut remainders = Vec::with_capacity(paths.len());
    let mut allocated = primitive_count;
    for (index, path) in paths.iter().enumerate() {
        let capacity = path.triangles - 1;
        let scaled = u128::from(capacity) * u128::from(remaining_target);
        let extra = (scaled / u128::from(reducible_total)) as u64;
        let remainder = (scaled % u128::from(reducible_total)) as u64;
        budgets[index] += extra;
        allocated += extra;
        remainders.push((index, remainder, capacity));
    }

    remainders.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| left.0.cmp(&right.0))
    });
    for (index, _, capacity) in remainders {
        if allocated >= target {
            break;
        }
        if budgets[index] < capacity + 1 {
            budgets[index] += 1;
            allocated += 1;
        }
    }
    budgets
}

fn allocate_scarce_triangle_budget(paths: &[PrimitiveTrianglePath], target: u64) -> Vec<u64> {
    let mut ranked = paths
        .iter()
        .enumerate()
        .map(|(index, path)| (index, path.triangles))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

    let mut budgets = vec![0; paths.len()];
    for (index, _) in ranked.into_iter().take(target as usize) {
        budgets[index] = 1;
    }
    budgets
}

/// Returns true when the topology-relaxed fallback was required.
fn simplify_primitive_to_triangle_count(
    primitive: &mut LitePrimitive,
    target_triangles: usize,
) -> bool {
    let total_triangles = primitive.triangle_count() as usize;
    if target_triangles >= total_triangles {
        return false;
    }
    if target_triangles == 0 {
        primitive.positions.clear();
        primitive.normals.clear();
        primitive.indices.clear();
        return false;
    }

    let positions = primitive
        .positions
        .iter()
        .copied()
        .map(MeshPosition)
        .collect::<Vec<_>>();
    let target_indices = target_triangles.saturating_mul(3);
    let mut indices = simplify_decoder(
        &primitive.indices,
        &positions,
        target_indices,
        MAX_RELATIVE_SIMPLIFICATION_ERROR,
        SimplifyOptions::None,
        None,
    );
    let topology_relaxed = indices.len() > target_indices;
    if topology_relaxed {
        indices = simplify_sloppy_decoder(
            &primitive.indices,
            &positions,
            target_indices,
            MAX_RELATIVE_SIMPLIFICATION_ERROR,
            None,
        );
    }

    compact_primitive_to_indices(primitive, &indices);
    topology_relaxed
}

fn compact_primitive_to_indices(primitive: &mut LitePrimitive, selected_indices: &[u32]) {
    let old_positions = std::mem::take(&mut primitive.positions);
    let old_normals = std::mem::take(&mut primitive.normals);
    let has_normals = old_normals.len() == old_positions.len();
    let mut remap = vec![None; old_positions.len()];

    primitive.indices = Vec::with_capacity(selected_indices.len());
    primitive.positions = Vec::with_capacity(old_positions.len().min(selected_indices.len()));
    primitive.normals = if has_normals {
        Vec::with_capacity(old_normals.len().min(selected_indices.len()))
    } else {
        Vec::new()
    };

    for old_index in selected_indices {
        let old_index_usize = *old_index as usize;
        let new_index = if let Some(new_index) = remap[old_index_usize] {
            new_index
        } else {
            let new_index = primitive.positions.len() as u32;
            primitive.positions.push(old_positions[old_index_usize]);
            if has_normals {
                primitive.normals.push(old_normals[old_index_usize]);
            }
            remap[old_index_usize] = Some(new_index);
            new_index
        };
        primitive.indices.push(new_index);
    }
}

#[derive(Clone, Copy)]
struct MeshPosition([f32; 3]);

impl DecodePosition for MeshPosition {
    fn decode_position(&self) -> [f32; 3] {
        self.0
    }
}
