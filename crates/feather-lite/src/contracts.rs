//! Public JSON contract version identifiers.
//!
//! These values are emitted by stable CLI/API JSON surfaces. Change the
//! corresponding version only when that JSON contract makes a breaking change.

/// Contract version emitted by `format_capabilities_json` and `feather formats --json`.
pub const FORMAT_CAPABILITIES_CONTRACT_VERSION: &str = "feather.format-capabilities.v1";

/// Contract version emitted by `InspectReport::to_json_string` and `feather inspect --json`.
pub const INSPECT_REPORT_CONTRACT_VERSION: &str = "feather.inspect-report.v1";

/// Contract version emitted by `BatchReport::to_manifest_json` and batch manifests.
pub const BATCH_MANIFEST_CONTRACT_VERSION: &str = "feather.batch-manifest.v1";

/// Contract version emitted by `CacheDumpReport::to_manifest_json` and dump-cache manifests.
pub const CACHE_DUMP_MANIFEST_CONTRACT_VERSION: &str = "feather.cache-dump-manifest.v1";

/// Contract version emitted by local conversion job records.
pub const JOB_RECORD_CONTRACT_VERSION: &str = "feather.job-record.v1";

/// Contract version emitted by business asset package metadata and diagnostics.
pub const ASSET_PACKAGE_CONTRACT_VERSION: &str = "feather.asset-package.v1";
