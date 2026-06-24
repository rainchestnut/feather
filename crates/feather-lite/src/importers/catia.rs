//! Cache-first CATIA importer.
//!
//! CATPart/CATProduct/CGR are private Dassault formats. This importer only
//! accepts pre-tessellated visualization cache or embedded visual assets and
//! fails explicitly when the file exposes only proprietary B-Rep data.

use crate::assembly::resolve_external_references;
use crate::assets::import_embedded_visual_assets;
use crate::cache::{decode_cache_text, extract_cache_text};
use crate::document::LiteDocument;
use crate::importer::{CadLiteImporter, ImportError, ImportOptions, InputFile};
use crate::probe::{FileFormat, ProbeResult, catia_v5_container_profile, detect_format};

/// Imports CATIA-like private CAD files through lightweight cache data.
pub struct CatiaLiteImporter;

impl CadLiteImporter for CatiaLiteImporter {
    fn name(&self) -> &'static str {
        "catia-lite"
    }

    fn probe(&self, input: &InputFile<'_>) -> ProbeResult {
        let probe = detect_format(input.path, input.bytes);
        if matches!(
            probe.format,
            FileFormat::CatiaCatPart | FileFormat::CatiaCatProduct | FileFormat::CatiaCgr
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
            document.metadata.mode = "catia-cache-only".to_string();
            resolve_external_references(&mut document, input.path, options)?;
            document.refresh_metadata();
            return Ok(document);
        }

        if let Some(mut document) = import_embedded_visual_assets(
            input.bytes,
            label,
            input.path,
            "catia-embedded-visual-asset",
            &options.limits,
        )? {
            resolve_external_references(&mut document, input.path, options)?;
            document.refresh_metadata();
            return Ok(document);
        }

        if catia_v5_container_profile(input.bytes).is_some_and(|profile| profile.has_native_cgr) {
            return Err(ImportError::NativeVisualizationUnsupported {
                format: label.to_string(),
                representation: "CATCGRCont",
            });
        }

        Err(ImportError::NoLightweightCache {
            format: label.to_string(),
        })
    }
}
