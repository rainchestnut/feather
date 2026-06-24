//! Cache dump API for extracting embedded lightweight visual assets.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::assets::discover_embedded_visual_assets_with_limits;
use crate::contracts::CACHE_DUMP_MANIFEST_CONTRACT_VERSION;
use crate::importer::{ImportError, ImportLimits, read_input_path};
use crate::json::escape_json;

/// Metadata for one visual asset written by a cache dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpedVisualAsset {
    pub index: usize,
    pub kind: String,
    pub source: String,
    pub byte_start: usize,
    pub byte_end: usize,
    pub entry_name: Option<String>,
    pub file_name: String,
    pub output_path: PathBuf,
}

/// Result of dumping all embedded lightweight visual assets from one source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheDumpReport {
    pub source_path: String,
    pub manifest_path: PathBuf,
    pub assets: Vec<DumpedVisualAsset>,
}

impl CacheDumpReport {
    pub fn asset_count(&self) -> usize {
        self.assets.len()
    }

    /// Serializes the dump report using the stable dump-cache manifest contract.
    pub fn to_manifest_json(&self) -> String {
        let mut json = String::new();
        json.push_str("{\n");
        json.push_str("  \"contract_version\": \"");
        json.push_str(CACHE_DUMP_MANIFEST_CONTRACT_VERSION);
        json.push_str("\",\n");
        json.push_str("  \"source_path\": \"");
        json.push_str(&escape_json(&self.source_path));
        json.push_str("\",\n");
        json.push_str("  \"asset_count\": ");
        json.push_str(&self.assets.len().to_string());
        json.push_str(",\n");
        json.push_str("  \"assets\": [\n");
        for (position, asset) in self.assets.iter().enumerate() {
            if position > 0 {
                json.push_str(",\n");
            }
            json.push_str("    {\n");
            json.push_str("      \"index\": ");
            json.push_str(&asset.index.to_string());
            json.push_str(",\n");
            json.push_str("      \"kind\": \"");
            json.push_str(&escape_json(&asset.kind));
            json.push_str("\",\n");
            json.push_str("      \"source\": \"");
            json.push_str(&escape_json(&asset.source));
            json.push_str("\",\n");
            json.push_str("      \"byte_start\": ");
            json.push_str(&asset.byte_start.to_string());
            json.push_str(",\n");
            json.push_str("      \"byte_end\": ");
            json.push_str(&asset.byte_end.to_string());
            json.push_str(",\n");
            json.push_str("      \"entry_name\": ");
            if let Some(entry_name) = &asset.entry_name {
                json.push('"');
                json.push_str(&escape_json(entry_name));
                json.push('"');
            } else {
                json.push_str("null");
            }
            json.push_str(",\n");
            json.push_str("      \"file\": \"");
            json.push_str(&escape_json(&asset.file_name));
            json.push_str("\"\n");
            json.push_str("    }");
        }
        json.push_str("\n  ]\n");
        json.push_str("}\n");
        json
    }
}

/// Error produced before or during a cache dump.
#[derive(Debug)]
pub enum CacheDumpError {
    ReadInput(std::io::Error),
    AssetScan(ImportError),
    CreateOutputDir(std::io::Error),
    WriteAsset {
        path: PathBuf,
        source: std::io::Error,
    },
    WriteManifest(std::io::Error),
}

impl fmt::Display for CacheDumpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadInput(error) => write!(formatter, "failed to read input: {error}"),
            Self::AssetScan(error) => write!(formatter, "failed to scan visual assets: {error}"),
            Self::CreateOutputDir(error) => {
                write!(formatter, "failed to create output directory: {error}")
            }
            Self::WriteAsset { path, source } => {
                write!(formatter, "failed to write `{}`: {source}", path.display())
            }
            Self::WriteManifest(error) => write!(formatter, "failed to write manifest: {error}"),
        }
    }
}

impl Error for CacheDumpError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadInput(error) => Some(error),
            Self::AssetScan(error) => Some(error),
            Self::CreateOutputDir(error) => Some(error),
            Self::WriteAsset { source, .. } => Some(source),
            Self::WriteManifest(error) => Some(error),
        }
    }
}

/// Extracts embedded visual assets, writes them to a directory, and writes manifest.json.
pub fn dump_embedded_visual_assets(
    input_path: &Path,
    output_dir: &Path,
) -> Result<CacheDumpReport, CacheDumpError> {
    dump_embedded_visual_assets_with_limits(input_path, output_dir, &ImportLimits::default())
}

/// Extracts embedded visual assets with caller-provided input and container
/// limits, then writes the assets and manifest to an output directory.
pub fn dump_embedded_visual_assets_with_limits(
    input_path: &Path,
    output_dir: &Path,
    limits: &ImportLimits,
) -> Result<CacheDumpReport, CacheDumpError> {
    let bytes = read_input_path(input_path, limits).map_err(|error| match error {
        ImportError::Io(error) => CacheDumpError::ReadInput(error),
        error => CacheDumpError::AssetScan(error),
    })?;
    let assets = discover_embedded_visual_assets_with_limits(&bytes, limits)
        .map_err(CacheDumpError::AssetScan)?;

    fs::create_dir_all(output_dir).map_err(CacheDumpError::CreateOutputDir)?;

    let mut dumped_assets = Vec::new();
    for (asset_index, asset) in assets.iter().enumerate() {
        let file_name = format!("asset_{asset_index:03}.{}", asset.kind.extension());
        let output_path = output_dir.join(&file_name);
        fs::write(&output_path, &asset.payload).map_err(|source| CacheDumpError::WriteAsset {
            path: output_path.clone(),
            source,
        })?;
        dumped_assets.push(DumpedVisualAsset {
            index: asset_index,
            kind: asset.kind.label().to_string(),
            source: asset.source.label().to_string(),
            byte_start: asset.byte_start,
            byte_end: asset.byte_end,
            entry_name: asset.name.clone(),
            file_name,
            output_path,
        });
    }

    let manifest_path = output_dir.join("manifest.json");
    let report = CacheDumpReport {
        source_path: input_path.display().to_string(),
        manifest_path,
        assets: dumped_assets,
    };
    fs::write(&report.manifest_path, report.to_manifest_json())
        .map_err(CacheDumpError::WriteManifest)?;
    Ok(report)
}
