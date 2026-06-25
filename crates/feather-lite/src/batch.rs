//! Batch conversion report model and manifest JSON contract.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::capabilities::{format_capability, push_format_capability_json};
use crate::contracts::BATCH_MANIFEST_CONTRACT_VERSION;
use crate::diagnostics::{batch_failure_category, required_condition_for_failure};
use crate::importer::{ImportError, ImportOptions, ImporterRegistry, InputFile, read_input_path};
use crate::inspect::{ImportValidationSummary, validate_imported_input};
use crate::json::escape_json;
use crate::pipeline::{ConversionError, ConversionOptions, convert_path_to_glb};
use crate::probe::{FileFormat, has_supported_source_extension};

/// Number of leading bytes used to probe extensionless batch candidates.
pub const BATCH_HEADER_PROBE_BYTES: usize = 8192;

/// Diagnostic data captured for a batch input before or after a failed import.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchInputDiagnostic {
    pub source_format: Option<String>,
    pub probe_confidence: Option<String>,
    pub embedded_cache: Option<bool>,
    pub probe_reason: Option<String>,
    pub container_kind: Option<String>,
    pub source_version: Option<String>,
    pub native_visualization: Option<String>,
}

/// Error produced by batch check-only preflight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchCheckError {
    pub stage: &'static str,
    pub message: String,
}

/// Error produced while collecting batch inputs from files and directories.
#[derive(Debug)]
pub enum BatchInputCollectionError {
    ResolveOutputDir(std::io::Error),
    ReadDirectory {
        directory: PathBuf,
        source: std::io::Error,
    },
    MissingInput(PathBuf),
}

impl fmt::Display for BatchInputCollectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResolveOutputDir(error) => {
                write!(formatter, "failed to resolve output directory: {error}")
            }
            Self::ReadDirectory { directory, source } => {
                write!(
                    formatter,
                    "failed to read `{}`: {source}",
                    directory.display()
                )
            }
            Self::MissingInput(input) => {
                write!(
                    formatter,
                    "batch input `{}` does not exist",
                    input.display()
                )
            }
        }
    }
}

impl Error for BatchInputCollectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ResolveOutputDir(error) => Some(error),
            Self::ReadDirectory { source, .. } => Some(source),
            Self::MissingInput(_) => None,
        }
    }
}

/// Options for a complete batch conversion or check-only run.
#[derive(Debug, Clone)]
pub struct BatchConversionOptions {
    pub output_dir: PathBuf,
    pub manifest_path: Option<PathBuf>,
    pub check_only: bool,
    pub conversion: ConversionOptions,
}

/// Report returned after a batch run writes its manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchConversionReport {
    pub manifest_path: PathBuf,
    pub report: BatchReport,
}

/// Error produced before a batch run can emit a usable manifest.
#[derive(Debug)]
pub enum BatchConversionError {
    CreateOutputDir {
        directory: PathBuf,
        source: std::io::Error,
    },
    CollectInputs(BatchInputCollectionError),
    EmptyInputSet,
    WriteManifest {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for BatchConversionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateOutputDir { directory, source } => {
                write!(
                    formatter,
                    "failed to create batch output directory `{}`: {source}",
                    directory.display()
                )
            }
            Self::CollectInputs(error) => write!(formatter, "{error}"),
            Self::EmptyInputSet => write!(formatter, "batch found no supported input files"),
            Self::WriteManifest { path, source } => {
                write!(
                    formatter,
                    "failed to write batch manifest `{}`: {source}",
                    path.display()
                )
            }
        }
    }
}

impl Error for BatchConversionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CreateOutputDir { source, .. } | Self::WriteManifest { source, .. } => {
                Some(source)
            }
            Self::CollectInputs(error) => Some(error),
            Self::EmptyInputSet => None,
        }
    }
}

/// One input processed by a batch run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchItem {
    pub index: usize,
    pub input_path: String,
    pub input_size_bytes: Option<u64>,
    pub duration_ms: u128,
    pub status: BatchItemStatus,
}

/// Final state for one batch input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchItemStatus {
    Ok {
        source_format: String,
        output_path: String,
        metadata_path: Option<String>,
        output_size_bytes: Option<u64>,
        metadata_size_bytes: Option<u64>,
        node_count: usize,
        mesh_count: usize,
        primitive_count: usize,
        vertex_count: usize,
        triangle_count: u64,
    },
    Checked {
        source_format: String,
        node_count: usize,
        mesh_count: usize,
        primitive_count: usize,
        vertex_count: usize,
        triangle_count: u64,
    },
    Error {
        diagnostic: BatchInputDiagnostic,
        stage: &'static str,
        message: String,
    },
}

impl BatchItemStatus {
    /// Returns true when the input completed either conversion or preflight.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Ok { .. } | Self::Checked { .. })
    }

    /// Returns true when the input produced an output artifact.
    pub fn is_converted(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }

    /// Returns true when the input passed check-only preflight.
    pub fn is_checked(&self) -> bool {
        matches!(self, Self::Checked { .. })
    }

    fn source_format(&self) -> Option<&str> {
        match self {
            Self::Ok { source_format, .. } | Self::Checked { source_format, .. } => {
                Some(source_format)
            }
            Self::Error { diagnostic, .. } => diagnostic.source_format.as_deref(),
        }
    }

    fn mesh_count(&self) -> usize {
        match self {
            Self::Ok { mesh_count, .. } | Self::Checked { mesh_count, .. } => *mesh_count,
            Self::Error { .. } => 0,
        }
    }

    fn node_count(&self) -> usize {
        match self {
            Self::Ok { node_count, .. } | Self::Checked { node_count, .. } => *node_count,
            Self::Error { .. } => 0,
        }
    }

    fn primitive_count(&self) -> usize {
        match self {
            Self::Ok {
                primitive_count, ..
            }
            | Self::Checked {
                primitive_count, ..
            } => *primitive_count,
            Self::Error { .. } => 0,
        }
    }

    fn vertex_count(&self) -> usize {
        match self {
            Self::Ok { vertex_count, .. } | Self::Checked { vertex_count, .. } => *vertex_count,
            Self::Error { .. } => 0,
        }
    }

    fn triangle_count(&self) -> u64 {
        match self {
            Self::Ok { triangle_count, .. } | Self::Checked { triangle_count, .. } => {
                *triangle_count
            }
            Self::Error { .. } => 0,
        }
    }

    fn output_size_bytes(&self) -> u64 {
        match self {
            Self::Ok {
                output_size_bytes, ..
            } => output_size_bytes.unwrap_or(0),
            Self::Checked { .. } | Self::Error { .. } => 0,
        }
    }

    fn metadata_size_bytes(&self) -> u64 {
        match self {
            Self::Ok {
                metadata_size_bytes,
                ..
            } => metadata_size_bytes.unwrap_or(0),
            Self::Checked { .. } | Self::Error { .. } => 0,
        }
    }
}

/// Complete report for a batch run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchReport {
    pub items: Vec<BatchItem>,
}

impl BatchReport {
    pub fn new(items: Vec<BatchItem>) -> Self {
        Self { items }
    }

    pub fn input_count(&self) -> usize {
        self.items.len()
    }

    pub fn success_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.status.is_success())
            .count()
    }

    pub fn converted_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.status.is_converted())
            .count()
    }

    pub fn checked_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.status.is_checked())
            .count()
    }

    pub fn failed_count(&self) -> usize {
        self.input_count() - self.success_count()
    }

    /// Builds aggregate metrics for the manifest summary block.
    pub fn summary(&self) -> BatchManifestSummary {
        batch_manifest_summary(&self.items)
    }

    /// Serializes the batch report using the stable manifest JSON contract.
    pub fn to_manifest_json(&self) -> String {
        let summary = self.summary();

        let mut json = String::new();
        json.push_str("{\n");
        json.push_str("  \"contract_version\": \"");
        json.push_str(BATCH_MANIFEST_CONTRACT_VERSION);
        json.push_str("\",\n");
        json.push_str("  \"input_count\": ");
        json.push_str(&self.input_count().to_string());
        json.push_str(",\n");
        json.push_str("  \"success_count\": ");
        json.push_str(&self.success_count().to_string());
        json.push_str(",\n");
        json.push_str("  \"converted_count\": ");
        json.push_str(&self.converted_count().to_string());
        json.push_str(",\n");
        json.push_str("  \"checked_count\": ");
        json.push_str(&self.checked_count().to_string());
        json.push_str(",\n");
        json.push_str("  \"failed_count\": ");
        json.push_str(&self.failed_count().to_string());
        json.push_str(",\n");
        push_batch_summary_json(&mut json, &summary);
        json.push_str(",\n");
        json.push_str("  \"items\": [\n");
        for (position, item) in self.items.iter().enumerate() {
            if position > 0 {
                json.push_str(",\n");
            }
            push_batch_item_json(&mut json, item);
        }
        json.push_str("\n  ]\n");
        json.push_str("}\n");
        json
    }
}

/// Aggregate metrics written into the batch manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchManifestSummary {
    pub total_input_bytes: u64,
    pub total_output_bytes: u64,
    pub total_metadata_bytes: u64,
    pub total_duration_ms: u128,
    pub total_node_count: usize,
    pub total_mesh_count: usize,
    pub total_primitive_count: usize,
    pub total_vertex_count: usize,
    pub total_triangle_count: u64,
    pub formats: Vec<BatchFormatSummary>,
    pub failure_stages: Vec<BatchCountSummary>,
    pub failure_categories: Vec<BatchCountSummary>,
}

/// Per-format aggregate metrics in a batch manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchFormatSummary {
    pub source_format: String,
    pub input_count: usize,
    pub success_count: usize,
    pub failed_count: usize,
    pub node_count: usize,
    pub mesh_count: usize,
    pub primitive_count: usize,
    pub vertex_count: usize,
    pub triangle_count: u64,
}

/// Count entry for failure stages and categories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchCountSummary {
    pub name: String,
    pub count: usize,
}

#[derive(Default)]
struct BatchFormatAccumulator {
    input_count: usize,
    success_count: usize,
    failed_count: usize,
    node_count: usize,
    mesh_count: usize,
    primitive_count: usize,
    vertex_count: usize,
    triangle_count: u64,
}

/// Returns true when a path is a plausible batch input by extension or header probe.
pub fn is_supported_batch_candidate(path: &Path) -> bool {
    if has_supported_source_extension(path) {
        return true;
    }

    let Ok(bytes) = read_probe_prefix(path) else {
        return false;
    };
    let input = InputFile::new(Some(path), &bytes);
    ImporterRegistry::default().probe(&input).is_match()
}

/// Collects explicit batch files and recursively discovers supported directory inputs.
pub fn collect_batch_input_paths(
    inputs: &[PathBuf],
    output_dir: &Path,
) -> Result<Vec<PathBuf>, BatchInputCollectionError> {
    let output_dir =
        fs::canonicalize(output_dir).map_err(BatchInputCollectionError::ResolveOutputDir)?;
    let mut files = Vec::new();

    for input in inputs {
        if input.is_file() {
            push_unique_path(&mut files, input.clone());
            continue;
        }
        if input.is_dir() {
            collect_batch_directory(input, &output_dir, &mut files)?;
            continue;
        }
        return Err(BatchInputCollectionError::MissingInput(input.clone()));
    }

    files.sort();
    files.dedup();
    Ok(files)
}

/// Builds the stable GLB output file name for one batch item.
pub fn batch_output_file_name(index: usize, input_path: &Path) -> String {
    let stem = input_path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("input");
    format!("asset_{index:03}_{}.glb", sanitize_output_stem(stem))
}

/// Runs batch preflight or conversion, writes the manifest, and returns item results.
pub fn run_batch_conversion(
    inputs: &[PathBuf],
    options: &BatchConversionOptions,
) -> Result<BatchConversionReport, BatchConversionError> {
    fs::create_dir_all(&options.output_dir).map_err(|source| {
        BatchConversionError::CreateOutputDir {
            directory: options.output_dir.clone(),
            source,
        }
    })?;

    let input_files = collect_batch_input_paths(inputs, &options.output_dir)
        .map_err(BatchConversionError::CollectInputs)?;
    if input_files.is_empty() {
        return Err(BatchConversionError::EmptyInputSet);
    }

    let mut results = Vec::new();
    for (item_index, input_path) in input_files.iter().enumerate() {
        results.push(run_batch_item(item_index, input_path, options));
    }

    let manifest_path = options
        .manifest_path
        .clone()
        .unwrap_or_else(|| options.output_dir.join("manifest.json"));
    let report = BatchReport::new(results);
    fs::write(&manifest_path, report.to_manifest_json()).map_err(|source| {
        BatchConversionError::WriteManifest {
            path: manifest_path.clone(),
            source,
        }
    })?;

    Ok(BatchConversionReport {
        manifest_path,
        report,
    })
}

/// Runs batch preflight: real import plus validation, no export side effects.
pub fn validate_batch_input_path(
    input_path: &Path,
    options: &ImportOptions,
) -> Result<ImportValidationSummary, BatchCheckError> {
    let bytes = read_input_path(input_path, &options.limits).map_err(|error| match error {
        ImportError::Io(error) => BatchCheckError {
            stage: "io",
            message: error.to_string(),
        },
        error => BatchCheckError {
            stage: "import",
            message: error.to_string(),
        },
    })?;
    let input = InputFile::new(Some(input_path), &bytes);
    validate_imported_input(&input, options).map_err(|error| BatchCheckError {
        stage: "import",
        message: error.to_string(),
    })
}

/// Reads lightweight probe data used to explain failed batch items.
pub fn batch_input_diagnostic(input_path: &Path) -> BatchInputDiagnostic {
    batch_input_diagnostic_with_limits(input_path, &ImportOptions::default())
}

fn batch_input_diagnostic_with_limits(
    input_path: &Path,
    options: &ImportOptions,
) -> BatchInputDiagnostic {
    match read_input_path(input_path, &options.limits) {
        Ok(bytes) => {
            let input = InputFile::new(Some(input_path), &bytes);
            let probe = ImporterRegistry::default().probe(&input);
            BatchInputDiagnostic {
                source_format: Some(probe.format.label().to_string()),
                probe_confidence: Some(format!("{:?}", probe.confidence)),
                embedded_cache: Some(probe.has_embedded_cache),
                probe_reason: Some(probe.reason),
                container_kind: probe.container_kind.map(str::to_string),
                source_version: probe.source_version,
                native_visualization: probe.native_visualization.map(str::to_string),
            }
        }
        Err(error) => BatchInputDiagnostic {
            source_format: None,
            probe_confidence: None,
            embedded_cache: None,
            probe_reason: Some(format!("failed to read input for probe: {error}")),
            container_kind: None,
            source_version: None,
            native_visualization: None,
        },
    }
}

fn read_probe_prefix(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    let mut bytes = vec![0_u8; BATCH_HEADER_PROBE_BYTES];
    let read_len = file.read(&mut bytes)?;
    bytes.truncate(read_len);
    Ok(bytes)
}

/// Maps conversion errors to stable batch manifest stages.
pub fn conversion_error_stage(error: &ConversionError) -> &'static str {
    match error {
        ConversionError::Import(_) => "import",
        ConversionError::Export(_) => "export",
        ConversionError::Io(_) => "io",
    }
}

fn run_batch_item(
    item_index: usize,
    input_path: &Path,
    options: &BatchConversionOptions,
) -> BatchItem {
    let started_at = Instant::now();
    let input_size_bytes = file_size(input_path);
    let input_diagnostic =
        batch_input_diagnostic_with_limits(input_path, &options.conversion.import);

    let status = if options.check_only {
        match validate_batch_input_path(input_path, &options.conversion.import) {
            Ok(summary) => BatchItemStatus::Checked {
                source_format: summary.source_format,
                node_count: summary.node_count,
                mesh_count: summary.mesh_count,
                primitive_count: summary.primitive_count,
                vertex_count: summary.vertex_count,
                triangle_count: summary.triangle_count,
            },
            Err(error) => BatchItemStatus::Error {
                diagnostic: input_diagnostic,
                stage: error.stage,
                message: error.message,
            },
        }
    } else {
        let output_path = options
            .output_dir
            .join(batch_output_file_name(item_index, input_path));
        let mut conversion_options = options.conversion.clone();
        if conversion_options.write_metadata {
            conversion_options.metadata_path = Some(output_path.with_extension("metadata.json"));
        } else {
            conversion_options.metadata_path = None;
        }

        match convert_path_to_glb(input_path, &output_path, &conversion_options) {
            Ok(summary) => {
                let output_size_bytes = file_size(&summary.output_path);
                let metadata_size_bytes = summary
                    .metadata_path
                    .as_ref()
                    .and_then(|path| file_size(path));
                let metadata_path = summary
                    .metadata_path
                    .as_ref()
                    .map(|path| path.display().to_string());
                BatchItemStatus::Ok {
                    source_format: summary.source_format,
                    output_path: summary.output_path.display().to_string(),
                    metadata_path,
                    output_size_bytes,
                    metadata_size_bytes,
                    node_count: summary.node_count,
                    mesh_count: summary.mesh_count,
                    primitive_count: summary.primitive_count,
                    vertex_count: summary.vertex_count,
                    triangle_count: summary.triangle_count,
                }
            }
            Err(error) => BatchItemStatus::Error {
                diagnostic: input_diagnostic,
                stage: conversion_error_stage(&error),
                message: error.to_string(),
            },
        }
    };

    BatchItem {
        index: item_index,
        input_path: input_path.display().to_string(),
        input_size_bytes,
        duration_ms: elapsed_millis(started_at),
        status,
    }
}

fn file_size(path: &Path) -> Option<u64> {
    fs::metadata(path).map(|metadata| metadata.len()).ok()
}

fn elapsed_millis(started_at: Instant) -> u128 {
    started_at.elapsed().as_millis()
}

fn collect_batch_directory(
    directory: &Path,
    output_dir: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), BatchInputCollectionError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| BatchInputCollectionError::ReadDirectory {
            directory: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| BatchInputCollectionError::ReadDirectory {
            directory: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        if path_is_inside_directory(&path, output_dir) {
            continue;
        }
        if path.is_dir() {
            collect_batch_directory(&path, output_dir, files)?;
        } else if path.is_file() && is_supported_batch_candidate(&path) {
            push_unique_path(files, path);
        }
    }
    Ok(())
}

fn path_is_inside_directory(path: &Path, directory: &Path) -> bool {
    fs::canonicalize(path)
        .map(|candidate| candidate == directory || candidate.starts_with(directory))
        .unwrap_or(false)
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn sanitize_output_stem(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() {
        "input".to_string()
    } else {
        sanitized.to_string()
    }
}

fn push_batch_item_json(json: &mut String, item: &BatchItem) {
    json.push_str("    {\n");
    json.push_str("      \"index\": ");
    json.push_str(&item.index.to_string());
    json.push_str(",\n");
    json.push_str("      \"input_path\": \"");
    json.push_str(&escape_json(&item.input_path));
    json.push_str("\",\n");
    json.push_str("      \"input_size_bytes\": ");
    push_optional_json_u64(json, item.input_size_bytes);
    json.push_str(",\n");
    json.push_str("      \"duration_ms\": ");
    json.push_str(&item.duration_ms.to_string());
    json.push_str(",\n");
    match &item.status {
        BatchItemStatus::Ok {
            source_format,
            output_path,
            metadata_path,
            output_size_bytes,
            metadata_size_bytes,
            node_count,
            mesh_count,
            primitive_count,
            vertex_count,
            triangle_count,
        } => {
            json.push_str("      \"status\": \"ok\",\n");
            json.push_str("      \"importable\": true,\n");
            json.push_str("      \"source_format\": \"");
            json.push_str(&escape_json(source_format));
            json.push_str("\",\n");
            json.push_str("      \"capability\": ");
            push_capability_for_source_format(json, Some(source_format));
            json.push_str(",\n");
            json.push_str("      \"output_path\": \"");
            json.push_str(&escape_json(output_path));
            json.push_str("\",\n");
            json.push_str("      \"metadata_path\": ");
            if let Some(metadata_path) = metadata_path {
                json.push('"');
                json.push_str(&escape_json(metadata_path));
                json.push('"');
            } else {
                json.push_str("null");
            }
            json.push_str(",\n");
            json.push_str("      \"output_size_bytes\": ");
            push_optional_json_u64(json, *output_size_bytes);
            json.push_str(",\n");
            json.push_str("      \"metadata_size_bytes\": ");
            push_optional_json_u64(json, *metadata_size_bytes);
            json.push_str(",\n");
            json.push_str("      \"node_count\": ");
            json.push_str(&node_count.to_string());
            json.push_str(",\n");
            json.push_str("      \"mesh_count\": ");
            json.push_str(&mesh_count.to_string());
            json.push_str(",\n");
            json.push_str("      \"primitive_count\": ");
            json.push_str(&primitive_count.to_string());
            json.push_str(",\n");
            json.push_str("      \"vertex_count\": ");
            json.push_str(&vertex_count.to_string());
            json.push_str(",\n");
            json.push_str("      \"triangle_count\": ");
            json.push_str(&triangle_count.to_string());
            json.push('\n');
        }
        BatchItemStatus::Checked {
            source_format,
            node_count,
            mesh_count,
            primitive_count,
            vertex_count,
            triangle_count,
        } => {
            json.push_str("      \"status\": \"checked\",\n");
            json.push_str("      \"importable\": true,\n");
            json.push_str("      \"source_format\": \"");
            json.push_str(&escape_json(source_format));
            json.push_str("\",\n");
            json.push_str("      \"capability\": ");
            push_capability_for_source_format(json, Some(source_format));
            json.push_str(",\n");
            json.push_str("      \"output_path\": null,\n");
            json.push_str("      \"metadata_path\": null,\n");
            json.push_str("      \"output_size_bytes\": null,\n");
            json.push_str("      \"metadata_size_bytes\": null,\n");
            json.push_str("      \"node_count\": ");
            json.push_str(&node_count.to_string());
            json.push_str(",\n");
            json.push_str("      \"mesh_count\": ");
            json.push_str(&mesh_count.to_string());
            json.push_str(",\n");
            json.push_str("      \"primitive_count\": ");
            json.push_str(&primitive_count.to_string());
            json.push_str(",\n");
            json.push_str("      \"vertex_count\": ");
            json.push_str(&vertex_count.to_string());
            json.push_str(",\n");
            json.push_str("      \"triangle_count\": ");
            json.push_str(&triangle_count.to_string());
            json.push('\n');
        }
        BatchItemStatus::Error {
            diagnostic,
            stage,
            message,
        } => {
            json.push_str("      \"status\": \"error\",\n");
            json.push_str("      \"importable\": false,\n");
            json.push_str("      \"output_size_bytes\": null,\n");
            json.push_str("      \"metadata_size_bytes\": null,\n");
            json.push_str("      \"source_format\": ");
            push_optional_json_string(json, diagnostic.source_format.as_deref());
            json.push_str(",\n");
            json.push_str("      \"capability\": ");
            push_capability_for_source_format(json, diagnostic.source_format.as_deref());
            json.push_str(",\n");
            json.push_str("      \"probe_confidence\": ");
            push_optional_json_string(json, diagnostic.probe_confidence.as_deref());
            json.push_str(",\n");
            json.push_str("      \"embedded_cache\": ");
            push_optional_json_bool(json, diagnostic.embedded_cache);
            json.push_str(",\n");
            json.push_str("      \"probe_reason\": ");
            push_optional_json_string(json, diagnostic.probe_reason.as_deref());
            json.push_str(",\n");
            json.push_str("      \"container_kind\": ");
            push_optional_json_string(json, diagnostic.container_kind.as_deref());
            json.push_str(",\n");
            json.push_str("      \"source_version\": ");
            push_optional_json_string(json, diagnostic.source_version.as_deref());
            json.push_str(",\n");
            json.push_str("      \"native_visualization\": ");
            push_optional_json_string(json, diagnostic.native_visualization.as_deref());
            json.push_str(",\n");
            json.push_str("      \"error_stage\": \"");
            json.push_str(stage);
            json.push_str("\",\n");
            json.push_str("      \"error_category\": \"");
            let error_category = batch_failure_category(stage, message);
            json.push_str(error_category);
            json.push_str("\",\n");
            json.push_str("      \"required_condition\": ");
            push_optional_json_string(
                json,
                diagnostic
                    .source_format
                    .as_deref()
                    .and_then(format_from_label)
                    .and_then(|format| required_condition_for_failure(format, error_category)),
            );
            json.push_str(",\n");
            json.push_str("      \"error\": \"");
            json.push_str(&escape_json(message));
            json.push_str("\"\n");
        }
    }
    json.push_str("    }");
}

fn push_capability_for_source_format(json: &mut String, source_format: Option<&str>) {
    if let Some(capability) = source_format
        .and_then(format_from_label)
        .and_then(format_capability)
    {
        push_format_capability_json(json, capability, "      ");
    } else {
        json.push_str("null");
    }
}

fn format_from_label(source_format: &str) -> Option<FileFormat> {
    FileFormat::from_label(source_format).filter(|format| *format != FileFormat::Unknown)
}

fn batch_manifest_summary(items: &[BatchItem]) -> BatchManifestSummary {
    let mut formats = BTreeMap::<String, BatchFormatAccumulator>::new();
    let mut failure_stages = BTreeMap::<String, usize>::new();
    let mut failure_categories = BTreeMap::<String, usize>::new();
    let mut total_input_bytes = 0;
    let mut total_output_bytes = 0;
    let mut total_metadata_bytes = 0;
    let mut total_duration_ms = 0;
    let mut total_node_count = 0;
    let mut total_mesh_count = 0;
    let mut total_primitive_count = 0;
    let mut total_vertex_count = 0;
    let mut total_triangle_count = 0;

    for item in items {
        total_input_bytes += item.input_size_bytes.unwrap_or(0);
        total_output_bytes += item.status.output_size_bytes();
        total_metadata_bytes += item.status.metadata_size_bytes();
        total_duration_ms += item.duration_ms;
        let source_format = item.status.source_format().unwrap_or("UNKNOWN").to_string();
        let format = formats.entry(source_format).or_default();
        format.input_count += 1;

        if item.status.is_success() {
            let node_count = item.status.node_count();
            let mesh_count = item.status.mesh_count();
            let primitive_count = item.status.primitive_count();
            let vertex_count = item.status.vertex_count();
            let triangle_count = item.status.triangle_count();
            format.success_count += 1;
            format.node_count += node_count;
            format.mesh_count += mesh_count;
            format.primitive_count += primitive_count;
            format.vertex_count += vertex_count;
            format.triangle_count += triangle_count;
            total_node_count += node_count;
            total_mesh_count += mesh_count;
            total_primitive_count += primitive_count;
            total_vertex_count += vertex_count;
            total_triangle_count += triangle_count;
        } else if let BatchItemStatus::Error { stage, message, .. } = &item.status {
            format.failed_count += 1;
            increment_count(&mut failure_stages, stage);
            increment_count(
                &mut failure_categories,
                batch_failure_category(stage, message),
            );
        }
    }

    BatchManifestSummary {
        total_input_bytes,
        total_output_bytes,
        total_metadata_bytes,
        total_duration_ms,
        total_node_count,
        total_mesh_count,
        total_primitive_count,
        total_vertex_count,
        total_triangle_count,
        formats: formats
            .into_iter()
            .map(|(source_format, summary)| BatchFormatSummary {
                source_format,
                input_count: summary.input_count,
                success_count: summary.success_count,
                failed_count: summary.failed_count,
                node_count: summary.node_count,
                mesh_count: summary.mesh_count,
                primitive_count: summary.primitive_count,
                vertex_count: summary.vertex_count,
                triangle_count: summary.triangle_count,
            })
            .collect(),
        failure_stages: count_summaries(failure_stages),
        failure_categories: count_summaries(failure_categories),
    }
}

fn increment_count(counts: &mut BTreeMap<String, usize>, key: &str) {
    *counts.entry(key.to_string()).or_insert(0) += 1;
}

fn count_summaries(counts: BTreeMap<String, usize>) -> Vec<BatchCountSummary> {
    counts
        .into_iter()
        .map(|(name, count)| BatchCountSummary { name, count })
        .collect()
}

fn push_batch_summary_json(json: &mut String, summary: &BatchManifestSummary) {
    json.push_str("  \"summary\": {\n");
    json.push_str("    \"total_input_bytes\": ");
    json.push_str(&summary.total_input_bytes.to_string());
    json.push_str(",\n");
    json.push_str("    \"total_output_bytes\": ");
    json.push_str(&summary.total_output_bytes.to_string());
    json.push_str(",\n");
    json.push_str("    \"total_metadata_bytes\": ");
    json.push_str(&summary.total_metadata_bytes.to_string());
    json.push_str(",\n");
    json.push_str("    \"total_duration_ms\": ");
    json.push_str(&summary.total_duration_ms.to_string());
    json.push_str(",\n");
    json.push_str("    \"total_node_count\": ");
    json.push_str(&summary.total_node_count.to_string());
    json.push_str(",\n");
    json.push_str("    \"total_mesh_count\": ");
    json.push_str(&summary.total_mesh_count.to_string());
    json.push_str(",\n");
    json.push_str("    \"total_primitive_count\": ");
    json.push_str(&summary.total_primitive_count.to_string());
    json.push_str(",\n");
    json.push_str("    \"total_vertex_count\": ");
    json.push_str(&summary.total_vertex_count.to_string());
    json.push_str(",\n");
    json.push_str("    \"total_triangle_count\": ");
    json.push_str(&summary.total_triangle_count.to_string());
    json.push_str(",\n");
    json.push_str("    \"formats\": [\n");
    for (index, format) in summary.formats.iter().enumerate() {
        if index > 0 {
            json.push_str(",\n");
        }
        json.push_str("      {\n");
        json.push_str("        \"source_format\": \"");
        json.push_str(&escape_json(&format.source_format));
        json.push_str("\",\n");
        json.push_str("        \"input_count\": ");
        json.push_str(&format.input_count.to_string());
        json.push_str(",\n");
        json.push_str("        \"success_count\": ");
        json.push_str(&format.success_count.to_string());
        json.push_str(",\n");
        json.push_str("        \"failed_count\": ");
        json.push_str(&format.failed_count.to_string());
        json.push_str(",\n");
        json.push_str("        \"node_count\": ");
        json.push_str(&format.node_count.to_string());
        json.push_str(",\n");
        json.push_str("        \"mesh_count\": ");
        json.push_str(&format.mesh_count.to_string());
        json.push_str(",\n");
        json.push_str("        \"primitive_count\": ");
        json.push_str(&format.primitive_count.to_string());
        json.push_str(",\n");
        json.push_str("        \"vertex_count\": ");
        json.push_str(&format.vertex_count.to_string());
        json.push_str(",\n");
        json.push_str("        \"triangle_count\": ");
        json.push_str(&format.triangle_count.to_string());
        json.push('\n');
        json.push_str("      }");
    }
    json.push_str("\n    ],\n");
    push_count_summary_array(json, "failure_stages", "stage", &summary.failure_stages);
    json.push_str(",\n");
    push_count_summary_array(
        json,
        "failure_categories",
        "category",
        &summary.failure_categories,
    );
    json.push_str("\n  }");
}

fn push_count_summary_array(
    json: &mut String,
    field_name: &str,
    item_name: &str,
    counts: &[BatchCountSummary],
) {
    json.push_str("    \"");
    json.push_str(field_name);
    json.push_str("\": [\n");
    for (index, count) in counts.iter().enumerate() {
        if index > 0 {
            json.push_str(",\n");
        }
        json.push_str("      {\n");
        json.push_str("        \"");
        json.push_str(item_name);
        json.push_str("\": \"");
        json.push_str(&escape_json(&count.name));
        json.push_str("\",\n");
        json.push_str("        \"count\": ");
        json.push_str(&count.count.to_string());
        json.push('\n');
        json.push_str("      }");
    }
    json.push_str("\n    ]");
}

fn push_optional_json_string(json: &mut String, value: Option<&str>) {
    if let Some(value) = value {
        json.push('"');
        json.push_str(&escape_json(value));
        json.push('"');
    } else {
        json.push_str("null");
    }
}

fn push_optional_json_bool(json: &mut String, value: Option<bool>) {
    match value {
        Some(true) => json.push_str("true"),
        Some(false) => json.push_str("false"),
        None => json.push_str("null"),
    }
}

fn push_optional_json_u64(json: &mut String, value: Option<u64>) {
    if let Some(value) = value {
        json.push_str(&value.to_string());
    } else {
        json.push_str("null");
    }
}
