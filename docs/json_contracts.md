# Feather Lite JSON Contracts

This document defines the stable JSON contracts emitted by the public
Feather Lite API and CLI surfaces. Each contract has a top-level
`contract_version` string. Consumers should reject unknown major contract
families and treat additional fields as append-only compatible additions.

## Versioning Rules

- A contract version is an opaque string, not a numeric counter.
- A `*.v1` contract may add optional or append-only fields without changing
  the version.
- Removing a field, changing a field type, changing the meaning of a stable
  enum value, or moving a field requires a new version.
- Contract constants are exported by `feather_lite` so API consumers can
  compare generated JSON against the same source used by the crate.

## `feather.format-capabilities.v1`

Emitted by:

- `format_capabilities_json`
- `feather formats --json`

Required top-level fields:

- `contract_version`: always `feather.format-capabilities.v1`
- `formats`: array of format capability objects

Required capability fields:

- `format`: stable source format label, such as `CATIA_CATPart`, `NX_PRT`,
  `STEP`, or `FeatherLiteCache`
- `extensions`: array of supported file extensions
- `status`: `available`, `partial`, or `planned`
- `available`: boolean
- `requires_visual_payload`: boolean
- `supports_embedded_assets`: boolean
- `supports_external_references`: boolean
- `supports_native_tessellation`: boolean
- `native_brep_tessellation`: `ready`, `partial`, `pending`, `not_decoded`, or
  `not_applicable`
- `conversion_path`: human-readable conversion pipeline
- `limitation`: human-readable limitation or empty string

## `feather.inspect-report.v1`

Emitted by:

- `InspectReport::to_json_string`
- `feather inspect --json`

Required top-level fields:

- `contract_version`: always `feather.inspect-report.v1`
- `path`: inspected path, or `-` for byte-based inspection without a path
- `format`: detected source format label
- `confidence`: probe confidence label
- `embedded_cache`: boolean
- `reason`: probe reason
- `container_kind`: detected container family or `null`; CATIA V5 CFV2 emits
  `catia-v5-cfv2`
- `source_version`: detected source release or `null`
- `native_visualization`: detected native visual representation or `null`;
  native CATIA V5 CGR containers emit `catia-native-cgr-container`
- `coarse_format`: format label or `null`
- `capability`: capability object or `null`
- `import_check`: import validation object or `null`
- `visual_asset_count`: integer
- `visual_assets`: array

Required `import_check` fields when present:

- `importable`: boolean
- `failure_stage`: stage string or `null`
- `failure_category`: stable category string or `null`
- `required_condition`: actionable condition string or `null`
- `error`: error message or `null`
- `node_count`: integer or `null`
- `mesh_count`: integer or `null`
- `primitive_count`: integer or `null`
- `vertex_count`: integer or `null`
- `triangle_count`: integer or `null`

## `feather.batch-manifest.v1`

Emitted by:

- `BatchReport::to_manifest_json`
- `feather batch`

Required top-level fields:

- `contract_version`: always `feather.batch-manifest.v1`
- `input_count`: integer
- `success_count`: integer
- `converted_count`: integer
- `reused_count`: integer
- `checked_count`: integer
- `failed_count`: integer
- `summary`: aggregate manifest summary object
- `items`: array of batch item objects

Required aggregate quality fields in `summary`:

- `total_node_count`: integer across successful converted, reused, and checked
  items
- `total_mesh_count`: integer across successful converted, reused, and checked
  items
- `total_primitive_count`: integer across successful converted, reused, and
  checked items
- `total_vertex_count`: integer across successful converted, reused, and checked
  items
- `total_triangle_count`: integer across successful converted, reused, and
  checked items

Stable item statuses:

- `ok`: conversion succeeded in this run and output paths/sizes describe written
  artifacts
- `reused`: an existing output artifact was reused for an unchanged input
- `checked`: check-only import validation succeeded without writing GLB output
- `error`: conversion or validation failed for this item

Every item includes an append-only `operation` field. Successful operations are
`converted`, `reused`, and `checked`; failed items use `error`.

Successful `ok`, `reused`, and `checked` items include quality counts. For `ok`
and `reused` items these counts come from validated GLB output; for `checked`
items they come from the imported IR used by check-only validation, before
export-only mesh cleanup.

- `node_count`: integer
- `mesh_count`: integer
- `primitive_count`: integer
- `vertex_count`: integer
- `triangle_count`: integer

Per-format summary entries include the same successful quality counts as
`node_count`, `mesh_count`, `primitive_count`, `vertex_count`, and
`triangle_count`.

Required diagnostic fields for failed items:

- `error_stage`: stable stage such as `input`, `import`, `export`, or `io`
- `error_category`: stable category listed below
- `required_condition`: actionable condition string or `null`
- `error`: error message

Stable `error_category` values:

- `missing_external_reference`
- `no_readable_lightweight_cache`
- `native_visualization_not_decoded`
- `resource_limit_exceeded`
- `tessellation_pending`
- `unsupported_input`
- `invalid_source_data`
- `missing_data`
- `io`
- `export`
- `other`

`resource_limit_exceeded` covers source input, OLE stream, ZIP expansion, and
STEP curve-segment limits configured through `ImportLimits`.

Failed batch items may additionally include the append-only probe fields
`container_kind`, `source_version`, and `native_visualization`. These fields
carry the same values as the inspect contract and are `null` when unavailable.

## `feather.cache-dump-manifest.v1`

Emitted by:

- `CacheDumpReport::to_manifest_json`
- `feather dump-cache`

Required top-level fields:

- `contract_version`: always `feather.cache-dump-manifest.v1`
- `source_path`: dumped source path
- `asset_count`: integer
- `assets`: array of dumped visual asset objects

Required asset fields:

- `index`: integer
- `kind`: asset kind label, such as `feather-cache`
- `source`: asset discovery source label
- `byte_start`: integer
- `byte_end`: integer
- `entry_name`: archive/OLE entry name or `null`
- `file`: output file name

## `feather.job-record.v1`

Emitted by:

- `JobRecord::to_json_string`
- `feather job convert --json`
- `feather job batch --json`
- `feather job status --json`
- `feather job retry --json`

Required top-level fields:

- `contract_version`: always `feather.job-record.v1`
- `job_id`: stable local job identifier
- `status`: `queued`, `running`, `succeeded`, `failed`, or `cancelled`
- `stage`: `queued`, `running`, `import`, `export`, `io`, `batch`,
  `succeeded`, or `failed`
- `request`: persisted request object
- `artifacts`: reserved artifact paths
- `created_at_unix_ms`: integer
- `updated_at_unix_ms`: integer
- `started_at_unix_ms`: integer or `null`
- `finished_at_unix_ms`: integer or `null`
- `failure`: failure object or `null`
- `result`: result object or `null`

Request object variants:

- `{"kind":"convert", ...}` includes `input_path` and persisted conversion
  `settings`
- `{"kind":"batch", ...}` includes `input_paths`, `check_only`, and persisted
  conversion `settings`

Required artifact fields:

- `root_dir`: artifact package root
- `model_path`: single conversion GLB path or `null`
- `metadata_path`: single conversion metadata path or `null`
- `manifest_path`: batch manifest path or `null`
- `batch_output_dir`: batch output directory or `null`
- `source_info_path`: source package metadata path

When present, `failure` includes:

- `stage`: failed stage string
- `category`: stable category string
- `message`: human-readable failure message
- `retryable`: boolean

When present, `result.kind` is either:

- `conversion`: includes output path, optional metadata path, source format,
  node, mesh, primitive, vertex, and triangle counts
- `batch`: includes manifest path plus input, converted, checked, and failed
  counts

## `feather.asset-package.v1`

Emitted by:

- `convert_asset`
- `convert_batch_assets`
- `ensure_asset_package` when the package is stale and conversion runs
- `ensure_batch_asset_package` when the package is stale and batch conversion runs

Consumed by:

- `is_asset_package_current` and `is_batch_asset_package_current`
- `explain_asset_package_freshness` and
  `explain_batch_asset_package_freshness`

The explain APIs return `AssetPackageFreshness` instead of writing JSON. The
stable reason labels include `current`, `missing_source_info`,
`missing_diagnostics`, `missing_model`, `missing_metadata`, `missing_manifest`,
`missing_batch_output_directory`, `empty_batch_input_set`, `source_changed`,
`settings_changed`, `package_contract_mismatch`, `package_kind_mismatch`,
`source_info_mismatch`, `diagnostics_failed`, `diagnostics_mismatch`,
`manifest_mismatch`, `output_artifact_missing`, and
`incomplete_diagnostics`.

Required fields in `source-info.json`:

- `contract_version`: always `feather.asset-package.v1`
- `kind`: `conversion` or `batch_conversion`
- `profile`: profile label such as `mobile_preview`, `web_preview`,
  `standard_review`, `high_quality`, or `custom`
- `asset_id`: deterministic identifier derived from source hash and settings
- `source_sha256`: SHA-256 for one source, or aggregate SHA-256 for a batch
- `source_size_bytes`: source byte size, or total source byte size for a batch
- `settings_fingerprint`: SHA-256 of profile plus concrete conversion settings;
  batch packages also include conversion vs check-only mode
- `inputs`: array of source path, source SHA-256, and source byte size objects
- `created_at_unix_ms`: integer

Required fields in single conversion `diagnostics.json`:

- `contract_version`: always `feather.asset-package.v1`
- `status`: `succeeded` or `failed`
- `profile`: profile label
- `asset_id`: deterministic identifier derived from source hash and settings
- `source_sha256`: SHA-256 for the source
- `source_size_bytes`: source byte size
- `settings_fingerprint`: SHA-256 of profile plus concrete conversion settings
- `source_format`: converted format label or `null`
- `node_count`, `mesh_count`, `primitive_count`, `vertex_count`,
  `triangle_count`: quality counts or `null`
- `quality`: business quality object for succeeded conversions, or `null`
- `failure`: failure object or `null`
- `updated_at_unix_ms`: integer

Required fields in batch `diagnostics.json`:

- `contract_version`: always `feather.asset-package.v1`
- `status`: `succeeded` or `failed`
- `profile`: profile label
- `asset_id`: deterministic identifier derived from source hashes and settings
- `source_sha256`: aggregate source SHA-256
- `source_size_bytes`: total source byte size
- `settings_fingerprint`: SHA-256 of profile plus concrete conversion settings
  and conversion vs check-only mode
- `input_count`, `converted_count`, `reused_count`, `checked_count`,
  `failed_count`: integers
- `quality`: business quality object for succeeded batch conversions or checks,
  or `null`
- `failure`: failure object or `null`
- `updated_at_unix_ms`: integer

When present, `quality` includes:

- `previewable`: boolean
- `has_visual_geometry`: boolean
- `preview_status`: `ready`, `no_visual_geometry`, `no_preview_output`, or
  `partial_failure`
- `quality_level`: `empty`, `light`, `medium`, `heavy`, or `oversized`
- `input_count`, `successful_count`, `converted_count`, `reused_count`,
  `checked_count`, `failed_count`: integer counts
- `node_count`, `mesh_count`, `primitive_count`, `vertex_count`,
  `triangle_count`: aggregate geometry counts
- `input_size_bytes`, `output_size_bytes`, `metadata_size_bytes`: aggregate
  byte counts

When present, `failure` includes stable `stage`, `category`, `message`, and
`retryable` fields.
