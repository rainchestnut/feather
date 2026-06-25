# Compatibility and Corpus Acceptance

Feather separates container recognition from successful lightweight import.
An extension or file signature match is not sufficient evidence that geometry
can be converted.

## Support Levels

- `available`: representative inputs convert without an unmet implementation
  dependency.
- `partial`: a documented subset converts and unsupported native
  representations fail with a stable diagnostic category.
- `planned`: the format is identified for roadmap purposes but has no current
  conversion path.

CATPart, CATProduct, and CGR are `partial`. Feather can import supported
Feather caches, standard embedded mesh assets, readable ZIP/OLE payloads, and
polygonal 3DXML/3DRep data. It detects valid CATIA V5 CFV2 framing, source
release strings, and native `CATCGRCont` objects, but does not decode the
proprietary binary CGR representation.

## Corpus Requirements

A format may move to `available` only after tests cover representative,
legally usable files across the source versions claimed by the project. The
corpus should include:

- parts with and without saved visualization data;
- assemblies with resolved, missing, moved, and repeated references;
- transforms, colors, multiple bodies, and empty or suppressed components;
- small, large, truncated, and intentionally malformed inputs;
- expected mesh, triangle, hierarchy, transform, and material assertions.

Private customer files remain outside the repository. They should be checked
through the same public API used in production:

```bash
feather batch <input...> --out <report-dir> --check-only
```

The resulting versioned manifest is the compatibility record. A successful
probe with `importable: false` remains unsupported for conversion, regardless
of extension recognition.

STEP B-Rep promotion requires closed-shell fixtures with shared edges, concave
faces, face orientation, presentation colors, explicit units, configurable
curve precision, resource limits, and malformed topology. Implemented analytic
curves and surfaces, rational/non-rational B-Spline boundaries on supported
faces, closed-edge loops, and unit families must have positive and negative
corpus coverage. Unsupported geometry and inner bounds must continue to fail
without partial output.

## Promotion Gate

Before expanding a native format claim, all of the following must hold:

1. The implementation uses only open-source code and documented or
   independently verified file structures.
2. Public core APIs and the CLI are exercised by integration tests.
3. Generated GLB output passes structural validation and semantic assertions.
4. Malformed input fails without panics, unbounded allocation, or silent
   placeholder geometry.
5. Capability text, inspect output, batch manifests, and documentation agree.

Resource-limit fixtures must cover source input size; OLE stream count,
per-stream size, and cumulative stream size; and ZIP entry count, per-entry
uncompressed size, and cumulative uncompressed size. Rejection must be tested
through public APIs and CLI commands without partial conversion output.
