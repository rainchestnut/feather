//! End-to-end conversion pipeline used by the CLI and library embedders.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::export::{
    ExportError, GlbExportOptions, export_glb, export_metadata_json, validate_glb_payload,
};
use crate::importer::{ImportError, ImportOptions, ImporterRegistry, InputFile, read_input_path};
use crate::mesh::{MeshOptions, optimize_document, validate_document};

/// Options for converting one input file to GLB.
#[derive(Debug, Clone, Default)]
pub struct ConversionOptions {
    pub import: ImportOptions,
    pub mesh: MeshOptions,
    pub export: GlbExportOptions,
    pub write_metadata: bool,
    pub metadata_path: Option<PathBuf>,
}

/// Summary returned after a successful conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionSummary {
    pub source_format: String,
    pub output_path: PathBuf,
    pub metadata_path: Option<PathBuf>,
    pub node_count: usize,
    pub mesh_count: usize,
    pub primitive_count: usize,
    pub vertex_count: usize,
    pub triangle_count: u64,
}

/// Error type for full conversion.
#[derive(Debug)]
pub enum ConversionError {
    Import(ImportError),
    Export(ExportError),
    Io(std::io::Error),
}

impl fmt::Display for ConversionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Import(error) => write!(formatter, "{error}"),
            Self::Export(error) => write!(formatter, "{error}"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl Error for ConversionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Import(error) => Some(error),
            Self::Export(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}

impl From<ImportError> for ConversionError {
    fn from(error: ImportError) -> Self {
        Self::Import(error)
    }
}

impl From<ExportError> for ConversionError {
    fn from(error: ExportError) -> Self {
        Self::Export(error)
    }
}

impl From<std::io::Error> for ConversionError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Converts a source file to GLB and optional metadata JSON.
pub fn convert_path_to_glb(
    input_path: &Path,
    output_path: &Path,
    options: &ConversionOptions,
) -> Result<ConversionSummary, ConversionError> {
    let bytes = read_input_path(input_path, &options.import.limits)?;
    let input = InputFile::new(Some(input_path), &bytes);
    let registry = ImporterRegistry::default();
    let mut document = registry.import(&input, &options.import)?;

    validate_document(&document)?;
    optimize_document(&mut document, &options.mesh);
    validate_document(&document)?;
    if !options.export.include_normals {
        document
            .metadata
            .warnings
            .push("omitted normals from GLB export".to_string());
    }

    let glb = export_glb(&document, &options.export)?;
    let glb_validation = validate_glb_payload(&glb)?;
    fs::write(output_path, glb)?;

    let metadata_path = if options.write_metadata {
        let metadata_path = options
            .metadata_path
            .clone()
            .unwrap_or_else(|| output_path.with_extension("metadata.json"));
        fs::write(&metadata_path, export_metadata_json(&document))?;
        Some(metadata_path)
    } else {
        None
    };

    Ok(ConversionSummary {
        source_format: document.metadata.source_format,
        output_path: output_path.to_path_buf(),
        metadata_path,
        node_count: glb_validation.node_count,
        mesh_count: glb_validation.mesh_count,
        primitive_count: glb_validation.primitive_count,
        vertex_count: glb_validation.vertex_count,
        triangle_count: glb_validation.triangle_count,
    })
}
