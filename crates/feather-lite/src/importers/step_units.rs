//! STEP representation-unit resolution and conversion to SI units.
//!
//! Unit selection follows `GLOBAL_UNIT_ASSIGNED_CONTEXT` references and does
//! not infer length or angle units from coordinate magnitude, names, or values.

use std::collections::BTreeSet;

use crate::document::LiteDocument;
use crate::importer::ImportError;

use super::step_part21::{
    StepComponent, StepRecord, parse_float_list, parse_reference, parse_references,
    split_top_level_args,
};

/// One resolved STEP unit and its scale to the corresponding SI unit.
#[derive(Debug, Clone)]
pub struct StepResolvedUnit {
    pub scale_to_si: f32,
    pub label: String,
}

/// Units explicitly assigned by STEP representation contexts.
#[derive(Debug, Default)]
pub struct StepUnits {
    pub length: Option<StepResolvedUnit>,
    pub plane_angle: Option<StepResolvedUnit>,
}

#[derive(Clone, Copy)]
enum UnitKind {
    Length,
    PlaneAngle,
}

impl UnitKind {
    fn marker(self) -> &'static str {
        match self {
            Self::Length => "LENGTH_UNIT",
            Self::PlaneAngle => "PLANE_ANGLE_UNIT",
        }
    }

    fn si_name(self) -> &'static str {
        match self {
            Self::Length => ".METRE.",
            Self::PlaneAngle => ".RADIAN.",
        }
    }

    fn si_label(self) -> &'static str {
        match self {
            Self::Length => "metre",
            Self::PlaneAngle => "radian",
        }
    }

    fn measure_component(self) -> &'static str {
        match self {
            Self::Length => "LENGTH_MEASURE_WITH_UNIT",
            Self::PlaneAngle => "PLANE_ANGLE_MEASURE_WITH_UNIT",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Length => "length",
            Self::PlaneAngle => "plane-angle",
        }
    }
}

/// Resolves length and plane-angle units assigned by STEP contexts.
pub fn resolve_step_units(records: &[StepRecord]) -> Result<StepUnits, ImportError> {
    let mut units = StepUnits::default();

    for context in records {
        let Some(component) = context.component("GLOBAL_UNIT_ASSIGNED_CONTEXT") else {
            continue;
        };
        for unit_id in parse_references(&component.args) {
            let unit_record = records
                .iter()
                .find(|record| record.id == unit_id)
                .ok_or_else(|| {
                    ImportError::InvalidData(format!(
                        "STEP unit context #{} references missing entity #{unit_id}",
                        context.id
                    ))
                })?;
            for kind in [UnitKind::Length, UnitKind::PlaneAngle] {
                let Some(unit) = resolve_unit(unit_record, records, kind, &mut BTreeSet::new())?
                else {
                    continue;
                };
                let target = match kind {
                    UnitKind::Length => &mut units.length,
                    UnitKind::PlaneAngle => &mut units.plane_angle,
                };
                assign_consistent_unit(target, unit, kind)?;
            }
        }
    }

    Ok(units)
}

/// Converts all STEP mesh positions to glTF metres and refreshes bounds.
pub fn apply_step_length_unit(document: &mut LiteDocument, unit: Option<&StepResolvedUnit>) {
    let Some(unit) = unit else {
        document.metadata.warnings.push(
            "STEP length unit context was not found; source coordinate values were preserved"
                .to_string(),
        );
        return;
    };

    for mesh in &mut document.meshes {
        for primitive in &mut mesh.primitives {
            for position in &mut primitive.positions {
                for coordinate in position {
                    *coordinate *= unit.scale_to_si;
                }
            }
        }
        mesh.recompute_bbox();
    }
    for node in &mut document.nodes {
        for coordinate in &mut node.transform[3][0..3] {
            *coordinate *= unit.scale_to_si;
        }
    }
    document.metadata.warnings.push(format!(
        "converted STEP {} coordinates to metres with scale {}",
        unit.label, unit.scale_to_si
    ));
}

fn assign_consistent_unit(
    target: &mut Option<StepResolvedUnit>,
    unit: StepResolvedUnit,
    kind: UnitKind,
) -> Result<(), ImportError> {
    if let Some(previous) = target {
        let tolerance = previous
            .scale_to_si
            .abs()
            .max(unit.scale_to_si.abs())
            .max(f32::MIN_POSITIVE)
            * f32::EPSILON
            * 16.0;
        if (previous.scale_to_si - unit.scale_to_si).abs() > tolerance {
            return Err(ImportError::InvalidData(format!(
                "STEP representation contexts assign conflicting {} units ({}, {})",
                kind.description(),
                previous.label,
                unit.label
            )));
        }
    } else {
        *target = Some(unit);
    }
    Ok(())
}

fn resolve_unit(
    record: &StepRecord,
    records: &[StepRecord],
    kind: UnitKind,
    resolving: &mut BTreeSet<usize>,
) -> Result<Option<StepResolvedUnit>, ImportError> {
    if !resolving.insert(record.id) {
        return Err(ImportError::InvalidData(format!(
            "STEP {}-unit conversion contains a cycle at #{}",
            kind.description(),
            record.id
        )));
    }
    let result = if let Some(si_unit) = record.component("SI_UNIT") {
        resolve_si_unit(record, si_unit, kind)
    } else if let Some(conversion) = record.component("CONVERSION_BASED_UNIT") {
        resolve_conversion_unit(record, conversion, records, kind, resolving)
    } else {
        Ok(None)
    };
    resolving.remove(&record.id);
    result
}

fn resolve_si_unit(
    record: &StepRecord,
    si_unit: &StepComponent,
    kind: UnitKind,
) -> Result<Option<StepResolvedUnit>, ImportError> {
    let args = split_top_level_args(&si_unit.args);
    if args.len() != 2 {
        return Err(ImportError::InvalidData(format!(
            "#{} SI_UNIT expects prefix and unit name",
            record.id
        )));
    }
    if !args[1].trim().eq_ignore_ascii_case(kind.si_name()) {
        return Ok(None);
    }

    let prefix = args[0].trim().to_ascii_uppercase();
    let (scale_to_si, prefix_label) = match prefix.as_str() {
        "$" => (1.0, ""),
        ".EXA." => (1.0e18, "exa"),
        ".PETA." => (1.0e15, "peta"),
        ".TERA." => (1.0e12, "tera"),
        ".GIGA." => (1.0e9, "giga"),
        ".MEGA." => (1.0e6, "mega"),
        ".KILO." => (1.0e3, "kilo"),
        ".HECTO." => (1.0e2, "hecto"),
        ".DECA." => (1.0e1, "deca"),
        ".DECI." => (1.0e-1, "deci"),
        ".CENTI." => (1.0e-2, "centi"),
        ".MILLI." => (1.0e-3, "milli"),
        ".MICRO." => (1.0e-6, "micro"),
        ".NANO." => (1.0e-9, "nano"),
        ".PICO." => (1.0e-12, "pico"),
        ".FEMTO." => (1.0e-15, "femto"),
        ".ATTO." => (1.0e-18, "atto"),
        _ => {
            return Err(ImportError::InvalidData(format!(
                "#{} SI_UNIT uses unsupported prefix {}",
                record.id, args[0]
            )));
        }
    };
    Ok(Some(StepResolvedUnit {
        scale_to_si,
        label: format!("{prefix_label}{}", kind.si_label()),
    }))
}

fn resolve_conversion_unit(
    record: &StepRecord,
    conversion: &StepComponent,
    records: &[StepRecord],
    kind: UnitKind,
    resolving: &mut BTreeSet<usize>,
) -> Result<Option<StepResolvedUnit>, ImportError> {
    if record.component(kind.marker()).is_none() {
        return Ok(None);
    }
    let args = split_top_level_args(&conversion.args);
    if args.len() != 2 {
        return Err(ImportError::InvalidData(format!(
            "#{} CONVERSION_BASED_UNIT expects a name and conversion factor",
            record.id
        )));
    }
    let factor_id = parse_reference(args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{} CONVERSION_BASED_UNIT has no conversion-factor reference",
            record.id
        ))
    })?;
    let factor = records
        .iter()
        .find(|candidate| candidate.id == factor_id)
        .ok_or_else(|| {
            ImportError::InvalidData(format!(
                "#{} CONVERSION_BASED_UNIT references missing factor #{factor_id}",
                record.id
            ))
        })?;
    let measure = factor.component(kind.measure_component()).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{factor_id} is not a {} conversion factor",
            kind.measure_component()
        ))
    })?;
    let measure_args = split_top_level_args(&measure.args);
    if measure_args.len() != 2 {
        return Err(ImportError::InvalidData(format!(
            "#{factor_id} {} expects a value and unit",
            kind.measure_component()
        )));
    }
    let values = parse_float_list(measure_args[0]);
    if values.len() != 1 || !values[0].is_finite() || values[0] <= 0.0 {
        return Err(ImportError::InvalidData(format!(
            "#{factor_id} {} value must be finite and positive",
            kind.measure_component()
        )));
    }
    let base_id = parse_reference(measure_args[1]).ok_or_else(|| {
        ImportError::InvalidData(format!(
            "#{factor_id} {} has no base-unit reference",
            kind.measure_component()
        ))
    })?;
    let base = records
        .iter()
        .find(|candidate| candidate.id == base_id)
        .ok_or_else(|| {
            ImportError::InvalidData(format!(
                "#{factor_id} {} references missing unit #{base_id}",
                kind.measure_component()
            ))
        })?;
    let Some(base_unit) = resolve_unit(base, records, kind, resolving)? else {
        return Err(ImportError::InvalidData(format!(
            "#{base_id} is not a supported STEP {} unit",
            kind.description()
        )));
    };
    let label = args[0].trim().trim_matches('\'').to_ascii_lowercase();
    let scale_to_si = values[0] * base_unit.scale_to_si;
    if !scale_to_si.is_finite() || scale_to_si <= 0.0 {
        return Err(ImportError::InvalidData(format!(
            "#{} CONVERSION_BASED_UNIT resolves to an invalid scale",
            record.id
        )));
    }
    Ok(Some(StepResolvedUnit {
        scale_to_si,
        label: if label.is_empty() {
            format!("conversion-based {} unit", kind.description())
        } else {
            label
        },
    }))
}
