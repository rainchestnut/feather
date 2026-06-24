//! External reference resolution for lightweight assembly caches.
//!
//! Private assembly formats often point to part files beside the assembly. This
//! module resolves cache-declared references through the same open-source
//! importer registry and appends the imported visual parts under the reference
//! nodes in the Feather Lite scene.

use std::path::{Path, PathBuf};

use crate::document::LiteDocument;
use crate::importer::{
    ImportError, ImportOptions, ImporterRegistry, InputFile, ReferencePathMapping, read_input_path,
};

/// Resolves cache-declared external references in-place.
pub fn resolve_external_references(
    document: &mut LiteDocument,
    source_path: Option<&Path>,
    options: &ImportOptions,
) -> Result<(), ImportError> {
    if !options.load_assembly {
        return Ok(());
    }

    let references = external_references(document);
    if references.is_empty() {
        return Ok(());
    }
    if options.max_reference_depth == 0 {
        return Err(ImportError::InvalidData(
            "external reference depth exceeded while resolving assembly".to_string(),
        ));
    }

    let mut child_options = options.clone();
    child_options.max_reference_depth -= 1;

    for (node_index, reference_path) in references {
        let resolved_path = resolve_reference_path(&reference_path, source_path, options)?;
        let bytes = read_input_path(&resolved_path, &child_options.limits)?;
        let input = InputFile::new(Some(&resolved_path), &bytes);
        let registry = ImporterRegistry::default();
        let child_document = registry.import(&input, &child_options)?;

        document.metadata.warnings.push(format!(
            "resolved external reference `{}` from `{}`",
            reference_path,
            resolved_path.display()
        ));
        document.append_document_to_node(node_index, child_document);
    }

    document.refresh_metadata();
    Ok(())
}

fn external_references(document: &LiteDocument) -> Vec<(usize, String)> {
    document
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            let is_unresolved_reference =
                node.mesh.is_none() && node.children.is_empty() && node.source_id.is_some();
            is_unresolved_reference.then(|| {
                (
                    index,
                    node.source_id.clone().expect("checked source_id presence"),
                )
            })
        })
        .collect()
}

fn resolve_reference_path(
    reference_path: &str,
    source_path: Option<&Path>,
    options: &ImportOptions,
) -> Result<PathBuf, ImportError> {
    let candidates = reference_path_candidates(reference_path);

    for requested in &candidates {
        if requested.is_absolute() && requested.is_file() {
            return Ok(requested.to_path_buf());
        }
    }

    for requested in
        mapped_reference_path_candidates(reference_path, &options.reference_path_mappings)
    {
        if requested.is_file() {
            return Ok(requested);
        }
    }

    if let Some(parent) = source_path.and_then(Path::parent) {
        for requested in &candidates {
            if requested.is_absolute() {
                continue;
            }
            let candidate = parent.join(requested);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    for resolve_dir in &options.resolve_dirs {
        for requested in &candidates {
            if requested.is_absolute() {
                continue;
            }
            let candidate = resolve_dir.join(requested);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(ImportError::InvalidData(format!(
        "external reference `{reference_path}` could not be resolved"
    )))
}

fn mapped_reference_path_candidates(
    reference_path: &str,
    mappings: &[ReferencePathMapping],
) -> Vec<PathBuf> {
    let normalized_reference = normalize_reference_path_text(reference_path);
    let mut candidates = Vec::<PathBuf>::new();

    for mapping in mappings {
        let normalized_from = normalize_reference_path_text(&mapping.from);
        if normalized_from.is_empty() {
            continue;
        }
        let Some(suffix) = path_mapping_suffix(&normalized_reference, &normalized_from) else {
            continue;
        };
        push_unique_candidate(&mut candidates, mapping.to.join(suffix));
    }

    candidates
}

fn path_mapping_suffix<'a>(reference: &'a str, from: &str) -> Option<&'a str> {
    if reference == from {
        return Some("");
    }
    if let Some(suffix) = reference.strip_prefix(from)
        && suffix.starts_with('/')
    {
        return Some(suffix.trim_start_matches('/'));
    }

    let reference_lower = reference.to_ascii_lowercase();
    let from_lower = from.to_ascii_lowercase();
    if reference_lower == from_lower {
        return Some("");
    }
    if let Some(suffix) = reference_lower.strip_prefix(&from_lower)
        && suffix.starts_with('/')
    {
        return Some(reference[from.len()..].trim_start_matches('/'));
    }

    None
}

fn reference_path_candidates(reference_path: &str) -> Vec<PathBuf> {
    let without_file_scheme = trimmed_reference_path(reference_path);
    let normalized = normalize_reference_path_text(reference_path);

    let mut candidates = Vec::<PathBuf>::new();
    push_unique_candidate(&mut candidates, PathBuf::from(without_file_scheme));
    if normalized != without_file_scheme {
        push_unique_candidate(&mut candidates, PathBuf::from(&normalized));
    }
    if let Some(basename) = normalized.rsplit('/').find(|segment| !segment.is_empty()) {
        push_unique_candidate(&mut candidates, PathBuf::from(basename));
    }

    candidates
}

fn normalize_reference_path_text(reference_path: &str) -> String {
    trimmed_reference_path(reference_path)
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string()
}

fn trimmed_reference_path(reference_path: &str) -> &str {
    let trimmed = reference_path
        .split(['#', '?'])
        .next()
        .unwrap_or(reference_path)
        .trim();
    trimmed.strip_prefix("file://").unwrap_or(trimmed)
}

fn push_unique_candidate(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.as_os_str().is_empty() {
        return;
    }
    if !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}
