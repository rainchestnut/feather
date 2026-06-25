//! Business-facing asset conversion facade built on the core pipeline.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::atomic_write::write_atomic;
use crate::batch::{
    BatchConversionError, BatchConversionOptions, BatchReport, run_batch_conversion,
};
use crate::contracts::ASSET_PACKAGE_CONTRACT_VERSION;
use crate::diagnostics::batch_failure_category;
use crate::importer::{ImportLimits, ImportOptions, ReferencePathMapping};
use crate::inspect::{InspectError, InspectOptions, inspect_path};
use crate::jobs::{JobConversionSettings, JobImportLimits, JobReferencePathMapping};
use crate::pipeline::{ConversionError, ConversionSummary, convert_path_to_glb};

/// Business profile used to select conversion quality without exposing low-level knobs.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum AssetConversionProfile {
    MobilePreview,
    #[default]
    WebPreview,
    StandardReview,
    HighQuality,
    Custom(JobConversionSettings),
}

impl AssetConversionProfile {
    /// Returns the stable label emitted in source and diagnostics artifacts.
    pub fn label(&self) -> &'static str {
        match self {
            Self::MobilePreview => "mobile_preview",
            Self::WebPreview => "web_preview",
            Self::StandardReview => "standard_review",
            Self::HighQuality => "high_quality",
            Self::Custom(_) => "custom",
        }
    }

    /// Maps a business profile to concrete conversion settings.
    pub fn to_settings(&self) -> JobConversionSettings {
        let mut settings = match self {
            Self::MobilePreview => JobConversionSettings {
                include_normals: false,
                max_triangles: Some(50_000),
                quantize_step: Some(0.001),
                ..JobConversionSettings::default()
            },
            Self::WebPreview => JobConversionSettings {
                max_triangles: Some(150_000),
                quantize_step: Some(0.0005),
                ..JobConversionSettings::default()
            },
            Self::StandardReview => JobConversionSettings {
                max_triangles: Some(500_000),
                ..JobConversionSettings::default()
            },
            Self::HighQuality => JobConversionSettings::default(),
            Self::Custom(settings) => settings.clone(),
        };
        settings.write_metadata = true;
        settings
    }
}

/// Single-source business conversion request.
#[derive(Debug, Clone)]
pub struct AssetConversionRequest {
    pub input_path: PathBuf,
    pub output_dir: PathBuf,
    pub profile: AssetConversionProfile,
    pub resolve_dirs: Vec<PathBuf>,
    pub reference_path_mappings: Vec<ReferencePathMapping>,
    pub limits: ImportLimits,
}

impl AssetConversionRequest {
    /// Creates a request using the default web preview profile.
    pub fn new(input_path: impl Into<PathBuf>, output_dir: impl Into<PathBuf>) -> Self {
        Self {
            input_path: input_path.into(),
            output_dir: output_dir.into(),
            profile: AssetConversionProfile::default(),
            resolve_dirs: Vec::new(),
            reference_path_mappings: Vec::new(),
            limits: ImportLimits::default(),
        }
    }
}

/// Batch business conversion request.
#[derive(Debug, Clone)]
pub struct BatchAssetConversionRequest {
    pub input_paths: Vec<PathBuf>,
    pub output_dir: PathBuf,
    pub profile: AssetConversionProfile,
    pub check_only: bool,
    pub resolve_dirs: Vec<PathBuf>,
    pub reference_path_mappings: Vec<ReferencePathMapping>,
    pub limits: ImportLimits,
}

impl BatchAssetConversionRequest {
    /// Creates a batch request using the default web preview profile.
    pub fn new(input_paths: Vec<PathBuf>, output_dir: impl Into<PathBuf>) -> Self {
        Self {
            input_paths,
            output_dir: output_dir.into(),
            profile: AssetConversionProfile::default(),
            check_only: false,
            resolve_dirs: Vec::new(),
            reference_path_mappings: Vec::new(),
            limits: ImportLimits::default(),
        }
    }
}

/// Lightweight preflight request for a single source.
#[derive(Debug, Clone)]
pub struct AssetPreflightRequest {
    pub input_path: PathBuf,
    pub profile: AssetConversionProfile,
    pub resolve_dirs: Vec<PathBuf>,
    pub reference_path_mappings: Vec<ReferencePathMapping>,
    pub limits: ImportLimits,
}

impl AssetPreflightRequest {
    /// Creates a preflight request using the default web preview profile.
    pub fn new(input_path: impl Into<PathBuf>) -> Self {
        Self {
            input_path: input_path.into(),
            profile: AssetConversionProfile::default(),
            resolve_dirs: Vec::new(),
            reference_path_mappings: Vec::new(),
            limits: ImportLimits::default(),
        }
    }
}

/// Standard business artifact package paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetOutputPackage {
    pub root_dir: PathBuf,
    pub model_path: Option<PathBuf>,
    pub metadata_path: Option<PathBuf>,
    pub manifest_path: Option<PathBuf>,
    pub batch_output_dir: Option<PathBuf>,
    pub source_info_path: PathBuf,
    pub diagnostics_path: PathBuf,
}

/// Structured failure returned by business conversion APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssetFailure {
    pub stage: String,
    pub category: String,
    pub message: String,
    pub retryable: bool,
}

/// Successful single-source conversion result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetConversionResult {
    pub package: AssetOutputPackage,
    pub source_format: String,
    pub node_count: usize,
    pub mesh_count: usize,
    pub primitive_count: usize,
    pub vertex_count: usize,
    pub triangle_count: u64,
}

/// Successful batch conversion result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchAssetConversionResult {
    pub package: AssetOutputPackage,
    pub report: BatchReport,
}

/// Lightweight preflight result for business callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPreflightResult {
    pub source_format: String,
    pub capability_status: Option<String>,
    pub visual_asset_count: usize,
    pub importable: bool,
    pub mesh_count: Option<usize>,
    pub triangle_count: Option<u64>,
    pub failure: Option<AssetFailure>,
}

/// Error returned by business asset conversion APIs.
#[derive(Debug)]
pub enum AssetConversionError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    Inspect(InspectError),
    ConversionFailed {
        package: Box<AssetOutputPackage>,
        failure: Box<AssetFailure>,
    },
    BatchFailed {
        package: Box<AssetOutputPackage>,
        failure: Box<AssetFailure>,
    },
}

impl fmt::Display for AssetConversionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "failed to access `{}`: {source}", path.display())
            }
            Self::Json { path, source } => {
                write!(
                    formatter,
                    "failed to serialize `{}`: {source}",
                    path.display()
                )
            }
            Self::Inspect(error) => write!(formatter, "{error}"),
            Self::ConversionFailed { failure, .. } | Self::BatchFailed { failure, .. } => {
                write!(formatter, "asset conversion failed: {}", failure.message)
            }
        }
    }
}

impl Error for AssetConversionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Inspect(error) => Some(error),
            Self::ConversionFailed { .. } | Self::BatchFailed { .. } => None,
        }
    }
}

/// Converts one source into a standard business artifact package.
pub fn convert_asset(
    request: &AssetConversionRequest,
) -> Result<AssetConversionResult, AssetConversionError> {
    let package = single_asset_package(&request.output_dir);
    ensure_dir(&package.root_dir)?;
    let settings = settings_for_asset_request(
        &request.profile,
        &request.resolve_dirs,
        &request.reference_path_mappings,
        request.limits,
    );
    write_source_info(
        &package,
        request.profile.label(),
        std::slice::from_ref(&request.input_path),
        "conversion",
    )?;

    let metadata_path = package.metadata_path.clone();
    let model_path = package
        .model_path
        .as_ref()
        .expect("single asset package reserves model path");
    match convert_path_to_glb(
        &request.input_path,
        model_path,
        &settings.to_conversion_options(metadata_path),
    ) {
        Ok(summary) => {
            let result = AssetConversionResult::from_summary(package.clone(), summary);
            write_diagnostics(
                &package,
                request.profile.label(),
                "succeeded",
                Some(&result),
                None,
            )?;
            Ok(result)
        }
        Err(error) => {
            let failure = AssetFailure::from_conversion_error(&error);
            let _ = write_diagnostics(
                &package,
                request.profile.label(),
                "failed",
                None,
                Some(&failure),
            );
            Err(AssetConversionError::ConversionFailed {
                package: Box::new(package),
                failure: Box::new(failure),
            })
        }
    }
}

/// Converts or checks multiple sources into a standard batch artifact package.
pub fn convert_batch_assets(
    request: &BatchAssetConversionRequest,
) -> Result<BatchAssetConversionResult, AssetConversionError> {
    let package = batch_asset_package(&request.output_dir);
    ensure_dir(&package.root_dir)?;
    let batch_output_dir = package
        .batch_output_dir
        .clone()
        .expect("batch asset package reserves output dir");
    let manifest_path = package
        .manifest_path
        .clone()
        .expect("batch asset package reserves manifest path");
    let settings = settings_for_asset_request(
        &request.profile,
        &request.resolve_dirs,
        &request.reference_path_mappings,
        request.limits,
    );
    write_source_info(
        &package,
        request.profile.label(),
        &request.input_paths,
        "batch_conversion",
    )?;

    let run = run_batch_conversion(
        &request.input_paths,
        &BatchConversionOptions {
            output_dir: batch_output_dir,
            manifest_path: Some(manifest_path),
            check_only: request.check_only,
            conversion: settings.to_conversion_options(None),
        },
    );
    match run {
        Ok(report) => {
            let result = BatchAssetConversionResult {
                package: package.clone(),
                report: report.report,
            };
            let failed_count = result.report.failed_count();
            if failed_count == 0 {
                write_batch_diagnostics(
                    &package,
                    request.profile.label(),
                    "succeeded",
                    &result,
                    None,
                )?;
                Ok(result)
            } else {
                let failure = AssetFailure {
                    stage: "batch".to_string(),
                    category: "batch_item_failed".to_string(),
                    message: format!("batch completed with {failed_count} failed items"),
                    retryable: true,
                };
                let _ = write_batch_diagnostics(
                    &package,
                    request.profile.label(),
                    "failed",
                    &result,
                    Some(&failure),
                );
                Err(AssetConversionError::BatchFailed {
                    package: Box::new(package),
                    failure: Box::new(failure),
                })
            }
        }
        Err(error) => {
            let failure = AssetFailure::from_batch_error(&error);
            let _ = write_failure_only_diagnostics(
                &package,
                request.profile.label(),
                "failed",
                &failure,
            );
            Err(AssetConversionError::BatchFailed {
                package: Box::new(package),
                failure: Box::new(failure),
            })
        }
    }
}

/// Performs a real import preflight without writing conversion artifacts.
pub fn preflight_asset(
    request: &AssetPreflightRequest,
) -> Result<AssetPreflightResult, AssetConversionError> {
    let settings = settings_for_asset_request(
        &request.profile,
        &request.resolve_dirs,
        &request.reference_path_mappings,
        request.limits,
    );
    let inspect = inspect_path(
        &request.input_path,
        &InspectOptions {
            import: import_options_from_settings(&settings),
            check_import: true,
        },
    )
    .map_err(AssetConversionError::Inspect)?;
    let import_check = inspect
        .import_check
        .as_ref()
        .expect("preflight enables import check");
    let failure = if import_check.importable {
        None
    } else {
        Some(AssetFailure {
            stage: import_check.failure_stage.unwrap_or("import").to_string(),
            category: import_check.failure_category.unwrap_or("other").to_string(),
            message: import_check
                .error
                .clone()
                .unwrap_or_else(|| "asset is not importable".to_string()),
            retryable: retryable_failure(import_check.failure_category.unwrap_or("other")),
        })
    };

    Ok(AssetPreflightResult {
        source_format: inspect.probe.format.label().to_string(),
        capability_status: inspect
            .capability()
            .map(|capability| capability.status.to_string()),
        visual_asset_count: inspect.visual_assets.len(),
        importable: import_check.importable,
        mesh_count: import_check.mesh_count,
        triangle_count: import_check.triangle_count,
        failure,
    })
}

impl AssetConversionResult {
    fn from_summary(package: AssetOutputPackage, summary: ConversionSummary) -> Self {
        Self {
            package,
            source_format: summary.source_format,
            node_count: summary.node_count,
            mesh_count: summary.mesh_count,
            primitive_count: summary.primitive_count,
            vertex_count: summary.vertex_count,
            triangle_count: summary.triangle_count,
        }
    }
}

impl AssetFailure {
    fn from_conversion_error(error: &ConversionError) -> Self {
        let stage = crate::batch::conversion_error_stage(error);
        let message = error.to_string();
        let category = batch_failure_category(stage, &message);
        Self {
            stage: stage.to_string(),
            category: category.to_string(),
            message,
            retryable: retryable_failure(category),
        }
    }

    fn from_batch_error(error: &BatchConversionError) -> Self {
        let stage = match error {
            BatchConversionError::CreateOutputDir { .. }
            | BatchConversionError::WriteManifest { .. } => "io",
            BatchConversionError::CollectInputs(_) | BatchConversionError::EmptyInputSet => {
                "import"
            }
        };
        let message = error.to_string();
        let category = batch_failure_category(stage, &message);
        Self {
            stage: stage.to_string(),
            category: category.to_string(),
            message,
            retryable: retryable_failure(category),
        }
    }
}

fn settings_for_asset_request(
    profile: &AssetConversionProfile,
    resolve_dirs: &[PathBuf],
    reference_path_mappings: &[ReferencePathMapping],
    limits: ImportLimits,
) -> JobConversionSettings {
    let mut settings = profile.to_settings();
    settings.resolve_dirs = resolve_dirs.to_vec();
    settings.reference_path_mappings = reference_path_mappings
        .iter()
        .map(JobReferencePathMapping::from)
        .collect();
    settings.limits = JobImportLimits::from(limits);
    settings
}

fn import_options_from_settings(settings: &JobConversionSettings) -> ImportOptions {
    ImportOptions {
        max_lod_error: settings.max_lod_error,
        resolve_dirs: settings.resolve_dirs.clone(),
        reference_path_mappings: settings
            .reference_path_mappings
            .iter()
            .map(ReferencePathMapping::from)
            .collect(),
        limits: settings.limits.into(),
        ..ImportOptions::default()
    }
}

fn single_asset_package(output_dir: &Path) -> AssetOutputPackage {
    AssetOutputPackage {
        root_dir: output_dir.to_path_buf(),
        model_path: Some(output_dir.join("model.glb")),
        metadata_path: Some(output_dir.join("metadata.json")),
        manifest_path: None,
        batch_output_dir: None,
        source_info_path: output_dir.join("source-info.json"),
        diagnostics_path: output_dir.join("diagnostics.json"),
    }
}

fn batch_asset_package(output_dir: &Path) -> AssetOutputPackage {
    AssetOutputPackage {
        root_dir: output_dir.to_path_buf(),
        model_path: None,
        metadata_path: None,
        manifest_path: Some(output_dir.join("manifest.json")),
        batch_output_dir: Some(output_dir.join("outputs")),
        source_info_path: output_dir.join("source-info.json"),
        diagnostics_path: output_dir.join("diagnostics.json"),
    }
}

fn ensure_dir(path: &Path) -> Result<(), AssetConversionError> {
    fs::create_dir_all(path).map_err(|source| AssetConversionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Serialize)]
struct AssetSourceInfoJson<'a> {
    contract_version: &'static str,
    kind: &'a str,
    profile: &'a str,
    inputs: Vec<AssetSourceInputJson>,
    created_at_unix_ms: u64,
}

#[derive(Serialize)]
struct AssetSourceInputJson {
    path: PathBuf,
    size_bytes: Option<u64>,
}

fn write_source_info(
    package: &AssetOutputPackage,
    profile: &str,
    input_paths: &[PathBuf],
    kind: &str,
) -> Result<(), AssetConversionError> {
    let source_info = AssetSourceInfoJson {
        contract_version: ASSET_PACKAGE_CONTRACT_VERSION,
        kind,
        profile,
        inputs: input_paths
            .iter()
            .map(|path| AssetSourceInputJson {
                path: path.clone(),
                size_bytes: file_size(path),
            })
            .collect(),
        created_at_unix_ms: unix_timestamp_millis(),
    };
    write_json(&package.source_info_path, &source_info)
}

#[derive(Serialize)]
struct AssetDiagnosticsJson<'a> {
    contract_version: &'static str,
    status: &'a str,
    profile: &'a str,
    source_format: Option<&'a str>,
    node_count: Option<usize>,
    mesh_count: Option<usize>,
    primitive_count: Option<usize>,
    vertex_count: Option<usize>,
    triangle_count: Option<u64>,
    failure: Option<&'a AssetFailure>,
    updated_at_unix_ms: u64,
}

fn write_diagnostics(
    package: &AssetOutputPackage,
    profile: &str,
    status: &str,
    result: Option<&AssetConversionResult>,
    failure: Option<&AssetFailure>,
) -> Result<(), AssetConversionError> {
    let diagnostics = AssetDiagnosticsJson {
        contract_version: ASSET_PACKAGE_CONTRACT_VERSION,
        status,
        profile,
        source_format: result.map(|result| result.source_format.as_str()),
        node_count: result.map(|result| result.node_count),
        mesh_count: result.map(|result| result.mesh_count),
        primitive_count: result.map(|result| result.primitive_count),
        vertex_count: result.map(|result| result.vertex_count),
        triangle_count: result.map(|result| result.triangle_count),
        failure,
        updated_at_unix_ms: unix_timestamp_millis(),
    };
    write_json(&package.diagnostics_path, &diagnostics)
}

#[derive(Serialize)]
struct BatchAssetDiagnosticsJson<'a> {
    contract_version: &'static str,
    status: &'a str,
    profile: &'a str,
    input_count: usize,
    converted_count: usize,
    checked_count: usize,
    failed_count: usize,
    failure: Option<&'a AssetFailure>,
    updated_at_unix_ms: u64,
}

fn write_batch_diagnostics(
    package: &AssetOutputPackage,
    profile: &str,
    status: &str,
    result: &BatchAssetConversionResult,
    failure: Option<&AssetFailure>,
) -> Result<(), AssetConversionError> {
    let diagnostics = BatchAssetDiagnosticsJson {
        contract_version: ASSET_PACKAGE_CONTRACT_VERSION,
        status,
        profile,
        input_count: result.report.input_count(),
        converted_count: result.report.converted_count(),
        checked_count: result.report.checked_count(),
        failed_count: result.report.failed_count(),
        failure,
        updated_at_unix_ms: unix_timestamp_millis(),
    };
    write_json(&package.diagnostics_path, &diagnostics)
}

fn write_failure_only_diagnostics(
    package: &AssetOutputPackage,
    profile: &str,
    status: &str,
    failure: &AssetFailure,
) -> Result<(), AssetConversionError> {
    let diagnostics = BatchAssetDiagnosticsJson {
        contract_version: ASSET_PACKAGE_CONTRACT_VERSION,
        status,
        profile,
        input_count: 0,
        converted_count: 0,
        checked_count: 0,
        failed_count: 0,
        failure: Some(failure),
        updated_at_unix_ms: unix_timestamp_millis(),
    };
    write_json(&package.diagnostics_path, &diagnostics)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), AssetConversionError> {
    let mut bytes =
        serde_json::to_vec_pretty(value).map_err(|source| AssetConversionError::Json {
            path: path.to_path_buf(),
            source,
        })?;
    bytes.push(b'\n');
    write_atomic(path, bytes).map_err(|source| AssetConversionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn retryable_failure(category: &str) -> bool {
    matches!(
        category,
        "io" | "missing_external_reference" | "resource_limit_exceeded" | "batch_item_failed"
    )
}

fn file_size(path: &Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    metadata.is_file().then_some(metadata.len())
}

fn unix_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0)
}
