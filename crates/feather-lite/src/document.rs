//! Feather Lite scene document types.
//!
//! The IR is intentionally visual-only: it carries scene hierarchy, triangle
//! meshes, simple materials, and source metadata needed for preview/export.
//! It does not preserve B-Rep topology, feature trees, or editable CAD data.

/// Transform matrix stored in glTF-compatible column-major order.
pub type Transform = [[f32; 4]; 4];

/// Returns an identity transform for scene nodes.
pub fn identity_transform() -> Transform {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Axis-aligned bounds for a mesh or primitive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    /// Creates an empty bounding box that can be expanded by points.
    pub fn empty() -> Self {
        Self {
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        }
    }

    /// Creates a bounding box from a slice of positions.
    pub fn from_positions(positions: &[[f32; 3]]) -> Self {
        let mut bbox = Self::empty();
        for position in positions {
            bbox.include_point(*position);
        }
        bbox
    }

    /// Expands the box so it includes the given point.
    pub fn include_point(&mut self, point: [f32; 3]) {
        for (axis, value) in point.iter().enumerate() {
            self.min[axis] = self.min[axis].min(*value);
            self.max[axis] = self.max[axis].max(*value);
        }
    }

    /// Expands the box so it includes another box.
    pub fn include_box(&mut self, other: Aabb) {
        if other.is_empty() {
            return;
        }
        self.include_point(other.min);
        self.include_point(other.max);
    }

    /// Returns true when no point has been added.
    pub fn is_empty(&self) -> bool {
        self.min[0].is_infinite() || self.max[0].is_infinite()
    }

    /// Returns a glTF-safe min/max pair for empty or populated bounds.
    pub fn normalized(self) -> Self {
        if self.is_empty() {
            Self {
                min: [0.0, 0.0, 0.0],
                max: [0.0, 0.0, 0.0],
            }
        } else {
            self
        }
    }
}

impl Default for Aabb {
    fn default() -> Self {
        Self::empty()
    }
}

/// Project-level metadata emitted beside converted GLB files.
#[derive(Debug, Clone, PartialEq)]
pub struct LiteMetadata {
    pub source_format: String,
    pub mode: String,
    pub precision: String,
    pub mesh_count: usize,
    pub triangle_count: u64,
    pub has_brep: bool,
    pub brep_preserved: bool,
    pub source_path: Option<String>,
    pub warnings: Vec<String>,
}

impl LiteMetadata {
    /// Creates visual-only metadata for a detected source format.
    pub fn visual(source_format: impl Into<String>, mode: impl Into<String>) -> Self {
        Self {
            source_format: source_format.into(),
            mode: mode.into(),
            precision: "visual".to_string(),
            mesh_count: 0,
            triangle_count: 0,
            has_brep: false,
            brep_preserved: false,
            source_path: None,
            warnings: Vec::new(),
        }
    }
}

/// A lightweight CAD scene.
#[derive(Debug, Clone, PartialEq)]
pub struct LiteDocument {
    pub nodes: Vec<LiteNode>,
    pub meshes: Vec<LiteMesh>,
    pub materials: Vec<LiteMaterial>,
    pub metadata: LiteMetadata,
}

impl LiteDocument {
    /// Creates an empty visual document for a source format.
    pub fn new(source_format: impl Into<String>, mode: impl Into<String>) -> Self {
        Self {
            nodes: Vec::new(),
            meshes: Vec::new(),
            materials: Vec::new(),
            metadata: LiteMetadata::visual(source_format, mode),
        }
    }

    /// Recomputes derived metadata after importer or mesh pipeline changes.
    pub fn refresh_metadata(&mut self) {
        self.metadata.mesh_count = self.meshes.len();
        self.metadata.triangle_count = self.triangle_count();
    }

    /// Returns the total triangle count across all mesh primitives.
    pub fn triangle_count(&self) -> u64 {
        self.meshes.iter().map(LiteMesh::triangle_count).sum()
    }

    /// Returns the total primitive count across all meshes.
    pub fn primitive_count(&self) -> usize {
        self.meshes.iter().map(|mesh| mesh.primitives.len()).sum()
    }

    /// Returns the total position count across all mesh primitives.
    pub fn vertex_count(&self) -> usize {
        self.meshes
            .iter()
            .flat_map(|mesh| &mesh.primitives)
            .map(|primitive| primitive.positions.len())
            .sum()
    }

    /// Ensures that every mesh is reachable by at least one scene node.
    pub fn add_default_nodes_for_unreferenced_meshes(&mut self) {
        let mut referenced = vec![false; self.meshes.len()];
        for node in &self.nodes {
            if let Some(mesh) = node.mesh
                && mesh < referenced.len()
            {
                referenced[mesh] = true;
            }
        }

        for (mesh_index, mesh) in self.meshes.iter().enumerate() {
            if !referenced[mesh_index] {
                self.nodes
                    .push(LiteNode::new(mesh.name.clone(), Some(mesh_index)));
            }
        }
    }

    /// Appends another visual document under a new grouping node.
    ///
    /// Private CAD containers often expose each part preview as a separate
    /// stored stream. This method preserves the imported subdocument hierarchy
    /// while remapping material, mesh, and node indices into the receiving
    /// document.
    pub fn append_document_under_node(
        &mut self,
        group_name: impl Into<String>,
        mut document: LiteDocument,
    ) {
        document.add_default_nodes_for_unreferenced_meshes();

        let material_offset = self.materials.len();
        let mesh_offset = self.meshes.len();
        let group_index = self.nodes.len();
        let node_offset = group_index + 1;
        let roots = root_node_indices(&document.nodes);

        for mesh in &mut document.meshes {
            for primitive in &mut mesh.primitives {
                if let Some(material) = primitive.material {
                    primitive.material = Some(material + material_offset);
                }
            }
        }

        for node in &mut document.nodes {
            if let Some(mesh) = node.mesh {
                node.mesh = Some(mesh + mesh_offset);
            }
            for child in &mut node.children {
                *child += node_offset;
            }
        }

        let mut group = LiteNode::new(group_name, None);
        group.children = roots.into_iter().map(|index| index + node_offset).collect();

        self.metadata.warnings.extend(document.metadata.warnings);
        self.materials.extend(document.materials);
        self.meshes.extend(document.meshes);
        self.nodes.push(group);
        self.nodes.extend(document.nodes);
        self.refresh_metadata();
    }

    /// Appends another visual document below an existing assembly node.
    ///
    /// Cache-declared external references are represented by empty nodes with
    /// `source_id` set to the referenced file. Resolving the reference attaches
    /// the imported part roots under that node so the reference transform and
    /// parent/child relationship remain intact.
    pub fn append_document_to_node(
        &mut self,
        parent_node_index: usize,
        mut document: LiteDocument,
    ) {
        document.add_default_nodes_for_unreferenced_meshes();

        let material_offset = self.materials.len();
        let mesh_offset = self.meshes.len();
        let node_offset = self.nodes.len();
        let roots = root_node_indices(&document.nodes);

        for mesh in &mut document.meshes {
            for primitive in &mut mesh.primitives {
                if let Some(material) = primitive.material {
                    primitive.material = Some(material + material_offset);
                }
            }
        }

        for node in &mut document.nodes {
            if let Some(mesh) = node.mesh {
                node.mesh = Some(mesh + mesh_offset);
            }
            for child in &mut node.children {
                *child += node_offset;
            }
        }

        self.metadata.warnings.extend(document.metadata.warnings);
        self.materials.extend(document.materials);
        self.meshes.extend(document.meshes);
        self.nodes[parent_node_index]
            .children
            .extend(roots.into_iter().map(|index| index + node_offset));
        self.nodes.extend(document.nodes);
        self.refresh_metadata();
    }
}

fn root_node_indices(nodes: &[LiteNode]) -> Vec<usize> {
    let mut referenced = vec![false; nodes.len()];
    for node in nodes {
        for child in &node.children {
            if *child < referenced.len() {
                referenced[*child] = true;
            }
        }
    }

    referenced
        .iter()
        .enumerate()
        .filter_map(|(index, is_child)| (!is_child).then_some(index))
        .collect()
}

/// A scene node preserving assembly hierarchy and transforms.
#[derive(Debug, Clone, PartialEq)]
pub struct LiteNode {
    pub name: String,
    pub children: Vec<usize>,
    pub mesh: Option<usize>,
    pub transform: Transform,
    pub source_id: Option<String>,
}

impl LiteNode {
    /// Creates a node with identity transform.
    pub fn new(name: impl Into<String>, mesh: Option<usize>) -> Self {
        Self {
            name: name.into(),
            children: Vec::new(),
            mesh,
            transform: identity_transform(),
            source_id: None,
        }
    }
}

/// A mesh containing one or more material-compatible primitives.
#[derive(Debug, Clone, PartialEq)]
pub struct LiteMesh {
    pub name: String,
    pub primitives: Vec<LitePrimitive>,
    pub bbox: Aabb,
}

impl LiteMesh {
    /// Creates an empty mesh.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            primitives: Vec::new(),
            bbox: Aabb::empty(),
        }
    }

    /// Recomputes bounds from primitive positions.
    pub fn recompute_bbox(&mut self) {
        let mut bbox = Aabb::empty();
        for primitive in &self.primitives {
            bbox.include_box(Aabb::from_positions(&primitive.positions));
        }
        self.bbox = bbox;
    }

    /// Returns the triangle count for all primitives.
    pub fn triangle_count(&self) -> u64 {
        self.primitives
            .iter()
            .map(LitePrimitive::triangle_count)
            .sum()
    }
}

/// A triangle primitive that can be mapped directly to a glTF primitive.
#[derive(Debug, Clone, PartialEq)]
pub struct LitePrimitive {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
}

impl LitePrimitive {
    /// Creates an empty primitive for an optional material.
    pub fn new(material: Option<usize>) -> Self {
        Self {
            positions: Vec::new(),
            normals: Vec::new(),
            indices: Vec::new(),
            material,
        }
    }

    /// Returns triangle count from indexed or sequential geometry.
    pub fn triangle_count(&self) -> u64 {
        if self.indices.is_empty() {
            (self.positions.len() / 3) as u64
        } else {
            (self.indices.len() / 3) as u64
        }
    }
}

/// A simple PBR material suitable for CAD preview.
#[derive(Debug, Clone, PartialEq)]
pub struct LiteMaterial {
    pub name: String,
    pub base_color: [f32; 4],
}

impl LiteMaterial {
    /// Creates a material with an RGBA base color.
    pub fn new(name: impl Into<String>, base_color: [f32; 4]) -> Self {
        Self {
            name: name.into(),
            base_color,
        }
    }
}
