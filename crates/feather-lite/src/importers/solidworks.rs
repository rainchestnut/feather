//! Cache-first SolidWorks importer.
//!
//! SLDPRT/SLDASM are private SolidWorks formats. This importer accepts only
//! lightweight visualization payloads that can be extracted through open-source
//! paths and fails explicitly when only proprietary model data is present.

use crate::assembly::resolve_external_references;
use crate::assets::import_embedded_visual_assets;
use crate::cache::{decode_cache_text, extract_cache_text};
use crate::document::LiteDocument;
use crate::importer::{CadLiteImporter, ImportError, ImportOptions, InputFile};
use crate::probe::{FileFormat, ProbeResult, detect_format};

/// Imports SolidWorks part and assembly files through visual cache data.
pub struct SolidWorksLiteImporter;

impl CadLiteImporter for SolidWorksLiteImporter {
    fn name(&self) -> &'static str {
        "solidworks-lite"
    }

    fn probe(&self, input: &InputFile<'_>) -> ProbeResult {
        let probe = detect_format(input.path, input.bytes);
        if matches!(
            probe.format,
            FileFormat::SolidWorksPart | FileFormat::SolidWorksAssembly
        ) {
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
        let format = detect_format(input.path, input.bytes).format;
        let label = format.label();

        if options.prefer_cache
            && let Some(text) = extract_cache_text(input.bytes)?
        {
            let mut document = decode_cache_text(&text, label, input.path)?;
            document.metadata.mode = "solidworks-cache-only".to_string();
            resolve_external_references(&mut document, input.path, options)?;
            document.refresh_metadata();
            return Ok(document);
        }

        if let Some(mut document) = import_embedded_visual_assets(
            input.bytes,
            label,
            input.path,
            "solidworks-embedded-visual-asset",
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
