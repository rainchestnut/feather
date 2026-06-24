# Feather

Feather is a Rust-first lightweight CAD conversion pipeline. It targets visual
preview, web delivery, and GLB export rather than exact CAD editing.

## Project Shape

The workspace contains two deliverables:

- `crates/feather-lite`: the reusable core library. It owns probing, import,
  lightweight IR validation, mesh preparation, GLB export, inspect reports,
  batch manifests, and embedded cache dumping.
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
    BatchConversionOptions, ConversionOptions, InspectOptions, ReferencePathMapping,
    convert_path_to_glb, inspect_path, run_batch_conversion,
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
```

The main embeddable operations are:

- `detect_format` and `inspect_path`: probe, asset discovery, and optional real
  import validation.
- `convert_path_to_glb`: single-file conversion with mesh cleanup, GLB
  validation, and optional sidecar metadata.
- `run_batch_conversion`: recursive file/directory batch conversion or
  check-only preflight with a manifest.
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
The versioned JSON contracts for capabilities, inspect reports, batch
manifests, and cache-dump manifests are documented in
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
continues after per-file failures, and writes a manifest with `ok`, `checked`,
or `error` status for every attempted file. `batch --check-only` performs the
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
