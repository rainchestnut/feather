//! Importer for standalone STL mesh files.

use crate::assets::stl::{
    import_ascii_stl_document, import_binary_stl_document, is_ascii_stl, is_exact_binary_stl,
};
use crate::document::LiteDocument;
use crate::importer::{CadLiteImporter, ImportError, ImportOptions, InputFile};
use crate::probe::{FileFormat, ProbeConfidence, ProbeResult, detect_format};

/// Imports standalone STL files into the lightweight IR.
pub struct StlLiteImporter;

impl CadLiteImporter for StlLiteImporter {
    fn name(&self) -> &'static str {
        "stl-lite"
    }

    fn probe(&self, input: &InputFile<'_>) -> ProbeResult {
        let probe = detect_format(input.path, input.bytes);
        if probe.format == FileFormat::Stl {
            let confidence = if is_exact_binary_stl(input.bytes) || is_ascii_stl(input.bytes) {
                ProbeConfidence::Certain
            } else {
                probe.confidence
            };
            ProbeResult::matched(
                FileFormat::Stl,
                confidence,
                "STL mesh extension or STL structure",
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
        if is_exact_binary_stl(input.bytes) {
            return import_binary_stl_document(
                input.bytes,
                FileFormat::Stl.label(),
                "stl-binary",
                input.path,
            );
        }

        import_ascii_stl_document(
            input.bytes,
            FileFormat::Stl.label(),
            "stl-ascii",
            input.path,
        )
    }
}
