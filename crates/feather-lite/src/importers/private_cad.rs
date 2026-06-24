//! Generic cache-first importer for private CAD containers.
//!
//! This importer is deliberately vendor-neutral. It exists for private CAD
//! files that expose standard lightweight payloads but do not need a dedicated
//! open-source parser yet. It does not call commercial SDKs or attempt to
//! decode proprietary B-Rep sections.

use crate::assembly::resolve_external_references;
use crate::assets::import_embedded_visual_assets;
use crate::cache::{decode_cache_text, extract_cache_text};
use crate::document::LiteDocument;
use crate::importer::{CadLiteImporter, ImportError, ImportOptions, InputFile};
use crate::probe::{FileFormat, ProbeResult, detect_format};

/// Imports generic private CAD containers through readable visual payloads.
pub struct PrivateCadLiteImporter;

impl CadLiteImporter for PrivateCadLiteImporter {
    fn name(&self) -> &'static str {
        "private-cad-lite"
    }

    fn probe(&self, input: &InputFile<'_>) -> ProbeResult {
        let probe = detect_format(input.path, input.bytes);
        if probe.format == FileFormat::PrivateCad {
            probe
        } else {
            ProbeResult::unknown()
        }
    }

    fn import_lite(
        &self,
        input: &InputFile<'_>,
        options: &ImportOptions,
    ) -> Result<LiteDocument, ImportError> {
        let label = FileFormat::PrivateCad.label();

        if options.prefer_cache
            && let Some(text) = extract_cache_text(input.bytes)?
        {
            let mut document = decode_cache_text(&text, label, input.path)?;
            document.metadata.mode = "private-cad-cache-only".to_string();
            resolve_external_references(&mut document, input.path, options)?;
            document.refresh_metadata();
            return Ok(document);
        }

        if let Some(mut document) = import_embedded_visual_assets(
            input.bytes,
            label,
            input.path,
            "private-cad-embedded-visual-asset",
            &options.limits,
        )? {
            resolve_external_references(&mut document, input.path, options)?;
            document.refresh_metadata();
            return Ok(document);
        }

        Err(ImportError::NoLightweightCache {
            format: label.to_string(),
        })
    }
}
