//! Shared diagnostic helpers for inspection and batch manifests.

use crate::probe::FileFormat;

/// Classifies a conversion or preflight failure into a stable manifest category.
pub fn batch_failure_category(stage: &str, message: &str) -> &'static str {
    if stage == "io" {
        return "io";
    }
    if stage == "export" {
        return "export";
    }

    let message = message.to_ascii_lowercase();
    if message.contains("resource limit exceeded") {
        "resource_limit_exceeded"
    } else if message.contains("external reference") && message.contains("could not be resolved") {
        "missing_external_reference"
    } else if message.contains("contains native catcgrcont visualization")
        && message.contains("not decoded by the open-source importer")
    {
        "native_visualization_not_decoded"
    } else if message.contains("has no readable lightweight visualization cache") {
        "no_readable_lightweight_cache"
    } else if message.contains("tessellation is not implemented")
        || message.contains("b-rep surface tessellation is pending")
    {
        "tessellation_pending"
    } else if message.contains("unsupported input") {
        "unsupported_input"
    } else if message.contains("invalid source data") {
        "invalid_source_data"
    } else if message.contains("missing") {
        "missing_data"
    } else {
        "other"
    }
}

/// Explains the most likely operator action for a classified inspect failure.
pub(crate) fn required_condition_for_failure(
    format: FileFormat,
    category: &str,
) -> Option<&'static str> {
    match category {
        "missing_external_reference" => {
            Some("provide --resolve-dir or --map-root so assembly references can be loaded")
        }
        "no_readable_lightweight_cache" if format.is_private_cad() => Some(
            "provide a readable lightweight visualization payload: Feather cache, embedded mesh/GLB/glTF/STL/OBJ, ZIP/OLE preview, or resolvable cache-declared reference",
        ),
        "native_visualization_not_decoded" => Some(
            "provide a readable open visualization export such as polygonal 3DXML/3DRep, GLB, STL, OBJ, or Feather Lite cache",
        ),
        "resource_limit_exceeded" => Some(
            "reduce input/container size or raise ImportLimits after validating trusted workload requirements",
        ),
        "tessellation_pending" => Some(
            "use AP242 tessellated data, ADVANCED_FACE B-Rep with bounded outer/inner LINE/CIRCLE/ELLIPSE loops or parameter TRIMMED_CURVE spans on supported analytic surfaces, LINE/CIRCLE SURFACE_OF_LINEAR_EXTRUSION, B_SPLINE_CURVE_WITH_KNOTS boundaries or parameter TRIMMED_CURVE spans on supported analytic faces, regular ring TOROIDAL_SURFACE with meridian/parallel circles, rigid ITEM_DEFINED_TRANSFORMATION shape-representation assemblies, or an embedded tessellation cache; other analytic, spline, singular, and transformed representations require upstream tessellation",
        ),
        "unsupported_input" => {
            Some("use a supported source format or embed a readable Feather Lite cache payload")
        }
        "invalid_source_data" => {
            Some("provide a valid readable visual payload for the detected source format")
        }
        "missing_data" => {
            Some("provide the missing geometry, buffer, material, or referenced payload")
        }
        "io" => {
            Some("make the input and output paths readable and writable for the conversion process")
        }
        "export" => Some("fix the imported LiteDocument so GLB export and validation can complete"),
        _ => None,
    }
}
