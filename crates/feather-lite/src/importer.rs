//! Importer registry and errors for CAD lightweight conversion.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::document::LiteDocument;
use crate::importers;
use crate::probe::{ProbeConfidence, ProbeResult};

/// Source file passed to importer probes and conversions.
pub struct InputFile<'a> {
    pub path: Option<&'a Path>,
    pub bytes: &'a [u8],
}

impl<'a> InputFile<'a> {
    /// Creates an input wrapper from an optional path and bytes.
    pub fn new(path: Option<&'a Path>, bytes: &'a [u8]) -> Self {
        Self { path, bytes }
    }
}

/// Options that control visual-only import.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub prefer_cache: bool,
    pub load_assembly: bool,
    pub load_materials: bool,
    /// Maximum STEP curve chord error in source-file length units; zero selects
    /// the radius-relative default.
    pub max_lod_error: f32,
    pub resolve_dirs: Vec<PathBuf>,
    pub reference_path_mappings: Vec<ReferencePathMapping>,
    pub max_reference_depth: usize,
    pub limits: ImportLimits,
}

/// Resource limits applied while reading and expanding one CAD input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportLimits {
    /// Maximum bytes read from one source file or accepted by a byte API.
    pub max_input_bytes: usize,
    /// Maximum number of non-empty OLE streams reconstructed from one input.
    pub max_ole_streams: usize,
    /// Maximum declared size accepted for one OLE stream.
    pub max_ole_stream_bytes: usize,
    /// Maximum cumulative declared size accepted across OLE streams.
    pub max_ole_total_stream_bytes: usize,
    /// Maximum number of ZIP entries accepted from one input or OLE stream.
    pub max_archive_entries: usize,
    /// Maximum uncompressed size accepted for one ZIP entry.
    pub max_archive_entry_uncompressed_bytes: usize,
    /// Maximum cumulative uncompressed size accepted across ZIP entries.
    pub max_archive_total_uncompressed_bytes: usize,
    /// Maximum line segments generated for one STEP curve edge.
    pub max_step_curve_segments: usize,
    /// Maximum polynomial degree accepted for one STEP B-Spline curve.
    pub max_step_spline_degree: usize,
    /// Maximum control points accepted for one STEP B-Spline curve.
    pub max_step_spline_control_points: usize,
    /// Maximum outer and inner boundary loops accepted for one STEP face.
    pub max_step_face_loops: usize,
    /// Maximum tessellated boundary vertices accepted across one STEP face.
    pub max_step_face_vertices: usize,
    /// Maximum instantiated scene nodes accepted from one STEP assembly.
    pub max_step_assembly_nodes: usize,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 1024 * 1024 * 1024,
            max_ole_streams: 16_384,
            max_ole_stream_bytes: 512 * 1024 * 1024,
            max_ole_total_stream_bytes: 1024 * 1024 * 1024,
            max_archive_entries: 4_096,
            max_archive_entry_uncompressed_bytes: 512 * 1024 * 1024,
            max_archive_total_uncompressed_bytes: 1024 * 1024 * 1024,
            max_step_curve_segments: 16_384,
            max_step_spline_degree: 16,
            max_step_spline_control_points: 16_384,
            max_step_face_loops: 1_024,
            max_step_face_vertices: 262_144,
            max_step_assembly_nodes: 100_000,
        }
    }
}

/// Maps an archived CAD reference path prefix to a local directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferencePathMapping {
    pub from: String,
    pub to: PathBuf,
}

impl ReferencePathMapping {
    /// Creates a reference path mapping from an old CAD/PDM prefix to a local root.
    pub fn new(from: impl Into<String>, to: impl Into<PathBuf>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            prefer_cache: true,
            load_assembly: true,
            load_materials: true,
            max_lod_error: 0.0,
            resolve_dirs: Vec::new(),
            reference_path_mappings: Vec::new(),
            max_reference_depth: 16,
            limits: ImportLimits::default(),
        }
    }
}

/// Error type for probing and import failures.
#[derive(Debug)]
pub enum ImportError {
    Unsupported(String),
    NoLightweightCache {
        format: String,
    },
    NativeVisualizationUnsupported {
        format: String,
        representation: &'static str,
    },
    ResourceLimitExceeded {
        resource: &'static str,
        limit: usize,
        actual: usize,
    },
    TessellationUnsupported {
        format: String,
        reason: String,
    },
    InvalidData(String),
    Io(std::io::Error),
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(message) => write!(formatter, "unsupported input: {message}"),
            Self::NoLightweightCache { format } => {
                write!(
                    formatter,
                    "{format} has no readable lightweight visualization cache"
                )
            }
            Self::NativeVisualizationUnsupported {
                format,
                representation,
            } => write!(
                formatter,
                "{format} contains native {representation} visualization, but its binary representation is not decoded by the open-source importer"
            ),
            Self::ResourceLimitExceeded {
                resource,
                limit,
                actual,
            } => write!(
                formatter,
                "resource limit exceeded for {resource}: {actual} exceeds {limit}"
            ),
            Self::TessellationUnsupported { format, reason } => {
                write!(
                    formatter,
                    "{format} tessellation is not implemented: {reason}"
                )
            }
            Self::InvalidData(message) => write!(formatter, "invalid source data: {message}"),
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
        }
    }
}

impl Error for ImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ImportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Rejects an in-memory source before probes or importers scan it.
pub(crate) fn ensure_input_size(bytes: &[u8], limits: &ImportLimits) -> Result<(), ImportError> {
    if bytes.len() > limits.max_input_bytes {
        return Err(ImportError::ResourceLimitExceeded {
            resource: "input bytes",
            limit: limits.max_input_bytes,
            actual: bytes.len(),
        });
    }
    Ok(())
}

/// Reads one source file without allocating beyond the configured input limit.
pub(crate) fn read_input_path(path: &Path, limits: &ImportLimits) -> Result<Vec<u8>, ImportError> {
    let file = fs::File::open(path)?;
    let limit = u64::try_from(limits.max_input_bytes).unwrap_or(u64::MAX);
    let metadata_size = file.metadata()?.len();
    if metadata_size > limit {
        return Err(ImportError::ResourceLimitExceeded {
            resource: "input bytes",
            limit: limits.max_input_bytes,
            actual: usize::try_from(metadata_size).unwrap_or(usize::MAX),
        });
    }

    // The extra byte closes the metadata/read race if the file grows after the
    // size check, while still bounding allocation and I/O.
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    ensure_input_size(&bytes, limits)?;
    Ok(bytes)
}

/// Trait implemented by all lightweight importers.
pub trait CadLiteImporter {
    /// Returns a short importer name for diagnostics.
    fn name(&self) -> &'static str;

    /// Probes an input without doing expensive conversion work.
    fn probe(&self, input: &InputFile<'_>) -> ProbeResult;

    /// Imports the input as a visual-only Feather Lite document.
    fn import_lite(
        &self,
        input: &InputFile<'_>,
        options: &ImportOptions,
    ) -> Result<LiteDocument, ImportError>;
}

/// Ordered importer collection used by CLI and tests.
pub struct ImporterRegistry {
    importers: Vec<Box<dyn CadLiteImporter>>,
}

impl ImporterRegistry {
    /// Creates a registry with all built-in importers.
    pub fn with_builtin_importers() -> Self {
        let mut registry = Self::empty();
        for importer in importers::builtin_importers() {
            registry.register(importer);
        }
        registry
    }

    /// Creates an empty registry.
    pub fn empty() -> Self {
        Self {
            importers: Vec::new(),
        }
    }

    /// Adds one importer to the registry.
    pub fn register(&mut self, importer: Box<dyn CadLiteImporter>) {
        self.importers.push(importer);
    }

    /// Returns the best probe result across registered importers.
    pub fn probe(&self, input: &InputFile<'_>) -> ProbeResult {
        self.importers
            .iter()
            .map(|importer| importer.probe(input))
            .max_by_key(|probe| probe.confidence)
            .unwrap_or_else(ProbeResult::unknown)
    }

    /// Imports bytes using the best matching importer.
    pub fn import(
        &self,
        input: &InputFile<'_>,
        options: &ImportOptions,
    ) -> Result<LiteDocument, ImportError> {
        ensure_input_size(input.bytes, &options.limits)?;
        let mut best: Option<(&dyn CadLiteImporter, ProbeResult)> = None;

        for importer in &self.importers {
            let probe = importer.probe(input);
            if probe.confidence == ProbeConfidence::Unknown {
                continue;
            }
            let replace = best
                .as_ref()
                .map(|(_, best_probe)| probe.confidence > best_probe.confidence)
                .unwrap_or(true);
            if replace {
                best = Some((importer.as_ref(), probe));
            }
        }

        let Some((importer, _)) = best else {
            return Err(ImportError::Unsupported(
                "no importer matched the input".to_string(),
            ));
        };

        importer.import_lite(input, options)
    }
}

impl Default for ImporterRegistry {
    fn default() -> Self {
        Self::with_builtin_importers()
    }
}
