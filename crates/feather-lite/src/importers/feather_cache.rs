//! Importer for standalone Feather Lite cache files.

use crate::assembly::resolve_external_references;
use crate::cache::{decode_cache_text, extract_cache_text};
use crate::document::LiteDocument;
use crate::importer::{CadLiteImporter, ImportError, ImportOptions, InputFile};
use crate::probe::{FileFormat, ProbeResult, detect_format};

/// Imports `.flite` files and embedded Feather cache payloads.
pub struct FeatherCacheImporter;

impl CadLiteImporter for FeatherCacheImporter {
    fn name(&self) -> &'static str {
        "feather-cache"
    }

    fn probe(&self, input: &InputFile<'_>) -> ProbeResult {
        let probe = detect_format(input.path, input.bytes);
        if probe.format == FileFormat::FeatherCache {
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
        let Some(text) = extract_cache_text(input.bytes)? else {
            return Err(ImportError::NoLightweightCache {
                format: FileFormat::FeatherCache.label().to_string(),
            });
        };

        let mut document = decode_cache_text(&text, FileFormat::FeatherCache.label(), input.path)?;
        document.metadata.mode = "standalone-cache".to_string();
        resolve_external_references(&mut document, input.path, options)?;
        document.refresh_metadata();
        Ok(document)
    }
}
