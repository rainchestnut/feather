//! Cache-first Dassault 3DXML importer.
//!
//! 3DXML is a Dassault lightweight exchange/container format. This importer
//! reads only open, inspectable payloads such as Feather cache blocks, ZIP/XML
//! assembly manifests, readable XML 3DRep polygon data, and embedded standard
//! visual assets; binary or encrypted 3DRep streams are intentionally not
//! decoded here.

use crate::assembly::resolve_external_references;
use crate::assets::import_embedded_visual_assets;
use crate::cache::{decode_cache_text, extract_cache_text};
use crate::document::LiteDocument;
use crate::importer::{CadLiteImporter, ImportError, ImportOptions, InputFile};
use crate::probe::{FileFormat, ProbeResult, detect_format};

/// Imports Dassault 3DXML files through readable lightweight payloads.
pub struct Dassault3dxmlLiteImporter;

impl CadLiteImporter for Dassault3dxmlLiteImporter {
    fn name(&self) -> &'static str {
        "dassault-3dxml-lite"
    }

    fn probe(&self, input: &InputFile<'_>) -> ProbeResult {
        let probe = detect_format(input.path, input.bytes);
        if probe.format == FileFormat::Dassault3dxml {
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
        let label = FileFormat::Dassault3dxml.label();

        if options.prefer_cache
            && let Some(text) = extract_cache_text(input.bytes)?
        {
            let mut document = decode_cache_text(&text, label, input.path)?;
            document.metadata.mode = "3dxml-cache-only".to_string();
            resolve_external_references(&mut document, input.path, options)?;
            document.refresh_metadata();
            return Ok(document);
        }

        if let Some(mut document) = import_embedded_visual_assets(
            input.bytes,
            label,
            input.path,
            "3dxml-embedded-visual-asset",
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
