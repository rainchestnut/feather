//! Lightweight CAD conversion core.
//!
//! This crate owns the stable Feather Lite IR, importer boundary, mesh
//! preparation pipeline, and GLB export path. Format-specific modules are kept
//! behind the importer trait so private CAD support can grow without leaking
//! proprietary container details into the rest of the project.
//!
//! Public APIs are intentionally re-exported from the crate root. Internal
//! modules stay private so embedders depend on product-level contracts such as
//! `convert_path_to_glb`, `inspect_path`, `run_batch_conversion`, and
//! `dump_embedded_visual_assets` rather than on parser layout.
//!
//! ```no_run
//! use std::path::Path;
//!
//! use feather_lite::{ConversionOptions, convert_path_to_glb};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let summary = convert_path_to_glb(
//!     Path::new("model.CATPart"),
//!     Path::new("model.glb"),
//!     &ConversionOptions::default(),
//! )?;
//! println!("{} triangles", summary.triangle_count);
//! # Ok(())
//! # }
//! ```

mod assembly;
mod asset;
mod assets;
mod atomic_write;
mod batch;
mod cache;
mod cache_dump;
mod capabilities;
mod contracts;
mod diagnostics;
mod document;
mod export;
mod importer;
mod importers;
mod inspect;
mod jobs;
mod mesh;
mod pipeline;
mod probe;

mod json;

pub use asset::{
    AssetBusinessState, AssetBusinessStatus, AssetConversionError, AssetConversionProfile,
    AssetConversionRequest, AssetConversionResult, AssetFailure, AssetFailureAction, AssetIdentity,
    AssetOutputPackage, AssetPackageAudit, AssetPackageBounds, AssetPackageEnsureResult,
    AssetPackageFreshness, AssetPackageFreshnessReason, AssetPackageMetadataSummary,
    AssetPackageStatus, AssetPackageSummary, AssetPackageSummaryItem, AssetPackageSummaryOperation,
    AssetPreflightDecision, AssetPreflightRequest, AssetPreflightResult, AssetPreviewStatus,
    AssetQualityLevel, AssetQualityReport, BatchAssetConversionRequest, BatchAssetConversionResult,
    BatchAssetPackageEnsureResult, BatchAssetPreflightItem, BatchAssetPreflightResult,
    asset_conversion_identity, batch_asset_conversion_identity, convert_asset,
    convert_batch_assets, ensure_asset_package, ensure_batch_asset_package,
    explain_asset_package_freshness, explain_batch_asset_package_freshness, inspect_asset_package,
    is_asset_package_current, is_batch_asset_package_current, load_current_asset_package,
    load_current_batch_asset_package, preflight_asset, preflight_batch_assets,
    read_asset_package_summary,
};
pub use assets::embedded::{
    EmbeddedVisualAsset, EmbeddedVisualAssetKind, EmbeddedVisualAssetSource,
    discover_embedded_visual_assets, discover_embedded_visual_assets_with_limits,
};
pub use assets::three_dxml_rep::import_3dxml_rep_document;
pub use batch::{
    BATCH_HEADER_PROBE_BYTES, BatchCheckError, BatchConversionError, BatchConversionOptions,
    BatchConversionReport, BatchCountSummary, BatchFormatSummary, BatchInputCollectionError,
    BatchInputDiagnostic, BatchItem, BatchItemStatus, BatchManifestSummary, BatchReport,
    batch_input_diagnostic, batch_output_file_name, collect_batch_input_paths,
    conversion_error_stage, is_supported_batch_candidate, run_batch_conversion,
    validate_batch_input_path,
};
pub use cache_dump::{
    CacheDumpError, CacheDumpReport, DumpedVisualAsset, dump_embedded_visual_assets,
    dump_embedded_visual_assets_with_limits,
};
pub use capabilities::{
    FormatCapability, format_capabilities, format_capabilities_json, format_capability,
};
pub use contracts::{
    ASSET_PACKAGE_CONTRACT_VERSION, BATCH_MANIFEST_CONTRACT_VERSION,
    CACHE_DUMP_MANIFEST_CONTRACT_VERSION, FORMAT_CAPABILITIES_CONTRACT_VERSION,
    INSPECT_REPORT_CONTRACT_VERSION, JOB_RECORD_CONTRACT_VERSION,
};
pub use diagnostics::batch_failure_category;
pub use document::{
    Aabb, LiteDocument, LiteMaterial, LiteMesh, LiteMetadata, LiteNode, LitePrimitive,
    LiteSceneSummary, LiteSourceUnit, Transform, identity_transform,
};
pub use export::glb::{
    ExportError, GlbExportOptions, GlbValidationSummary, export_glb, validate_glb_payload,
};
pub use export::metadata::export_metadata_json;
pub use importer::{
    CadLiteImporter, ImportError, ImportLimits, ImportOptions, ImporterRegistry, InputFile,
    ReferencePathMapping,
};
pub use inspect::{
    ImportValidationSummary, InspectError, InspectImportCheck, InspectOptions, InspectReport,
    inspect_bytes, inspect_path, validate_imported_input,
};
pub use jobs::{
    JobArtifactPaths, JobConversionSettings, JobError, JobFailure, JobImportLimits, JobRecord,
    JobReferencePathMapping, JobRequest, JobResult, JobSourceInfo, JobSourceInputInfo, JobStage,
    JobStatus, LocalJobStore,
};
pub use mesh::clean::{MeshOptions, optimize_document};
pub use mesh::validate::validate_document;
pub use pipeline::{ConversionError, ConversionOptions, ConversionSummary, convert_path_to_glb};
pub use probe::{
    FileFormat, ProbeConfidence, ProbeResult, detect_format, has_supported_source_extension,
};
