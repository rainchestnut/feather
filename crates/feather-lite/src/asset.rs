//! Business-facing asset conversion facade built on the core pipeline.

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

/// Successful single-source conversion result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetConversionResult {
    pub package: AssetOutputPackage,
    pub asset_id: String,
    pub source_sha256: String,
    pub source_size_bytes: u64,
    pub settings_fingerprint: String,
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
    pub report: BatchReport,
}

/// How `ensure_asset_package` satisfied a single-source asset package request.
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

/// Result returned by `ensure_asset_package`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPackageEnsureResult {
    pub status: AssetPackageStatus,
    pub asset: AssetConversionResult,
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
    let source = source_identity(&request.input_path)?;
    let identity = identity_from_parts(
        std::slice::from_ref(&source),
        request.profile.label(),
        &settings,
    );
    write_source_info(
        &package,
        request.profile.label(),
        std::slice::from_ref(&source),
        &identity,
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
            let result =
                AssetConversionResult::from_summary(package.clone(), identity.clone(), summary);
            write_diagnostics(
                &package,
                request.profile.label(),
                "succeeded",
                &identity,
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
                &identity,
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
    let input_identities = source_identities(&request.input_paths)?;
    let identity = identity_from_parts(&input_identities, request.profile.label(), &settings);
    write_source_info(
        &package,
        request.profile.label(),
        &input_identities,
        &identity,
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
                asset_id: identity.asset_id.clone(),
                source_sha256: identity.source_sha256.clone(),
                source_size_bytes: identity.source_size_bytes,
                settings_fingerprint: identity.settings_fingerprint.clone(),
                report: report.report,
            };
            let failed_count = result.report.failed_count();
            if failed_count == 0 {
                write_batch_diagnostics(
                    &package,
                    request.profile.label(),
                    "succeeded",
                    &result,
                    &identity,
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
                    &identity,
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
                Some(&identity),
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

/// Returns true when an existing single-file asset package matches this request.
pub fn is_asset_package_current(
    request: &AssetConversionRequest,
) -> Result<bool, AssetConversionError> {
    Ok(current_asset_result(request)?.is_some())
}

fn current_asset_result(
    request: &AssetConversionRequest,
) -> Result<Option<AssetConversionResult>, AssetConversionError> {
    let package = single_asset_package(&request.output_dir);
    if !package.source_info_path.is_file() || !package.diagnostics_path.is_file() {
        return Ok(None);
    }
    if !package
        .model_path
        .as_ref()
        .expect("single asset package reserves model path")
        .is_file()
    {
        return Ok(None);
    }
    if !package
        .metadata_path
        .as_ref()
        .expect("single asset package reserves metadata path")
        .is_file()
    {
        return Ok(None);
    }

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
    );
    let source_info: AssetSourceInfoJson = read_json(&package.source_info_path)?;
    let diagnostics: AssetDiagnosticsJson = read_json(&package.diagnostics_path)?;

    if !single_asset_metadata_matches(
        &source_info,
        &diagnostics,
        &source,
        &identity,
        request.profile.label(),
    ) {
        return Ok(None);
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
        return Ok(None);
    };

    Ok(Some(AssetConversionResult {
        package,
        asset_id: identity.asset_id,
        source_sha256: identity.source_sha256,
        source_size_bytes: identity.source_size_bytes,
        settings_fingerprint: identity.settings_fingerprint,
        source_format,
        node_count,
        mesh_count,
        primitive_count,
        vertex_count,
        triangle_count,
    }))
}

fn single_asset_metadata_matches(
    source_info: &AssetSourceInfoJson,
    diagnostics: &AssetDiagnosticsJson,
    source: &SourceIdentity,
    identity: &AssetIdentity,
    profile: &str,
) -> bool {
    let input_matches = source_info.inputs.len() == 1
        && source_info.inputs[0].path.as_path() == source.path.as_path()
        && source_info.inputs[0].source_sha256 == source.source_sha256
        && source_info.inputs[0].source_size_bytes == source.source_size_bytes;

    source_info.contract_version == ASSET_PACKAGE_CONTRACT_VERSION
        && source_info.kind == "conversion"
        && source_info.profile == profile
        && source_info.asset_id == identity.asset_id
        && source_info.source_sha256 == identity.source_sha256
        && source_info.source_size_bytes == identity.source_size_bytes
        && source_info.settings_fingerprint == identity.settings_fingerprint
        && input_matches
        && diagnostics.contract_version == ASSET_PACKAGE_CONTRACT_VERSION
        && diagnostics.status == "succeeded"
        && diagnostics.profile == profile
        && diagnostics.asset_id == identity.asset_id
        && diagnostics.source_sha256 == identity.source_sha256
        && diagnostics.source_size_bytes == identity.source_size_bytes
        && diagnostics.settings_fingerprint == identity.settings_fingerprint
}

impl AssetConversionResult {
    fn from_summary(
        package: AssetOutputPackage,
        identity: AssetIdentity,
        summary: ConversionSummary,
    ) -> Self {
        Self {
            package,
            asset_id: identity.asset_id,
            source_sha256: identity.source_sha256,
            source_size_bytes: identity.source_size_bytes,
            settings_fingerprint: identity.settings_fingerprint,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceIdentity {
    path: PathBuf,
    source_sha256: String,
    source_size_bytes: u64,
}

fn identity_from_parts(
    sources: &[SourceIdentity],
    profile: &str,
    settings: &JobConversionSettings,
) -> AssetIdentity {
    let source_sha256 = aggregate_source_sha256(sources);
    let source_size_bytes = sources.iter().map(|source| source.source_size_bytes).sum();
    let settings_fingerprint = conversion_settings_fingerprint(profile, settings);
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

fn conversion_settings_fingerprint(profile: &str, settings: &JobConversionSettings) -> String {
    let mut hasher = Sha256::new();
    hasher.update(ASSET_PACKAGE_CONTRACT_VERSION.as_bytes());
    hasher.update(b"\0profile\0");
    hasher.update(profile.as_bytes());
    hasher.update(b"\0settings\0");
    let settings_json =
        serde_json::to_vec(settings).expect("asset conversion settings should serialize");
    hasher.update(settings_json);
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
    failure: Option<AssetFailure>,
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
        failure: failure.cloned(),
        updated_at_unix_ms: unix_timestamp_millis(),
    };
    write_json(&package.diagnostics_path, &diagnostics)
}

#[derive(Serialize)]
struct BatchAssetDiagnosticsJson<'a> {
    contract_version: &'static str,
    status: &'a str,
    profile: &'a str,
    asset_id: &'a str,
    source_sha256: &'a str,
    source_size_bytes: u64,
    settings_fingerprint: &'a str,
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
    identity: &AssetIdentity,
    failure: Option<&AssetFailure>,
) -> Result<(), AssetConversionError> {
    let diagnostics = BatchAssetDiagnosticsJson {
        contract_version: ASSET_PACKAGE_CONTRACT_VERSION,
        status,
        profile,
        asset_id: &identity.asset_id,
        source_sha256: &identity.source_sha256,
        source_size_bytes: identity.source_size_bytes,
        settings_fingerprint: &identity.settings_fingerprint,
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
    identity: Option<&AssetIdentity>,
    failure: &AssetFailure,
) -> Result<(), AssetConversionError> {
    let diagnostics = BatchAssetDiagnosticsJson {
        contract_version: ASSET_PACKAGE_CONTRACT_VERSION,
        status,
        profile,
        asset_id: identity
            .map(|identity| identity.asset_id.as_str())
            .unwrap_or(""),
        source_sha256: identity
            .map(|identity| identity.source_sha256.as_str())
            .unwrap_or(""),
        source_size_bytes: identity
            .map(|identity| identity.source_size_bytes)
            .unwrap_or(0),
        settings_fingerprint: identity
            .map(|identity| identity.settings_fingerprint.as_str())
            .unwrap_or(""),
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
