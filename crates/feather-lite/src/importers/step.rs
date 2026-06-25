//! STEP importer boundary.
//!
//! The importer shares one structured Part 21 parse across AP242 tessellated
//! entities and the supported native B-Rep subset. Explicit STEP length and
//! plane-angle units are resolved structurally; lengths are normalized to glTF
//! metres before the document leaves this boundary.

use crate::cache::{decode_cache_text, extract_cache_text};
use crate::document::LiteDocument;
use crate::importer::{CadLiteImporter, ImportError, ImportOptions, InputFile};
use crate::probe::{FileFormat, ProbeResult, detect_format};

use super::step_brep::import_brep_step;
use super::step_part21::parse_step_records;
use super::step_tessellated::import_tessellated_step;
use super::step_units::{apply_step_length_unit, resolve_step_units};

/// Imports supported tessellated and native B-Rep STEP representations.
pub struct StepLiteImporter;

impl CadLiteImporter for StepLiteImporter {
    fn name(&self) -> &'static str {
        "step-lite"
    }

    fn probe(&self, input: &InputFile<'_>) -> ProbeResult {
        let probe = detect_format(input.path, input.bytes);
        if probe.format == FileFormat::Step {
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
        let label = FileFormat::Step.label();

        if let Some(text) = extract_cache_text(input.bytes)? {
            let mut document = decode_cache_text(&text, label, input.path)?;
            document.metadata.mode = "step-embedded-tessellation".to_string();
            document.metadata.has_brep = true;
            document.metadata.brep_preserved = false;
            document.refresh_metadata();
            return Ok(document);
        }

        let text = std::str::from_utf8(input.bytes).map_err(|error| {
            ImportError::InvalidData(format!("STEP input is not valid UTF-8: {error}"))
        })?;
        let records = parse_step_records(text)?;
        let units = resolve_step_units(&records)?;
        if let Some(mut document) = import_tessellated_step(&records, input.path, options)? {
            apply_step_length_unit(&mut document, units.length.as_ref());
            return Ok(document);
        }
        if let Some(mut document) =
            import_brep_step(&records, input.path, options, units.plane_angle.as_ref())?
        {
            if records
                .iter()
                .any(|record| record.kind == "CONICAL_SURFACE")
                && let Some(unit) = units.plane_angle.as_ref()
            {
                document.metadata.warnings.push(format!(
                    "interpreted STEP {} plane angles as radians with scale {}",
                    unit.label, unit.scale_to_si
                ));
            }
            apply_step_length_unit(&mut document, units.length.as_ref());
            return Ok(document);
        }

        Err(ImportError::TessellationUnsupported {
            format: label.to_string(),
            reason: "native AP242 tessellated faces, ADVANCED_FACE B-Rep with bounded outer/inner LINE/CIRCLE/ELLIPSE loops and parameter TRIMMED_CURVE spans on supported analytic surfaces, rational or non-rational B_SPLINE_CURVE_WITH_KNOTS boundaries and parameter TRIMMED_CURVE spans on supported analytic faces, regular ring TOROIDAL_SURFACE with meridian/parallel circles, and rigid ITEM_DEFINED_TRANSFORMATION shape-representation assemblies are supported; other analytic or spline geometry, singular surfaces, and other assembly transforms are pending".to_string(),
        })
    }
}
