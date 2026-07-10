# Changelog

All notable changes to Feather are tracked here.

## 0.1.0 - Unreleased

- Added a Rust workspace with `feather-lite` core library and `feather` CLI.
- Added cache-first lightweight import for private CAD containers with embedded Feather cache, STL, OBJ, glTF, GLB, ZIP, and OLE payload discovery.
- Added CATPart, CATProduct, CGR, NX PRT, SolidWorks, 3DXML, STEP, STL, OBJ, GLB, and generic private CAD probing/import paths within the documented open-source limits.
- Added native STEP AP242 tessellated import, AP203/AP214/AP242 analytic B-Rep tessellation, STEP length/angle unit handling, presentation colors, assembly hierarchy preservation, reusable mesh instances, and bounded resource limits.
- Added STEP support for planar holes, analytic surface holes, torus patches, B-Spline/NURBS edge boundaries, and parameter `TRIMMED_CURVE` spans over supported curve bases.
- Added STEP `SURFACE_OF_REVOLUTION` tessellation for line-generated planes, cylinders, and cones; circle-generated spheres and regular ring tori; and centered principal-axis ellipse-generated spheroids.
- Added CLI commands for inspect, convert, batch conversion, cache dumping, and machine-readable format capability reporting.
- Added public JSON contracts, compatibility documentation, fixtures, and integration tests for conversion behavior.
- Replaced triangle sampling LOD with deterministic meshoptimizer-based simplification, primitive-aware budget allocation, and explicit quality fallback warnings.
