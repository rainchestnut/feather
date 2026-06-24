//! Source format detection for lightweight conversion.
//!
//! Detection deliberately separates "looks like this CAD container" from
//! "has a readable visual representation". Private formats may probe
//! successfully while still requiring a visualization cache.

use std::path::Path;

use crate::cache::contains_cache;

/// CAD/source formats understood by the lightweight importer registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    CatiaCatPart,
    CatiaCatProduct,
    CatiaCgr,
    Dassault3dxml,
    NxPrt,
    SolidWorksPart,
    SolidWorksAssembly,
    PrivateCad,
    Step,
    Stl,
    Obj,
    Glb,
    FeatherCache,
    Unknown,
}

impl FileFormat {
    /// Returns a stable label used in metadata and CLI output.
    pub fn label(self) -> &'static str {
        match self {
            Self::CatiaCatPart => "CATIA_CATPart",
            Self::CatiaCatProduct => "CATIA_CATProduct",
            Self::CatiaCgr => "CATIA_CGR",
            Self::Dassault3dxml => "DASSAULT_3DXML",
            Self::NxPrt => "NX_PRT",
            Self::SolidWorksPart => "SOLIDWORKS_SLDPRT",
            Self::SolidWorksAssembly => "SOLIDWORKS_SLDASM",
            Self::PrivateCad => "PRIVATE_CAD",
            Self::Step => "STEP",
            Self::Stl => "STL",
            Self::Obj => "OBJ",
            Self::Glb => "GLB",
            Self::FeatherCache => "FeatherLiteCache",
            Self::Unknown => "Unknown",
        }
    }

    /// Parses a stable format label emitted by metadata, CLI, and manifests.
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "CATIA_CATPart" => Some(Self::CatiaCatPart),
            "CATIA_CATProduct" => Some(Self::CatiaCatProduct),
            "CATIA_CGR" => Some(Self::CatiaCgr),
            "DASSAULT_3DXML" => Some(Self::Dassault3dxml),
            "NX_PRT" => Some(Self::NxPrt),
            "SOLIDWORKS_SLDPRT" => Some(Self::SolidWorksPart),
            "SOLIDWORKS_SLDASM" => Some(Self::SolidWorksAssembly),
            "PRIVATE_CAD" => Some(Self::PrivateCad),
            "STEP" => Some(Self::Step),
            "STL" => Some(Self::Stl),
            "OBJ" => Some(Self::Obj),
            "GLB" => Some(Self::Glb),
            "FeatherLiteCache" => Some(Self::FeatherCache),
            "Unknown" => Some(Self::Unknown),
            _ => None,
        }
    }

    /// Returns true for private CAD formats that need cache-first handling.
    pub fn is_private_cad(self) -> bool {
        matches!(
            self,
            Self::CatiaCatPart
                | Self::CatiaCatProduct
                | Self::CatiaCgr
                | Self::Dassault3dxml
                | Self::NxPrt
                | Self::SolidWorksPart
                | Self::SolidWorksAssembly
                | Self::PrivateCad
        )
    }
}

/// Confidence assigned by a probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProbeConfidence {
    Unknown = 0,
    Low = 1,
    Medium = 2,
    High = 3,
    Certain = 4,
}

/// Result of probing one source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub format: FileFormat,
    pub confidence: ProbeConfidence,
    pub reason: String,
    pub has_embedded_cache: bool,
    pub container_kind: Option<&'static str>,
    pub source_version: Option<String>,
    pub native_visualization: Option<&'static str>,
}

impl ProbeResult {
    /// Creates an unknown probe result.
    pub fn unknown() -> Self {
        Self {
            format: FileFormat::Unknown,
            confidence: ProbeConfidence::Unknown,
            reason: "no known signature or extension matched".to_string(),
            has_embedded_cache: false,
            container_kind: None,
            source_version: None,
            native_visualization: None,
        }
    }

    /// Creates a probe result for a matched format.
    pub fn matched(
        format: FileFormat,
        confidence: ProbeConfidence,
        reason: impl Into<String>,
        has_embedded_cache: bool,
    ) -> Self {
        Self {
            format,
            confidence,
            reason: reason.into(),
            has_embedded_cache,
            container_kind: None,
            source_version: None,
            native_visualization: None,
        }
    }

    /// Returns true when the result is actionable.
    pub fn is_match(&self) -> bool {
        self.confidence != ProbeConfidence::Unknown && self.format != FileFormat::Unknown
    }
}

/// Returns true when a path extension is part of Feather's supported source set.
///
/// Directory batch scans use this as a cheap first-pass filter before reading
/// file bytes. Full probing still happens during explicit inspection/conversion.
pub fn has_supported_source_extension(path: &Path) -> bool {
    extension_is(
        Some(path),
        &[
            "flite",
            "step",
            "stp",
            "stl",
            "obj",
            "glb",
            "3dxml",
            "catproduct",
            "catpart",
            "cadpart",
            "cgr",
            "ugpart",
            "nxpart",
            "sldprt",
            "sldasm",
        ],
    ) || is_generic_private_cad_extension(Some(path))
}

/// Detects the most likely source format from path and leading bytes.
pub fn detect_format(path: Option<&Path>, bytes: &[u8]) -> ProbeResult {
    let embedded_cache = contains_cache(bytes);

    if starts_with_ascii(bytes, b"FEATHER_CAD_LITE_CACHE_V1") || extension_is(path, &["flite"]) {
        return ProbeResult::matched(
            FileFormat::FeatherCache,
            ProbeConfidence::Certain,
            "Feather Lite cache marker",
            embedded_cache,
        );
    }

    if starts_with_ascii(bytes, b"ISO-10303-21") || extension_is(path, &["step", "stp"]) {
        return ProbeResult::matched(
            FileFormat::Step,
            if starts_with_ascii(bytes, b"ISO-10303-21") {
                ProbeConfidence::Certain
            } else {
                ProbeConfidence::High
            },
            "STEP Part 21 marker or extension",
            embedded_cache,
        );
    }

    if extension_is(path, &["stl"]) {
        return ProbeResult::matched(
            FileFormat::Stl,
            ProbeConfidence::High,
            "STL mesh extension",
            embedded_cache,
        );
    }

    if extension_is(path, &["obj"]) {
        return ProbeResult::matched(
            FileFormat::Obj,
            ProbeConfidence::High,
            "Wavefront OBJ mesh extension",
            embedded_cache,
        );
    }

    if starts_with_ascii(bytes, b"glTF") || extension_is(path, &["glb"]) {
        return ProbeResult::matched(
            FileFormat::Glb,
            if starts_with_ascii(bytes, b"glTF") {
                ProbeConfidence::Certain
            } else {
                ProbeConfidence::High
            },
            "binary glTF/GLB marker or extension",
            embedded_cache,
        );
    }

    if extension_is(path, &["3dxml"]) || contains_ascii(bytes, b"3DXML") {
        return ProbeResult::matched(
            FileFormat::Dassault3dxml,
            ProbeConfidence::High,
            "Dassault 3DXML extension/signature",
            embedded_cache,
        );
    }

    if extension_is(path, &["catproduct"]) || contains_ascii(bytes, b"CATProduct") {
        return catia_probe_result(
            FileFormat::CatiaCatProduct,
            "product",
            embedded_cache,
            bytes,
        );
    }

    if extension_is(path, &["catpart", "cadpart"]) || contains_ascii(bytes, b"CATPart") {
        return catia_probe_result(FileFormat::CatiaCatPart, "part", embedded_cache, bytes);
    }

    if extension_is(path, &["cgr"]) || contains_ascii(bytes, b"CGR") {
        return catia_probe_result(FileFormat::CatiaCgr, "CGR", embedded_cache, bytes);
    }

    if extension_is(path, &["ugpart", "nxpart"]) || contains_ascii(bytes, b"Unigraphics") {
        return ProbeResult::matched(
            FileFormat::NxPrt,
            ProbeConfidence::High,
            "NX/UG part extension/signature",
            embedded_cache,
        );
    }

    if extension_is(path, &["sldprt"]) || contains_ascii(bytes, b"SLDPRT") {
        return ProbeResult::matched(
            FileFormat::SolidWorksPart,
            ProbeConfidence::High,
            "SolidWorks part extension/signature",
            embedded_cache,
        );
    }

    if extension_is(path, &["sldasm"]) || contains_ascii(bytes, b"SLDASM") {
        return ProbeResult::matched(
            FileFormat::SolidWorksAssembly,
            ProbeConfidence::High,
            "SolidWorks assembly extension/signature",
            embedded_cache,
        );
    }

    if contains_ascii(bytes, b"SolidWorks") {
        return ProbeResult::matched(
            FileFormat::SolidWorksPart,
            ProbeConfidence::Medium,
            "SolidWorks signature",
            embedded_cache,
        );
    }

    if is_generic_private_cad_extension(path) || contains_known_private_cad_signature(bytes) {
        return ProbeResult::matched(
            FileFormat::PrivateCad,
            ProbeConfidence::Medium,
            "known private CAD extension/signature with cache-first fallback",
            embedded_cache,
        );
    }

    if embedded_cache {
        return ProbeResult::matched(
            FileFormat::FeatherCache,
            ProbeConfidence::High,
            "embedded Feather Lite cache marker",
            embedded_cache,
        );
    }

    ProbeResult::unknown()
}

/// Verified metadata from a CATIA V5 CFV2 container header and object labels.
pub(crate) struct CatiaV5ContainerProfile {
    pub source_version: Option<String>,
    pub has_native_cgr: bool,
}

/// Parses the stable CFV2 framing shared by CATIA V5 part, product, and CGR
/// containers without attempting to decode proprietary object payloads.
pub(crate) fn catia_v5_container_profile(bytes: &[u8]) -> Option<CatiaV5ContainerProfile> {
    const HEADER_LEN: usize = 16;
    if bytes.len() < HEADER_LEN || !starts_with_ascii(bytes, b"V5_CFV2") {
        return None;
    }

    let first_section = u32::from_be_bytes(bytes.get(8..12)?.try_into().ok()?) as usize;
    let second_section = u32::from_be_bytes(bytes.get(12..16)?.try_into().ok()?) as usize;
    if first_section.checked_add(second_section)? != bytes.len() {
        return None;
    }

    Some(CatiaV5ContainerProfile {
        source_version: find_catia_v5_release(bytes),
        has_native_cgr: contains_ascii(bytes, b"CATCGRCont"),
    })
}

fn catia_probe_result(
    format: FileFormat,
    document_kind: &str,
    embedded_cache: bool,
    bytes: &[u8],
) -> ProbeResult {
    let Some(profile) = catia_v5_container_profile(bytes) else {
        return ProbeResult::matched(
            format,
            ProbeConfidence::High,
            format!("CATIA {document_kind} extension/signature"),
            embedded_cache,
        );
    };

    let mut reason = format!("CATIA V5 CFV2 {document_kind} container");
    if profile.has_native_cgr {
        reason.push_str(" with native CATCGRCont visualization");
    }
    ProbeResult {
        format,
        confidence: ProbeConfidence::Certain,
        reason,
        has_embedded_cache: embedded_cache,
        container_kind: Some("catia-v5-cfv2"),
        source_version: profile.source_version,
        native_visualization: profile
            .has_native_cgr
            .then_some("catia-native-cgr-container"),
    }
}

fn find_catia_v5_release(bytes: &[u8]) -> Option<String> {
    for start in 0..bytes.len().saturating_sub(3) {
        if !bytes[start..].starts_with(b"V5R") {
            continue;
        }
        let mut end = start + 3;
        while end < bytes.len()
            && end - start < 32
            && (bytes[end].is_ascii_uppercase() || bytes[end].is_ascii_digit())
        {
            end += 1;
        }
        let candidate = std::str::from_utf8(&bytes[start..end]).ok()?;
        if candidate[3..]
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
        {
            return Some(candidate.to_string());
        }
    }
    None
}

fn is_generic_private_cad_extension(path: Option<&Path>) -> bool {
    extension_is(
        path,
        &[
            "prt", "asm", "ipt", "iam", "par", "psm", "x_t", "x_b", "jt", "sat", "sab", "igs",
            "iges", "neu", "model", "session", "exp", "dlv",
        ],
    )
}

fn contains_known_private_cad_signature(bytes: &[u8]) -> bool {
    [
        b"Creo".as_slice(),
        b"Pro/ENGINEER".as_slice(),
        b"Autodesk Inventor".as_slice(),
        b"Solid Edge".as_slice(),
        b"Parasolid".as_slice(),
        b"ACIS".as_slice(),
        b"IGES".as_slice(),
        b"CATIA V4".as_slice(),
    ]
    .iter()
    .any(|needle| contains_ascii(bytes, needle))
}

fn extension_is(path: Option<&Path>, extensions: &[&str]) -> bool {
    let Some(extension) = path
        .and_then(Path::extension)
        .and_then(|value| value.to_str())
    else {
        return false;
    };

    extensions
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

fn starts_with_ascii(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn contains_ascii(bytes: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || bytes.len() < needle.len() {
        return false;
    }

    bytes
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}
