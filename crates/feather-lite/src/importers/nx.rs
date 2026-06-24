//! Cache-first NX/UG importer.
//!
//! NX `.prt` is a private format. The importer extracts embedded visual cache
//! payloads when present and otherwise fails explicitly.

use crate::assembly::resolve_external_references;
use crate::assets::import_first_embedded_visual_asset;
use crate::cache::{decode_cache_text, extract_cache_text};
use crate::document::LiteDocument;
use crate::importer::{CadLiteImporter, ImportError, ImportOptions, InputFile};
use crate::probe::{FileFormat, ProbeResult, detect_format};

/// Imports NX/UG part files through visual cache or external tessellation.
pub struct NxLiteImporter;

impl CadLiteImporter for NxLiteImporter {
    fn name(&self) -> &'static str {
        "nx-lite"
    }

    fn probe(&self, input: &InputFile<'_>) -> ProbeResult {
        let probe = detect_format(input.path, input.bytes);
        if probe.format == FileFormat::NxPrt {
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
        let label = FileFormat::NxPrt.label();

        if options.prefer_cache
            && let Some(text) = extract_cache_text(input.bytes)?
        {
            let mut document = decode_cache_text(&text, label, input.path)?;
            document.metadata.mode = "nx-cache-only".to_string();
            resolve_external_references(&mut document, input.path, options)?;
            document.refresh_metadata();
            return Ok(document);
        }

        if let Some(mut document) = import_first_embedded_visual_asset(
            input.bytes,
            label,
            input.path,
            "nx-embedded-visual-asset",
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
