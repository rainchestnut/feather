//! Public capability matrix for supported source formats.
//!
//! The matrix is part of the product contract: it tells callers which formats
//! are accepted today, which conversion path is used, and where native
//! open-source tessellation is still pending.

use crate::contracts::FORMAT_CAPABILITIES_CONTRACT_VERSION;
use crate::json::escape_json;
use crate::probe::FileFormat;

/// User-facing capability description for one source format family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct FormatCapability {
    pub format: FileFormat,
    pub extensions: &'static [&'static str],
    pub status: &'static str,
    pub requires_visual_payload: bool,
    pub supports_embedded_assets: bool,
    pub supports_external_references: bool,
    pub supports_native_tessellation: bool,
    pub native_brep_tessellation: &'static str,
    pub conversion_path: &'static str,
    pub limitation: &'static str,
}

impl FormatCapability {
    /// Returns true when the format can produce output today without pending work.
    pub fn is_available(self) -> bool {
        self.status == "available"
    }
}

/// Returns the built-in format capability matrix.
pub fn format_capabilities() -> &'static [FormatCapability] {
    &[
        FormatCapability {
            format: FileFormat::CatiaCatPart,
            extensions: &[".CATPart", ".CADPart"],
            status: "partial",
            requires_visual_payload: true,
            supports_embedded_assets: true,
            supports_external_references: false,
            supports_native_tessellation: false,
            native_brep_tessellation: "not_decoded",
            conversion_path: "embedded visualization cache, OLE regular/mini streams, embedded STL/OBJ/glTF(BIN/data URI)/GLB, ZIP XML assembly manifests, or stored/deflated ZIP entries with ZIP64 metadata -> Feather Lite IR -> mesh cleaning/quantization/LOD -> GLB",
            limitation: "native V5 CFV2 CATCGRCont visualization and proprietary B-Rep are detected but not decoded",
        },
        FormatCapability {
            format: FileFormat::CatiaCatProduct,
            extensions: &[".CATProduct"],
            status: "partial",
            requires_visual_payload: true,
            supports_embedded_assets: true,
            supports_external_references: true,
            supports_native_tessellation: false,
            native_brep_tessellation: "not_decoded",
            conversion_path: "embedded visualization cache, cache-declared external references, OLE regular/mini streams, embedded STL/OBJ/glTF(BIN/data URI)/GLB, ZIP XML assembly manifests with embedded assets, resolve-dir external references, or map-root remapped external references, or stored/deflated ZIP entries with ZIP64 metadata -> Feather Lite IR -> mesh cleaning/quantization/LOD -> GLB",
            limitation: "native V5 CFV2 CATCGRCont visualization plus proprietary assembly references and transforms are detected but not decoded",
        },
        FormatCapability {
            format: FileFormat::CatiaCgr,
            extensions: &[".CGR"],
            status: "partial",
            requires_visual_payload: true,
            supports_embedded_assets: true,
            supports_external_references: false,
            supports_native_tessellation: false,
            native_brep_tessellation: "not_applicable",
            conversion_path: "visualization cache, OLE regular/mini streams, embedded STL/OBJ/glTF(BIN/data URI)/GLB, ZIP XML assembly manifests, or stored/deflated ZIP entries with ZIP64 metadata -> Feather Lite IR -> mesh cleaning/quantization/LOD -> GLB",
            limitation: "native binary CATCGRCont sections are detected but not decoded",
        },
        FormatCapability {
            format: FileFormat::Dassault3dxml,
            extensions: &[".3dxml"],
            status: "available",
            requires_visual_payload: true,
            supports_embedded_assets: true,
            supports_external_references: true,
            supports_native_tessellation: true,
            native_brep_tessellation: "not_decoded",
            conversion_path: "3DXML ZIP/XML manifests with ProductStructure ID relationships, urn:3DXML references, RelativeMatrix transforms, XML 3DRep polygonal tessellation, resolve-dir or map-root remapped external references, embedded visualization cache, OLE regular/mini streams, embedded STL/OBJ/glTF(BIN/data URI)/GLB, or stored/deflated ZIP entries with ZIP64 metadata -> Feather Lite IR -> mesh cleaning/quantization/LOD -> GLB",
            limitation: "binary/encrypted 3DRep streams and surface 3DRep data beyond readable XML polygonal tessellation are not decoded",
        },
        FormatCapability {
            format: FileFormat::NxPrt,
            extensions: &[".prt", ".ugpart", ".nxpart"],
            status: "available",
            requires_visual_payload: true,
            supports_embedded_assets: true,
            supports_external_references: false,
            supports_native_tessellation: false,
            native_brep_tessellation: "not_decoded",
            conversion_path: "embedded visualization cache, OLE regular/mini streams, embedded STL/OBJ/glTF(BIN/data URI)/GLB, ZIP XML assembly manifests, or stored/deflated ZIP entries with ZIP64 metadata -> Feather Lite IR -> mesh cleaning/quantization/LOD -> GLB",
            limitation: "proprietary B-Rep is not decoded",
        },
        FormatCapability {
            format: FileFormat::SolidWorksPart,
            extensions: &[".SLDPRT"],
            status: "available",
            requires_visual_payload: true,
            supports_embedded_assets: true,
            supports_external_references: false,
            supports_native_tessellation: false,
            native_brep_tessellation: "not_decoded",
            conversion_path: "embedded visualization cache, OLE regular/mini streams, embedded STL/OBJ/glTF(BIN/data URI)/GLB, ZIP XML assembly manifests, or stored/deflated ZIP entries with ZIP64 metadata -> Feather Lite IR -> mesh cleaning/quantization/LOD -> GLB",
            limitation: "proprietary B-Rep and feature tree are not decoded",
        },
        FormatCapability {
            format: FileFormat::SolidWorksAssembly,
            extensions: &[".SLDASM"],
            status: "available",
            requires_visual_payload: true,
            supports_embedded_assets: true,
            supports_external_references: true,
            supports_native_tessellation: false,
            native_brep_tessellation: "not_decoded",
            conversion_path: "embedded visualization cache, cache-declared external references, OLE regular/mini streams, embedded STL/OBJ/glTF(BIN/data URI)/GLB, ZIP XML assembly manifests with embedded assets, resolve-dir external references, or map-root remapped external references, or stored/deflated ZIP entries with ZIP64 metadata -> Feather Lite IR -> mesh cleaning/quantization/LOD -> GLB",
            limitation: "proprietary assembly references and transforms without readable cache declarations are not expanded yet",
        },
        FormatCapability {
            format: FileFormat::PrivateCad,
            extensions: &[
                ".prt", ".asm", ".ipt", ".iam", ".par", ".psm", ".x_t", ".x_b", ".jt", ".sat",
                ".sab", ".igs", ".iges", ".neu", ".model", ".session", ".exp", ".dlv",
            ],
            status: "available",
            requires_visual_payload: true,
            supports_embedded_assets: true,
            supports_external_references: true,
            supports_native_tessellation: false,
            native_brep_tessellation: "not_decoded",
            conversion_path: "generic private CAD cache-first fallback: embedded visualization cache, cache-declared external references, OLE regular/mini streams, embedded STL/OBJ/glTF(BIN/data URI)/GLB, ZIP XML assembly manifests with embedded assets, resolve-dir external references, or map-root remapped external references, or stored/deflated ZIP entries with ZIP64 metadata -> Feather Lite IR -> mesh cleaning/quantization/LOD -> GLB",
            limitation: "vendor-specific B-Rep, feature trees, and proprietary assembly semantics without readable cache declarations are not decoded",
        },
        FormatCapability {
            format: FileFormat::FeatherCache,
            extensions: &[".flite"],
            status: "available",
            requires_visual_payload: false,
            supports_embedded_assets: false,
            supports_external_references: true,
            supports_native_tessellation: false,
            native_brep_tessellation: "not_applicable",
            conversion_path: "standalone Feather Lite cache -> Feather Lite IR -> mesh cleaning/quantization/LOD -> GLB",
            limitation: "cache format is a lightweight visualization contract, not a CAD B-Rep format",
        },
        FormatCapability {
            format: FileFormat::Stl,
            extensions: &[".stl"],
            status: "available",
            requires_visual_payload: false,
            supports_embedded_assets: false,
            supports_external_references: false,
            supports_native_tessellation: false,
            native_brep_tessellation: "not_applicable",
            conversion_path: "binary or ASCII STL -> Feather Lite IR -> mesh cleaning/quantization/LOD -> GLB",
            limitation: "STL has no assembly hierarchy or CAD B-Rep",
        },
        FormatCapability {
            format: FileFormat::Obj,
            extensions: &[".obj"],
            status: "available",
            requires_visual_payload: false,
            supports_embedded_assets: false,
            supports_external_references: false,
            supports_native_tessellation: false,
            native_brep_tessellation: "not_applicable",
            conversion_path: "Wavefront OBJ with usemtl/MTL color subset -> Feather Lite IR -> mesh cleaning/quantization/LOD -> GLB",
            limitation: "texture maps and advanced MTL shading parameters are not imported",
        },
        FormatCapability {
            format: FileFormat::Glb,
            extensions: &[".glb"],
            status: "available",
            requires_visual_payload: false,
            supports_embedded_assets: false,
            supports_external_references: false,
            supports_native_tessellation: false,
            native_brep_tessellation: "not_applicable",
            conversion_path: "binary GLB static mesh or ZIP-packaged glTF preview with node TRS/matrix transforms, interleaved/offset bufferViews, and sibling BIN/data URI buffers -> Feather Lite IR -> mesh cleaning/quantization/LOD -> GLB",
            limitation: "animations, skins, textures, and standalone glTF external files are not imported",
        },
        FormatCapability {
            format: FileFormat::Step,
            extensions: &[".step", ".stp"],
            status: "partial",
            requires_visual_payload: false,
            supports_embedded_assets: true,
            supports_external_references: false,
            supports_native_tessellation: true,
            native_brep_tessellation: "partial",
            conversion_path: "AP242 tessellated faces, AP203/AP214/AP242 ADVANCED_FACE B-Rep with bounded outer/inner loops, adaptive LINE/CIRCLE/ELLIPSE boundaries (including single-edge closed conics) and parameter TRIMMED_CURVE spans over those bases on PLANE/CYLINDRICAL_SURFACE/CONICAL_SURFACE/SPHERICAL_SURFACE and regular ring TOROIDAL_SURFACE with meridian/parallel circular boundaries, rational or non-rational B_SPLINE_CURVE_WITH_KNOTS boundaries and parameter TRIMMED_CURVE spans over B-Spline basis on supported analytic faces, shape-representation assembly hierarchy with reusable meshes and rigid ITEM_DEFINED_TRANSFORMATION instances, SI or conversion-based length and plane-angle units, presentation colors, or embedded tessellation cache -> Feather Lite IR -> validated constrained triangulation -> mesh cleaning/quantization/LOD -> GLB",
            limitation: "Cartesian-only trimmed curves, trimmed curves over unsupported bases, B-spline boundaries on unsupported or singular surfaces, other curves, spline surfaces, horn and spindle tori, non-meridian/non-parallel torus circles, cone faces reaching or crossing the apex, sphere faces touching parameterization poles, and non-rigid or non-ITEM_DEFINED_TRANSFORMATION assembly transforms are not tessellated",
        },
    ]
}

/// Looks up the capability contract for one detected source format.
pub fn format_capability(format: FileFormat) -> Option<&'static FormatCapability> {
    format_capabilities()
        .iter()
        .find(|capability| capability.format == format)
}

/// Serializes the full capability matrix as stable JSON for tooling.
pub fn format_capabilities_json() -> String {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"contract_version\": \"");
    json.push_str(FORMAT_CAPABILITIES_CONTRACT_VERSION);
    json.push_str("\",\n");
    json.push_str("  \"formats\": [\n");
    for (index, capability) in format_capabilities().iter().enumerate() {
        if index > 0 {
            json.push_str(",\n");
        }
        push_format_capability_json(&mut json, capability, "    ");
    }
    json.push_str("\n  ]\n");
    json.push_str("}\n");
    json
}

pub(crate) fn push_format_capability_json(
    json: &mut String,
    capability: &FormatCapability,
    indent: &str,
) {
    let field_indent = format!("{indent}  ");
    json.push_str(indent);
    json.push_str("{\n");
    push_json_string_field(
        json,
        &field_indent,
        "format",
        capability.format.label(),
        true,
    );
    push_json_string_array_field(
        json,
        &field_indent,
        "extensions",
        capability.extensions,
        true,
    );
    push_json_string_field(json, &field_indent, "status", capability.status, true);
    push_json_bool_field(
        json,
        &field_indent,
        "available",
        capability.is_available(),
        true,
    );
    push_json_bool_field(
        json,
        &field_indent,
        "requires_visual_payload",
        capability.requires_visual_payload,
        true,
    );
    push_json_bool_field(
        json,
        &field_indent,
        "supports_embedded_assets",
        capability.supports_embedded_assets,
        true,
    );
    push_json_bool_field(
        json,
        &field_indent,
        "supports_external_references",
        capability.supports_external_references,
        true,
    );
    push_json_bool_field(
        json,
        &field_indent,
        "supports_native_tessellation",
        capability.supports_native_tessellation,
        true,
    );
    push_json_string_field(
        json,
        &field_indent,
        "native_brep_tessellation",
        capability.native_brep_tessellation,
        true,
    );
    push_json_string_field(
        json,
        &field_indent,
        "conversion_path",
        capability.conversion_path,
        true,
    );
    push_json_string_field(
        json,
        &field_indent,
        "limitation",
        capability.limitation,
        false,
    );
    json.push_str(indent);
    json.push('}');
}

fn push_json_string_field(
    json: &mut String,
    indent: &str,
    name: &str,
    value: &str,
    trailing_comma: bool,
) {
    json.push_str(indent);
    json.push('"');
    json.push_str(name);
    json.push_str("\": \"");
    json.push_str(&escape_json(value));
    json.push('"');
    if trailing_comma {
        json.push(',');
    }
    json.push('\n');
}

fn push_json_bool_field(
    json: &mut String,
    indent: &str,
    name: &str,
    value: bool,
    trailing_comma: bool,
) {
    json.push_str(indent);
    json.push('"');
    json.push_str(name);
    json.push_str("\": ");
    json.push_str(if value { "true" } else { "false" });
    if trailing_comma {
        json.push(',');
    }
    json.push('\n');
}

fn push_json_string_array_field(
    json: &mut String,
    indent: &str,
    name: &str,
    values: &[&str],
    trailing_comma: bool,
) {
    json.push_str(indent);
    json.push('"');
    json.push_str(name);
    json.push_str("\": [");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            json.push_str(", ");
        }
        json.push('"');
        json.push_str(&escape_json(value));
        json.push('"');
    }
    json.push(']');
    if trailing_comma {
        json.push(',');
    }
    json.push('\n');
}
