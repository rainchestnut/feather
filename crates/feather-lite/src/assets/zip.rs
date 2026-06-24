//! ZIP entry scanner for embedded visual assets.
//!
//! This reader scans local file headers embedded inside larger CAD containers
//! and imports method 0 (stored) and method 8 (deflated) entries when their
//! sizes are available in local headers, central directories, or ZIP64 extra
//! fields.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use miniz_oxide::inflate::decompress_to_vec_with_limit;

use crate::assets::glb::{
    GltfExternalBuffer, import_glb_document, import_gltf_document, is_exact_glb, is_gltf_json,
};
use crate::assets::obj::{
    ObjMaterial, import_obj_document, import_obj_document_with_materials, is_obj,
    parse_mtl_materials,
};
use crate::assets::stl::{
    import_ascii_stl_document, import_binary_stl_document, is_ascii_stl, is_exact_binary_stl,
};
use crate::assets::three_dxml_rep::{import_3dxml_rep_document, is_3dxml_rep};
use crate::cache::{decode_cache_text, extract_cache_text};
use crate::document::{LiteDocument, LiteNode, Transform, identity_transform};
use crate::importer::{ImportError, ImportLimits};

const LOCAL_FILE_HEADER_SIGNATURE: u32 = 0x0403_4B50;
const CENTRAL_DIRECTORY_HEADER_SIGNATURE: u32 = 0x0201_4B50;
const LOCAL_FILE_HEADER_LEN: usize = 30;
const CENTRAL_DIRECTORY_HEADER_LEN: usize = 46;
const ZIP64_EXTRA_FIELD_ID: u16 = 0x0001;
const ZIP64_U32_SENTINEL: u32 = u32::MAX;
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATED: u16 = 8;
const FLAG_DATA_DESCRIPTOR: u16 = 0x0008;
const ZIP_ASSEMBLY_REFERENCE_ATTRIBUTES: &[&str] = &[
    "associatedfile",
    "externalfile",
    "href",
    "source",
    "src",
    "file",
    "filepath",
    "filename",
    "path",
    "uri",
    "url",
    "reference",
    "ref",
];

/// One local ZIP entry discovered inside a larger CAD container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ZipEntry {
    pub(crate) name: String,
    flags: u16,
    method: u16,
    uncompressed_size: usize,
    pub(crate) data_start: usize,
    pub(crate) data_end: usize,
}

impl ZipEntry {
    /// Returns true when the entry uses a supported payload method.
    fn is_supported_payload(&self) -> bool {
        matches!(self.method, METHOD_STORED | METHOD_DEFLATED)
    }

    /// Returns a stable method label for warnings and manifests.
    fn method_label(&self) -> &'static str {
        match self.method {
            METHOD_STORED => "stored",
            METHOD_DEFLATED => "deflated",
            _ => "unsupported",
        }
    }

    /// Returns the decoded payload for supported entries.
    pub(crate) fn decoded_payload<'a>(
        &self,
        bytes: &'a [u8],
        limits: &ImportLimits,
    ) -> Result<Option<Cow<'a, [u8]>>, ImportError> {
        if !self.is_supported_payload() {
            return Ok(None);
        }
        if self.uncompressed_size > limits.max_archive_entry_uncompressed_bytes {
            return Err(resource_limit_exceeded(
                "ZIP entry uncompressed bytes",
                limits.max_archive_entry_uncompressed_bytes,
                self.uncompressed_size,
            ));
        }

        let Some(payload) = bytes.get(self.data_start..self.data_end) else {
            return Ok(None);
        };

        match self.method {
            METHOD_STORED => Ok(Some(Cow::Borrowed(payload))),
            METHOD_DEFLATED => {
                let decompressed = decompress_to_vec_with_limit(payload, self.uncompressed_size)
                    .map_err(|error| {
                        ImportError::InvalidData(format!(
                            "ZIP deflated entry `{}` failed to decompress: {error}",
                            self.name
                        ))
                    })?;
                if decompressed.len() != self.uncompressed_size {
                    return Err(ImportError::InvalidData(format!(
                        "ZIP deflated entry `{}` decompressed to {} bytes, expected {}",
                        self.name,
                        decompressed.len(),
                        self.uncompressed_size
                    )));
                }
                Ok(Some(Cow::Owned(decompressed)))
            }
            _ => Ok(None),
        }
    }
}

/// Imported visual document from one ZIP entry.
#[derive(Debug, Clone)]
struct ZipAsset {
    entry: ZipEntry,
    document: LiteDocument,
}

/// Lists local ZIP entries found by scanning the byte stream.
pub(crate) fn zip_entries(
    bytes: &[u8],
    limits: &ImportLimits,
) -> Result<Vec<ZipEntry>, ImportError> {
    let local_headers = local_zip_headers(bytes, limits.max_archive_entries)?;
    let mut entries = Vec::new();

    for header in &local_headers {
        if let Some(entry) = header.to_sized_entry(bytes) {
            push_unique_entry(&mut entries, entry, limits.max_archive_entries)?;
        }
    }

    for central_entry in central_directory_entries(bytes, limits.max_archive_entries)? {
        if let Some(entry) = central_entry.to_zip_entry(bytes, &local_headers) {
            push_unique_entry(&mut entries, entry, limits.max_archive_entries)?;
        }
    }

    validate_zip_entry_sizes(&entries, limits)?;
    entries.sort_by_key(|entry| (entry.data_start, entry.data_end, entry.name.clone()));
    Ok(entries)
}

/// Imports a ZIP-backed lightweight document.
///
/// When a ZIP XML assembly manifest references visual entries in the same
/// container, the manifest hierarchy is used. Otherwise every importable visual
/// ZIP entry is merged under one grouping node per entry.
pub(crate) fn import_zip_document(
    bytes: &[u8],
    source_format: &str,
    source_path: Option<&Path>,
    mode: &str,
    limits: &ImportLimits,
) -> Result<Option<LiteDocument>, ImportError> {
    let decoded_entries = decoded_zip_entries(bytes, limits)?;
    let zip_assets = import_decoded_zip_assets(&decoded_entries, source_format, source_path, mode)?;

    if let Some(document) = import_decoded_zip_assembly_document(
        &decoded_entries,
        &zip_assets,
        source_format,
        source_path,
        mode,
    )? {
        return Ok(Some(document));
    }

    merge_zip_assets(zip_assets, source_format, source_path, mode)
}

fn import_decoded_zip_assets(
    decoded_entries: &[DecodedZipEntry],
    source_format: &str,
    source_path: Option<&Path>,
    mode: &str,
) -> Result<Vec<ZipAsset>, ImportError> {
    let mut assets = Vec::new();
    let materials = zip_materials(decoded_entries)?;
    for decoded in decoded_entries {
        let imported = if decoded.name_has_extension("gltf") && is_gltf_json(&decoded.payload) {
            let buffers = zip_gltf_external_buffers(decoded_entries, &decoded.entry.name);
            import_gltf_document(&decoded.payload, &buffers, source_format, mode, source_path)
                .map(Some)?
        } else {
            import_zip_entry_payload(
                &decoded.payload,
                &materials,
                source_format,
                source_path,
                mode,
            )?
        };

        if let Some(mut document) = imported {
            document.metadata.warnings.push(format!(
                "extracted ZIP {} entry `{}` at byte range {}..{}",
                decoded.entry.method_label(),
                decoded.entry.name,
                decoded.entry.data_start,
                decoded.entry.data_end
            ));
            assets.push(ZipAsset {
                entry: decoded.entry.clone(),
                document,
            });
        }
    }

    Ok(assets)
}

fn merge_zip_assets(
    zip_assets: Vec<ZipAsset>,
    source_format: &str,
    source_path: Option<&Path>,
    mode: &str,
) -> Result<Option<LiteDocument>, ImportError> {
    if zip_assets.len() == 1 {
        return Ok(zip_assets.into_iter().next().map(|asset| asset.document));
    }
    if zip_assets.is_empty() {
        return Ok(None);
    }

    let mut document = LiteDocument::new(source_format, mode);
    if let Some(path) = source_path {
        document.metadata.source_path = Some(path.display().to_string());
    }
    document.metadata.warnings.push(format!(
        "merged {} ZIP visual assets into one scene",
        zip_assets.len()
    ));

    for (asset_index, asset) in zip_assets.into_iter().enumerate() {
        let group_name = if asset.entry.name.is_empty() {
            format!("zip-entry-{asset_index}")
        } else {
            asset.entry.name
        };
        document.append_document_under_node(group_name, asset.document);
    }

    document.refresh_metadata();
    Ok(Some(document))
}

fn import_decoded_zip_assembly_document(
    decoded_entries: &[DecodedZipEntry],
    zip_assets: &[ZipAsset],
    source_format: &str,
    source_path: Option<&Path>,
    mode: &str,
) -> Result<Option<LiteDocument>, ImportError> {
    let visual_names = zip_assets
        .iter()
        .map(|asset| asset.entry.name.as_str())
        .collect::<Vec<_>>();

    for entry in decoded_entries {
        if !entry.looks_like_xml_manifest() {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&entry.payload) else {
            continue;
        };
        let assembly =
            if let Some(assembly) = parse_3dxml_product_structure_xml(text, &visual_names)? {
                Some(assembly)
            } else {
                parse_zip_assembly_xml(text, &visual_names)?
            };
        let Some(assembly) = assembly else {
            continue;
        };
        let mut document = LiteDocument::new(source_format, mode);
        if let Some(path) = source_path {
            document.metadata.source_path = Some(path.display().to_string());
        }
        let resolution = append_zip_assembly_nodes(&mut document, &assembly, None, zip_assets);
        if resolution.zip_visual_refs == 0 && resolution.external_refs == 0 {
            continue;
        }
        let unreferenced = zip_assets.len().saturating_sub(resolution.zip_visual_refs);
        if resolution.external_refs == 0 {
            document.metadata.warnings.push(format!(
                "applied ZIP assembly manifest `{}` with {} visual references",
                entry.entry.name, resolution.zip_visual_refs
            ));
        } else {
            document.metadata.warnings.push(format!(
                "applied ZIP assembly manifest `{}` with {} visual references and {} external references",
                entry.entry.name, resolution.zip_visual_refs, resolution.external_refs
            ));
        }
        if unreferenced > 0 {
            document.metadata.warnings.push(format!(
                "{unreferenced} ZIP visual assets were not referenced by assembly manifest `{}`",
                entry.entry.name
            ));
        }
        document.refresh_metadata();
        return Ok(Some(document));
    }

    Ok(None)
}

fn import_zip_entry_payload(
    payload: &[u8],
    materials: &[ObjMaterial],
    source_format: &str,
    source_path: Option<&Path>,
    mode: &str,
) -> Result<Option<LiteDocument>, ImportError> {
    if let Some(text) = extract_cache_text(payload)? {
        let mut document = decode_cache_text(&text, source_format, source_path)?;
        document.metadata.mode = mode.to_string();
        document.refresh_metadata();
        return Ok(Some(document));
    }

    if is_exact_binary_stl(payload) {
        return import_binary_stl_document(payload, source_format, mode, source_path).map(Some);
    }
    if is_ascii_stl(payload) {
        return import_ascii_stl_document(payload, source_format, mode, source_path).map(Some);
    }
    if is_obj(payload) {
        if materials.is_empty() {
            return import_obj_document(payload, source_format, mode, source_path).map(Some);
        }
        return import_obj_document_with_materials(
            payload,
            materials,
            source_format,
            mode,
            source_path,
        )
        .map(Some);
    }
    if is_3dxml_rep(payload) {
        return import_3dxml_rep_document(payload, source_format, mode, source_path).map(Some);
    }
    if is_exact_glb(payload) {
        return import_glb_document(payload, source_format, mode, source_path).map(Some);
    }

    Ok(None)
}

struct DecodedZipEntry {
    entry: ZipEntry,
    payload: Vec<u8>,
}

impl DecodedZipEntry {
    fn name_has_extension(&self, extension: &str) -> bool {
        self.entry
            .name
            .rsplit_once('.')
            .map(|(_, candidate)| candidate.eq_ignore_ascii_case(extension))
            .unwrap_or(false)
    }

    fn looks_like_xml_manifest(&self) -> bool {
        self.name_has_extension("xml")
            || self.name_has_extension("3dxml")
            || std::str::from_utf8(&self.payload)
                .map(|text| text.trim_start().starts_with('<'))
                .unwrap_or(false)
    }
}

fn decoded_zip_entries(
    bytes: &[u8],
    limits: &ImportLimits,
) -> Result<Vec<DecodedZipEntry>, ImportError> {
    let mut decoded = Vec::new();
    for entry in zip_entries(bytes, limits)? {
        let Some(payload) = entry.decoded_payload(bytes, limits)? else {
            continue;
        };
        decoded.push(DecodedZipEntry {
            entry,
            payload: payload.into_owned(),
        });
    }
    Ok(decoded)
}

fn zip_materials(entries: &[DecodedZipEntry]) -> Result<Vec<ObjMaterial>, ImportError> {
    let mut materials = Vec::new();
    for entry in entries {
        if entry.name_has_extension("mtl") {
            materials.extend(parse_mtl_materials(&entry.payload)?);
        }
    }
    Ok(materials)
}

fn zip_gltf_external_buffers<'a>(
    entries: &'a [DecodedZipEntry],
    gltf_name: &str,
) -> Vec<GltfExternalBuffer<'a>> {
    let mut buffers = Vec::new();
    let gltf_dir = gltf_name
        .rsplit_once('/')
        .map(|(directory, _)| format!("{directory}/"));

    for entry in entries {
        if entry.name_has_extension("gltf") {
            continue;
        }

        push_gltf_buffer_alias(&mut buffers, &entry.entry.name, &entry.payload);
        if let Some(directory) = &gltf_dir
            && let Some(relative_name) = entry.entry.name.strip_prefix(directory)
        {
            push_gltf_buffer_alias(&mut buffers, relative_name, &entry.payload);
        }
        if let Some((_, basename)) = entry.entry.name.rsplit_once('/') {
            push_gltf_buffer_alias(&mut buffers, basename, &entry.payload);
        }
    }

    buffers
}

fn push_gltf_buffer_alias<'a>(
    buffers: &mut Vec<GltfExternalBuffer<'a>>,
    uri: &'a str,
    bytes: &'a [u8],
) {
    if !buffers.iter().any(|buffer| buffer.uri == uri) {
        buffers.push(GltfExternalBuffer { uri, bytes });
    }
}

#[derive(Debug, Clone)]
struct ZipAssemblyNode {
    name: String,
    href: Option<String>,
    transform: Transform,
    children: Vec<ZipAssemblyNode>,
}

impl ZipAssemblyNode {
    fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            href: None,
            transform: identity_transform(),
            children: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct PendingZipAssemblyNode {
    tag_name: String,
    node: ZipAssemblyNode,
}

/// Reference counts collected while materializing a ZIP/XML assembly tree.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ZipAssemblyResolution {
    zip_visual_refs: usize,
    external_refs: usize,
}

impl ZipAssemblyResolution {
    fn add(&mut self, other: ZipAssemblyResolution) {
        self.zip_visual_refs += other.zip_visual_refs;
        self.external_refs += other.external_refs;
    }
}

#[derive(Debug)]
struct XmlStartTag {
    name: String,
    attributes: Vec<(String, String)>,
    self_closing: bool,
}

#[derive(Debug)]
struct XmlElement {
    attributes: Vec<(String, String)>,
    body: String,
}

#[derive(Debug, Clone)]
struct ThreeDxmlReference3d {
    name: String,
}

#[derive(Debug, Clone)]
struct ThreeDxmlInstance3d {
    name: String,
    aggregated_by: Option<String>,
    instance_of: Option<String>,
    transform: Transform,
}

#[derive(Debug, Clone)]
struct ThreeDxmlReferenceRep {
    name: String,
    href: String,
}

#[derive(Debug, Clone)]
struct ThreeDxmlInstanceRep {
    name: String,
    aggregated_by: Option<String>,
    instance_of: Option<String>,
    transform: Transform,
}

fn parse_3dxml_product_structure_xml(
    text: &str,
    visual_names: &[&str],
) -> Result<Option<ZipAssemblyNode>, ImportError> {
    let references = xml_elements_by_local_names(text, &["reference3d"])
        .into_iter()
        .filter_map(|element| {
            let id = three_dxml_id(&element.attributes)?;
            let name = zip_assembly_node_name(&element.attributes).unwrap_or_else(|| id.clone());
            Some((id, ThreeDxmlReference3d { name }))
        })
        .collect::<HashMap<_, _>>();
    if references.is_empty() {
        return Ok(None);
    }

    let reference_reps = xml_elements_by_local_names(text, &["referencerep"])
        .into_iter()
        .filter_map(|element| {
            let id = three_dxml_id(&element.attributes)?;
            let href = three_dxml_href(&element, visual_names)?;
            let name = zip_assembly_node_name(&element.attributes)
                .unwrap_or_else(|| zip_assembly_basename(&href));
            Some((id, ThreeDxmlReferenceRep { name, href }))
        })
        .collect::<HashMap<_, _>>();
    if reference_reps.is_empty() {
        return Ok(None);
    }

    let instance3ds = xml_elements_by_local_names(text, &["instance3d"])
        .into_iter()
        .map(|element| {
            Ok(ThreeDxmlInstance3d {
                name: zip_assembly_node_name(&element.attributes)
                    .unwrap_or_else(|| three_dxml_id(&element.attributes).unwrap_or_default()),
                aggregated_by: three_dxml_relation(&element, &["isaggregatedby", "aggregatedby"]),
                instance_of: three_dxml_relation(&element, &["isinstanceof", "instanceof"]),
                transform: three_dxml_element_transform(&element)?,
            })
        })
        .collect::<Result<Vec<_>, ImportError>>()?;
    let instance_reps = xml_elements_by_local_names(text, &["instancerep"])
        .into_iter()
        .map(|element| {
            Ok(ThreeDxmlInstanceRep {
                name: zip_assembly_node_name(&element.attributes)
                    .unwrap_or_else(|| three_dxml_id(&element.attributes).unwrap_or_default()),
                aggregated_by: three_dxml_relation(&element, &["isaggregatedby", "aggregatedby"]),
                instance_of: three_dxml_relation(&element, &["isinstanceof", "instanceof"]),
                transform: three_dxml_element_transform(&element)?,
            })
        })
        .collect::<Result<Vec<_>, ImportError>>()?;
    if instance3ds.is_empty() && instance_reps.is_empty() {
        return Ok(None);
    }

    let instanced_references = instance3ds
        .iter()
        .filter_map(|instance| instance.instance_of.as_deref())
        .collect::<HashSet<_>>();
    let mut root_ids = references
        .keys()
        .filter(|id| !instanced_references.contains(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    root_ids.sort();

    let mut roots = root_ids
        .iter()
        .filter_map(|id| {
            build_3dxml_reference_node(
                id,
                &references,
                &instance3ds,
                &reference_reps,
                &instance_reps,
                &mut HashSet::new(),
            )
        })
        .collect::<Vec<_>>();
    if roots.is_empty() {
        let mut all_reference_ids = references.keys().cloned().collect::<Vec<_>>();
        all_reference_ids.sort();
        roots = all_reference_ids
            .iter()
            .filter_map(|id| {
                build_3dxml_reference_node(
                    id,
                    &references,
                    &instance3ds,
                    &reference_reps,
                    &instance_reps,
                    &mut HashSet::new(),
                )
            })
            .collect();
    }

    if roots.is_empty() {
        return Ok(None);
    }
    let root = if roots.len() == 1 {
        roots.remove(0)
    } else {
        let mut root = ZipAssemblyNode::new("3DXMLProductStructure");
        root.children = roots;
        root
    };
    if count_zip_assembly_refs(&root) == 0 {
        return Ok(None);
    }
    Ok(Some(root))
}

fn build_3dxml_reference_node(
    reference_id: &str,
    references: &HashMap<String, ThreeDxmlReference3d>,
    instance3ds: &[ThreeDxmlInstance3d],
    reference_reps: &HashMap<String, ThreeDxmlReferenceRep>,
    instance_reps: &[ThreeDxmlInstanceRep],
    visited: &mut HashSet<String>,
) -> Option<ZipAssemblyNode> {
    if !visited.insert(reference_id.to_string()) {
        return None;
    }
    let Some(reference) = references.get(reference_id) else {
        visited.remove(reference_id);
        return None;
    };
    let mut node = ZipAssemblyNode::new(&reference.name);

    for instance in instance3ds
        .iter()
        .filter(|instance| instance.aggregated_by.as_deref() == Some(reference_id))
    {
        let fallback_name = instance
            .instance_of
            .as_deref()
            .and_then(|id| references.get(id))
            .map(|reference| reference.name.as_str())
            .unwrap_or("Instance3D");
        let mut instance_node = ZipAssemblyNode::new(non_empty_name(&instance.name, fallback_name));
        instance_node.transform = instance.transform;
        if let Some(child_reference_id) = &instance.instance_of
            && let Some(child_reference) = build_3dxml_reference_node(
                child_reference_id,
                references,
                instance3ds,
                reference_reps,
                instance_reps,
                visited,
            )
        {
            instance_node.children = child_reference.children;
        }
        if !instance_node.children.is_empty() {
            node.children.push(instance_node);
        }
    }

    for instance in instance_reps
        .iter()
        .filter(|instance| instance.aggregated_by.as_deref() == Some(reference_id))
    {
        let Some(reference_rep) = instance
            .instance_of
            .as_deref()
            .and_then(|id| reference_reps.get(id))
        else {
            continue;
        };
        let mut asset_node =
            ZipAssemblyNode::new(non_empty_name(&instance.name, &reference_rep.name));
        asset_node.href = Some(reference_rep.href.clone());
        asset_node.transform = instance.transform;
        node.children.push(asset_node);
    }

    visited.remove(reference_id);
    (!node.children.is_empty()).then_some(node)
}

fn non_empty_name<'a>(name: &'a str, fallback: &'a str) -> &'a str {
    if name.trim().is_empty() {
        fallback
    } else {
        name
    }
}

fn three_dxml_id(attributes: &[(String, String)]) -> Option<String> {
    find_attribute(attributes, &["id"])
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn three_dxml_href(element: &XmlElement, visual_names: &[&str]) -> Option<String> {
    zip_assembly_href(&element.attributes, visual_names).or_else(|| {
        xml_child_text_with_name(&element.body, ZIP_ASSEMBLY_REFERENCE_ATTRIBUTES)
            .and_then(|(_, value)| zip_assembly_reference_from_value(&value, visual_names))
    })
}

fn three_dxml_relation(element: &XmlElement, names: &[&str]) -> Option<String> {
    find_attribute(&element.attributes, names)
        .map(ToOwned::to_owned)
        .or_else(|| xml_child_text(&element.body, names))
        .filter(|value| !value.trim().is_empty())
}

fn three_dxml_element_transform(element: &XmlElement) -> Result<Transform, ImportError> {
    let mut transform = zip_assembly_transform(&element.attributes)?;
    if let Some((tag_name, value)) = xml_child_text_with_name(
        &element.body,
        &[
            "relativematrix",
            "matrix",
            "transform",
            "transformation",
            "translation",
            "position",
        ],
    ) && let Some(text_transform) = zip_assembly_transform_from_text(&tag_name, &value)?
    {
        transform = text_transform;
    }
    Ok(transform)
}

fn parse_zip_assembly_xml(
    text: &str,
    visual_names: &[&str],
) -> Result<Option<ZipAssemblyNode>, ImportError> {
    let mut cursor = 0;
    let mut stack = Vec::<PendingZipAssemblyNode>::new();
    let mut roots = Vec::<ZipAssemblyNode>::new();
    let mut text_transform_tag = None::<String>;

    while let Some(relative_start) = text[cursor..].find('<') {
        let start = cursor + relative_start;
        let text_segment = text[cursor..start].trim();
        if !text_segment.is_empty()
            && let Some(tag_name) = text_transform_tag.as_deref()
        {
            apply_zip_assembly_text_transform(&mut stack, tag_name, text_segment)?;
        }

        let Some(relative_end) = text[start..].find('>') else {
            break;
        };
        let end = start + relative_end;
        let content = text[start + 1..end].trim();
        cursor = end + 1;

        if content.is_empty()
            || content.starts_with('?')
            || content.starts_with('!')
            || content.starts_with("!--")
        {
            continue;
        }

        if let Some(close_name) = content.strip_prefix('/') {
            let close_name = xml_local_name(close_name.trim());
            if text_transform_tag
                .as_deref()
                .is_some_and(|tag_name| tag_name == close_name)
            {
                text_transform_tag = None;
                continue;
            }
            if stack
                .last()
                .map(|pending| pending.tag_name == close_name)
                .unwrap_or(false)
            {
                let node = stack.pop().expect("checked stack entry").node;
                push_completed_zip_assembly_node(&mut stack, &mut roots, node);
            }
            continue;
        }

        let Some(tag) = parse_xml_start_tag(content) else {
            continue;
        };
        let tag_name = xml_local_name(&tag.name);
        let Some(node) = zip_assembly_node_from_tag(&tag, visual_names)? else {
            if !tag.self_closing && is_zip_assembly_text_transform_tag(&tag_name) {
                text_transform_tag = Some(tag_name);
            }
            continue;
        };

        if tag.self_closing {
            push_completed_zip_assembly_node(&mut stack, &mut roots, node);
        } else {
            stack.push(PendingZipAssemblyNode { tag_name, node });
        }
    }

    while let Some(pending) = stack.pop() {
        push_completed_zip_assembly_node(&mut stack, &mut roots, pending.node);
    }

    let mut roots = roots
        .into_iter()
        .filter(|node| node.href.is_some() || !node.children.is_empty())
        .collect::<Vec<_>>();

    if roots.is_empty() {
        return Ok(None);
    }

    let root = if roots.len() == 1 {
        roots.remove(0)
    } else {
        let mut root = ZipAssemblyNode::new("ZIPAssembly");
        root.children = roots;
        root
    };

    if count_zip_assembly_refs(&root) == 0 {
        return Ok(None);
    }

    Ok(Some(root))
}

fn push_completed_zip_assembly_node(
    stack: &mut [PendingZipAssemblyNode],
    roots: &mut Vec<ZipAssemblyNode>,
    node: ZipAssemblyNode,
) {
    if let Some(parent) = stack.last_mut() {
        parent.node.children.push(node);
    } else {
        roots.push(node);
    }
}

fn apply_zip_assembly_text_transform(
    stack: &mut [PendingZipAssemblyNode],
    tag_name: &str,
    value: &str,
) -> Result<(), ImportError> {
    let Some(parent) = stack.last_mut() else {
        return Ok(());
    };
    if let Some(transform) = zip_assembly_transform_from_text(tag_name, value)? {
        parent.node.transform = transform;
    }
    Ok(())
}

fn is_zip_assembly_text_transform_tag(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "relativematrix" | "matrix" | "transform" | "transformation" | "translation" | "position"
    )
}

fn zip_assembly_node_from_tag(
    tag: &XmlStartTag,
    visual_names: &[&str],
) -> Result<Option<ZipAssemblyNode>, ImportError> {
    let tag_name = xml_local_name(&tag.name);
    let href = zip_assembly_href(&tag.attributes, visual_names);
    if href.is_none() && !is_zip_assembly_node_tag(&tag_name) {
        return Ok(None);
    }

    let name = zip_assembly_node_name(&tag.attributes)
        .or_else(|| href.as_deref().map(zip_assembly_basename))
        .unwrap_or_else(|| tag_name.to_string());
    let mut node = ZipAssemblyNode::new(name);
    node.href = href;
    node.transform = zip_assembly_transform(&tag.attributes)?;
    Ok(Some(node))
}

fn is_zip_assembly_node_tag(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "assembly"
            | "productstructure"
            | "product"
            | "component"
            | "componentinstance"
            | "instance"
            | "instance3d"
            | "reference3d"
            | "productoccurrence"
            | "occurrence"
            | "part"
            | "node"
    )
}

fn zip_assembly_node_name(attributes: &[(String, String)]) -> Option<String> {
    find_attribute(
        attributes,
        &["name", "label", "title", "partnumber", "id", "instanceof"],
    )
    .filter(|value| !value.trim().is_empty())
    .map(ToOwned::to_owned)
}

fn zip_assembly_href(attributes: &[(String, String)], visual_names: &[&str]) -> Option<String> {
    for value in zip_assembly_reference_attribute_values(attributes) {
        if let Some(reference) = zip_assembly_reference_from_value(value, visual_names) {
            return Some(reference);
        }
    }
    None
}

fn zip_assembly_reference_attribute_values(attributes: &[(String, String)]) -> Vec<&str> {
    attributes
        .iter()
        .filter_map(|(name, value)| {
            let local = xml_local_name(name).to_ascii_lowercase();
            ZIP_ASSEMBLY_REFERENCE_ATTRIBUTES
                .contains(&local.as_str())
                .then_some(value.as_str())
        })
        .collect()
}

fn zip_assembly_reference_from_value(value: &str, visual_names: &[&str]) -> Option<String> {
    let candidate = normalize_zip_reference_value(value);
    if let Some(name) = match_visual_zip_entry(&candidate, visual_names) {
        return Some(name.to_string());
    }
    looks_like_external_reference(&candidate).then_some(candidate)
}

fn normalize_zip_reference_value(value: &str) -> String {
    let without_fragment = value
        .split(['#', '?'])
        .next()
        .unwrap_or(value)
        .replace('\\', "/");
    let normalized = without_fragment
        .trim()
        .trim_start_matches("file://")
        .trim_start_matches('/')
        .trim_start_matches("./")
        .to_string();
    strip_ascii_prefix(&normalized, "urn:3DXML:").to_string()
}

fn strip_ascii_prefix<'a>(value: &'a str, prefix: &str) -> &'a str {
    if value
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
    {
        &value[prefix.len()..]
    } else {
        value
    }
}

fn match_visual_zip_entry<'a>(candidate: &str, visual_names: &'a [&str]) -> Option<&'a str> {
    if candidate.is_empty() {
        return None;
    }
    if let Some(name) = visual_names.iter().find(|name| **name == candidate) {
        return Some(*name);
    }
    let suffix = format!("/{candidate}");
    let matches = visual_names
        .iter()
        .filter(|name| name.ends_with(&suffix))
        .copied()
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        return Some(matches[0]);
    }
    None
}

fn looks_like_external_reference(candidate: &str) -> bool {
    let Some((_, extension)) = candidate.rsplit_once('.') else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "catpart"
            | "cadpart"
            | "catproduct"
            | "cgr"
            | "3dxml"
            | "prt"
            | "ugpart"
            | "nxpart"
            | "sldprt"
            | "sldasm"
            | "asm"
            | "ipt"
            | "iam"
            | "par"
            | "psm"
            | "x_t"
            | "x_b"
            | "neu"
            | "step"
            | "stp"
            | "stl"
            | "obj"
            | "glb"
            | "flite"
    )
}

fn zip_assembly_transform(attributes: &[(String, String)]) -> Result<Transform, ImportError> {
    let mut transform = identity_transform();
    if let Some(value) = find_attribute(
        attributes,
        &["transform", "matrix", "transformation", "relativematrix"],
    ) {
        let values = parse_float_list(value);
        return zip_assembly_matrix_transform(&values, "ZIP assembly transform");
    }

    if let Some(value) = find_attribute(attributes, &["translation", "position"]) {
        let values = parse_float_list(value);
        if values.len() != 3 {
            return Err(ImportError::InvalidData(format!(
                "ZIP assembly translation expects 3 values, found {}",
                values.len()
            )));
        }
        transform[3][0] = values[0];
        transform[3][1] = values[1];
        transform[3][2] = values[2];
    }

    Ok(transform)
}

fn zip_assembly_transform_from_text(
    tag_name: &str,
    value: &str,
) -> Result<Option<Transform>, ImportError> {
    let values = parse_float_list(value);
    match tag_name.to_ascii_lowercase().as_str() {
        "relativematrix" | "matrix" | "transform" | "transformation" => {
            zip_assembly_matrix_transform(&values, tag_name).map(Some)
        }
        "translation" | "position" => {
            if values.len() != 3 {
                return Err(ImportError::InvalidData(format!(
                    "{tag_name} expects 3 values, found {}",
                    values.len()
                )));
            }
            let mut transform = identity_transform();
            transform[3][0] = values[0];
            transform[3][1] = values[1];
            transform[3][2] = values[2];
            Ok(Some(transform))
        }
        _ => Ok(None),
    }
}

fn zip_assembly_matrix_transform(values: &[f32], context: &str) -> Result<Transform, ImportError> {
    let mut transform = identity_transform();
    match values.len() {
        16 => {
            for (matrix_index, value) in values.iter().enumerate() {
                transform[matrix_index / 4][matrix_index % 4] = *value;
            }
        }
        12 => {
            for row in 0..3 {
                for column in 0..3 {
                    transform[row][column] = values[row * 3 + column];
                }
            }
            transform[3][0] = values[9];
            transform[3][1] = values[10];
            transform[3][2] = values[11];
        }
        count => {
            return Err(ImportError::InvalidData(format!(
                "{context} expects 12 or 16 matrix values, found {count}"
            )));
        }
    }
    Ok(transform)
}

fn find_attribute<'a>(attributes: &'a [(String, String)], names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|candidate| {
        attributes.iter().find_map(|(name, value)| {
            let local = xml_local_name(name).to_ascii_lowercase();
            (local == *candidate).then_some(value.as_str())
        })
    })
}

fn parse_float_list(value: &str) -> Vec<f32> {
    value
        .split(|character: char| {
            character.is_whitespace() || matches!(character, ',' | ';' | '[' | ']' | '(' | ')')
        })
        .filter(|token| !token.is_empty())
        .filter_map(|token| token.parse::<f32>().ok())
        .collect()
}

fn count_zip_assembly_refs(node: &ZipAssemblyNode) -> usize {
    usize::from(node.href.is_some())
        + node
            .children
            .iter()
            .map(count_zip_assembly_refs)
            .sum::<usize>()
}

fn append_zip_assembly_nodes(
    document: &mut LiteDocument,
    assembly: &ZipAssemblyNode,
    parent: Option<usize>,
    zip_assets: &[ZipAsset],
) -> ZipAssemblyResolution {
    let node_index = document.nodes.len();
    let mut node = LiteNode::new(&assembly.name, None);
    node.transform = assembly.transform;
    node.source_id = assembly.href.clone();
    document.nodes.push(node);
    if let Some(parent_index) = parent {
        document.nodes[parent_index].children.push(node_index);
    }

    let mut resolution = ZipAssemblyResolution::default();
    if let Some(href) = &assembly.href {
        if let Some(asset) = zip_assets.iter().find(|asset| asset.entry.name == *href) {
            document.append_document_to_node(node_index, asset.document.clone());
            resolution.zip_visual_refs += 1;
        } else {
            document.metadata.warnings.push(format!(
                "ZIP assembly reference `{href}` was left for external resolution"
            ));
            resolution.external_refs += 1;
        }
    }

    for child in &assembly.children {
        resolution.add(append_zip_assembly_nodes(
            document,
            child,
            Some(node_index),
            zip_assets,
        ));
    }

    resolution
}

fn xml_elements_by_local_names(text: &str, names: &[&str]) -> Vec<XmlElement> {
    let mut elements = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = text[cursor..].find('<') {
        let start = cursor + relative_start;
        let Some(relative_end) = text[start..].find('>') else {
            break;
        };
        let end = start + relative_end;
        let content = text[start + 1..end].trim();
        cursor = end + 1;

        if content.is_empty()
            || content.starts_with('/')
            || content.starts_with('?')
            || content.starts_with('!')
            || content.starts_with("!--")
        {
            continue;
        }

        let Some(tag) = parse_xml_start_tag(content) else {
            continue;
        };
        let tag_name = xml_local_name(&tag.name);
        if !names.iter().any(|name| tag_name.eq_ignore_ascii_case(name)) {
            continue;
        }

        let body = if tag.self_closing {
            String::new()
        } else if let Some((body_end, after_end)) = find_xml_element_end(text, cursor, &tag_name) {
            let body_start = end + 1;
            let body = text[body_start..body_end].to_string();
            cursor = after_end;
            body
        } else {
            String::new()
        };
        elements.push(XmlElement {
            attributes: tag.attributes,
            body,
        });
    }

    elements
}

fn find_xml_element_end(text: &str, cursor: usize, local_name: &str) -> Option<(usize, usize)> {
    let mut scan = cursor;
    let mut depth = 1_usize;

    while let Some(relative_start) = text[scan..].find('<') {
        let start = scan + relative_start;
        let Some(relative_end) = text[start..].find('>') else {
            break;
        };
        let end = start + relative_end;
        let content = text[start + 1..end].trim();
        scan = end + 1;

        if content.is_empty()
            || content.starts_with('?')
            || content.starts_with('!')
            || content.starts_with("!--")
        {
            continue;
        }

        if let Some(close_name) = content.strip_prefix('/') {
            if xml_local_name(close_name.trim()).eq_ignore_ascii_case(local_name) {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((start, scan));
                }
            }
            continue;
        }

        let Some(tag) = parse_xml_start_tag(content) else {
            continue;
        };
        if !tag.self_closing && xml_local_name(&tag.name).eq_ignore_ascii_case(local_name) {
            depth += 1;
        }
    }

    None
}

fn xml_child_text(body: &str, names: &[&str]) -> Option<String> {
    xml_child_text_with_name(body, names).map(|(_, value)| value)
}

fn xml_child_text_with_name(body: &str, names: &[&str]) -> Option<(String, String)> {
    let mut cursor = 0;
    while let Some(relative_start) = body[cursor..].find('<') {
        let start = cursor + relative_start;
        let Some(relative_end) = body[start..].find('>') else {
            break;
        };
        let end = start + relative_end;
        let content = body[start + 1..end].trim();
        cursor = end + 1;

        if content.is_empty()
            || content.starts_with('/')
            || content.starts_with('?')
            || content.starts_with('!')
            || content.starts_with("!--")
        {
            continue;
        }

        let Some(tag) = parse_xml_start_tag(content) else {
            continue;
        };
        let tag_name = xml_local_name(&tag.name);
        if !names.iter().any(|name| tag_name.eq_ignore_ascii_case(name)) {
            continue;
        }
        if tag.self_closing {
            return Some((tag_name, String::new()));
        }
        let (body_end, _after_end) = find_xml_element_end(body, cursor, &tag_name)?;
        let value = decode_xml_entities(body[cursor..body_end].trim());
        return Some((tag_name, value));
    }
    None
}

fn parse_xml_start_tag(content: &str) -> Option<XmlStartTag> {
    let content = content.trim();
    let self_closing = content.ends_with('/');
    let content = content.trim_end_matches('/').trim_end();
    let name_end = content
        .find(|character: char| character.is_whitespace())
        .unwrap_or(content.len());
    if name_end == 0 {
        return None;
    }

    let name = content[..name_end].to_string();
    let attributes = parse_xml_attributes(&content[name_end..]);
    Some(XmlStartTag {
        name,
        attributes,
        self_closing,
    })
}

fn parse_xml_attributes(input: &str) -> Vec<(String, String)> {
    let bytes = input.as_bytes();
    let mut attributes = Vec::new();
    let mut cursor = 0;

    while cursor < bytes.len() {
        while bytes
            .get(cursor)
            .map(|byte| byte.is_ascii_whitespace())
            .unwrap_or(false)
        {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }

        let key_start = cursor;
        while cursor < bytes.len()
            && !bytes[cursor].is_ascii_whitespace()
            && bytes[cursor] != b'='
            && bytes[cursor] != b'/'
        {
            cursor += 1;
        }
        if key_start == cursor {
            cursor += 1;
            continue;
        }
        let key = &input[key_start..cursor];

        while bytes
            .get(cursor)
            .map(|byte| byte.is_ascii_whitespace())
            .unwrap_or(false)
        {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'=') {
            attributes.push((key.to_string(), String::new()));
            continue;
        }
        cursor += 1;
        while bytes
            .get(cursor)
            .map(|byte| byte.is_ascii_whitespace())
            .unwrap_or(false)
        {
            cursor += 1;
        }

        let Some(quote) = bytes.get(cursor).copied() else {
            attributes.push((key.to_string(), String::new()));
            break;
        };
        let value = if quote == b'"' || quote == b'\'' {
            cursor += 1;
            let value_start = cursor;
            while cursor < bytes.len() && bytes[cursor] != quote {
                cursor += 1;
            }
            let value = decode_xml_entities(&input[value_start..cursor]);
            if cursor < bytes.len() {
                cursor += 1;
            }
            value
        } else {
            let value_start = cursor;
            while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            decode_xml_entities(&input[value_start..cursor])
        };
        attributes.push((key.to_string(), value));
    }

    attributes
}

fn decode_xml_entities(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn xml_local_name(name: &str) -> String {
    name.rsplit_once(':')
        .map(|(_, local)| local)
        .unwrap_or(name)
        .trim()
        .to_string()
}

fn zip_assembly_basename(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(_, basename)| basename)
        .unwrap_or(path)
        .to_string()
}

fn local_zip_headers(bytes: &[u8], max_entries: usize) -> Result<Vec<LocalZipHeader>, ImportError> {
    let mut headers = Vec::new();
    let mut cursor = 0;

    while cursor + LOCAL_FILE_HEADER_LEN <= bytes.len() {
        let Some(header_start) = find_signature(bytes, cursor, LOCAL_FILE_HEADER_SIGNATURE) else {
            break;
        };
        if let Some(header) = read_local_header(bytes, header_start) {
            if headers.len() >= max_entries {
                return Err(resource_limit_exceeded(
                    "ZIP entry count",
                    max_entries,
                    headers.len() + 1,
                ));
            }
            headers.push(header);
        }
        cursor = header_start + 4;
    }

    Ok(headers)
}

fn central_directory_entries(
    bytes: &[u8],
    max_entries: usize,
) -> Result<Vec<CentralDirectoryEntry>, ImportError> {
    let mut entries = Vec::new();
    let mut cursor = 0;

    while cursor + CENTRAL_DIRECTORY_HEADER_LEN <= bytes.len() {
        let Some(header_start) = find_signature(bytes, cursor, CENTRAL_DIRECTORY_HEADER_SIGNATURE)
        else {
            break;
        };
        if let Some(entry) = read_central_directory_entry(bytes, header_start) {
            if entries.len() >= max_entries {
                return Err(resource_limit_exceeded(
                    "ZIP entry count",
                    max_entries,
                    entries.len() + 1,
                ));
            }
            entries.push(entry);
        }
        cursor = header_start + 4;
    }

    Ok(entries)
}

fn find_signature(bytes: &[u8], start: usize, signature: u32) -> Option<usize> {
    let signature = signature.to_le_bytes();
    bytes
        .get(start..)?
        .windows(signature.len())
        .position(|window| window == signature)
        .map(|position| start + position)
}

#[derive(Debug, Clone)]
struct LocalZipHeader {
    start: usize,
    name: String,
    flags: u16,
    method: u16,
    compressed_size: usize,
    uncompressed_size: usize,
    data_start: usize,
}

impl LocalZipHeader {
    fn to_sized_entry(&self, bytes: &[u8]) -> Option<ZipEntry> {
        if self.flags & FLAG_DATA_DESCRIPTOR != 0 {
            return None;
        }
        if self.method == METHOD_STORED && self.compressed_size != self.uncompressed_size {
            return None;
        }

        let data_end = self.data_start.checked_add(self.compressed_size)?;
        if data_end > bytes.len() {
            return None;
        }

        Some(ZipEntry {
            name: self.name.clone(),
            flags: self.flags,
            method: self.method,
            uncompressed_size: self.uncompressed_size,
            data_start: self.data_start,
            data_end,
        })
    }
}

fn read_local_header(bytes: &[u8], start: usize) -> Option<LocalZipHeader> {
    let header = bytes.get(start..start + LOCAL_FILE_HEADER_LEN)?;
    if read_u32(header, 0)? != LOCAL_FILE_HEADER_SIGNATURE {
        return None;
    }

    let flags = read_u16(header, 6)?;
    let method = read_u16(header, 8)?;
    let compressed_size_32 = read_u32(header, 18)?;
    let uncompressed_size_32 = read_u32(header, 22)?;
    let name_len = read_u16(header, 26)? as usize;
    let extra_len = read_u16(header, 28)? as usize;

    let name_start = start + LOCAL_FILE_HEADER_LEN;
    let name_end = name_start.checked_add(name_len)?;
    let data_start = name_end.checked_add(extra_len)?;
    if data_start > bytes.len() {
        return None;
    }

    let extra = bytes.get(name_end..data_start)?;
    let needs_uncompressed_size = uncompressed_size_32 == ZIP64_U32_SENTINEL;
    let needs_compressed_size = compressed_size_32 == ZIP64_U32_SENTINEL;
    let zip64 = parse_zip64_extra(extra, needs_uncompressed_size, needs_compressed_size, false)?;
    let compressed_size = size32_or_zip64(compressed_size_32, zip64.compressed_size)?;
    let uncompressed_size = size32_or_zip64(uncompressed_size_32, zip64.uncompressed_size)?;
    let name = String::from_utf8_lossy(bytes.get(name_start..name_end)?).to_string();
    Some(LocalZipHeader {
        start,
        name,
        flags,
        method,
        compressed_size,
        uncompressed_size,
        data_start,
    })
}

#[derive(Debug, Clone)]
struct CentralDirectoryEntry {
    start: usize,
    name: String,
    flags: u16,
    method: u16,
    compressed_size: usize,
    uncompressed_size: usize,
    local_header_offset: usize,
}

impl CentralDirectoryEntry {
    fn to_zip_entry(&self, bytes: &[u8], local_headers: &[LocalZipHeader]) -> Option<ZipEntry> {
        let local_header = self.matching_local_header(local_headers)?;
        if self.method == METHOD_STORED && self.compressed_size != self.uncompressed_size {
            return None;
        }

        let data_end = local_header.data_start.checked_add(self.compressed_size)?;
        if data_end > bytes.len() {
            return None;
        }

        Some(ZipEntry {
            name: self.name.clone(),
            flags: self.flags,
            method: self.method,
            uncompressed_size: self.uncompressed_size,
            data_start: local_header.data_start,
            data_end,
        })
    }

    fn matching_local_header<'a>(
        &self,
        local_headers: &'a [LocalZipHeader],
    ) -> Option<&'a LocalZipHeader> {
        local_headers
            .iter()
            .filter(|header| header.name == self.name && header.method == self.method)
            .filter(|header| {
                let Some(base_offset) = header.start.checked_sub(self.local_header_offset) else {
                    return false;
                };
                let Some(data_end) = header.data_start.checked_add(self.compressed_size) else {
                    return false;
                };
                base_offset <= self.start && data_end <= self.start
            })
            .max_by_key(|header| header.start - self.local_header_offset)
    }
}

fn read_central_directory_entry(bytes: &[u8], start: usize) -> Option<CentralDirectoryEntry> {
    let header = bytes.get(start..start + CENTRAL_DIRECTORY_HEADER_LEN)?;
    if read_u32(header, 0)? != CENTRAL_DIRECTORY_HEADER_SIGNATURE {
        return None;
    }

    let flags = read_u16(header, 8)?;
    let method = read_u16(header, 10)?;
    let compressed_size_32 = read_u32(header, 20)?;
    let uncompressed_size_32 = read_u32(header, 24)?;
    let name_len = read_u16(header, 28)? as usize;
    let extra_len = read_u16(header, 30)? as usize;
    let comment_len = read_u16(header, 32)? as usize;
    let local_header_offset_32 = read_u32(header, 42)?;

    let name_start = start + CENTRAL_DIRECTORY_HEADER_LEN;
    let name_end = name_start.checked_add(name_len)?;
    let end = name_end.checked_add(extra_len)?.checked_add(comment_len)?;
    if end > bytes.len() {
        return None;
    }

    let extra = bytes.get(name_end..name_end + extra_len)?;
    let needs_uncompressed_size = uncompressed_size_32 == ZIP64_U32_SENTINEL;
    let needs_compressed_size = compressed_size_32 == ZIP64_U32_SENTINEL;
    let needs_local_header_offset = local_header_offset_32 == ZIP64_U32_SENTINEL;
    let zip64 = parse_zip64_extra(
        extra,
        needs_uncompressed_size,
        needs_compressed_size,
        needs_local_header_offset,
    )?;
    let compressed_size = size32_or_zip64(compressed_size_32, zip64.compressed_size)?;
    let uncompressed_size = size32_or_zip64(uncompressed_size_32, zip64.uncompressed_size)?;
    let local_header_offset = size32_or_zip64(local_header_offset_32, zip64.local_header_offset)?;
    let name = String::from_utf8_lossy(bytes.get(name_start..name_end)?).to_string();
    Some(CentralDirectoryEntry {
        start,
        name,
        flags,
        method,
        compressed_size,
        uncompressed_size,
        local_header_offset,
    })
}

fn push_unique_entry(
    entries: &mut Vec<ZipEntry>,
    entry: ZipEntry,
    max_entries: usize,
) -> Result<(), ImportError> {
    let exists = entries.iter().any(|existing| {
        existing.name == entry.name
            && existing.data_start == entry.data_start
            && existing.data_end == entry.data_end
    });
    if !exists {
        if entries.len() >= max_entries {
            return Err(resource_limit_exceeded(
                "ZIP entry count",
                max_entries,
                entries.len() + 1,
            ));
        }
        entries.push(entry);
    }
    Ok(())
}

fn validate_zip_entry_sizes(
    entries: &[ZipEntry],
    limits: &ImportLimits,
) -> Result<(), ImportError> {
    let mut total_uncompressed_bytes = 0_usize;
    for entry in entries.iter().filter(|entry| entry.is_supported_payload()) {
        if entry.uncompressed_size > limits.max_archive_entry_uncompressed_bytes {
            return Err(resource_limit_exceeded(
                "ZIP entry uncompressed bytes",
                limits.max_archive_entry_uncompressed_bytes,
                entry.uncompressed_size,
            ));
        }
        total_uncompressed_bytes = total_uncompressed_bytes.saturating_add(entry.uncompressed_size);
        if total_uncompressed_bytes > limits.max_archive_total_uncompressed_bytes {
            return Err(resource_limit_exceeded(
                "ZIP total uncompressed bytes",
                limits.max_archive_total_uncompressed_bytes,
                total_uncompressed_bytes,
            ));
        }
    }
    Ok(())
}

fn resource_limit_exceeded(resource: &'static str, limit: usize, actual: usize) -> ImportError {
    ImportError::ResourceLimitExceeded {
        resource,
        limit,
        actual,
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Zip64Extra {
    uncompressed_size: Option<usize>,
    compressed_size: Option<usize>,
    local_header_offset: Option<usize>,
}

fn parse_zip64_extra(
    extra: &[u8],
    needs_uncompressed_size: bool,
    needs_compressed_size: bool,
    needs_local_header_offset: bool,
) -> Option<Zip64Extra> {
    let needs_zip64 = needs_uncompressed_size || needs_compressed_size || needs_local_header_offset;
    if !needs_zip64 {
        return Some(Zip64Extra::default());
    }

    let mut cursor = 0;
    while cursor + 4 <= extra.len() {
        let field_id = read_u16(extra, cursor)?;
        let field_len = read_u16(extra, cursor + 2)? as usize;
        let data_start = cursor + 4;
        let data_end = data_start.checked_add(field_len)?;
        if data_end > extra.len() {
            return None;
        }

        if field_id == ZIP64_EXTRA_FIELD_ID {
            return parse_zip64_extra_payload(
                &extra[data_start..data_end],
                needs_uncompressed_size,
                needs_compressed_size,
                needs_local_header_offset,
            );
        }

        cursor = data_end;
    }

    None
}

fn parse_zip64_extra_payload(
    payload: &[u8],
    needs_uncompressed_size: bool,
    needs_compressed_size: bool,
    needs_local_header_offset: bool,
) -> Option<Zip64Extra> {
    let mut cursor = 0;
    let mut extra = Zip64Extra::default();

    if needs_uncompressed_size {
        extra.uncompressed_size = Some(read_zip64_usize(payload, cursor)?);
        cursor += 8;
    }
    if needs_compressed_size {
        extra.compressed_size = Some(read_zip64_usize(payload, cursor)?);
        cursor += 8;
    }
    if needs_local_header_offset {
        extra.local_header_offset = Some(read_zip64_usize(payload, cursor)?);
    }

    Some(extra)
}

fn size32_or_zip64(size: u32, zip64_size: Option<usize>) -> Option<usize> {
    if size == ZIP64_U32_SENTINEL {
        zip64_size
    } else {
        Some(size as usize)
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_zip64_usize(bytes: &[u8], offset: usize) -> Option<usize> {
    let value = u64::from_le_bytes(bytes.get(offset..offset + 8)?.try_into().ok()?);
    usize::try_from(value).ok()
}
