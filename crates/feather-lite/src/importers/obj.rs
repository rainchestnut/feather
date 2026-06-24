//! Importer for standalone Wavefront OBJ mesh files.

use crate::assets::obj::{import_obj_document, is_obj};
use crate::document::LiteDocument;
use crate::importer::{CadLiteImporter, ImportError, ImportOptions, InputFile};
use crate::probe::{FileFormat, ProbeConfidence, ProbeResult, detect_format};

/// Imports standalone OBJ mesh files into the lightweight IR.
pub struct ObjLiteImporter;

impl CadLiteImporter for ObjLiteImporter {
    fn name(&self) -> &'static str {
        "obj-lite"
    }

    fn probe(&self, input: &InputFile<'_>) -> ProbeResult {
        let probe = detect_format(input.path, input.bytes);
        if probe.format == FileFormat::Obj {
            let confidence = if is_obj(input.bytes) {
                ProbeConfidence::Certain
            } else {
                probe.confidence
            };
            ProbeResult::matched(
                FileFormat::Obj,
                confidence,
                "Wavefront OBJ mesh extension or structure",
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
        import_obj_document(
            input.bytes,
            FileFormat::Obj.label(),
            "obj-ascii",
            input.path,
        )
    }
}
