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
- `mesh_count`: integer or `null`
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
- `checked_count`: integer
- `failed_count`: integer
- `summary`: aggregate manifest summary object
- `items`: array of batch item objects

Stable item statuses:

- `ok`: conversion succeeded and output paths/sizes describe written artifacts
- `checked`: check-only import validation succeeded without writing GLB output
- `error`: conversion or validation failed for this item

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
