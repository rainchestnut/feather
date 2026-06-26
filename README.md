# Feather

Feather is a Rust-first lightweight CAD conversion pipeline. It targets visual
preview, web delivery, and GLB export rather than exact CAD editing.

## Project Shape

The workspace contains two deliverables:

- `crates/feather-lite`: the reusable core library. It owns probing, import,
  lightweight IR validation, mesh preparation, GLB export, inspect reports,
  business asset packages, batch manifests, local conversion job records, and
  embedded cache dumping.
- `crates/feather-cli`: the `feather` executable. It is intentionally a thin
  command-line shell around `feather-lite`, responsible for argument parsing,
  status output, and process exit codes.

Feather is open-source-first. It does not include commercial CAD SDK adapters
or placeholder abstraction layers for proprietary adapter backends. Private CAD
support is limited to readable lightweight payloads, explicit external
reference loading, and native open-source tessellation paths already represented
in `feather-lite`.

## Scope

The supported conversion path is:

```text
CATPart / CATProduct / CGR / 3DXML / NX PRT / SLDPRT / SLDASM / JT / ACIS / IGES / PRIVATE_CAD / Feather Lite cache / STEP / STL / OBJ / GLB
        -> visualization cache, standalone Feather Lite cache, cache-declared external references, OLE regular/mini streams, embedded STL/OBJ/glTF(BIN/data URI)/GLB, 3DXML or generic ZIP XML assembly manifests, stored/deflated ZIP entries with ZIP64 metadata, or native open-source tessellation
        -> Feather Lite IR
        -> mesh cleaning / coordinate quantization / triangle-budget LOD
        -> GLB + metadata.json
```

Private CAD files are handled cache-first. If CATIA, NX, or SolidWorks data does
not expose a readable visualization cache, embedded STL/OBJ/glTF(BIN/data URI)/GLB asset, ZIP XML
assembly manifest, or stored ZIP entry, conversion fails explicitly instead of
pretending to recover geometry.
Native CATIA V5 `V5_CFV2` containers are identified structurally. Inspection
reports the embedded CATIA release and detects native `CATCGRCont`
visualization objects, but the proprietary binary CGR representation is not
decoded. Consequently, CATPart, CATProduct, and CGR capabilities are published
as `partial`: files with supported open/lightweight payloads convert, while a
native-only CFV2 file fails with the machine-readable
`native_visualization_not_decoded` category.
STEP AP242 tessellated faces and presentation colors are parsed natively.
AP203/AP214/AP242 `ADVANCED_FACE` B-Rep with `EDGE_LOOP`/`POLY_LOOP`,
adaptive `LINE`/`CIRCLE`/`ELLIPSE` boundaries (including single-edge closed
conics), `PLANE`/`CYLINDRICAL_SURFACE`/`CONICAL_SURFACE`/`SPHERICAL_SURFACE` and regular ring
`TOROIDAL_SURFACE`, closed shells, concave polygons, face orientation, and
presentation colors is tessellated through the built-in open-source path.
`TRIMMED_CURVE` spans over `LINE`/`CIRCLE`/`ELLIPSE` basis curves are accepted
with parameter trims. Planar faces also accept complete rational or
non-rational `B_SPLINE_CURVE_WITH_KNOTS` edge boundaries and parameter
`TRIMMED_CURVE` spans over B-Spline basis, sampled with bounded de Boor
evaluation and the same constrained triangulation path.
Toroidal faces accept meridian and parallel circular boundaries and unwrap both
periodic parameters. Faces may contain multiple validated `FACE_BOUND` inner
loops; planar and analytic-surface holes use the same constrained open-source
triangulation path, including periodic parameter alignment. Self-intersecting,
intersecting, touching, outside, overlapping, or nested loops are rejected.
Explicit SI and conversion-based length and plane-angle units (including
millimetres, inches, radians, and degrees) are resolved structurally;
coordinates are normalized to metres for GLB. Cartesian-only trimmed curves,
trimmed curves over unsupported bases, B-spline boundaries on non-planar
surfaces, other curve families, spline surfaces, horn/spindle tori,
non-meridian/non-parallel torus circles, cone faces reaching the apex, sphere
faces touching parameterization poles, and non-rigid or
non-`ITEM_DEFINED_TRANSFORMATION` assembly transforms remain explicitly
unsupported. AP214/AP242 shape-representation assembly
relationships are preserved as reusable mesh instances with hierarchy and
rigid transforms; product and occurrence names are recovered when their
definition chains are present.
When a private container exposes multiple stored or deflated ZIP visual assets, Feather
uses a ZIP XML assembly manifest when present; 3DXML `ProductStructure` ID
relationships are resolved across `Reference3D`, `Instance3D`, `ReferenceRep`,
and `InstanceRep`, manifest references such as `urn:3DXML:preview/part.glb`
are resolved to ZIP entries, and `RelativeMatrix` text transforms are
preserved. Readable XML `.3DRep` polygonal representations with
`VertexBuffer` positions/normals and `Face` triangles/strips/fans are imported;
binary/encrypted 3DRep streams and surface 3DRep data remain unsupported.
ZIP XML manifest references to external supported CAD/mesh/lightweight files
are preserved as assembly reference nodes and can be loaded through
`--resolve-dir` or `--map-root`.
Otherwise, it merges assets into one scene with one grouping node per entry.
ZIP entries may use central directory sizes, data descriptors, or ZIP64 size
metadata.
ZIP-packaged glTF previews can reference sibling `.bin` buffers or base64 data
URI buffers, preserve glTF/GLB node matrix or TRS transforms, and read
accessor/bufferView offsets plus interleaved vertex strides.
When a cache-backed assembly declares external part references, `--resolve-dir`
can load those referenced lightweight files and attach them under the cached
assembly nodes.
Reference resolution accepts native paths, Windows-style backslash paths, and
basename fallback through `--resolve-dir` for archived assemblies whose original
absolute paths no longer exist. Use
`--map-root <old-prefix>=<new-root>` when a PDM/Vault package has been moved and
assembly references still contain legacy absolute path prefixes.
When a private container uses OLE/Compound File Binary storage, Feather can
reconstruct regular and mini streams, then scan those streams for the same
lightweight payloads.
OBJ payloads preserve `usemtl` groups, and ZIP-packaged OBJ previews can apply
simple MTL colors from sibling `.mtl` entries.
Conversion can apply deterministic coordinate quantization through
`--quantize <grid-step>` and triangle-budget LOD through `--max-triangles` or
the coarse `--lod low|medium|high|none` presets for large visual previews.
STEP curve tessellation accepts `--chord-error <source-unit-value>`; zero in
the core API uses a curve-size-relative default. `--max-step-curve-segments`
bounds work for each curve edge, and the B-spline path is additionally bounded
by `--max-step-spline-degree` and `--max-step-spline-control-points`. Limit
failures are explicit instead of silently reducing requested precision.
GLB export automatically writes 16-bit index buffers when the mesh index range
allows it, while retaining 32-bit indices for larger assemblies. Use
`--no-normals` when a downstream preview pipeline can rebuild or ignore normals
and the smallest GLB payload is preferred.

`PRIVATE_CAD` is a vendor-neutral fallback for common private extensions such as
`.prt`, `.asm`, `.ipt`, `.iam`, `.par`, `.psm`, `.x_t`, `.x_b`, `.jt`,
`.sat`, `.sab`, `.igs`, `.iges`, `.neu`, `.model`, `.session`, `.exp`, and
`.dlv`.
It only extracts readable lightweight payloads; it does not claim
native support for each vendor's proprietary B-Rep or assembly semantics.

## Feather Lite Cache

The current private-format ingestion contract is an embedded or standalone
Feather Lite cache payload:

```text
FEATHER_CAD_LITE_CACHE_V1
material Default 0.7 0.7 0.72 1.0
mesh Part
primitive 0
v 0 0 0
v 1 0 0
v 0 1 0
tri 0 1 2
endprimitive
endmesh
node Part 0 root
END_FEATHER_CAD_LITE_CACHE
```

Cache tokens that contain spaces can be quoted, for example
`reference "Part A Instance" "parts/Part A.CATPart" root`.

## Library API

Applications should use the `feather_lite` crate root APIs. Internal parser
modules are not part of the public contract; the root exports are the stable
integration surface for production code.

```rust
use std::path::{Path, PathBuf};

use feather_lite::{
    AssetConversionProfile, AssetConversionRequest, BatchAssetConversionRequest,
    BatchConversionOptions, ConversionOptions, InspectOptions, JobConversionSettings,
    LocalJobStore, ReferencePathMapping, asset_conversion_identity,
    batch_asset_conversion_identity, convert_path_to_glb, ensure_asset_package,
    ensure_batch_asset_package, explain_asset_package_freshness,
    explain_batch_asset_package_freshness, inspect_asset_package, inspect_path,
    is_asset_package_current, is_batch_asset_package_current, load_current_asset_package,
    load_current_batch_asset_package, preflight_batch_assets, read_asset_package_summary,
    run_batch_conversion,
};

let inspect = inspect_path(
    Path::new("assembly.CATProduct"),
    &InspectOptions {
        check_import: true,
        ..InspectOptions::default()
    },
)?;
println!("{} assets", inspect.visual_assets.len());

let mut conversion = ConversionOptions::default();
conversion.import.resolve_dirs.push(PathBuf::from("./parts"));
conversion.import.limits.max_archive_total_uncompressed_bytes = 2 * 1024 * 1024 * 1024;
conversion
    .import
    .reference_path_mappings
    .push(ReferencePathMapping::new(r"C:\vault\old", "./released"));

let summary = convert_path_to_glb(
    Path::new("assembly.CATProduct"),
    Path::new("assembly.glb"),
    &conversion,
)?;
println!("{} triangles", summary.triangle_count);

let mut asset_request = AssetConversionRequest::new(
    PathBuf::from("assembly.CATProduct"),
    PathBuf::from("./asset-package"),
);
asset_request.profile = AssetConversionProfile::StandardReview;
let planned_asset = asset_conversion_identity(&asset_request)?;
println!("{}", planned_asset.asset_id);
let asset = ensure_asset_package(&asset_request)?;
println!("{}", asset.status.as_str());
println!(
    "{}",
    asset
        .asset
        .package
        .model_path
        .as_ref()
        .expect("asset packages reserve model paths")
        .display()
);
println!("{}", is_asset_package_current(&asset_request)?);
if let Some(current_asset) = load_current_asset_package(&asset_request)? {
    println!("{}", current_asset.asset_id);
}
println!("{}", asset.asset.quality.preview_status.as_str());
let freshness = explain_asset_package_freshness(&asset_request)?;
println!("{}", freshness.reason.as_str());
let package_audit = inspect_asset_package("./asset-package")?;
println!("{}", package_audit.usable);
let package_summary = read_asset_package_summary("./asset-package")?;
println!("{}", package_summary.items.len());

let batch_asset_request = BatchAssetConversionRequest::new(
    vec![PathBuf::from("./incoming-cad")],
    PathBuf::from("./batch-asset-package"),
);
let batch_preflight = preflight_batch_assets(&batch_asset_request)?;
println!("{}", batch_preflight.decision.as_str());
let planned_batch_asset = batch_asset_conversion_identity(&batch_asset_request)?;
println!("{}", planned_batch_asset.settings_fingerprint);
let batch_asset = ensure_batch_asset_package(&batch_asset_request)?;
println!("{}", batch_asset.status.as_str());
println!("{}", is_batch_asset_package_current(&batch_asset_request)?);
if let Some(current_batch_asset) = load_current_batch_asset_package(&batch_asset_request)? {
    println!("{}", current_batch_asset.asset_id);
}
println!("{}", batch_asset.asset.quality.quality_level.as_str());
let batch_freshness = explain_batch_asset_package_freshness(&batch_asset_request)?;
println!("{}", batch_freshness.reason.as_str());

let batch = run_batch_conversion(
    &[PathBuf::from("./incoming-cad")],
    &BatchConversionOptions {
        output_dir: PathBuf::from("./converted-glb"),
        manifest_path: None,
        check_only: false,
        conversion,
    },
)?;
println!("{} converted", batch.report.converted_count());

let store = LocalJobStore::new(".feather-jobs");
let job = store.create_conversion_job(
    PathBuf::from("assembly.CATProduct"),
    JobConversionSettings::default(),
)?;
let job = store.run_job(&job.job_id)?;
println!("job {} {}", job.job_id, job.status.as_str());
```

The main embeddable operations are:

- `ensure_asset_package`, `ensure_batch_asset_package`, `convert_asset`,
  `convert_batch_assets`, `preflight_asset`, and `preflight_batch_assets`:
  business facade APIs that write
  or reuse a standard artifact package and hide low-level mesh options behind
  `AssetConversionProfile`; conversion results include `asset_id`, source
  SHA-256, source byte size, settings fingerprint, and an `AssetQualityReport`
  with preview readiness, geometry size class, aggregate counts, and artifact
  sizes. `preflight_asset` performs a real import check without writing
  artifacts and returns a stable `AssetPreflightDecision`, optional
  `required_condition`, geometry counts, and preflight quality.
  `preflight_batch_assets` applies the same decision/action contract to a
  discovered batch input set without writing a package. Failed business
  conversions return `AssetFailure`, whose `decision()` and `action()` methods
  expose stable routing labels for recovery workflows.
- `asset_conversion_identity` and `batch_asset_conversion_identity`: compute the
  same stable `asset_id`, source hash, source byte size, and settings
  fingerprint that conversion and reuse checks will use, without running mesh
  export.
- `is_asset_package_current`, `is_batch_asset_package_current`,
  `explain_asset_package_freshness`, and
  `explain_batch_asset_package_freshness`: validate that business asset
  packages still match the current source hash, source size, conversion profile,
  and batch mode; the explain APIs return stable reason codes for stale
  packages.
- `load_current_asset_package` and `load_current_batch_asset_package`: read and
  validate an existing package without triggering conversion, returning `None`
  when the package is missing, failed, incomplete, or stale.
- `inspect_asset_package`: audits an existing single or batch asset package
  directory without reading source CAD files, returning package kind, identity,
  quality or failure details, and the first internal completeness reason.
- `read_asset_package_summary`: reads the usable package output list without
  manual JSON parsing, including GLB paths, metadata sidecars, per-item geometry
  counts, sidecar metadata summary, and aggregate output sizes.
- `detect_format` and `inspect_path`: probe, asset discovery, and optional real
  import validation.
- `convert_path_to_glb`: single-file conversion with mesh cleanup, GLB
  validation, and optional sidecar metadata.
- `run_batch_conversion`: recursive file/directory batch conversion or
  check-only preflight with a manifest.
- `LocalJobStore`: file-backed business job records for queued, running,
  succeeded, failed, and retried conversion or batch jobs.
- `dump_embedded_visual_assets`: extraction diagnostics for readable preview
  payloads inside private containers.
- `format_capabilities` and `format_capabilities_json`: the machine-readable
  support matrix used by `feather formats` and `feather formats --json`.

## Error and Manifest Contracts

Conversion errors are separated by stage: import errors mean the source could
not produce a valid Feather Lite IR, export errors mean GLB generation or GLB
validation failed, and I/O errors mean filesystem reads or writes failed. Batch
manifests preserve those stages per item and additionally publish stable
`error_category` values:

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

The manifest is intentionally append-safe for operations teams: each item has
input path, input size, duration, status, detected format data when available,
output paths and sizes for successful conversions, and stage/category/message
for failures. A failed item does not stop later items from running, but the CLI
returns a non-zero exit code after writing the manifest when any item fails.
Business asset diagnostics additionally include `failure_decision`,
`failure_action`, and `failure_required_condition` so callers do not need to
parse human-readable messages before deciding whether to request readable
visualization, resolve references, run upstream tessellation, raise limits, or
reject invalid input.
The versioned JSON contracts for capabilities, inspect reports, batch
manifests, local job records, and cache-dump manifests are documented in
[`docs/json_contracts.md`](docs/json_contracts.md).
Compatibility levels and corpus acceptance requirements are documented in
[`docs/compatibility.md`](docs/compatibility.md).

## CLI

```bash
cargo run -p feather-cli -- formats
cargo run -p feather-cli -- formats --json
cargo run -p feather-cli -- inspect model.CATPart
cargo run -p feather-cli -- inspect model.CATPart --json --check
cargo run -p feather-cli -- inspect model.CATPart --check --max-input-bytes 1073741824
cargo run -p feather-cli -- inspect model.CATPart --check --max-archive-entry-bytes 536870912
cargo run -p feather-cli -- inspect assembly.CATProduct --json --check --resolve-dir ./parts
cargo run -p feather-cli -- inspect assembly.CATProduct --json --check --map-root 'C:\vault\old=./released'
cargo run -p feather-cli -- convert model.CATPart -o model.glb
cargo run -p feather-cli -- convert assembly.CATProduct -o assembly.glb --resolve-dir ./parts
cargo run -p feather-cli -- convert assembly.CATProduct -o assembly.glb --map-root 'C:\vault\old=./released'
cargo run -p feather-cli -- convert assembly.CATProduct -o assembly.glb --resolve-dir ./parts --lod medium
cargo run -p feather-cli -- convert assembly.CATProduct -o assembly.glb --max-triangles 150000
cargo run -p feather-cli -- convert assembly.CATProduct -o assembly.glb --quantize 0.001
cargo run -p feather-cli -- convert assembly.CATProduct -o assembly.glb --no-normals
cargo run -p feather-cli -- convert model.step -o model.glb --chord-error 0.05 --max-step-curve-segments 16384 --max-step-spline-degree 16 --max-step-spline-control-points 16384
cargo run -p feather-cli -- batch ./incoming-cad --out ./converted-glb --resolve-dir ./parts --map-root 'C:\vault\old=./released'
cargo run -p feather-cli -- batch ./incoming-cad --out ./preflight --check-only --resolve-dir ./parts
cargo run -p feather-cli -- job convert model.CATPart --store ./.feather-jobs --json
cargo run -p feather-cli -- job batch ./incoming-cad --store ./.feather-jobs --json
cargo run -p feather-cli -- job status job-123 --store ./.feather-jobs --json
cargo run -p feather-cli -- job retry job-123 --store ./.feather-jobs --json
cargo run -p feather-cli -- dump-cache model.CATPart --out ./dump
```

## Input and Container Resource Limits

Every import, inspection, batch, and cache-dump path or byte API applies
`ImportLimits` before scanning container payloads or conversion.
Defaults accept at most 1 GiB per source input; 16,384 non-empty OLE streams,
512 MiB per OLE stream, and 1 GiB cumulative OLE stream data; and 4,096 ZIP
entries, 512 MiB per uncompressed ZIP entry, and 1 GiB cumulative ZIP data.
STEP curve tessellation defaults to at most 16,384 segments per edge, B-spline
degree at most 16, and B-spline control points at most 16,384 per curve.
ZIP limits also apply to archives nested in OLE streams and are checked before
decompression. Core API callers set `ImportOptions::limits`; CLI commands
accept `--max-input-bytes`, `--max-ole-streams`,
`--max-ole-stream-bytes`, `--max-ole-total-bytes`,
`--max-archive-entries`, `--max-archive-entry-bytes`, and
`--max-archive-total-bytes`. STEP curve expansion is bounded by
`--max-step-curve-segments`, `--max-step-spline-degree`, and
`--max-step-spline-control-points`. Resource failures use the stable
`resource_limit_exceeded` category and never emit partial conversion output.

`feather formats` is the human-readable source of truth for the current support
matrix; `feather formats --json` emits the same contract with structured fields
such as `requires_visual_payload`, `supports_external_references`,
`supports_native_tessellation`, and `native_brep_tessellation`. `inspect
--json --check` embeds the relevant capability object and, for failed imports,
adds `failure_stage`, `failure_category`, and `required_condition` fields so
callers can distinguish missing preview payloads, missing external references,
unsupported input, and pending tessellation work without scraping error text.
For library callers, `preflight_asset` exposes the same classification as
`AssetPreflightDecision` values such as `ready`, `needs_readable_visualization`,
`needs_external_references`, `needs_upstream_tessellation`,
`resource_limit_exceeded`, and `unsupported_input`.
The converter fails explicitly when a file requires B-Rep geometry outside the
supported planar subset; it does not emit placeholder or partial meshes. `inspect`
reports discovered lightweight assets, and `inspect --check` performs a real
lightweight import/validation pass so batch screening can distinguish
recognised containers from files that are actually importable.
For assemblies, `inspect --check` uses the same `--resolve-dir` and `--map-root`
external reference rules as conversion, avoiding false negatives during
preflight screening.
Conversion sidecar metadata includes source format, mode, mesh and triangle
counts, B-Rep flags, warnings, and the transformed scene bounding box.
`dump-cache` extracts discovered lightweight assets and writes a manifest so
production operators can see which cache or preview payload was actually used.
ZIP glTF previews dump both the `.gltf` JSON and same-directory `.bin` buffers.
`batch` accepts files or directories, recursively converts supported inputs,
continues after per-file failures, and writes a manifest with `ok`, `reused`,
`checked`, or `error` status for every attempted file. The append-only
`operation` field reports `converted`, `reused`, `checked`, or `error` for
business callers. `batch --check-only` performs the
same real lightweight import and Lite IR validation used by conversion but does
not write GLB or metadata outputs, which makes it suitable as a production
preflight before committing private CAD packages to a conversion queue.
Manifest items include machine-readable importability, detected format, probe
confidence, embedded-cache presence, failure-stage fields, and stable
`error_category` values, plus the same format `capability` object and
`required_condition` guidance used by inspect diagnostics. They also include
input size, output size, metadata size, and per-file duration fields. The
manifest includes a `summary` block with
format counts, total input/output bytes, total mesh/triangle counts, total
duration, failure stages, and failure categories, so production jobs can
separate missing external references, unsupported inputs, pending tessellation
work, and recognized private CAD files that lack readable lightweight payloads.
The command exits non-zero when any item fails so CI or production jobs do not
silently miss conversions.
For directory inputs, batch discovery uses supported extensions first and only
reads a small file header for unknown extensions to avoid loading unrelated
large files during candidate screening. Passing a file path explicitly still
forces an attempted conversion for that file.

## Business Asset Packages

`ensure_asset_package` and `ensure_batch_asset_package` are the recommended
library entries when a business system wants an idempotent lightweight package
instead of a loose GLB path. They reuse a current package and return `reused`;
otherwise they call `convert_asset` or `convert_batch_assets` and return
`converted`. Use the direct conversion functions when the caller explicitly
wants to force a rewrite.
Single-file packages contain `model.glb`, `metadata.json`, `source-info.json`,
and `diagnostics.json`. Batch packages write `manifest.json`,
`source-info.json`, `diagnostics.json`, and an `outputs` directory; directory
inputs are expanded to the actual supported file set before identity and
freshness checks are computed. Both package metadata files include a
deterministic `asset_id`, source SHA-256, source byte size, and settings
fingerprint so callers can decide whether a package still matches the current
source, conversion profile, and batch mode.
Successful asset results also expose `quality`, a business-level
`AssetQualityReport`. `preview_status` is `ready`, `no_visual_geometry`,
`no_preview_output`, or `partial_failure`; `quality_level` is `empty`, `light`,
`medium`, `heavy`, or `oversized` using the same triangle budgets as the built
in profiles (`50k`, `150k`, and `500k`). Batch check-only runs can have visual
geometry counts while still reporting `no_preview_output` because they do not
write GLB preview artifacts.
Use `preflight_batch_assets` to get one `AssetPreflightDecision` per discovered
input plus an aggregate decision/action before creating or rewriting a package.
For UI, logs, or orchestration code that needs more than a boolean,
`explain_asset_package_freshness` and
`explain_batch_asset_package_freshness` return `AssetPackageFreshness` with a
stable reason label such as `source_changed`, `settings_changed`,
`missing_metadata`, `missing_manifest`, `manifest_mismatch`,
`output_artifact_missing`, or `diagnostics_failed`.
Use `inspect_asset_package` when the caller only has a package directory and
needs to audit its internal completeness. It returns `AssetPackageAudit` without
reading original CAD sources; request-relative freshness still belongs to
`explain_asset_package_freshness` and
`explain_batch_asset_package_freshness`.
Use `read_asset_package_summary` after audit when the caller needs the actual
lightweight result list. Single-file packages return one `converted` item with
`model.glb` and `metadata.json`; batch packages return one item per manifest
entry, with `converted`, `reused`, or `checked` operations and typed sidecar
metadata when a GLB output was written.
Failed package diagnostics keep the original `failure` object and append
business fields such as `failure_decision=needs_readable_visualization` and
`failure_action=provide_readable_visualization`, matching
`AssetFailure::decision()` and `AssetFailure::action()`.
When an existing batch package is stale, `ensure_batch_asset_package` reuses
unchanged successful items and converts only changed or new inputs; deleted
inputs are removed from the rewritten manifest.

## Local Job Store

`feather job` wraps conversion and batch work in a persistent business record
without requiring a database or service framework. A store contains
`jobs/<job-id>/job.json` plus an `artifacts` directory. Single-file conversion
jobs reserve `model.glb`, optional `metadata.json`, and `source-info.json`.
Batch jobs reserve `manifest.json`, `source-info.json`, and an `outputs`
directory.

Job records use `queued`, `running`, `succeeded`, `failed`, and `cancelled`
statuses. Failed jobs keep a structured `failure` object with `stage`,
`category`, `message`, and `retryable`, so a service layer can expose the same
record through HTTP without scraping CLI text. `feather job retry` reuses the
persisted request and artifact paths.
