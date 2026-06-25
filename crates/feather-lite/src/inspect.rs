//! Structured inspection API shared by CLI, services, and batch preflight.

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::assets::{EmbeddedVisualAsset, discover_embedded_visual_assets_with_limits};
use crate::capabilities::{FormatCapability, format_capability, push_format_capability_json};
use crate::contracts::INSPECT_REPORT_CONTRACT_VERSION;
use crate::diagnostics::{batch_failure_category, required_condition_for_failure};
use crate::importer::{ImportError, ImportOptions, ImporterRegistry, InputFile, read_input_path};
use crate::json::escape_json;
use crate::mesh::validate_document;
use crate::probe::{FileFormat, ProbeResult, detect_format};

/// Options for inspecting one source file.
#[derive(Debug, Clone, Default)]
pub struct InspectOptions {
    pub import: ImportOptions,
    pub check_import: bool,
}

/// Structured report returned by inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectReport {
    pub path: Option<PathBuf>,
    pub probe: ProbeResult,
    pub coarse_format: Option<FileFormat>,
    pub visual_assets: Vec<EmbeddedVisualAsset>,
    pub import_check: Option<InspectImportCheck>,
}

impl InspectReport {
    /// Returns the published format capability contract for the detected format.
    pub fn capability(&self) -> Option<&'static FormatCapability> {
        format_capability(self.probe.format)
    }

    /// Serializes the inspection report using the stable CLI/API JSON contract.
    pub fn to_json_string(&self) -> String {
        let mut json = String::new();
        json.push_str("{\n");
        json.push_str("  \"contract_version\": \"");
        json.push_str(INSPECT_REPORT_CONTRACT_VERSION);
        json.push_str("\",\n");
        json.push_str("  \"path\": \"");
        let path = self
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        json.push_str(&escape_json(&path));
        json.push_str("\",\n");
        json.push_str("  \"format\": \"");
        json.push_str(&escape_json(self.probe.format.label()));
        json.push_str("\",\n");
        json.push_str("  \"confidence\": \"");
        json.push_str(&escape_json(&format!("{:?}", self.probe.confidence)));
        json.push_str("\",\n");
        json.push_str("  \"embedded_cache\": ");
        json.push_str(if self.probe.has_embedded_cache {
            "true"
        } else {
            "false"
        });
        json.push_str(",\n");
        json.push_str("  \"reason\": \"");
        json.push_str(&escape_json(&self.probe.reason));
        json.push_str("\",\n");
        json.push_str("  \"container_kind\": ");
        push_optional_json_string(&mut json, self.probe.container_kind);
        json.push_str(",\n");
        json.push_str("  \"source_version\": ");
        push_optional_json_string(&mut json, self.probe.source_version.as_deref());
        json.push_str(",\n");
        json.push_str("  \"native_visualization\": ");
        push_optional_json_string(&mut json, self.probe.native_visualization);
        json.push_str(",\n");
        json.push_str("  \"coarse_format\": ");
        if let Some(coarse_format) = self.coarse_format {
            json.push('"');
            json.push_str(&escape_json(coarse_format.label()));
            json.push('"');
        } else {
            json.push_str("null");
        }
        json.push_str(",\n");
        json.push_str("  \"capability\": ");
        if let Some(capability) = self.capability() {
            push_format_capability_json(&mut json, capability, "");
        } else {
            json.push_str("null");
        }
        json.push_str(",\n");
        json.push_str("  \"import_check\": ");
        if let Some(import_check) = &self.import_check {
            push_import_check_json(&mut json, import_check);
        } else {
            json.push_str("null");
        }
        json.push_str(",\n");
        json.push_str("  \"visual_asset_count\": ");
        json.push_str(&self.visual_assets.len().to_string());
        json.push_str(",\n");
        json.push_str("  \"visual_assets\": [\n");
        for (index, asset) in self.visual_assets.iter().enumerate() {
            if index > 0 {
                json.push_str(",\n");
            }
            json.push_str("    {\n");
            json.push_str("      \"index\": ");
            json.push_str(&index.to_string());
            json.push_str(",\n");
            json.push_str("      \"kind\": \"");
            json.push_str(&escape_json(asset.kind.label()));
            json.push_str("\",\n");
            json.push_str("      \"source\": \"");
            json.push_str(&escape_json(asset.source.label()));
            json.push_str("\",\n");
            json.push_str("      \"byte_start\": ");
            json.push_str(&asset.byte_start.to_string());
            json.push_str(",\n");
            json.push_str("      \"byte_end\": ");
            json.push_str(&asset.byte_end.to_string());
            json.push_str(",\n");
            json.push_str("      \"name\": ");
            if let Some(name) = &asset.name {
                json.push('"');
                json.push_str(&escape_json(name));
                json.push('"');
            } else {
                json.push_str("null");
            }
            json.push('\n');
            json.push_str("    }");
        }
        json.push_str("\n  ]\n");
        json.push_str("}\n");
        json
    }
}

/// Result of a real lightweight import validation pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectImportCheck {
    pub importable: bool,
    pub mesh_count: Option<usize>,
    pub triangle_count: Option<u64>,
    pub failure_stage: Option<&'static str>,
    pub failure_category: Option<&'static str>,
    pub required_condition: Option<&'static str>,
    pub error: Option<String>,
}

/// Summary returned after a successful import validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportValidationSummary {
    pub source_format: String,
    pub node_count: usize,
    pub mesh_count: usize,
    pub primitive_count: usize,
    pub vertex_count: usize,
    pub triangle_count: u64,
}

/// Error type for inspection before optional import validation.
#[derive(Debug)]
pub enum InspectError {
    Io(std::io::Error),
    AssetScan(ImportError),
}

impl fmt::Display for InspectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "failed to read input: {error}"),
            Self::AssetScan(error) => write!(formatter, "failed to scan visual assets: {error}"),
        }
    }
}

impl Error for InspectError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::AssetScan(error) => Some(error),
        }
    }
}

/// Inspects a source path by reading it and running format/asset probes.
pub fn inspect_path(path: &Path, options: &InspectOptions) -> Result<InspectReport, InspectError> {
    let bytes = read_input_path(path, &options.import.limits).map_err(|error| match error {
        ImportError::Io(error) => InspectError::Io(error),
        error => InspectError::AssetScan(error),
    })?;
    inspect_bytes(Some(path), &bytes, options)
}

/// Inspects source bytes with an optional path for extension-aware probing.
pub fn inspect_bytes(
    path: Option<&Path>,
    bytes: &[u8],
    options: &InspectOptions,
) -> Result<InspectReport, InspectError> {
    let input = InputFile::new(path, bytes);
    let registry = ImporterRegistry::default();
    let probe = registry.probe(&input);
    let coarse_probe = detect_format(path, bytes);
    let coarse_format = (coarse_probe.format != probe.format).then_some(coarse_probe.format);
    let visual_assets = discover_embedded_visual_assets_with_limits(bytes, &options.import.limits)
        .map_err(InspectError::AssetScan)?;
    let import_check = options
        .check_import
        .then(|| inspect_import_check(&registry, &input, &options.import, probe.format));

    Ok(InspectReport {
        path: path.map(Path::to_path_buf),
        probe,
        coarse_format,
        visual_assets,
        import_check,
    })
}

/// Imports and validates a document without optimizing or exporting it.
pub fn validate_imported_input(
    input: &InputFile<'_>,
    options: &ImportOptions,
) -> Result<ImportValidationSummary, ImportError> {
    let registry = ImporterRegistry::default();
    validate_imported_input_with_registry(&registry, input, options)
}

fn inspect_import_check(
    registry: &ImporterRegistry,
    input: &InputFile<'_>,
    options: &ImportOptions,
    format: FileFormat,
) -> InspectImportCheck {
    match validate_imported_input_with_registry(registry, input, options) {
        Ok(summary) => InspectImportCheck {
            importable: true,
            mesh_count: Some(summary.mesh_count),
            triangle_count: Some(summary.triangle_count),
            failure_stage: None,
            failure_category: None,
            required_condition: None,
            error: None,
        },
        Err(error) => {
            let message = error.to_string();
            let failure_category = batch_failure_category("import", &message);
            InspectImportCheck {
                importable: false,
                mesh_count: None,
                triangle_count: None,
                failure_stage: Some("import"),
                failure_category: Some(failure_category),
                required_condition: required_condition_for_failure(format, failure_category),
                error: Some(message),
            }
        }
    }
}

fn push_import_check_json(json: &mut String, import_check: &InspectImportCheck) {
    json.push_str("{\n");
    json.push_str("    \"importable\": ");
    json.push_str(if import_check.importable {
        "true"
    } else {
        "false"
    });
    json.push_str(",\n");
    json.push_str("    \"mesh_count\": ");
    if let Some(mesh_count) = import_check.mesh_count {
        json.push_str(&mesh_count.to_string());
    } else {
        json.push_str("null");
    }
    json.push_str(",\n");
    json.push_str("    \"triangle_count\": ");
    if let Some(triangle_count) = import_check.triangle_count {
        json.push_str(&triangle_count.to_string());
    } else {
        json.push_str("null");
    }
    json.push_str(",\n");
    json.push_str("    \"failure_stage\": ");
    push_optional_json_string(json, import_check.failure_stage);
    json.push_str(",\n");
    json.push_str("    \"failure_category\": ");
    push_optional_json_string(json, import_check.failure_category);
    json.push_str(",\n");
    json.push_str("    \"required_condition\": ");
    push_optional_json_string(json, import_check.required_condition);
    json.push_str(",\n");
    json.push_str("    \"error\": ");
    if let Some(error) = &import_check.error {
        json.push('"');
        json.push_str(&escape_json(error));
        json.push('"');
    } else {
        json.push_str("null");
    }
    json.push_str("\n  }");
}

fn push_optional_json_string(json: &mut String, value: Option<&str>) {
    if let Some(value) = value {
        json.push('"');
        json.push_str(&escape_json(value));
        json.push('"');
    } else {
        json.push_str("null");
    }
}

fn validate_imported_input_with_registry(
    registry: &ImporterRegistry,
    input: &InputFile<'_>,
    options: &ImportOptions,
) -> Result<ImportValidationSummary, ImportError> {
    let document = registry.import(input, options)?;
    validate_document(&document)?;
    let node_count = document.nodes.len();
    let primitive_count = document.primitive_count();
    let vertex_count = document.vertex_count();
    Ok(ImportValidationSummary {
        source_format: document.metadata.source_format,
        node_count,
        mesh_count: document.metadata.mesh_count,
        primitive_count,
        vertex_count,
        triangle_count: document.metadata.triangle_count,
    })
}
