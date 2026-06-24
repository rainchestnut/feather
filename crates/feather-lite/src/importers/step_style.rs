//! Shared STEP presentation-style resolution for visual importers.

use std::collections::HashMap;

use crate::document::LiteMaterial;
use crate::importer::ImportError;

use super::step_part21::{
    StepRecord, parse_references, parse_required_float, split_top_level_args,
};

/// Quantized color key used to group STEP faces into stable materials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StepColorKey {
    rgba: [i32; 4],
}

impl StepColorKey {
    /// Returns true when a material color represents this quantized STEP color.
    pub fn matches(self, color: [f32; 4]) -> bool {
        Self {
            rgba: [
                quantize_color_channel(color[0]),
                quantize_color_channel(color[1]),
                quantize_color_channel(color[2]),
                quantize_color_channel(color[3]),
            ],
        } == self
    }

    fn from_rgb(red: f32, green: f32, blue: f32) -> Self {
        Self {
            rgba: [
                quantize_color_channel(red),
                quantize_color_channel(green),
                quantize_color_channel(blue),
                1_000_000,
            ],
        }
    }

    fn to_rgba(self) -> [f32; 4] {
        [
            self.rgba[0] as f32 / 1_000_000.0,
            self.rgba[1] as f32 / 1_000_000.0,
            self.rgba[2] as f32 / 1_000_000.0,
            self.rgba[3] as f32 / 1_000_000.0,
        ]
    }
}

/// Resolves colors assigned to entity ids through STEP style carrier records.
pub fn collect_styled_item_colors(
    records: &[StepRecord],
) -> Result<HashMap<usize, StepColorKey>, ImportError> {
    let style_colors = collect_style_record_colors(records)?;
    let mut item_colors = HashMap::new();

    for record in records {
        if record.kind != "STYLED_ITEM" {
            continue;
        }
        let args = split_top_level_args(&record.args);
        if args.len() < 3 {
            continue;
        }
        let Some(color) = parse_references(args[1])
            .into_iter()
            .find_map(|style_id| style_colors.get(&style_id).copied())
        else {
            continue;
        };
        for target_id in parse_references(args[2]) {
            item_colors.insert(target_id, color);
        }
    }

    Ok(item_colors)
}

/// Builds stable Feather materials for a sequence of STEP colors.
pub fn collect_step_materials(colors: impl IntoIterator<Item = StepColorKey>) -> Vec<LiteMaterial> {
    let mut unique = Vec::<StepColorKey>::new();
    for color in colors {
        if !unique.contains(&color) {
            unique.push(color);
        }
    }
    unique
        .into_iter()
        .enumerate()
        .map(|(index, color)| {
            LiteMaterial::new(format!("STEP_Color_{}", index + 1), color.to_rgba())
        })
        .collect()
}

fn collect_style_record_colors(
    records: &[StepRecord],
) -> Result<HashMap<usize, StepColorKey>, ImportError> {
    let mut colors = HashMap::<usize, StepColorKey>::new();

    for record in records {
        if record.kind != "COLOUR_RGB" {
            continue;
        }
        let args = split_top_level_args(&record.args);
        if args.len() < 4 {
            return Err(ImportError::InvalidData(format!(
                "#{} COLOUR_RGB expects name, red, green, blue",
                record.id
            )));
        }
        let red = parse_required_float(args[1], record.id, "red color channel")?;
        let green = parse_required_float(args[2], record.id, "green color channel")?;
        let blue = parse_required_float(args[3], record.id, "blue color channel")?;
        colors.insert(record.id, StepColorKey::from_rgb(red, green, blue));
    }

    let mut changed = true;
    while changed {
        changed = false;
        for record in records {
            if colors.contains_key(&record.id) || !is_step_style_carrier(&record.kind) {
                continue;
            }
            if let Some(color) = parse_references(&record.args)
                .into_iter()
                .find_map(|reference_id| colors.get(&reference_id).copied())
            {
                colors.insert(record.id, color);
                changed = true;
            }
        }
    }

    Ok(colors)
}

fn is_step_style_carrier(kind: &str) -> bool {
    matches!(
        kind,
        "FILL_AREA_STYLE_COLOUR"
            | "FILL_AREA_STYLE"
            | "SURFACE_STYLE_FILL_AREA"
            | "SURFACE_SIDE_STYLE"
            | "SURFACE_STYLE_USAGE"
            | "PRESENTATION_STYLE_ASSIGNMENT"
    )
}

fn quantize_color_channel(value: f32) -> i32 {
    let value = if value.is_finite() { value } else { 0.0 };
    (value.clamp(0.0, 1.0) * 1_000_000.0).round() as i32
}
