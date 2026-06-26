//! Business-facing asset conversion facade built on the core pipeline.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::atomic_write::{remove_file_if_created, write_atomic};
use crate::batch::{
    BatchConversionError, BatchConversionOptions, BatchItem, BatchItemStatus, BatchReport,
    batch_output_file_name, collect_batch_input_paths, run_batch_conversion, run_batch_item,
};
use crate::contracts::{ASSET_PACKAGE_CONTRACT_VERSION, BATCH_MANIFEST_CONTRACT_VERSION};
use crate::diagnostics::batch_failure_category;
use crate::importer::{ImportLimits, ImportOptions, ReferencePathMapping};
use crate::inspect::{InspectError, InspectOptions, inspect_path};
use crate::jobs::{JobConversionSettings, JobImportLimits, JobReferencePathMapping};
use crate::pipeline::{ConversionError, ConversionSummary, convert_path_to_glb};

const LIGHT_TRIANGLE_LIMIT: u64 = 50_000;
const MEDIUM_TRIANGLE_LIMIT: u64 = 150_000;
const HEAVY_TRIANGLE_LIMIT: u64 = 500_000;

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

/// Stable identity for one source or source set under a conversion profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetIdentity {
    pub asset_id: String,
    pub source_sha256: String,
    pub source_size_bytes: u64,
    pub settings_fingerprint: String,
}

/// Structured failure returned by business conversion APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetFailure {
    pub stage: String,
    pub category: String,
    pub message: String,
    pub retryable: bool,
}

/// Stable business action recommended for a conversion failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetFailureAction {
    ProvideReadableVisualization,
    ResolveExternalReferences,
    RunUpstreamTessellation,
    IncreaseResourceLimits,
    UseSupportedInput,
    RepairSourceData,
    CompleteSourcePackage,
    CheckStorageAccess,
    FixExportPipeline,
    ReviewBatchFailures,
    InspectFailure,
}

impl AssetFailureAction {
    /// Returns the stable lowercase action label for API responses and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProvideReadableVisualization => "provide_readable_visualization",
            Self::ResolveExternalReferences => "resolve_external_references",
            Self::RunUpstreamTessellation => "run_upstream_tessellation",
            Self::IncreaseResourceLimits => "increase_resource_limits",
            Self::UseSupportedInput => "use_supported_input",
            Self::RepairSourceData => "repair_source_data",
            Self::CompleteSourcePackage => "complete_source_package",
            Self::CheckStorageAccess => "check_storage_access",
            Self::FixExportPipeline => "fix_export_pipeline",
            Self::ReviewBatchFailures => "review_batch_failures",
            Self::InspectFailure => "inspect_failure",
        }
    }
}

/// Successful single-source conversion result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetConversionResult {
    pub package: AssetOutputPackage,
    pub asset_id: String,
    pub source_sha256: String,
    pub source_size_bytes: u64,
    pub settings_fingerprint: String,
    pub quality: AssetQualityReport,
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
    pub asset_id: String,
    pub source_sha256: String,
    pub source_size_bytes: u64,
    pub settings_fingerprint: String,
    pub quality: AssetQualityReport,
    pub report: BatchReport,
}

/// Business quality summary derived from converted or checked geometry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetQualityReport {
    pub previewable: bool,
    pub has_visual_geometry: bool,
    pub preview_status: AssetPreviewStatus,
    pub quality_level: AssetQualityLevel,
    pub input_count: usize,
    pub successful_count: usize,
    pub converted_count: usize,
    pub reused_count: usize,
    pub checked_count: usize,
    pub failed_count: usize,
    pub node_count: usize,
    pub mesh_count: usize,
    pub primitive_count: usize,
    pub vertex_count: usize,
    pub triangle_count: u64,
    pub input_size_bytes: u64,
    pub output_size_bytes: u64,
    pub metadata_size_bytes: u64,
}

/// Preview readiness for a successful quality report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetPreviewStatus {
    Ready,
    NoVisualGeometry,
    NoPreviewOutput,
    PartialFailure,
}

impl AssetPreviewStatus {
    /// Returns the stable lowercase status label for API responses and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NoVisualGeometry => "no_visual_geometry",
            Self::NoPreviewOutput => "no_preview_output",
            Self::PartialFailure => "partial_failure",
        }
    }
}

/// Geometry size class aligned with the built-in business profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetQualityLevel {
    Empty,
    Light,
    Medium,
    Heavy,
    Oversized,
}

impl AssetQualityLevel {
    /// Returns the stable lowercase level label for API responses and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Light => "light",
            Self::Medium => "medium",
            Self::Heavy => "heavy",
            Self::Oversized => "oversized",
        }
    }
}

/// How an ensure-style asset package API satisfied a package request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetPackageStatus {
    Converted,
    Reused,
}

impl AssetPackageStatus {
    /// Returns the stable lowercase status label for logs or API responses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Converted => "converted",
            Self::Reused => "reused",
        }
    }
}

/// Reuse diagnostic for an existing business asset package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPackageFreshness {
    pub current: bool,
    pub reason: AssetPackageFreshnessReason,
}

impl AssetPackageFreshness {
    /// Creates a current freshness result.
    pub fn current() -> Self {
        Self {
            current: true,
            reason: AssetPackageFreshnessReason::Current,
        }
    }

    /// Creates a stale freshness result with the first blocking reason.
    pub fn stale(reason: AssetPackageFreshnessReason) -> Self {
        Self {
            current: false,
            reason,
        }
    }
}

/// Read-only audit of a business asset package directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPackageAudit {
    pub package: AssetOutputPackage,
    pub usable: bool,
    pub reason: AssetPackageFreshnessReason,
    pub kind: Option<String>,
    pub profile: Option<String>,
    pub status: Option<String>,
    pub identity: Option<AssetIdentity>,
    pub input_count: usize,
    pub quality: Option<AssetQualityReport>,
    pub failure: Option<AssetFailure>,
}

impl AssetPackageAudit {
    /// Returns true when the audited package has a ready preview artifact.
    pub fn previewable(&self) -> bool {
        self.quality
            .as_ref()
            .is_some_and(|quality| quality.previewable)
    }

    fn missing(package: AssetOutputPackage, reason: AssetPackageFreshnessReason) -> Self {
        Self {
            package,
            usable: false,
            reason,
            kind: None,
            profile: None,
            status: None,
            identity: None,
            input_count: 0,
            quality: None,
            failure: None,
        }
    }
}

/// Read-only summary of converted artifacts inside a business asset package.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetPackageSummary {
    pub audit: AssetPackageAudit,
    pub items: Vec<AssetPackageSummaryItem>,
    pub output_size_bytes: u64,
    pub metadata_size_bytes: u64,
}

/// One output or checked input represented in a package summary.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetPackageSummaryItem {
    pub index: usize,
    pub operation: AssetPackageSummaryOperation,
    pub input_path: Option<PathBuf>,
    pub source_format: Option<String>,
    pub output_path: Option<PathBuf>,
    pub metadata_path: Option<PathBuf>,
    pub output_size_bytes: u64,
    pub metadata_size_bytes: u64,
    pub node_count: Option<usize>,
    pub mesh_count: Option<usize>,
    pub primitive_count: Option<usize>,
    pub vertex_count: Option<usize>,
    pub triangle_count: Option<u64>,
    pub metadata: Option<AssetPackageMetadataSummary>,
}

impl AssetPackageSummaryItem {
    /// Returns true when this item has a GLB preview artifact.
    pub fn previewable(&self) -> bool {
        self.output_path.is_some()
    }
}

/// Operation represented by one package summary item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetPackageSummaryOperation {
    Converted,
    Reused,
    Checked,
    Failed,
}

impl AssetPackageSummaryOperation {
    /// Returns the stable lowercase operation label for API responses and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Converted => "converted",
            Self::Reused => "reused",
            Self::Checked => "checked",
            Self::Failed => "failed",
        }
    }
}

/// Typed summary of a GLB sidecar metadata file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetPackageMetadataSummary {
    pub source_format: String,
    pub mode: String,
    pub precision: String,
    pub node_count: usize,
    pub mesh_count: usize,
    pub primitive_count: usize,
    pub vertex_count: usize,
    pub triangle_count: u64,
    pub has_brep: bool,
    pub brep_preserved: bool,
    pub bbox: Option<AssetPackageBounds>,
    pub source_path: Option<String>,
    pub warnings: Vec<String>,
}

/// Axis-aligned bounds read from a package metadata sidecar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetPackageBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Stable reason codes explaining why a package can or cannot be reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetPackageFreshnessReason {
    Current,
    MissingSourceInfo,
    MissingDiagnostics,
    MissingModel,
    MissingMetadata,
    MissingManifest,
    MissingBatchOutputDirectory,
    EmptyBatchInputSet,
    SourceChanged,
    SettingsChanged,
    PackageContractMismatch,
    PackageKindMismatch,
    SourceInfoMismatch,
    DiagnosticsFailed,
    DiagnosticsMismatch,
    ManifestMismatch,
    OutputArtifactMissing,
    IncompleteDiagnostics,
}

impl AssetPackageFreshnessReason {
    /// Returns the stable lowercase reason label for API responses and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::MissingSourceInfo => "missing_source_info",
            Self::MissingDiagnostics => "missing_diagnostics",
            Self::MissingModel => "missing_model",
            Self::MissingMetadata => "missing_metadata",
            Self::MissingManifest => "missing_manifest",
            Self::MissingBatchOutputDirectory => "missing_batch_output_directory",
            Self::EmptyBatchInputSet => "empty_batch_input_set",
            Self::SourceChanged => "source_changed",
            Self::SettingsChanged => "settings_changed",
            Self::PackageContractMismatch => "package_contract_mismatch",
            Self::PackageKindMismatch => "package_kind_mismatch",
            Self::SourceInfoMismatch => "source_info_mismatch",
            Self::DiagnosticsFailed => "diagnostics_failed",
            Self::DiagnosticsMismatch => "diagnostics_mismatch",
            Self::ManifestMismatch => "manifest_mismatch",
            Self::OutputArtifactMissing => "output_artifact_missing",
            Self::IncompleteDiagnostics => "incomplete_diagnostics",
        }
    }
}

/// Result returned by `ensure_asset_package`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPackageEnsureResult {
    pub status: AssetPackageStatus,
    pub asset: AssetConversionResult,
}

/// Result returned by `ensure_batch_asset_package`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchAssetPackageEnsureResult {
    pub status: AssetPackageStatus,
    pub asset: BatchAssetConversionResult,
}

/// Lightweight preflight result for business callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPreflightResult {
    pub decision: AssetPreflightDecision,
    pub source_format: String,
    pub capability_status: Option<String>,
    pub visual_asset_count: usize,
    pub importable: bool,
    pub required_condition: Option<String>,
    pub quality: Option<AssetQualityReport>,
    pub node_count: Option<usize>,
    pub mesh_count: Option<usize>,
    pub primitive_count: Option<usize>,
    pub vertex_count: Option<usize>,
    pub triangle_count: Option<u64>,
    pub failure: Option<AssetFailure>,
}

/// Business decision returned by `preflight_asset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetPreflightDecision {
    Ready,
    NeedsReadableVisualization,
    NeedsExternalReferences,
    NeedsUpstreamTessellation,
    ResourceLimitExceeded,
    UnsupportedInput,
    InvalidSourceData,
    MissingData,
    IoBlocked,
    ExportBlocked,
    Failed,
}

impl AssetPreflightDecision {
    /// Returns the stable lowercase decision label for API responses and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NeedsReadableVisualization => "needs_readable_visualization",
            Self::NeedsExternalReferences => "needs_external_references",
            Self::NeedsUpstreamTessellation => "needs_upstream_tessellation",
            Self::ResourceLimitExceeded => "resource_limit_exceeded",
            Self::UnsupportedInput => "unsupported_input",
            Self::InvalidSourceData => "invalid_source_data",
            Self::MissingData => "missing_data",
            Self::IoBlocked => "io_blocked",
            Self::ExportBlocked => "export_blocked",
            Self::Failed => "failed",
        }
    }
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

/// Computes the stable identity for a single-source conversion request.
///
/// This reads the source bytes and resolves the effective conversion settings,
/// but it does not write package artifacts or run mesh export.
pub fn asset_conversion_identity(
    request: &AssetConversionRequest,
) -> Result<AssetIdentity, AssetConversionError> {
    Ok(prepare_single_asset_identity(request)?.identity)
}

/// Computes the stable identity for a batch conversion request.
///
/// Directory inputs are expanded with the same deterministic ordering and
/// output-directory exclusion rules used by batch conversion.
pub fn batch_asset_conversion_identity(
    request: &BatchAssetConversionRequest,
) -> Result<AssetIdentity, AssetConversionError> {
    let package = batch_asset_package(&request.output_dir);
    let input_paths = collect_batch_input_paths(&request.input_paths, &package.root_dir)
        .map_err(|error| batch_collect_error(package.clone(), error))?;
    if input_paths.is_empty() {
        return Err(empty_batch_error(package));
    }

    Ok(prepare_batch_asset_identity_from_paths(request, input_paths)?.identity)
}

/// Converts one source into a standard business artifact package.
pub fn convert_asset(
    request: &AssetConversionRequest,
) -> Result<AssetConversionResult, AssetConversionError> {
    let package = single_asset_package(&request.output_dir);
    ensure_dir(&package.root_dir)?;
    let prepared = prepare_single_asset_identity(request)?;
    write_source_info(
        &package,
        request.profile.label(),
        std::slice::from_ref(&prepared.source),
        &prepared.identity,
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
        &prepared.settings.to_conversion_options(metadata_path),
    ) {
        Ok(summary) => {
            let result = AssetConversionResult::from_summary(
                package.clone(),
                prepared.identity.clone(),
                summary,
            );
            write_diagnostics(
                &package,
                request.profile.label(),
                "succeeded",
                &prepared.identity,
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
                &prepared.identity,
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
    ensure_dir(&batch_output_dir)?;
    let manifest_path = package
        .manifest_path
        .clone()
        .expect("batch asset package reserves manifest path");
    let input_paths = collect_batch_inputs_or_write_failure(
        request,
        &package,
        &package.root_dir,
        request.profile.label(),
    )?;
    let prepared = prepare_batch_asset_identity_from_paths(request, input_paths)?;
    write_source_info(
        &package,
        request.profile.label(),
        &prepared.sources,
        &prepared.identity,
        "batch_conversion",
    )?;

    let run = run_batch_conversion(
        &prepared.input_paths,
        &BatchConversionOptions {
            output_dir: batch_output_dir,
            manifest_path: Some(manifest_path),
            check_only: request.check_only,
            conversion: prepared.settings.to_conversion_options(None),
        },
    );
    match run {
        Ok(report) => batch_asset_result_from_report(
            &package,
            request.profile.label(),
            prepared.identity,
            report.report,
        ),
        Err(error) => {
            let failure = AssetFailure::from_batch_error(&error);
            let _ = write_failure_only_diagnostics(
                &package,
                request.profile.label(),
                "failed",
                Some(&prepared.identity),
                &failure,
            );
            Err(AssetConversionError::BatchFailed {
                package: Box::new(package),
                failure: Box::new(failure),
            })
        }
    }
}

fn convert_batch_assets_incremental(
    request: &BatchAssetConversionRequest,
) -> Result<BatchAssetConversionResult, AssetConversionError> {
    let package = batch_asset_package(&request.output_dir);
    ensure_dir(&package.root_dir)?;
    let batch_output_dir = package
        .batch_output_dir
        .clone()
        .expect("batch asset package reserves output dir");
    ensure_dir(&batch_output_dir)?;
    let manifest_path = package
        .manifest_path
        .clone()
        .expect("batch asset package reserves manifest path");
    let input_paths = collect_batch_inputs_or_write_failure(
        request,
        &package,
        &package.root_dir,
        request.profile.label(),
    )?;
    let prepared = prepare_batch_asset_identity_from_paths(request, input_paths)?;
    let reusable_items = if request.check_only {
        BTreeMap::new()
    } else {
        batch_reuse_candidates(&package)?
    };
    write_source_info(
        &package,
        request.profile.label(),
        &prepared.sources,
        &prepared.identity,
        "batch_conversion",
    )?;

    let report = run_incremental_batch_items(
        &prepared.input_paths,
        &prepared.sources,
        &batch_output_dir,
        &manifest_path,
        &prepared.settings,
        request.check_only,
        &reusable_items,
    )
    .map_err(|error| {
        let failure = AssetFailure::from_batch_error(&error);
        let _ = write_failure_only_diagnostics(
            &package,
            request.profile.label(),
            "failed",
            Some(&prepared.identity),
            &failure,
        );
        AssetConversionError::BatchFailed {
            package: Box::new(package.clone()),
            failure: Box::new(failure),
        }
    })?;

    batch_asset_result_from_report(&package, request.profile.label(), prepared.identity, report)
}

fn batch_asset_result_from_report(
    package: &AssetOutputPackage,
    profile: &str,
    identity: AssetIdentity,
    report: BatchReport,
) -> Result<BatchAssetConversionResult, AssetConversionError> {
    let result = BatchAssetConversionResult {
        package: package.clone(),
        asset_id: identity.asset_id.clone(),
        source_sha256: identity.source_sha256.clone(),
        source_size_bytes: identity.source_size_bytes,
        settings_fingerprint: identity.settings_fingerprint.clone(),
        quality: batch_quality_report(&report),
        report,
    };
    let failed_count = result.report.failed_count();
    if failed_count == 0 {
        write_batch_diagnostics(package, profile, "succeeded", &result, &identity, None)?;
        Ok(result)
    } else {
        let failure = AssetFailure {
            stage: "batch".to_string(),
            category: "batch_item_failed".to_string(),
            message: format!("batch completed with {failed_count} failed items"),
            retryable: true,
        };
        let _ = write_batch_diagnostics(
            package,
            profile,
            "failed",
            &result,
            &identity,
            Some(&failure),
        );
        Err(AssetConversionError::BatchFailed {
            package: Box::new(package.clone()),
            failure: Box::new(failure),
        })
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
    let source_size_bytes = fs::metadata(&request.input_path)
        .map_err(|source| AssetConversionError::Io {
            path: request.input_path.clone(),
            source,
        })?
        .len();
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
    let decision = preflight_decision(import_check.importable, import_check.failure_category);
    let quality = preflight_quality_report(source_size_bytes, import_check);

    Ok(AssetPreflightResult {
        decision,
        source_format: inspect.probe.format.label().to_string(),
        capability_status: inspect
            .capability()
            .map(|capability| capability.status.to_string()),
        visual_asset_count: inspect.visual_assets.len(),
        importable: import_check.importable,
        required_condition: import_check.required_condition.map(str::to_string),
        quality,
        node_count: import_check.node_count,
        mesh_count: import_check.mesh_count,
        primitive_count: import_check.primitive_count,
        vertex_count: import_check.vertex_count,
        triangle_count: import_check.triangle_count,
        failure,
    })
}

/// Reuses a current single-file package, or converts the source when it is stale.
pub fn ensure_asset_package(
    request: &AssetConversionRequest,
) -> Result<AssetPackageEnsureResult, AssetConversionError> {
    if let Some(asset) = current_asset_result(request)? {
        return Ok(AssetPackageEnsureResult {
            status: AssetPackageStatus::Reused,
            asset,
        });
    }

    Ok(AssetPackageEnsureResult {
        status: AssetPackageStatus::Converted,
        asset: convert_asset(request)?,
    })
}

/// Reuses a current batch package, or runs the batch when the package is stale.
pub fn ensure_batch_asset_package(
    request: &BatchAssetConversionRequest,
) -> Result<BatchAssetPackageEnsureResult, AssetConversionError> {
    if let Some(asset) = current_batch_asset_result(request)? {
        return Ok(BatchAssetPackageEnsureResult {
            status: AssetPackageStatus::Reused,
            asset,
        });
    }

    Ok(BatchAssetPackageEnsureResult {
        status: AssetPackageStatus::Converted,
        asset: convert_batch_assets_incremental(request)?,
    })
}

/// Loads a current single-file asset package without running conversion.
///
/// Returns `None` when the package is missing, incomplete, failed, or stale for
/// the request's source and conversion settings.
pub fn load_current_asset_package(
    request: &AssetConversionRequest,
) -> Result<Option<AssetConversionResult>, AssetConversionError> {
    current_asset_result(request)
}

/// Loads a current batch asset package without running conversion.
///
/// Returns `None` when the package is missing, incomplete, failed, or stale for
/// the request's source set, conversion settings, or batch mode.
pub fn load_current_batch_asset_package(
    request: &BatchAssetConversionRequest,
) -> Result<Option<BatchAssetConversionResult>, AssetConversionError> {
    current_batch_asset_result(request)
}

/// Inspects an existing business asset package without reading source files.
///
/// The audit verifies package JSON consistency and required output artifacts.
/// It does not prove that the package still matches an external source path;
/// use the freshness APIs for request-relative validation.
pub fn inspect_asset_package(
    output_dir: impl AsRef<Path>,
) -> Result<AssetPackageAudit, AssetConversionError> {
    let output_dir = output_dir.as_ref();
    let source_info_path = output_dir.join("source-info.json");
    if !source_info_path.is_file() {
        return Ok(AssetPackageAudit::missing(
            single_asset_package(output_dir),
            AssetPackageFreshnessReason::MissingSourceInfo,
        ));
    }

    let source_info: AssetSourceInfoJson = read_json(&source_info_path)?;
    let package = match source_info.kind.as_str() {
        "batch_conversion" => batch_asset_package(output_dir),
        _ => single_asset_package(output_dir),
    };
    if source_info.contract_version != ASSET_PACKAGE_CONTRACT_VERSION {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            None,
            false,
            AssetPackageFreshnessReason::PackageContractMismatch,
            None,
            None,
        ));
    }

    match source_info.kind.as_str() {
        "conversion" => inspect_single_asset_package(package, source_info),
        "batch_conversion" => inspect_batch_asset_package(package, source_info),
        _ => Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            None,
            false,
            AssetPackageFreshnessReason::PackageKindMismatch,
            None,
            None,
        )),
    }
}

/// Reads a business summary of package output artifacts.
///
/// The returned audit is always populated. `items` is populated only when the
/// package is internally usable; stale or failed packages return an empty item
/// list with the audit reason explaining why.
pub fn read_asset_package_summary(
    output_dir: impl AsRef<Path>,
) -> Result<AssetPackageSummary, AssetConversionError> {
    let output_dir = output_dir.as_ref();
    let audit = inspect_asset_package(output_dir)?;
    if !audit.usable {
        return Ok(empty_asset_package_summary(audit));
    }

    match audit.kind.as_deref() {
        Some("conversion") => read_single_asset_package_summary(audit),
        Some("batch_conversion") => read_batch_asset_package_summary(audit),
        _ => Ok(empty_asset_package_summary(audit)),
    }
}

/// Returns true when an existing single-file asset package matches this request.
pub fn is_asset_package_current(
    request: &AssetConversionRequest,
) -> Result<bool, AssetConversionError> {
    Ok(explain_asset_package_freshness(request)?.current)
}

/// Returns true when an existing batch asset package matches this request.
pub fn is_batch_asset_package_current(
    request: &BatchAssetConversionRequest,
) -> Result<bool, AssetConversionError> {
    Ok(explain_batch_asset_package_freshness(request)?.current)
}

/// Explains whether an existing single-file asset package matches this request.
pub fn explain_asset_package_freshness(
    request: &AssetConversionRequest,
) -> Result<AssetPackageFreshness, AssetConversionError> {
    Ok(inspect_current_asset_package(request)?.0)
}

/// Explains whether an existing batch asset package matches this request.
pub fn explain_batch_asset_package_freshness(
    request: &BatchAssetConversionRequest,
) -> Result<AssetPackageFreshness, AssetConversionError> {
    Ok(inspect_current_batch_asset_package(request)?.0)
}

fn current_asset_result(
    request: &AssetConversionRequest,
) -> Result<Option<AssetConversionResult>, AssetConversionError> {
    Ok(inspect_current_asset_package(request)?.1)
}

fn current_batch_asset_result(
    request: &BatchAssetConversionRequest,
) -> Result<Option<BatchAssetConversionResult>, AssetConversionError> {
    Ok(inspect_current_batch_asset_package(request)?.1)
}

fn inspect_current_asset_package(
    request: &AssetConversionRequest,
) -> Result<(AssetPackageFreshness, Option<AssetConversionResult>), AssetConversionError> {
    let package = single_asset_package(&request.output_dir);
    if !package.source_info_path.is_file() {
        return Ok(stale_asset(AssetPackageFreshnessReason::MissingSourceInfo));
    }
    if !package.diagnostics_path.is_file() {
        return Ok(stale_asset(AssetPackageFreshnessReason::MissingDiagnostics));
    }
    if !package
        .model_path
        .as_ref()
        .expect("single asset package reserves model path")
        .is_file()
    {
        return Ok(stale_asset(AssetPackageFreshnessReason::MissingModel));
    }
    if !package
        .metadata_path
        .as_ref()
        .expect("single asset package reserves metadata path")
        .is_file()
    {
        return Ok(stale_asset(AssetPackageFreshnessReason::MissingMetadata));
    }

    let prepared = prepare_single_asset_identity(request)?;
    let source_info: AssetSourceInfoJson = read_json(&package.source_info_path)?;
    let diagnostics: AssetDiagnosticsJson = read_json(&package.diagnostics_path)?;

    if let Some(reason) = single_asset_metadata_mismatch_reason(
        &source_info,
        &diagnostics,
        &prepared.source,
        &prepared.identity,
        request.profile.label(),
    ) {
        return Ok(stale_asset(reason));
    }

    let (
        Some(source_format),
        Some(node_count),
        Some(mesh_count),
        Some(primitive_count),
        Some(vertex_count),
        Some(triangle_count),
    ) = (
        diagnostics.source_format,
        diagnostics.node_count,
        diagnostics.mesh_count,
        diagnostics.primitive_count,
        diagnostics.vertex_count,
        diagnostics.triangle_count,
    )
    else {
        return Ok(stale_asset(
            AssetPackageFreshnessReason::IncompleteDiagnostics,
        ));
    };
    let model_path = package
        .model_path
        .as_ref()
        .expect("single asset package reserves model path");
    let metadata_path = package
        .metadata_path
        .as_ref()
        .expect("single asset package reserves metadata path");
    let quality = single_quality_report(SingleQualityInput {
        input_size_bytes: prepared.identity.source_size_bytes,
        node_count,
        mesh_count,
        primitive_count,
        vertex_count,
        triangle_count,
        output_path: model_path,
        metadata_path: Some(metadata_path.as_path()),
    });

    Ok((
        AssetPackageFreshness::current(),
        Some(AssetConversionResult {
            package,
            asset_id: prepared.identity.asset_id,
            source_sha256: prepared.identity.source_sha256,
            source_size_bytes: prepared.identity.source_size_bytes,
            settings_fingerprint: prepared.identity.settings_fingerprint,
            quality,
            source_format,
            node_count,
            mesh_count,
            primitive_count,
            vertex_count,
            triangle_count,
        }),
    ))
}

fn inspect_current_batch_asset_package(
    request: &BatchAssetConversionRequest,
) -> Result<(AssetPackageFreshness, Option<BatchAssetConversionResult>), AssetConversionError> {
    let package = batch_asset_package(&request.output_dir);
    let batch_output_dir = package
        .batch_output_dir
        .as_ref()
        .expect("batch asset package reserves output dir");
    let manifest_path = package
        .manifest_path
        .as_ref()
        .expect("batch asset package reserves manifest path");
    if !package.source_info_path.is_file() {
        return Ok(stale_batch(AssetPackageFreshnessReason::MissingSourceInfo));
    }
    if !package.diagnostics_path.is_file() {
        return Ok(stale_batch(AssetPackageFreshnessReason::MissingDiagnostics));
    }
    if !manifest_path.is_file() {
        return Ok(stale_batch(AssetPackageFreshnessReason::MissingManifest));
    }
    if !batch_output_dir.is_dir() {
        return Ok(stale_batch(
            AssetPackageFreshnessReason::MissingBatchOutputDirectory,
        ));
    }

    let input_paths = collect_batch_input_paths(&request.input_paths, &package.root_dir)
        .map_err(|error| batch_collect_error(package.clone(), error))?;
    if input_paths.is_empty() {
        return Ok(stale_batch(AssetPackageFreshnessReason::EmptyBatchInputSet));
    }

    let prepared = prepare_batch_asset_identity_from_paths(request, input_paths)?;
    let source_info: AssetSourceInfoJson = read_json(&package.source_info_path)?;
    let diagnostics: BatchAssetDiagnosticsJson = read_json(&package.diagnostics_path)?;
    if let Some(reason) = batch_asset_metadata_mismatch_reason(
        &source_info,
        &diagnostics,
        &prepared.sources,
        &prepared.identity,
        request.profile.label(),
    ) {
        return Ok(stale_batch(reason));
    }

    let Some(report) = read_batch_report(manifest_path)? else {
        return Ok(stale_batch(AssetPackageFreshnessReason::ManifestMismatch));
    };
    if let Some(reason) =
        batch_report_mismatch_reason(&report, request.check_only, &prepared.settings)
    {
        return Ok(stale_batch(reason));
    }
    if diagnostics.input_count != report.input_count()
        || diagnostics.converted_count != report.converted_count()
        || diagnostics.reused_count != report.reused_count()
        || diagnostics.checked_count != report.checked_count()
        || diagnostics.failed_count != report.failed_count()
    {
        return Ok(stale_batch(
            AssetPackageFreshnessReason::DiagnosticsMismatch,
        ));
    }

    Ok((
        AssetPackageFreshness::current(),
        Some(BatchAssetConversionResult {
            package,
            asset_id: prepared.identity.asset_id,
            source_sha256: prepared.identity.source_sha256,
            source_size_bytes: prepared.identity.source_size_bytes,
            settings_fingerprint: prepared.identity.settings_fingerprint,
            quality: batch_quality_report(&report),
            report,
        }),
    ))
}

fn stale_asset(
    reason: AssetPackageFreshnessReason,
) -> (AssetPackageFreshness, Option<AssetConversionResult>) {
    (AssetPackageFreshness::stale(reason), None)
}

fn stale_batch(
    reason: AssetPackageFreshnessReason,
) -> (AssetPackageFreshness, Option<BatchAssetConversionResult>) {
    (AssetPackageFreshness::stale(reason), None)
}

fn inspect_single_asset_package(
    package: AssetOutputPackage,
    source_info: AssetSourceInfoJson,
) -> Result<AssetPackageAudit, AssetConversionError> {
    if !package.diagnostics_path.is_file() {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            None,
            false,
            AssetPackageFreshnessReason::MissingDiagnostics,
            None,
            None,
        ));
    }
    let diagnostics: AssetDiagnosticsJson = read_json(&package.diagnostics_path)?;
    if let Some(reason) = audit_diagnostics_mismatch_reason(
        &source_info,
        DiagnosticsAuditFields {
            contract_version: diagnostics.contract_version.as_str(),
            profile: diagnostics.profile.as_str(),
            asset_id: diagnostics.asset_id.as_str(),
            source_sha256: diagnostics.source_sha256.as_str(),
            source_size_bytes: diagnostics.source_size_bytes,
            settings_fingerprint: diagnostics.settings_fingerprint.as_str(),
        },
    ) {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            Some(diagnostics.status),
            false,
            reason,
            diagnostics.quality,
            diagnostics.failure,
        ));
    }
    if diagnostics.status != "succeeded" {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            Some(diagnostics.status),
            false,
            AssetPackageFreshnessReason::DiagnosticsFailed,
            diagnostics.quality,
            diagnostics.failure,
        ));
    }
    let model_path = package
        .model_path
        .as_ref()
        .expect("single asset package reserves model path");
    if !model_path.is_file() {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            Some(diagnostics.status),
            false,
            AssetPackageFreshnessReason::MissingModel,
            diagnostics.quality,
            diagnostics.failure,
        ));
    }
    let metadata_path = package
        .metadata_path
        .as_ref()
        .expect("single asset package reserves metadata path");
    if !metadata_path.is_file() {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            Some(diagnostics.status),
            false,
            AssetPackageFreshnessReason::MissingMetadata,
            diagnostics.quality,
            diagnostics.failure,
        ));
    }

    let (
        Some(node_count),
        Some(mesh_count),
        Some(primitive_count),
        Some(vertex_count),
        Some(triangle_count),
    ) = (
        diagnostics.node_count,
        diagnostics.mesh_count,
        diagnostics.primitive_count,
        diagnostics.vertex_count,
        diagnostics.triangle_count,
    )
    else {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            Some(diagnostics.status),
            false,
            AssetPackageFreshnessReason::IncompleteDiagnostics,
            diagnostics.quality,
            diagnostics.failure,
        ));
    };

    let quality = diagnostics.quality.or_else(|| {
        source_info_identity(&source_info).map(|identity| {
            single_quality_report(SingleQualityInput {
                input_size_bytes: identity.source_size_bytes,
                node_count,
                mesh_count,
                primitive_count,
                vertex_count,
                triangle_count,
                output_path: model_path,
                metadata_path: Some(metadata_path.as_path()),
            })
        })
    });

    Ok(asset_package_audit_from_source_info(
        package,
        &source_info,
        Some(diagnostics.status),
        true,
        AssetPackageFreshnessReason::Current,
        quality,
        diagnostics.failure,
    ))
}

fn inspect_batch_asset_package(
    package: AssetOutputPackage,
    source_info: AssetSourceInfoJson,
) -> Result<AssetPackageAudit, AssetConversionError> {
    if !package.diagnostics_path.is_file() {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            None,
            false,
            AssetPackageFreshnessReason::MissingDiagnostics,
            None,
            None,
        ));
    }
    let diagnostics: BatchAssetDiagnosticsJson = read_json(&package.diagnostics_path)?;
    if let Some(reason) = audit_diagnostics_mismatch_reason(
        &source_info,
        DiagnosticsAuditFields {
            contract_version: diagnostics.contract_version.as_str(),
            profile: diagnostics.profile.as_str(),
            asset_id: diagnostics.asset_id.as_str(),
            source_sha256: diagnostics.source_sha256.as_str(),
            source_size_bytes: diagnostics.source_size_bytes,
            settings_fingerprint: diagnostics.settings_fingerprint.as_str(),
        },
    ) {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            Some(diagnostics.status),
            false,
            reason,
            diagnostics.quality,
            diagnostics.failure,
        ));
    }
    if diagnostics.status != "succeeded" {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            Some(diagnostics.status),
            false,
            AssetPackageFreshnessReason::DiagnosticsFailed,
            diagnostics.quality,
            diagnostics.failure,
        ));
    }

    let manifest_path = package
        .manifest_path
        .as_ref()
        .expect("batch asset package reserves manifest path");
    if !manifest_path.is_file() {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            Some(diagnostics.status),
            false,
            AssetPackageFreshnessReason::MissingManifest,
            diagnostics.quality,
            diagnostics.failure,
        ));
    }
    let batch_output_dir = package
        .batch_output_dir
        .as_ref()
        .expect("batch asset package reserves output dir");
    if !batch_output_dir.is_dir() {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            Some(diagnostics.status),
            false,
            AssetPackageFreshnessReason::MissingBatchOutputDirectory,
            diagnostics.quality,
            diagnostics.failure,
        ));
    }

    let Some(report) = read_batch_report(manifest_path)? else {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            Some(diagnostics.status),
            false,
            AssetPackageFreshnessReason::ManifestMismatch,
            diagnostics.quality,
            diagnostics.failure,
        ));
    };
    if diagnostics.input_count != report.input_count()
        || diagnostics.converted_count != report.converted_count()
        || diagnostics.reused_count != report.reused_count()
        || diagnostics.checked_count != report.checked_count()
        || diagnostics.failed_count != report.failed_count()
        || source_info.inputs.len() != report.input_count()
    {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            Some(diagnostics.status),
            false,
            AssetPackageFreshnessReason::DiagnosticsMismatch,
            diagnostics.quality,
            diagnostics.failure,
        ));
    }
    if let Some(reason) = batch_report_audit_mismatch_reason(&report) {
        return Ok(asset_package_audit_from_source_info(
            package,
            &source_info,
            Some(diagnostics.status),
            false,
            reason,
            diagnostics.quality,
            diagnostics.failure,
        ));
    }

    let quality = diagnostics
        .quality
        .or_else(|| Some(batch_quality_report(&report)));
    Ok(asset_package_audit_from_source_info(
        package,
        &source_info,
        Some(diagnostics.status),
        true,
        AssetPackageFreshnessReason::Current,
        quality,
        diagnostics.failure,
    ))
}

struct DiagnosticsAuditFields<'a> {
    contract_version: &'a str,
    profile: &'a str,
    asset_id: &'a str,
    source_sha256: &'a str,
    source_size_bytes: u64,
    settings_fingerprint: &'a str,
}

fn audit_diagnostics_mismatch_reason(
    source_info: &AssetSourceInfoJson,
    diagnostics: DiagnosticsAuditFields<'_>,
) -> Option<AssetPackageFreshnessReason> {
    if diagnostics.contract_version != ASSET_PACKAGE_CONTRACT_VERSION {
        return Some(AssetPackageFreshnessReason::PackageContractMismatch);
    }
    if source_info_identity(source_info).is_none() {
        return Some(AssetPackageFreshnessReason::SourceInfoMismatch);
    }
    if diagnostics.profile != source_info.profile
        || diagnostics.asset_id != source_info.asset_id
        || diagnostics.source_sha256 != source_info.source_sha256
        || diagnostics.source_size_bytes != source_info.source_size_bytes
        || diagnostics.settings_fingerprint != source_info.settings_fingerprint
    {
        return Some(AssetPackageFreshnessReason::DiagnosticsMismatch);
    }
    None
}

fn asset_package_audit_from_source_info(
    package: AssetOutputPackage,
    source_info: &AssetSourceInfoJson,
    status: Option<String>,
    usable: bool,
    reason: AssetPackageFreshnessReason,
    quality: Option<AssetQualityReport>,
    failure: Option<AssetFailure>,
) -> AssetPackageAudit {
    AssetPackageAudit {
        package,
        usable,
        reason,
        kind: Some(source_info.kind.clone()),
        profile: Some(source_info.profile.clone()),
        status,
        identity: source_info_identity(source_info),
        input_count: source_info.inputs.len(),
        quality,
        failure,
    }
}

fn source_info_identity(source_info: &AssetSourceInfoJson) -> Option<AssetIdentity> {
    if source_info.asset_id.is_empty()
        || source_info.source_sha256.is_empty()
        || source_info.settings_fingerprint.is_empty()
    {
        return None;
    }
    Some(AssetIdentity {
        asset_id: source_info.asset_id.clone(),
        source_sha256: source_info.source_sha256.clone(),
        source_size_bytes: source_info.source_size_bytes,
        settings_fingerprint: source_info.settings_fingerprint.clone(),
    })
}

fn empty_asset_package_summary(audit: AssetPackageAudit) -> AssetPackageSummary {
    AssetPackageSummary {
        audit,
        items: Vec::new(),
        output_size_bytes: 0,
        metadata_size_bytes: 0,
    }
}

fn read_single_asset_package_summary(
    audit: AssetPackageAudit,
) -> Result<AssetPackageSummary, AssetConversionError> {
    let package = audit.package.clone();
    let source_info: AssetSourceInfoJson = read_json(&package.source_info_path)?;
    let model_path = package
        .model_path
        .as_ref()
        .expect("single asset package reserves model path");
    let metadata_path = package
        .metadata_path
        .as_ref()
        .expect("single asset package reserves metadata path");
    let metadata = read_package_metadata(metadata_path)?;
    let output_size_bytes = file_size(model_path).unwrap_or(0);
    let metadata_size_bytes = file_size(metadata_path).unwrap_or(0);
    let item = AssetPackageSummaryItem {
        index: 0,
        operation: AssetPackageSummaryOperation::Converted,
        input_path: source_info.inputs.first().map(|input| input.path.clone()),
        source_format: Some(metadata.source_format.clone()),
        output_path: Some(model_path.clone()),
        metadata_path: Some(metadata_path.clone()),
        output_size_bytes,
        metadata_size_bytes,
        node_count: Some(metadata.node_count),
        mesh_count: Some(metadata.mesh_count),
        primitive_count: Some(metadata.primitive_count),
        vertex_count: Some(metadata.vertex_count),
        triangle_count: Some(metadata.triangle_count),
        metadata: Some(metadata),
    };

    Ok(AssetPackageSummary {
        audit,
        items: vec![item],
        output_size_bytes,
        metadata_size_bytes,
    })
}

fn read_batch_asset_package_summary(
    audit: AssetPackageAudit,
) -> Result<AssetPackageSummary, AssetConversionError> {
    let manifest_path = audit
        .package
        .manifest_path
        .as_ref()
        .expect("batch asset package reserves manifest path");
    let Some(report) = read_batch_report(manifest_path)? else {
        return Ok(empty_asset_package_summary(audit));
    };

    let mut items = Vec::with_capacity(report.items.len());
    for item in &report.items {
        items.push(asset_package_summary_item_from_batch_item(item)?);
    }
    let output_size_bytes = items.iter().map(|item| item.output_size_bytes).sum();
    let metadata_size_bytes = items.iter().map(|item| item.metadata_size_bytes).sum();
    Ok(AssetPackageSummary {
        audit,
        items,
        output_size_bytes,
        metadata_size_bytes,
    })
}

fn asset_package_summary_item_from_batch_item(
    item: &BatchItem,
) -> Result<AssetPackageSummaryItem, AssetConversionError> {
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
        } => summary_item_from_output_batch_status(OutputBatchStatusSummary {
            item,
            operation: AssetPackageSummaryOperation::Converted,
            source_format,
            output_path,
            metadata_path,
            output_size_bytes,
            metadata_size_bytes,
            node_count: *node_count,
            mesh_count: *mesh_count,
            primitive_count: *primitive_count,
            vertex_count: *vertex_count,
            triangle_count: *triangle_count,
        }),
        BatchItemStatus::Reused {
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
        } => summary_item_from_output_batch_status(OutputBatchStatusSummary {
            item,
            operation: AssetPackageSummaryOperation::Reused,
            source_format,
            output_path,
            metadata_path,
            output_size_bytes,
            metadata_size_bytes,
            node_count: *node_count,
            mesh_count: *mesh_count,
            primitive_count: *primitive_count,
            vertex_count: *vertex_count,
            triangle_count: *triangle_count,
        }),
        BatchItemStatus::Checked {
            source_format,
            node_count,
            mesh_count,
            primitive_count,
            vertex_count,
            triangle_count,
        } => Ok(AssetPackageSummaryItem {
            index: item.index,
            operation: AssetPackageSummaryOperation::Checked,
            input_path: Some(PathBuf::from(&item.input_path)),
            source_format: Some(source_format.clone()),
            output_path: None,
            metadata_path: None,
            output_size_bytes: 0,
            metadata_size_bytes: 0,
            node_count: Some(*node_count),
            mesh_count: Some(*mesh_count),
            primitive_count: Some(*primitive_count),
            vertex_count: Some(*vertex_count),
            triangle_count: Some(*triangle_count),
            metadata: None,
        }),
        BatchItemStatus::Error { .. } => Ok(AssetPackageSummaryItem {
            index: item.index,
            operation: AssetPackageSummaryOperation::Failed,
            input_path: Some(PathBuf::from(&item.input_path)),
            source_format: None,
            output_path: None,
            metadata_path: None,
            output_size_bytes: 0,
            metadata_size_bytes: 0,
            node_count: None,
            mesh_count: None,
            primitive_count: None,
            vertex_count: None,
            triangle_count: None,
            metadata: None,
        }),
    }
}

struct OutputBatchStatusSummary<'a> {
    item: &'a BatchItem,
    operation: AssetPackageSummaryOperation,
    source_format: &'a str,
    output_path: &'a str,
    metadata_path: &'a Option<String>,
    output_size_bytes: &'a Option<u64>,
    metadata_size_bytes: &'a Option<u64>,
    node_count: usize,
    mesh_count: usize,
    primitive_count: usize,
    vertex_count: usize,
    triangle_count: u64,
}

fn summary_item_from_output_batch_status(
    status: OutputBatchStatusSummary<'_>,
) -> Result<AssetPackageSummaryItem, AssetConversionError> {
    let metadata_path = status.metadata_path.as_ref().map(PathBuf::from);
    let metadata = metadata_path
        .as_ref()
        .map(|path| read_package_metadata(path))
        .transpose()?;
    Ok(AssetPackageSummaryItem {
        index: status.item.index,
        operation: status.operation,
        input_path: Some(PathBuf::from(&status.item.input_path)),
        source_format: Some(status.source_format.to_string()),
        output_path: Some(PathBuf::from(status.output_path)),
        metadata_path,
        output_size_bytes: status
            .output_size_bytes
            .unwrap_or_else(|| file_size(Path::new(status.output_path)).unwrap_or(0)),
        metadata_size_bytes: status.metadata_size_bytes.unwrap_or_else(|| {
            status
                .metadata_path
                .as_ref()
                .and_then(|path| file_size(Path::new(path)))
                .unwrap_or(0)
        }),
        node_count: Some(status.node_count),
        mesh_count: Some(status.mesh_count),
        primitive_count: Some(status.primitive_count),
        vertex_count: Some(status.vertex_count),
        triangle_count: Some(status.triangle_count),
        metadata,
    })
}

fn read_package_metadata(path: &Path) -> Result<AssetPackageMetadataSummary, AssetConversionError> {
    read_json(path)
}

fn single_asset_metadata_mismatch_reason(
    source_info: &AssetSourceInfoJson,
    diagnostics: &AssetDiagnosticsJson,
    source: &SourceIdentity,
    identity: &AssetIdentity,
    profile: &str,
) -> Option<AssetPackageFreshnessReason> {
    source_info_mismatch_reason(
        source_info,
        "conversion",
        std::slice::from_ref(source),
        identity,
        profile,
    )
    .or_else(|| single_diagnostics_mismatch_reason(diagnostics, identity, profile))
}

fn batch_asset_metadata_mismatch_reason(
    source_info: &AssetSourceInfoJson,
    diagnostics: &BatchAssetDiagnosticsJson,
    sources: &[SourceIdentity],
    identity: &AssetIdentity,
    profile: &str,
) -> Option<AssetPackageFreshnessReason> {
    source_info_mismatch_reason(source_info, "batch_conversion", sources, identity, profile)
        .or_else(|| batch_diagnostics_mismatch_reason(diagnostics, identity, profile))
}

fn source_info_mismatch_reason(
    source_info: &AssetSourceInfoJson,
    expected_kind: &str,
    sources: &[SourceIdentity],
    identity: &AssetIdentity,
    profile: &str,
) -> Option<AssetPackageFreshnessReason> {
    if source_info.contract_version != ASSET_PACKAGE_CONTRACT_VERSION {
        return Some(AssetPackageFreshnessReason::PackageContractMismatch);
    }
    if source_info.kind != expected_kind {
        return Some(AssetPackageFreshnessReason::PackageKindMismatch);
    }
    if source_info.source_sha256 != identity.source_sha256
        || source_info.source_size_bytes != identity.source_size_bytes
        || !source_info_inputs_match(&source_info.inputs, sources)
    {
        return Some(AssetPackageFreshnessReason::SourceChanged);
    }
    if source_info.profile != profile
        || source_info.settings_fingerprint != identity.settings_fingerprint
    {
        return Some(AssetPackageFreshnessReason::SettingsChanged);
    }
    if source_info.asset_id != identity.asset_id {
        return Some(AssetPackageFreshnessReason::SourceInfoMismatch);
    }
    None
}

fn single_diagnostics_mismatch_reason(
    diagnostics: &AssetDiagnosticsJson,
    identity: &AssetIdentity,
    profile: &str,
) -> Option<AssetPackageFreshnessReason> {
    diagnostics_mismatch_reason(
        DiagnosticsFreshnessFields {
            contract_version: diagnostics.contract_version.as_str(),
            status: diagnostics.status.as_str(),
            profile: diagnostics.profile.as_str(),
            asset_id: diagnostics.asset_id.as_str(),
            source_sha256: diagnostics.source_sha256.as_str(),
            source_size_bytes: diagnostics.source_size_bytes,
            settings_fingerprint: diagnostics.settings_fingerprint.as_str(),
        },
        identity,
        profile,
    )
}

fn batch_diagnostics_mismatch_reason(
    diagnostics: &BatchAssetDiagnosticsJson,
    identity: &AssetIdentity,
    profile: &str,
) -> Option<AssetPackageFreshnessReason> {
    diagnostics_mismatch_reason(
        DiagnosticsFreshnessFields {
            contract_version: diagnostics.contract_version.as_str(),
            status: diagnostics.status.as_str(),
            profile: diagnostics.profile.as_str(),
            asset_id: diagnostics.asset_id.as_str(),
            source_sha256: diagnostics.source_sha256.as_str(),
            source_size_bytes: diagnostics.source_size_bytes,
            settings_fingerprint: diagnostics.settings_fingerprint.as_str(),
        },
        identity,
        profile,
    )
}

struct DiagnosticsFreshnessFields<'a> {
    contract_version: &'a str,
    status: &'a str,
    profile: &'a str,
    asset_id: &'a str,
    source_sha256: &'a str,
    source_size_bytes: u64,
    settings_fingerprint: &'a str,
}

fn diagnostics_mismatch_reason(
    diagnostics: DiagnosticsFreshnessFields<'_>,
    identity: &AssetIdentity,
    expected_profile: &str,
) -> Option<AssetPackageFreshnessReason> {
    if diagnostics.contract_version != ASSET_PACKAGE_CONTRACT_VERSION {
        return Some(AssetPackageFreshnessReason::PackageContractMismatch);
    }
    if diagnostics.status != "succeeded" {
        return Some(AssetPackageFreshnessReason::DiagnosticsFailed);
    }
    if diagnostics.source_sha256 != identity.source_sha256
        || diagnostics.source_size_bytes != identity.source_size_bytes
    {
        return Some(AssetPackageFreshnessReason::SourceChanged);
    }
    if diagnostics.profile != expected_profile
        || diagnostics.settings_fingerprint != identity.settings_fingerprint
    {
        return Some(AssetPackageFreshnessReason::SettingsChanged);
    }
    if diagnostics.asset_id != identity.asset_id {
        return Some(AssetPackageFreshnessReason::DiagnosticsMismatch);
    }
    None
}

fn source_info_inputs_match(inputs: &[AssetSourceInputJson], sources: &[SourceIdentity]) -> bool {
    inputs.len() == sources.len()
        && inputs.iter().zip(sources).all(|(input, source)| {
            input.path.as_path() == source.path.as_path()
                && input.source_sha256 == source.source_sha256
                && input.source_size_bytes == source.source_size_bytes
        })
}

#[derive(Debug, Clone)]
struct AssetQualityMetrics {
    input_count: usize,
    successful_count: usize,
    converted_count: usize,
    reused_count: usize,
    checked_count: usize,
    failed_count: usize,
    preview_output_count: usize,
    node_count: usize,
    mesh_count: usize,
    primitive_count: usize,
    vertex_count: usize,
    triangle_count: u64,
    input_size_bytes: u64,
    output_size_bytes: u64,
    metadata_size_bytes: u64,
}

struct SingleQualityInput<'a> {
    input_size_bytes: u64,
    node_count: usize,
    mesh_count: usize,
    primitive_count: usize,
    vertex_count: usize,
    triangle_count: u64,
    output_path: &'a Path,
    metadata_path: Option<&'a Path>,
}

fn single_quality_report(input: SingleQualityInput<'_>) -> AssetQualityReport {
    quality_report_from_metrics(AssetQualityMetrics {
        input_count: 1,
        successful_count: 1,
        converted_count: 1,
        reused_count: 0,
        checked_count: 0,
        failed_count: 0,
        preview_output_count: usize::from(input.output_path.is_file()),
        node_count: input.node_count,
        mesh_count: input.mesh_count,
        primitive_count: input.primitive_count,
        vertex_count: input.vertex_count,
        triangle_count: input.triangle_count,
        input_size_bytes: input.input_size_bytes,
        output_size_bytes: file_size(input.output_path).unwrap_or(0),
        metadata_size_bytes: input.metadata_path.and_then(file_size).unwrap_or(0),
    })
}

fn batch_quality_report(report: &BatchReport) -> AssetQualityReport {
    let summary = report.summary();
    quality_report_from_metrics(AssetQualityMetrics {
        input_count: report.input_count(),
        successful_count: report.success_count(),
        converted_count: report.converted_count(),
        reused_count: report.reused_count(),
        checked_count: report.checked_count(),
        failed_count: report.failed_count(),
        preview_output_count: report.converted_count() + report.reused_count(),
        node_count: summary.total_node_count,
        mesh_count: summary.total_mesh_count,
        primitive_count: summary.total_primitive_count,
        vertex_count: summary.total_vertex_count,
        triangle_count: summary.total_triangle_count,
        input_size_bytes: summary.total_input_bytes,
        output_size_bytes: summary.total_output_bytes,
        metadata_size_bytes: summary.total_metadata_bytes,
    })
}

fn quality_report_from_metrics(metrics: AssetQualityMetrics) -> AssetQualityReport {
    let has_visual_geometry = metrics.mesh_count > 0
        && metrics.primitive_count > 0
        && metrics.vertex_count > 0
        && metrics.triangle_count > 0;
    let preview_status = if metrics.failed_count > 0 {
        AssetPreviewStatus::PartialFailure
    } else if !has_visual_geometry {
        AssetPreviewStatus::NoVisualGeometry
    } else if metrics.preview_output_count == 0 {
        AssetPreviewStatus::NoPreviewOutput
    } else {
        AssetPreviewStatus::Ready
    };

    AssetQualityReport {
        previewable: preview_status == AssetPreviewStatus::Ready,
        has_visual_geometry,
        preview_status,
        quality_level: quality_level_for_triangles(metrics.triangle_count),
        input_count: metrics.input_count,
        successful_count: metrics.successful_count,
        converted_count: metrics.converted_count,
        reused_count: metrics.reused_count,
        checked_count: metrics.checked_count,
        failed_count: metrics.failed_count,
        node_count: metrics.node_count,
        mesh_count: metrics.mesh_count,
        primitive_count: metrics.primitive_count,
        vertex_count: metrics.vertex_count,
        triangle_count: metrics.triangle_count,
        input_size_bytes: metrics.input_size_bytes,
        output_size_bytes: metrics.output_size_bytes,
        metadata_size_bytes: metrics.metadata_size_bytes,
    }
}

fn quality_level_for_triangles(triangle_count: u64) -> AssetQualityLevel {
    match triangle_count {
        0 => AssetQualityLevel::Empty,
        1..=LIGHT_TRIANGLE_LIMIT => AssetQualityLevel::Light,
        _ if triangle_count <= MEDIUM_TRIANGLE_LIMIT => AssetQualityLevel::Medium,
        _ if triangle_count <= HEAVY_TRIANGLE_LIMIT => AssetQualityLevel::Heavy,
        _ => AssetQualityLevel::Oversized,
    }
}

impl AssetConversionResult {
    fn from_summary(
        package: AssetOutputPackage,
        identity: AssetIdentity,
        summary: ConversionSummary,
    ) -> Self {
        let quality = single_quality_report(SingleQualityInput {
            input_size_bytes: identity.source_size_bytes,
            node_count: summary.node_count,
            mesh_count: summary.mesh_count,
            primitive_count: summary.primitive_count,
            vertex_count: summary.vertex_count,
            triangle_count: summary.triangle_count,
            output_path: &summary.output_path,
            metadata_path: summary.metadata_path.as_deref(),
        });
        Self {
            package,
            asset_id: identity.asset_id,
            source_sha256: identity.source_sha256,
            source_size_bytes: identity.source_size_bytes,
            settings_fingerprint: identity.settings_fingerprint,
            quality,
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
    /// Returns the business decision represented by this failure category.
    pub fn decision(&self) -> AssetPreflightDecision {
        failure_decision_for_category(&self.category)
    }

    /// Returns the recommended next action for operators or upstream systems.
    pub fn action(&self) -> AssetFailureAction {
        failure_action_for_category(&self.category)
    }

    /// Returns the missing condition that would make this failure actionable.
    pub fn required_condition(&self) -> Option<&'static str> {
        failure_required_condition(&self.category)
    }

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

fn failure_decision_for_category(category: &str) -> AssetPreflightDecision {
    match category {
        "no_readable_lightweight_cache" | "native_visualization_not_decoded" => {
            AssetPreflightDecision::NeedsReadableVisualization
        }
        "missing_external_reference" => AssetPreflightDecision::NeedsExternalReferences,
        "tessellation_pending" => AssetPreflightDecision::NeedsUpstreamTessellation,
        "resource_limit_exceeded" => AssetPreflightDecision::ResourceLimitExceeded,
        "unsupported_input" => AssetPreflightDecision::UnsupportedInput,
        "invalid_source_data" => AssetPreflightDecision::InvalidSourceData,
        "missing_data" => AssetPreflightDecision::MissingData,
        "io" => AssetPreflightDecision::IoBlocked,
        "export" => AssetPreflightDecision::ExportBlocked,
        _ => AssetPreflightDecision::Failed,
    }
}

fn failure_action_for_category(category: &str) -> AssetFailureAction {
    match category {
        "no_readable_lightweight_cache" | "native_visualization_not_decoded" => {
            AssetFailureAction::ProvideReadableVisualization
        }
        "missing_external_reference" => AssetFailureAction::ResolveExternalReferences,
        "tessellation_pending" => AssetFailureAction::RunUpstreamTessellation,
        "resource_limit_exceeded" => AssetFailureAction::IncreaseResourceLimits,
        "unsupported_input" => AssetFailureAction::UseSupportedInput,
        "invalid_source_data" => AssetFailureAction::RepairSourceData,
        "missing_data" => AssetFailureAction::CompleteSourcePackage,
        "io" => AssetFailureAction::CheckStorageAccess,
        "export" => AssetFailureAction::FixExportPipeline,
        "batch_item_failed" => AssetFailureAction::ReviewBatchFailures,
        _ => AssetFailureAction::InspectFailure,
    }
}

fn failure_required_condition(category: &str) -> Option<&'static str> {
    match category {
        "no_readable_lightweight_cache" | "native_visualization_not_decoded" => {
            Some("readable lightweight visualization payload")
        }
        "missing_external_reference" => {
            Some("all external references resolved through resolve_dirs or reference mappings")
        }
        "tessellation_pending" => Some("upstream tessellation or supported analytic geometry"),
        "resource_limit_exceeded" => Some("larger import limits or a smaller source package"),
        "unsupported_input" => Some("a supported CAD, mesh, lightweight, or STEP input"),
        "invalid_source_data" => Some("valid source data"),
        "missing_data" => Some("complete source package data"),
        "io" => Some("readable input and writable output filesystem paths"),
        "export" => Some("valid exportable geometry"),
        "batch_item_failed" => Some("all failed batch items corrected or removed"),
        _ => None,
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

fn preflight_decision(importable: bool, failure_category: Option<&str>) -> AssetPreflightDecision {
    if importable {
        return AssetPreflightDecision::Ready;
    }
    failure_decision_for_category(failure_category.unwrap_or("other"))
}

fn preflight_quality_report(
    input_size_bytes: u64,
    import_check: &crate::inspect::InspectImportCheck,
) -> Option<AssetQualityReport> {
    import_check.importable.then(|| {
        quality_report_from_metrics(AssetQualityMetrics {
            input_count: 1,
            successful_count: 1,
            converted_count: 0,
            reused_count: 0,
            checked_count: 1,
            failed_count: 0,
            preview_output_count: 0,
            node_count: import_check.node_count.unwrap_or(0),
            mesh_count: import_check.mesh_count.unwrap_or(0),
            primitive_count: import_check.primitive_count.unwrap_or(0),
            vertex_count: import_check.vertex_count.unwrap_or(0),
            triangle_count: import_check.triangle_count.unwrap_or(0),
            input_size_bytes,
            output_size_bytes: 0,
            metadata_size_bytes: 0,
        })
    })
}

fn collect_batch_inputs_or_write_failure(
    request: &BatchAssetConversionRequest,
    package: &AssetOutputPackage,
    excluded_package_dir: &Path,
    profile: &str,
) -> Result<Vec<PathBuf>, AssetConversionError> {
    match collect_batch_input_paths(&request.input_paths, excluded_package_dir) {
        Ok(paths) if !paths.is_empty() => Ok(paths),
        Ok(_) => {
            let error = BatchConversionError::EmptyInputSet;
            let failure = AssetFailure::from_batch_error(&error);
            let _ = write_failure_only_diagnostics(package, profile, "failed", None, &failure);
            Err(AssetConversionError::BatchFailed {
                package: Box::new(package.clone()),
                failure: Box::new(failure),
            })
        }
        Err(error) => {
            let error = BatchConversionError::CollectInputs(error);
            let failure = AssetFailure::from_batch_error(&error);
            let _ = write_failure_only_diagnostics(package, profile, "failed", None, &failure);
            Err(AssetConversionError::BatchFailed {
                package: Box::new(package.clone()),
                failure: Box::new(failure),
            })
        }
    }
}

fn batch_collect_error(
    package: AssetOutputPackage,
    error: crate::batch::BatchInputCollectionError,
) -> AssetConversionError {
    let error = BatchConversionError::CollectInputs(error);
    let failure = AssetFailure::from_batch_error(&error);
    AssetConversionError::BatchFailed {
        package: Box::new(package),
        failure: Box::new(failure),
    }
}

fn empty_batch_error(package: AssetOutputPackage) -> AssetConversionError {
    let failure = AssetFailure::from_batch_error(&BatchConversionError::EmptyInputSet);
    AssetConversionError::BatchFailed {
        package: Box::new(package),
        failure: Box::new(failure),
    }
}

#[derive(Debug, Clone)]
struct ReusableBatchItem {
    source_sha256: String,
    source_size_bytes: u64,
    status: BatchItemStatus,
}

fn batch_reuse_candidates(
    package: &AssetOutputPackage,
) -> Result<BTreeMap<PathBuf, ReusableBatchItem>, AssetConversionError> {
    let manifest_path = package
        .manifest_path
        .as_ref()
        .expect("batch asset package reserves manifest path");
    if !package.source_info_path.is_file() || !manifest_path.is_file() {
        return Ok(BTreeMap::new());
    }

    let source_info: AssetSourceInfoJson = read_json(&package.source_info_path)?;
    if source_info.contract_version != ASSET_PACKAGE_CONTRACT_VERSION
        || source_info.kind != "batch_conversion"
    {
        return Ok(BTreeMap::new());
    }
    let Some(report) = read_batch_report(manifest_path)? else {
        return Ok(BTreeMap::new());
    };

    let mut source_by_path = BTreeMap::new();
    for input in source_info.inputs {
        source_by_path.insert(input.path, (input.source_sha256, input.source_size_bytes));
    }

    let mut candidates = BTreeMap::new();
    for item in report.items {
        let path = PathBuf::from(&item.input_path);
        let Some((source_sha256, source_size_bytes)) = source_by_path.get(&path) else {
            continue;
        };
        let Some(status) = reusable_batch_status(&item.status) else {
            continue;
        };
        candidates.insert(
            path,
            ReusableBatchItem {
                source_sha256: source_sha256.clone(),
                source_size_bytes: *source_size_bytes,
                status,
            },
        );
    }
    Ok(candidates)
}

fn reusable_batch_status(status: &BatchItemStatus) -> Option<BatchItemStatus> {
    match status {
        BatchItemStatus::Ok {
            source_format,
            output_path,
            metadata_path,
            node_count,
            mesh_count,
            primitive_count,
            vertex_count,
            triangle_count,
            ..
        }
        | BatchItemStatus::Reused {
            source_format,
            output_path,
            metadata_path,
            node_count,
            mesh_count,
            primitive_count,
            vertex_count,
            triangle_count,
            ..
        } => {
            if !Path::new(output_path).is_file()
                || metadata_path
                    .as_ref()
                    .is_some_and(|path| !Path::new(path).is_file())
            {
                return None;
            }
            Some(BatchItemStatus::Reused {
                source_format: source_format.clone(),
                output_path: output_path.clone(),
                metadata_path: metadata_path.clone(),
                output_size_bytes: file_size(Path::new(output_path)),
                metadata_size_bytes: metadata_path
                    .as_ref()
                    .and_then(|path| file_size(Path::new(path))),
                node_count: *node_count,
                mesh_count: *mesh_count,
                primitive_count: *primitive_count,
                vertex_count: *vertex_count,
                triangle_count: *triangle_count,
            })
        }
        BatchItemStatus::Checked { .. } | BatchItemStatus::Error { .. } => None,
    }
}

fn run_incremental_batch_items(
    input_paths: &[PathBuf],
    input_identities: &[SourceIdentity],
    output_dir: &Path,
    manifest_path: &Path,
    settings: &JobConversionSettings,
    check_only: bool,
    reusable_items: &BTreeMap<PathBuf, ReusableBatchItem>,
) -> Result<BatchReport, BatchConversionError> {
    let options = BatchConversionOptions {
        output_dir: output_dir.to_path_buf(),
        manifest_path: None,
        check_only,
        conversion: settings.to_conversion_options(None),
    };
    let snapshots =
        collect_incremental_output_snapshots(input_paths, output_dir, settings, check_only);
    let mut items = Vec::with_capacity(input_paths.len());
    for (index, (input_path, source)) in input_paths.iter().zip(input_identities).enumerate() {
        if !check_only
            && let Some(reusable) = reusable_items.get(input_path)
            && reusable.source_sha256 == source.source_sha256
            && reusable.source_size_bytes == source.source_size_bytes
        {
            items.push(BatchItem {
                index,
                input_path: input_path.display().to_string(),
                input_size_bytes: Some(source.source_size_bytes),
                duration_ms: 0,
                status: reusable.status.clone(),
            });
            continue;
        }

        items.push(run_batch_item(index, input_path, &options));
    }

    let report = BatchReport::new(items);
    if let Err(source) = write_atomic(manifest_path, report.to_manifest_json()) {
        cleanup_incremental_created_outputs(&report, &snapshots);
        return Err(BatchConversionError::WriteManifest {
            path: manifest_path.to_path_buf(),
            source,
        });
    }
    Ok(report)
}

#[derive(Debug, Clone)]
struct IncrementalOutputSnapshot {
    output_path: PathBuf,
    output_existed: bool,
    metadata_path: Option<PathBuf>,
    metadata_existed: bool,
}

fn collect_incremental_output_snapshots(
    input_paths: &[PathBuf],
    output_dir: &Path,
    settings: &JobConversionSettings,
    check_only: bool,
) -> Vec<IncrementalOutputSnapshot> {
    if check_only {
        return Vec::new();
    }

    input_paths
        .iter()
        .enumerate()
        .map(|(index, input_path)| {
            let output_path = output_dir.join(batch_output_file_name(index, input_path));
            let metadata_path = settings
                .write_metadata
                .then(|| output_path.with_extension("metadata.json"));
            let metadata_existed = metadata_path.as_ref().is_some_and(|path| path.exists());
            IncrementalOutputSnapshot {
                output_existed: output_path.exists(),
                output_path,
                metadata_path,
                metadata_existed,
            }
        })
        .collect()
}

fn cleanup_incremental_created_outputs(
    report: &BatchReport,
    snapshots: &[IncrementalOutputSnapshot],
) {
    for item in &report.items {
        if !item.status.is_converted() {
            continue;
        }
        let Some(snapshot) = snapshots.get(item.index) else {
            continue;
        };
        remove_file_if_created(&snapshot.output_path, snapshot.output_existed);
        if let Some(metadata_path) = &snapshot.metadata_path {
            remove_file_if_created(metadata_path, snapshot.metadata_existed);
        }
    }
}

fn file_size(path: &Path) -> Option<u64> {
    fs::metadata(path).map(|metadata| metadata.len()).ok()
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceIdentity {
    path: PathBuf,
    source_sha256: String,
    source_size_bytes: u64,
}

#[derive(Debug, Clone)]
struct PreparedSingleAssetIdentity {
    settings: JobConversionSettings,
    source: SourceIdentity,
    identity: AssetIdentity,
}

#[derive(Debug, Clone)]
struct PreparedBatchAssetIdentity {
    settings: JobConversionSettings,
    input_paths: Vec<PathBuf>,
    sources: Vec<SourceIdentity>,
    identity: AssetIdentity,
}

fn prepare_single_asset_identity(
    request: &AssetConversionRequest,
) -> Result<PreparedSingleAssetIdentity, AssetConversionError> {
    let settings = settings_for_asset_request(
        &request.profile,
        &request.resolve_dirs,
        &request.reference_path_mappings,
        request.limits,
    );
    let source = source_identity(&request.input_path)?;
    let identity = identity_from_parts(
        std::slice::from_ref(&source),
        request.profile.label(),
        &settings,
        None,
    );

    Ok(PreparedSingleAssetIdentity {
        settings,
        source,
        identity,
    })
}

fn prepare_batch_asset_identity_from_paths(
    request: &BatchAssetConversionRequest,
    input_paths: Vec<PathBuf>,
) -> Result<PreparedBatchAssetIdentity, AssetConversionError> {
    let settings = settings_for_asset_request(
        &request.profile,
        &request.resolve_dirs,
        &request.reference_path_mappings,
        request.limits,
    );
    let sources = source_identities(&input_paths)?;
    let identity = batch_identity_from_parts(
        &sources,
        request.profile.label(),
        &settings,
        request.check_only,
    );

    Ok(PreparedBatchAssetIdentity {
        settings,
        input_paths,
        sources,
        identity,
    })
}

fn identity_from_parts(
    sources: &[SourceIdentity],
    profile: &str,
    settings: &JobConversionSettings,
    mode: Option<&str>,
) -> AssetIdentity {
    let source_sha256 = aggregate_source_sha256(sources);
    let source_size_bytes = sources.iter().map(|source| source.source_size_bytes).sum();
    let settings_fingerprint = conversion_settings_fingerprint(profile, settings, mode);
    let mut hasher = Sha256::new();
    hasher.update(ASSET_PACKAGE_CONTRACT_VERSION.as_bytes());
    hasher.update(b"\0asset\0");
    hasher.update(source_sha256.as_bytes());
    hasher.update(b"\0settings\0");
    hasher.update(settings_fingerprint.as_bytes());
    let digest = hasher.finalize();
    let asset_id = format!("asset-{}", hex_digest(&digest));
    AssetIdentity {
        asset_id,
        source_sha256,
        source_size_bytes,
        settings_fingerprint,
    }
}

fn batch_identity_from_parts(
    sources: &[SourceIdentity],
    profile: &str,
    settings: &JobConversionSettings,
    check_only: bool,
) -> AssetIdentity {
    let mode = if check_only { "check_only" } else { "convert" };
    identity_from_parts(sources, profile, settings, Some(mode))
}

fn source_identities(paths: &[PathBuf]) -> Result<Vec<SourceIdentity>, AssetConversionError> {
    paths
        .iter()
        .map(|path| source_identity(path))
        .collect::<Result<Vec<_>, _>>()
}

fn source_identity(path: &Path) -> Result<SourceIdentity, AssetConversionError> {
    let mut file = fs::File::open(path).map_err(|source| AssetConversionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read_len = file
            .read(&mut buffer)
            .map_err(|source| AssetConversionError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if read_len == 0 {
            break;
        }
        size += read_len as u64;
        hasher.update(&buffer[..read_len]);
    }
    Ok(SourceIdentity {
        path: path.to_path_buf(),
        source_sha256: {
            let digest = hasher.finalize();
            hex_digest(&digest)
        },
        source_size_bytes: size,
    })
}

fn aggregate_source_sha256(sources: &[SourceIdentity]) -> String {
    if sources.len() == 1 {
        return sources[0].source_sha256.clone();
    }

    let mut hasher = Sha256::new();
    hasher.update(ASSET_PACKAGE_CONTRACT_VERSION.as_bytes());
    hasher.update(b"\0sources\0");
    for source in sources {
        hasher.update(source.source_sha256.as_bytes());
        hasher.update(b"\0");
        hasher.update(source.source_size_bytes.to_le_bytes());
        hasher.update(b"\0");
    }
    let digest = hasher.finalize();
    hex_digest(&digest)
}

fn conversion_settings_fingerprint(
    profile: &str,
    settings: &JobConversionSettings,
    mode: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(ASSET_PACKAGE_CONTRACT_VERSION.as_bytes());
    hasher.update(b"\0profile\0");
    hasher.update(profile.as_bytes());
    hasher.update(b"\0settings\0");
    let settings_json =
        serde_json::to_vec(settings).expect("asset conversion settings should serialize");
    hasher.update(settings_json);
    if let Some(mode) = mode {
        hasher.update(b"\0mode\0");
        hasher.update(mode.as_bytes());
    }
    let digest = hasher.finalize();
    hex_digest(&digest)
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn ensure_dir(path: &Path) -> Result<(), AssetConversionError> {
    fs::create_dir_all(path).map_err(|source| AssetConversionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Serialize, Deserialize)]
struct AssetSourceInfoJson {
    contract_version: String,
    kind: String,
    profile: String,
    #[serde(default)]
    asset_id: String,
    #[serde(default)]
    source_sha256: String,
    #[serde(default)]
    source_size_bytes: u64,
    #[serde(default)]
    settings_fingerprint: String,
    #[serde(default)]
    inputs: Vec<AssetSourceInputJson>,
    created_at_unix_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct AssetSourceInputJson {
    path: PathBuf,
    #[serde(default)]
    source_sha256: String,
    #[serde(default)]
    source_size_bytes: u64,
}

fn write_source_info(
    package: &AssetOutputPackage,
    profile: &str,
    sources: &[SourceIdentity],
    identity: &AssetIdentity,
    kind: &str,
) -> Result<(), AssetConversionError> {
    let source_info = AssetSourceInfoJson {
        contract_version: ASSET_PACKAGE_CONTRACT_VERSION.to_string(),
        kind: kind.to_string(),
        profile: profile.to_string(),
        asset_id: identity.asset_id.clone(),
        source_sha256: identity.source_sha256.clone(),
        source_size_bytes: identity.source_size_bytes,
        settings_fingerprint: identity.settings_fingerprint.clone(),
        inputs: sources
            .iter()
            .map(|source| AssetSourceInputJson {
                path: source.path.clone(),
                source_sha256: source.source_sha256.clone(),
                source_size_bytes: source.source_size_bytes,
            })
            .collect(),
        created_at_unix_ms: unix_timestamp_millis(),
    };
    write_json(&package.source_info_path, &source_info)
}

#[derive(Serialize, Deserialize)]
struct AssetDiagnosticsJson {
    contract_version: String,
    status: String,
    profile: String,
    #[serde(default)]
    asset_id: String,
    #[serde(default)]
    source_sha256: String,
    #[serde(default)]
    source_size_bytes: u64,
    #[serde(default)]
    settings_fingerprint: String,
    source_format: Option<String>,
    node_count: Option<usize>,
    mesh_count: Option<usize>,
    primitive_count: Option<usize>,
    vertex_count: Option<usize>,
    triangle_count: Option<u64>,
    quality: Option<AssetQualityReport>,
    failure: Option<AssetFailure>,
    #[serde(default)]
    failure_decision: Option<AssetPreflightDecision>,
    #[serde(default)]
    failure_action: Option<AssetFailureAction>,
    #[serde(default)]
    failure_required_condition: Option<String>,
    updated_at_unix_ms: u64,
}

fn write_diagnostics(
    package: &AssetOutputPackage,
    profile: &str,
    status: &str,
    identity: &AssetIdentity,
    result: Option<&AssetConversionResult>,
    failure: Option<&AssetFailure>,
) -> Result<(), AssetConversionError> {
    let diagnostics = AssetDiagnosticsJson {
        contract_version: ASSET_PACKAGE_CONTRACT_VERSION.to_string(),
        status: status.to_string(),
        profile: profile.to_string(),
        asset_id: identity.asset_id.clone(),
        source_sha256: identity.source_sha256.clone(),
        source_size_bytes: identity.source_size_bytes,
        settings_fingerprint: identity.settings_fingerprint.clone(),
        source_format: result.map(|result| result.source_format.clone()),
        node_count: result.map(|result| result.node_count),
        mesh_count: result.map(|result| result.mesh_count),
        primitive_count: result.map(|result| result.primitive_count),
        vertex_count: result.map(|result| result.vertex_count),
        triangle_count: result.map(|result| result.triangle_count),
        quality: result.map(|result| result.quality.clone()),
        failure: failure.cloned(),
        failure_decision: failure.map(AssetFailure::decision),
        failure_action: failure.map(AssetFailure::action),
        failure_required_condition: failure
            .and_then(AssetFailure::required_condition)
            .map(str::to_string),
        updated_at_unix_ms: unix_timestamp_millis(),
    };
    write_json(&package.diagnostics_path, &diagnostics)
}

#[derive(Serialize, Deserialize)]
struct BatchAssetDiagnosticsJson {
    contract_version: String,
    status: String,
    profile: String,
    #[serde(default)]
    asset_id: String,
    #[serde(default)]
    source_sha256: String,
    #[serde(default)]
    source_size_bytes: u64,
    #[serde(default)]
    settings_fingerprint: String,
    input_count: usize,
    converted_count: usize,
    #[serde(default)]
    reused_count: usize,
    checked_count: usize,
    failed_count: usize,
    quality: Option<AssetQualityReport>,
    failure: Option<AssetFailure>,
    #[serde(default)]
    failure_decision: Option<AssetPreflightDecision>,
    #[serde(default)]
    failure_action: Option<AssetFailureAction>,
    #[serde(default)]
    failure_required_condition: Option<String>,
    updated_at_unix_ms: u64,
}

fn write_batch_diagnostics(
    package: &AssetOutputPackage,
    profile: &str,
    status: &str,
    result: &BatchAssetConversionResult,
    identity: &AssetIdentity,
    failure: Option<&AssetFailure>,
) -> Result<(), AssetConversionError> {
    let diagnostics = BatchAssetDiagnosticsJson {
        contract_version: ASSET_PACKAGE_CONTRACT_VERSION.to_string(),
        status: status.to_string(),
        profile: profile.to_string(),
        asset_id: identity.asset_id.clone(),
        source_sha256: identity.source_sha256.clone(),
        source_size_bytes: identity.source_size_bytes,
        settings_fingerprint: identity.settings_fingerprint.clone(),
        input_count: result.report.input_count(),
        converted_count: result.report.converted_count(),
        reused_count: result.report.reused_count(),
        checked_count: result.report.checked_count(),
        failed_count: result.report.failed_count(),
        quality: Some(result.quality.clone()),
        failure: failure.cloned(),
        failure_decision: failure.map(AssetFailure::decision),
        failure_action: failure.map(AssetFailure::action),
        failure_required_condition: failure
            .and_then(AssetFailure::required_condition)
            .map(str::to_string),
        updated_at_unix_ms: unix_timestamp_millis(),
    };
    write_json(&package.diagnostics_path, &diagnostics)
}

fn write_failure_only_diagnostics(
    package: &AssetOutputPackage,
    profile: &str,
    status: &str,
    identity: Option<&AssetIdentity>,
    failure: &AssetFailure,
) -> Result<(), AssetConversionError> {
    let diagnostics = BatchAssetDiagnosticsJson {
        contract_version: ASSET_PACKAGE_CONTRACT_VERSION.to_string(),
        status: status.to_string(),
        profile: profile.to_string(),
        asset_id: identity
            .map(|identity| identity.asset_id.clone())
            .unwrap_or_default(),
        source_sha256: identity
            .map(|identity| identity.source_sha256.clone())
            .unwrap_or_default(),
        source_size_bytes: identity
            .map(|identity| identity.source_size_bytes)
            .unwrap_or(0),
        settings_fingerprint: identity
            .map(|identity| identity.settings_fingerprint.clone())
            .unwrap_or_default(),
        input_count: 0,
        converted_count: 0,
        reused_count: 0,
        checked_count: 0,
        failed_count: 0,
        quality: None,
        failure: Some(failure.clone()),
        failure_decision: Some(failure.decision()),
        failure_action: Some(failure.action()),
        failure_required_condition: failure.required_condition().map(str::to_string),
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

#[derive(Deserialize)]
struct BatchManifestJson {
    contract_version: String,
    items: Vec<BatchManifestItemJson>,
}

#[derive(Deserialize)]
struct BatchManifestItemJson {
    index: usize,
    input_path: String,
    input_size_bytes: Option<u64>,
    duration_ms: u128,
    status: String,
    source_format: Option<String>,
    output_path: Option<String>,
    metadata_path: Option<String>,
    output_size_bytes: Option<u64>,
    metadata_size_bytes: Option<u64>,
    node_count: Option<usize>,
    mesh_count: Option<usize>,
    primitive_count: Option<usize>,
    vertex_count: Option<usize>,
    triangle_count: Option<u64>,
}

fn read_batch_report(path: &Path) -> Result<Option<BatchReport>, AssetConversionError> {
    let manifest: BatchManifestJson = read_json(path)?;
    if manifest.contract_version != BATCH_MANIFEST_CONTRACT_VERSION {
        return Ok(None);
    }

    let mut items = Vec::with_capacity(manifest.items.len());
    for item in manifest.items {
        let Some(item) = item.into_batch_item() else {
            return Ok(None);
        };
        items.push(item);
    }
    Ok(Some(BatchReport::new(items)))
}

impl BatchManifestItemJson {
    fn into_batch_item(self) -> Option<BatchItem> {
        let status = match self.status.as_str() {
            "ok" => BatchItemStatus::Ok {
                source_format: self.source_format?,
                output_path: self.output_path?,
                metadata_path: self.metadata_path,
                output_size_bytes: self.output_size_bytes,
                metadata_size_bytes: self.metadata_size_bytes,
                node_count: self.node_count?,
                mesh_count: self.mesh_count?,
                primitive_count: self.primitive_count?,
                vertex_count: self.vertex_count?,
                triangle_count: self.triangle_count?,
            },
            "reused" => BatchItemStatus::Reused {
                source_format: self.source_format?,
                output_path: self.output_path?,
                metadata_path: self.metadata_path,
                output_size_bytes: self.output_size_bytes,
                metadata_size_bytes: self.metadata_size_bytes,
                node_count: self.node_count?,
                mesh_count: self.mesh_count?,
                primitive_count: self.primitive_count?,
                vertex_count: self.vertex_count?,
                triangle_count: self.triangle_count?,
            },
            "checked" => BatchItemStatus::Checked {
                source_format: self.source_format?,
                node_count: self.node_count?,
                mesh_count: self.mesh_count?,
                primitive_count: self.primitive_count?,
                vertex_count: self.vertex_count?,
                triangle_count: self.triangle_count?,
            },
            _ => return None,
        };

        Some(BatchItem {
            index: self.index,
            input_path: self.input_path,
            input_size_bytes: self.input_size_bytes,
            duration_ms: self.duration_ms,
            status,
        })
    }
}

fn batch_report_mismatch_reason(
    report: &BatchReport,
    check_only: bool,
    settings: &JobConversionSettings,
) -> Option<AssetPackageFreshnessReason> {
    if report.input_count() == 0 || report.failed_count() != 0 {
        return Some(AssetPackageFreshnessReason::ManifestMismatch);
    }
    if check_only {
        return (report.checked_count() != report.input_count())
            .then_some(AssetPackageFreshnessReason::ManifestMismatch);
    }

    if report.converted_count() + report.reused_count() != report.input_count() {
        return Some(AssetPackageFreshnessReason::ManifestMismatch);
    }

    for item in &report.items {
        let output_artifacts_exist = match &item.status {
            BatchItemStatus::Ok {
                output_path,
                metadata_path,
                ..
            }
            | BatchItemStatus::Reused {
                output_path,
                metadata_path,
                ..
            } => {
                Path::new(output_path).is_file()
                    && metadata_path
                        .as_ref()
                        .is_none_or(|path| Path::new(path).is_file())
                    && (!settings.write_metadata
                        || metadata_path
                            .as_ref()
                            .is_some_and(|path| Path::new(path).is_file()))
            }
            BatchItemStatus::Checked { .. } | BatchItemStatus::Error { .. } => {
                return Some(AssetPackageFreshnessReason::ManifestMismatch);
            }
        };
        if !output_artifacts_exist {
            return Some(AssetPackageFreshnessReason::OutputArtifactMissing);
        }
    }

    None
}

fn batch_report_audit_mismatch_reason(report: &BatchReport) -> Option<AssetPackageFreshnessReason> {
    if report.input_count() == 0 || report.failed_count() != 0 {
        return Some(AssetPackageFreshnessReason::ManifestMismatch);
    }
    if report.checked_count() == report.input_count() {
        return None;
    }
    if report.converted_count() + report.reused_count() != report.input_count() {
        return Some(AssetPackageFreshnessReason::ManifestMismatch);
    }

    for item in &report.items {
        match &item.status {
            BatchItemStatus::Ok {
                output_path,
                metadata_path,
                ..
            }
            | BatchItemStatus::Reused {
                output_path,
                metadata_path,
                ..
            } => {
                if !Path::new(output_path).is_file()
                    || metadata_path
                        .as_ref()
                        .is_some_and(|path| !Path::new(path).is_file())
                {
                    return Some(AssetPackageFreshnessReason::OutputArtifactMissing);
                }
            }
            BatchItemStatus::Checked { .. } | BatchItemStatus::Error { .. } => {
                return Some(AssetPackageFreshnessReason::ManifestMismatch);
            }
        }
    }

    None
}

fn read_json<T>(path: &Path) -> Result<T, AssetConversionError>
where
    T: for<'de> Deserialize<'de>,
{
    let bytes = fs::read(path).map_err(|source| AssetConversionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| AssetConversionError::Json {
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

fn unix_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0)
}
