//! Importer for standalone binary glTF mesh files.

use crate::assets::glb::{import_glb_document, is_exact_glb};
use crate::document::LiteDocument;
use crate::importer::{CadLiteImporter, ImportError, ImportOptions, InputFile};
use crate::probe::{FileFormat, ProbeConfidence, ProbeResult, detect_format};

/// Imports standalone GLB files into the lightweight IR.
pub struct GlbLiteImporter;

impl CadLiteImporter for GlbLiteImporter {
    fn name(&self) -> &'static str {
        "glb-lite"
    }

    fn probe(&self, input: &InputFile<'_>) -> ProbeResult {
        let probe = detect_format(input.path, input.bytes);
        if probe.format == FileFormat::Glb {
            let confidence = if is_exact_glb(input.bytes) {
                ProbeConfidence::Certain
            } else {
                probe.confidence
            };
            ProbeResult::matched(
                FileFormat::Glb,
                confidence,
                "binary glTF/GLB marker or extension",
                probe.has_embedded_cache,
            )
        } else {
            ProbeResult::unknown()
        }
    }

    fn import_lite(
        &self,
        input: &InputFile<'_>,
        _options: &ImportOptions,
    ) -> Result<LiteDocument, ImportError> {
        import_glb_document(
            input.bytes,
            FileFormat::Glb.label(),
            "glb-binary",
            input.path,
        )
    }
}
