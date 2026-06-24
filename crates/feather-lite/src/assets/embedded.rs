//! Scanners for standard visual assets embedded inside larger CAD containers.

use std::path::Path;

use crate::assets::glb::{glb_len, import_glb_document, is_exact_glb, is_gltf_json};
use crate::assets::obj::{import_obj_document, is_obj};
use crate::assets::ole::extract_ole_streams;
use crate::assets::stl::{
    binary_stl_len, import_ascii_stl_document, import_binary_stl_document, is_ascii_stl,
    is_exact_binary_stl,
};
use crate::assets::three_dxml_rep::{import_3dxml_rep_document, is_3dxml_rep};
use crate::assets::zip::{import_zip_document, zip_entries};
use crate::cache::extract_cache_payload;
use crate::document::LiteDocument;
use crate::importer::{ImportError, ImportLimits, ensure_input_size};

/// Supported lightweight asset kinds discoverable inside CAD containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedVisualAssetKind {
    FeatherCache,
    BinaryStl,
    AsciiStl,
    Obj,
    Gltf,
    GltfBinaryBuffer,
    Glb,
    ThreeDxmlRep,
}

impl EmbeddedVisualAssetKind {
    /// Stable lower-case label used in manifests.
    pub fn label(self) -> &'static str {
        match self {
            Self::FeatherCache => "feather-cache",
            Self::BinaryStl => "binary-stl",
            Self::AsciiStl => "ascii-stl",
            Self::Obj => "obj",
            Self::Gltf => "gltf",
            Self::GltfBinaryBuffer => "gltf-bin",
            Self::Glb => "glb",
            Self::ThreeDxmlRep => "3dxml-rep",
        }
    }

    /// File extension used when dumping the extracted payload.
    pub fn extension(self) -> &'static str {
        match self {
            Self::FeatherCache => "flite",
            Self::BinaryStl | Self::AsciiStl => "stl",
            Self::Obj => "obj",
            Self::Gltf => "gltf",
            Self::GltfBinaryBuffer => "bin",
            Self::Glb => "glb",
            Self::ThreeDxmlRep => "3drep",
        }
    }
}

/// Where a visual asset was found in the source byte stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedVisualAssetSource {
    ExactInput,
    EmbeddedBytes,
    ZipEntry,
    OleStream,
}

impl EmbeddedVisualAssetSource {
    /// Stable lower-case label used in manifests.
    pub fn label(self) -> &'static str {
        match self {
            Self::ExactInput => "exact-input",
            Self::EmbeddedBytes => "embedded-bytes",
            Self::ZipEntry => "zip-entry",
            Self::OleStream => "ole-stream",
        }
    }
}

/// Extracted candidate payload used by diagnostics and cache dumping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedVisualAsset {
    pub kind: EmbeddedVisualAssetKind,
    pub source: EmbeddedVisualAssetSource,
    pub byte_start: usize,
    pub byte_end: usize,
    pub name: Option<String>,
    pub payload: Vec<u8>,
}

impl EmbeddedVisualAsset {
    fn new(
        kind: EmbeddedVisualAssetKind,
        source: EmbeddedVisualAssetSource,
        byte_start: usize,
        byte_end: usize,
        name: Option<String>,
        payload: &[u8],
    ) -> Self {
        Self {
            kind,
            source,
            byte_start,
            byte_end,
            name,
            payload: payload.to_vec(),
        }
    }
}

/// Finds importable visual assets inside a CAD-like byte stream.
pub fn discover_embedded_visual_assets(
    bytes: &[u8],
) -> Result<Vec<EmbeddedVisualAsset>, ImportError> {
    discover_embedded_visual_assets_with_limits(bytes, &ImportLimits::default())
}

/// Finds importable visual assets while enforcing caller-provided input and
/// container expansion limits.
pub fn discover_embedded_visual_assets_with_limits(
    bytes: &[u8],
    limits: &ImportLimits,
) -> Result<Vec<EmbeddedVisualAsset>, ImportError> {
    ensure_input_size(bytes, limits)?;
    let mut assets = Vec::new();
    let ole_streams = extract_ole_streams(bytes, limits)?;
    if !ole_streams.is_empty() {
        for stream in ole_streams {
            discover_ole_stream_assets(&mut assets, &stream.name, &stream.payload, limits)?;
        }
        assets.sort_by_key(|asset| (asset.byte_start, asset.byte_end, asset.name.clone()));
        return Ok(assets);
    }

    if let Some(cache) = extract_cache_payload(bytes)? {
        let source = if cache.start == 0 && cache.end == bytes.len() {
            EmbeddedVisualAssetSource::ExactInput
        } else {
            EmbeddedVisualAssetSource::EmbeddedBytes
        };
        push_unique_asset(
            &mut assets,
            EmbeddedVisualAsset::new(
                EmbeddedVisualAssetKind::FeatherCache,
                source,
                cache.start,
                cache.end,
                None,
                &bytes[cache.start..cache.end],
            ),
        );
    }

    let zip_assets = decoded_zip_payloads(bytes, limits)?;
    let zip_gltf_names = gltf_entry_names(&zip_assets);
    for (entry, payload) in &zip_assets {
        let Some(kind) = classify_zip_payload(&entry.name, payload, &zip_gltf_names)? else {
            continue;
        };
        push_unique_asset(
            &mut assets,
            EmbeddedVisualAsset::new(
                kind,
                EmbeddedVisualAssetSource::ZipEntry,
                entry.data_start,
                entry.data_end,
                Some(entry.name.clone()),
                payload,
            ),
        );
    }

    if let Some(kind) = classify_exact_payload(bytes)? {
        push_unique_asset(
            &mut assets,
            EmbeddedVisualAsset::new(
                kind,
                EmbeddedVisualAssetSource::ExactInput,
                0,
                bytes.len(),
                None,
                bytes,
            ),
        );
    }

    if let Some(range) = find_embedded_binary_stl(bytes) {
        push_unique_asset(
            &mut assets,
            EmbeddedVisualAsset::new(
                EmbeddedVisualAssetKind::BinaryStl,
                EmbeddedVisualAssetSource::EmbeddedBytes,
                range.start,
                range.end,
                None,
                &bytes[range.start..range.end],
            ),
        );
    }
    if let Some(range) = find_embedded_ascii_stl(bytes) {
        push_unique_asset(
            &mut assets,
            EmbeddedVisualAsset::new(
                EmbeddedVisualAssetKind::AsciiStl,
                EmbeddedVisualAssetSource::EmbeddedBytes,
                range.start,
                range.end,
                None,
                &bytes[range.start..range.end],
            ),
        );
    }
    if let Some(range) = find_embedded_obj(bytes) {
        push_unique_asset(
            &mut assets,
            EmbeddedVisualAsset::new(
                EmbeddedVisualAssetKind::Obj,
                EmbeddedVisualAssetSource::EmbeddedBytes,
                range.start,
                range.end,
                None,
                &bytes[range.start..range.end],
            ),
        );
    }
    if let Some(range) = find_embedded_glb(bytes) {
        push_unique_asset(
            &mut assets,
            EmbeddedVisualAsset::new(
                EmbeddedVisualAssetKind::Glb,
                EmbeddedVisualAssetSource::EmbeddedBytes,
                range.start,
                range.end,
                None,
                &bytes[range.start..range.end],
            ),
        );
    }

    assets.sort_by_key(|asset| (asset.byte_start, asset.byte_end));
    Ok(assets)
}

/// Attempts to import supported visual assets embedded in a source byte stream.
pub fn import_embedded_visual_assets(
    bytes: &[u8],
    source_format: &str,
    source_path: Option<&Path>,
    mode: &str,
    limits: &ImportLimits,
) -> Result<Option<LiteDocument>, ImportError> {
    import_embedded_visual_assets_inner(bytes, source_format, source_path, mode, limits, true)
}

fn import_embedded_visual_assets_inner(
    bytes: &[u8],
    source_format: &str,
    source_path: Option<&Path>,
    mode: &str,
    limits: &ImportLimits,
    include_ole: bool,
) -> Result<Option<LiteDocument>, ImportError> {
    if is_exact_binary_stl(bytes) {
        return import_binary_stl_document(bytes, source_format, mode, source_path).map(Some);
    }
    if is_ascii_stl(bytes) {
        return import_ascii_stl_document(bytes, source_format, mode, source_path).map(Some);
    }
    if is_exact_glb(bytes) {
        return import_glb_document(bytes, source_format, mode, source_path).map(Some);
    }
    if is_3dxml_rep(bytes) {
        return import_3dxml_rep_document(bytes, source_format, mode, source_path).map(Some);
    }
    if let Some(document) = import_zip_document(bytes, source_format, source_path, mode, limits)? {
        return Ok(Some(document));
    }
    if include_ole
        && let Some(document) =
            import_ole_stream_visual_assets(bytes, source_format, source_path, mode, limits)?
    {
        return Ok(Some(document));
    }
    let Some(range) = find_embedded_binary_stl(bytes) else {
        if let Some(range) = find_embedded_ascii_stl(bytes) {
            let mut document = import_ascii_stl_document(
                &bytes[range.start..range.end],
                source_format,
                mode,
                source_path,
            )?;
            document.metadata.warnings.push(format!(
                "extracted embedded ASCII STL visual asset at byte range {}..{}",
                range.start, range.end
            ));
            return Ok(Some(document));
        }
        if let Some(range) = find_embedded_obj(bytes) {
            let mut document = import_obj_document(
                &bytes[range.start..range.end],
                source_format,
                mode,
                source_path,
            )?;
            document.metadata.warnings.push(format!(
                "extracted embedded OBJ visual asset at byte range {}..{}",
                range.start, range.end
            ));
            return Ok(Some(document));
        }
        if let Some(range) = find_embedded_glb(bytes) {
            let mut document = import_glb_document(
                &bytes[range.start..range.end],
                source_format,
                mode,
                source_path,
            )?;
            document.metadata.warnings.push(format!(
                "extracted embedded GLB visual asset at byte range {}..{}",
                range.start, range.end
            ));
            return Ok(Some(document));
        }
        return Ok(None);
    };

    let mut document = import_binary_stl_document(
        &bytes[range.start..range.end],
        source_format,
        mode,
        source_path,
    )?;
    document.metadata.warnings.push(format!(
        "extracted embedded binary STL visual asset at byte range {}..{}",
        range.start, range.end
    ));
    Ok(Some(document))
}

fn import_ole_stream_visual_assets(
    bytes: &[u8],
    source_format: &str,
    source_path: Option<&Path>,
    mode: &str,
    limits: &ImportLimits,
) -> Result<Option<LiteDocument>, ImportError> {
    let streams = extract_ole_streams(bytes, limits)?;
    let mut documents = Vec::new();

    for stream in streams {
        if let Some(mut document) = import_embedded_visual_assets_inner(
            &stream.payload,
            source_format,
            source_path,
            mode,
            limits,
            false,
        )? {
            document
                .metadata
                .warnings
                .push(format!("extracted OLE stream `{}`", stream.name));
            documents.push((stream.name, document));
        }
    }

    if documents.len() == 1 {
        return Ok(documents.into_iter().next().map(|(_, document)| document));
    }
    if documents.is_empty() {
        return Ok(None);
    }

    let mut document = LiteDocument::new(source_format, mode);
    if let Some(path) = source_path {
        document.metadata.source_path = Some(path.display().to_string());
    }
    document.metadata.warnings.push(format!(
        "merged {} OLE visual streams into one scene",
        documents.len()
    ));

    for (stream_name, stream_document) in documents {
        document.append_document_under_node(stream_name, stream_document);
    }
    document.refresh_metadata();
    Ok(Some(document))
}

/// Backward-compatible wrapper for callers that expect one imported document.
pub fn import_first_embedded_visual_asset(
    bytes: &[u8],
    source_format: &str,
    source_path: Option<&Path>,
    mode: &str,
    limits: &ImportLimits,
) -> Result<Option<LiteDocument>, ImportError> {
    import_embedded_visual_assets(bytes, source_format, source_path, mode, limits)
}

fn classify_exact_payload(payload: &[u8]) -> Result<Option<EmbeddedVisualAssetKind>, ImportError> {
    if let Some(cache) = extract_cache_payload(payload)?
        && cache.start == 0
        && cache.end == payload.len()
    {
        return Ok(Some(EmbeddedVisualAssetKind::FeatherCache));
    }
    if is_exact_binary_stl(payload) {
        return Ok(Some(EmbeddedVisualAssetKind::BinaryStl));
    }
    if is_ascii_stl(payload) {
        return Ok(Some(EmbeddedVisualAssetKind::AsciiStl));
    }
    if is_obj(payload) {
        return Ok(Some(EmbeddedVisualAssetKind::Obj));
    }
    if is_gltf_json(payload) {
        return Ok(Some(EmbeddedVisualAssetKind::Gltf));
    }
    if is_exact_glb(payload) {
        return Ok(Some(EmbeddedVisualAssetKind::Glb));
    }
    if is_3dxml_rep(payload) {
        return Ok(Some(EmbeddedVisualAssetKind::ThreeDxmlRep));
    }
    Ok(None)
}

fn classify_zip_payload(
    entry_name: &str,
    payload: &[u8],
    gltf_names: &[String],
) -> Result<Option<EmbeddedVisualAssetKind>, ImportError> {
    if is_gltf_buffer_entry(entry_name, gltf_names) {
        return Ok(Some(EmbeddedVisualAssetKind::GltfBinaryBuffer));
    }
    classify_exact_payload(payload)
}

fn decoded_zip_payloads(
    bytes: &[u8],
    limits: &ImportLimits,
) -> Result<Vec<(crate::assets::zip::ZipEntry, Vec<u8>)>, ImportError> {
    let mut payloads = Vec::new();
    for entry in zip_entries(bytes, limits)? {
        let Some(payload) = entry.decoded_payload(bytes, limits)? else {
            continue;
        };
        payloads.push((entry, payload.into_owned()));
    }
    Ok(payloads)
}

fn gltf_entry_names(zip_assets: &[(crate::assets::zip::ZipEntry, Vec<u8>)]) -> Vec<String> {
    zip_assets
        .iter()
        .filter_map(|(entry, payload)| is_gltf_json(payload).then_some(entry.name.clone()))
        .collect()
}

fn is_gltf_buffer_entry(entry_name: &str, gltf_names: &[String]) -> bool {
    if !entry_name
        .rsplit_once('.')
        .map(|(_, extension)| extension.eq_ignore_ascii_case("bin"))
        .unwrap_or(false)
    {
        return false;
    }

    gltf_names
        .iter()
        .any(|gltf_name| same_directory(entry_name, gltf_name))
}

fn same_directory(left: &str, right: &str) -> bool {
    let left_dir = left.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let right_dir = right.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    left_dir == right_dir
}

fn push_unique_asset(assets: &mut Vec<EmbeddedVisualAsset>, asset: EmbeddedVisualAsset) {
    let exists = assets.iter().any(|existing| {
        let same_range = existing.kind == asset.kind
            && existing.byte_start == asset.byte_start
            && existing.byte_end == asset.byte_end;
        let both_top_level = existing.source != EmbeddedVisualAssetSource::OleStream
            && asset.source != EmbeddedVisualAssetSource::OleStream;
        same_range
            && (both_top_level || (existing.source == asset.source && existing.name == asset.name))
    });
    if !exists {
        assets.push(asset);
    }
}

fn discover_ole_stream_assets(
    assets: &mut Vec<EmbeddedVisualAsset>,
    stream_name: &str,
    bytes: &[u8],
    limits: &ImportLimits,
) -> Result<(), ImportError> {
    if let Some(cache) = extract_cache_payload(bytes)? {
        push_unique_asset(
            assets,
            EmbeddedVisualAsset::new(
                EmbeddedVisualAssetKind::FeatherCache,
                EmbeddedVisualAssetSource::OleStream,
                cache.start,
                cache.end,
                Some(stream_name.to_string()),
                &bytes[cache.start..cache.end],
            ),
        );
    }

    let zip_assets = decoded_zip_payloads(bytes, limits)?;
    let zip_gltf_names = gltf_entry_names(&zip_assets);
    for (entry, payload) in &zip_assets {
        let Some(kind) = classify_zip_payload(&entry.name, payload, &zip_gltf_names)? else {
            continue;
        };
        push_unique_asset(
            assets,
            EmbeddedVisualAsset::new(
                kind,
                EmbeddedVisualAssetSource::OleStream,
                entry.data_start,
                entry.data_end,
                Some(format!("{stream_name}:{}", entry.name)),
                payload,
            ),
        );
    }

    if let Some(kind) = classify_exact_payload(bytes)? {
        push_unique_asset(
            assets,
            EmbeddedVisualAsset::new(
                kind,
                EmbeddedVisualAssetSource::OleStream,
                0,
                bytes.len(),
                Some(stream_name.to_string()),
                bytes,
            ),
        );
    }
    if let Some(range) = find_embedded_binary_stl(bytes) {
        push_unique_asset(
            assets,
            EmbeddedVisualAsset::new(
                EmbeddedVisualAssetKind::BinaryStl,
                EmbeddedVisualAssetSource::OleStream,
                range.start,
                range.end,
                Some(stream_name.to_string()),
                &bytes[range.start..range.end],
            ),
        );
    }
    if let Some(range) = find_embedded_ascii_stl(bytes) {
        push_unique_asset(
            assets,
            EmbeddedVisualAsset::new(
                EmbeddedVisualAssetKind::AsciiStl,
                EmbeddedVisualAssetSource::OleStream,
                range.start,
                range.end,
                Some(stream_name.to_string()),
                &bytes[range.start..range.end],
            ),
        );
    }
    if let Some(range) = find_embedded_obj(bytes) {
        push_unique_asset(
            assets,
            EmbeddedVisualAsset::new(
                EmbeddedVisualAssetKind::Obj,
                EmbeddedVisualAssetSource::OleStream,
                range.start,
                range.end,
                Some(stream_name.to_string()),
                &bytes[range.start..range.end],
            ),
        );
    }
    if let Some(range) = find_embedded_glb(bytes) {
        push_unique_asset(
            assets,
            EmbeddedVisualAsset::new(
                EmbeddedVisualAssetKind::Glb,
                EmbeddedVisualAssetSource::OleStream,
                range.start,
                range.end,
                Some(stream_name.to_string()),
                &bytes[range.start..range.end],
            ),
        );
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ByteRange {
    start: usize,
    end: usize,
}

fn find_embedded_binary_stl(bytes: &[u8]) -> Option<ByteRange> {
    if bytes.len() < 84 {
        return None;
    }

    let mut best = None::<ByteRange>;
    let max_start = bytes.len().saturating_sub(84);
    for start in 0..=max_start {
        let header = &bytes[start..start + 80];
        if !looks_like_embedded_stl_header(header, bytes, start) {
            continue;
        }

        let Some(length) = binary_stl_len(&bytes[start..]) else {
            continue;
        };
        let end = start.checked_add(length)?;
        if end > bytes.len() {
            continue;
        }
        if !is_exact_binary_stl(&bytes[start..end]) {
            continue;
        }

        let candidate = ByteRange { start, end };
        let is_better = best
            .map(|current| candidate.end - candidate.start > current.end - current.start)
            .unwrap_or(true);
        if is_better {
            best = Some(candidate);
        }
    }

    best
}

fn find_embedded_ascii_stl(bytes: &[u8]) -> Option<ByteRange> {
    let lower = bytes
        .iter()
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mut cursor = 0;

    while cursor < lower.len() {
        let relative_start = find_ascii_bytes(&lower[cursor..], b"solid")?;
        let start = cursor + relative_start;
        let relative_end = find_ascii_bytes(&lower[start..], b"endsolid")?;
        let mut end = start + relative_end + b"endsolid".len();
        while end < bytes.len() && !matches!(bytes[end], b'\r' | b'\n' | 0) {
            end += 1;
        }

        if end <= bytes.len() && is_ascii_stl(&bytes[start..end]) {
            return Some(ByteRange { start, end });
        }
        cursor = start + b"solid".len();
    }

    None
}

fn find_embedded_obj(bytes: &[u8]) -> Option<ByteRange> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut current_start = None::<usize>;
    let mut current_end = None::<usize>;
    let mut has_vertex = false;
    let mut has_face = false;
    let mut cursor = 0;

    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let is_obj_line = trimmed.starts_with("# Wavefront")
            || trimmed.starts_with("# OBJ")
            || trimmed.starts_with("o ")
            || trimmed.starts_with("g ")
            || trimmed.starts_with("v ")
            || trimmed.starts_with("vn ")
            || trimmed.starts_with("vt ")
            || trimmed.starts_with("f ");

        if is_obj_line {
            if current_start.is_none() {
                current_start = Some(cursor);
            }
            current_end = Some(cursor + line.len());
            has_vertex |= trimmed.starts_with("v ");
            has_face |= trimmed.starts_with("f ");
        } else if current_start.is_some() && !trimmed.is_empty() {
            if has_vertex && has_face {
                break;
            }
            current_start = None;
            current_end = None;
            has_vertex = false;
            has_face = false;
        }

        cursor += line.len();
    }

    let start = current_start?;
    let end = current_end?;
    if has_vertex && has_face && is_obj(&bytes[start..end]) {
        Some(ByteRange { start, end })
    } else {
        None
    }
}

fn find_embedded_glb(bytes: &[u8]) -> Option<ByteRange> {
    let magic = b"glTF";
    let mut cursor = 0;
    while cursor + 12 <= bytes.len() {
        let relative_start = bytes[cursor..]
            .windows(magic.len())
            .position(|window| window == magic)?;
        let start = cursor + relative_start;
        if let Some(length) = glb_len(&bytes[start..]) {
            return Some(ByteRange {
                start,
                end: start + length,
            });
        }
        cursor = start + magic.len();
    }
    None
}

fn find_ascii_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn looks_like_embedded_stl_header(header: &[u8], bytes: &[u8], start: usize) -> bool {
    // Binary STL has no magic value. Scanning an arbitrary CAD byte stream is
    // therefore only safe when an explicit STL marker frames the candidate.
    contains_ascii(header, b"stl") || nearby_marker(bytes, start, b"STL")
}

fn nearby_marker(bytes: &[u8], start: usize, marker: &[u8]) -> bool {
    let context_start = start.saturating_sub(64);
    contains_ascii(&bytes[context_start..start], marker)
}

fn contains_ascii(bytes: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || bytes.len() < needle.len() {
        return false;
    }

    bytes
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}
