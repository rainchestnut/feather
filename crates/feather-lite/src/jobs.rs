//! File-backed business job model for conversion and batch workflows.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::atomic_write::write_atomic;
use crate::batch::{
    BatchConversionError, BatchConversionOptions, BatchConversionReport, conversion_error_stage,
    run_batch_conversion,
};
use crate::contracts::JOB_RECORD_CONTRACT_VERSION;
use crate::diagnostics::batch_failure_category;
use crate::importer::{ImportLimits, ReferencePathMapping};
use crate::pipeline::{ConversionError, ConversionOptions, ConversionSummary, convert_path_to_glb};

const JOBS_DIR_NAME: &str = "jobs";
const JOB_RECORD_FILE_NAME: &str = "job.json";
const ARTIFACTS_DIR_NAME: &str = "artifacts";

/// Persistent status for a local conversion job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    /// Returns the stable string emitted in human-readable CLI output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Coarse business stage for a job record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStage {
    Queued,
    Running,
    Import,
    Export,
    Io,
    Batch,
    Succeeded,
    Failed,
}

impl JobStage {
    /// Returns the stable string emitted in human-readable CLI output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Import => "import",
            Self::Export => "export",
            Self::Io => "io",
            Self::Batch => "batch",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    /// Returns the stage corresponding to a conversion error stage string.
    pub fn from_conversion_stage(stage: &str) -> Self {
        match stage {
            "import" => Self::Import,
            "export" => Self::Export,
            "io" => Self::Io,
            "batch" => Self::Batch,
            _ => Self::Failed,
        }
    }
}

/// Serializable mirror of import limits persisted with each job request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobImportLimits {
    pub max_input_bytes: usize,
    pub max_ole_streams: usize,
    pub max_ole_stream_bytes: usize,
    pub max_ole_total_stream_bytes: usize,
    pub max_archive_entries: usize,
    pub max_archive_entry_uncompressed_bytes: usize,
    pub max_archive_total_uncompressed_bytes: usize,
    pub max_step_curve_segments: usize,
    pub max_step_spline_degree: usize,
    pub max_step_spline_control_points: usize,
    pub max_step_face_loops: usize,
    pub max_step_face_vertices: usize,
    pub max_step_assembly_nodes: usize,
}

impl From<ImportLimits> for JobImportLimits {
    fn from(limits: ImportLimits) -> Self {
        Self {
            max_input_bytes: limits.max_input_bytes,
            max_ole_streams: limits.max_ole_streams,
            max_ole_stream_bytes: limits.max_ole_stream_bytes,
            max_ole_total_stream_bytes: limits.max_ole_total_stream_bytes,
            max_archive_entries: limits.max_archive_entries,
            max_archive_entry_uncompressed_bytes: limits.max_archive_entry_uncompressed_bytes,
            max_archive_total_uncompressed_bytes: limits.max_archive_total_uncompressed_bytes,
            max_step_curve_segments: limits.max_step_curve_segments,
            max_step_spline_degree: limits.max_step_spline_degree,
            max_step_spline_control_points: limits.max_step_spline_control_points,
            max_step_face_loops: limits.max_step_face_loops,
            max_step_face_vertices: limits.max_step_face_vertices,
            max_step_assembly_nodes: limits.max_step_assembly_nodes,
        }
    }
}

impl From<JobImportLimits> for ImportLimits {
    fn from(limits: JobImportLimits) -> Self {
        Self {
            max_input_bytes: limits.max_input_bytes,
            max_ole_streams: limits.max_ole_streams,
            max_ole_stream_bytes: limits.max_ole_stream_bytes,
            max_ole_total_stream_bytes: limits.max_ole_total_stream_bytes,
            max_archive_entries: limits.max_archive_entries,
            max_archive_entry_uncompressed_bytes: limits.max_archive_entry_uncompressed_bytes,
            max_archive_total_uncompressed_bytes: limits.max_archive_total_uncompressed_bytes,
            max_step_curve_segments: limits.max_step_curve_segments,
            max_step_spline_degree: limits.max_step_spline_degree,
            max_step_spline_control_points: limits.max_step_spline_control_points,
            max_step_face_loops: limits.max_step_face_loops,
            max_step_face_vertices: limits.max_step_face_vertices,
            max_step_assembly_nodes: limits.max_step_assembly_nodes,
        }
    }
}

impl Default for JobImportLimits {
    fn default() -> Self {
        ImportLimits::default().into()
    }
}

/// Serializable mapping from an archived reference prefix to a local root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobReferencePathMapping {
    pub from: String,
    pub to: PathBuf,
}

impl From<&ReferencePathMapping> for JobReferencePathMapping {
    fn from(mapping: &ReferencePathMapping) -> Self {
        Self {
            from: mapping.from.clone(),
            to: mapping.to.clone(),
        }
    }
}

impl From<&JobReferencePathMapping> for ReferencePathMapping {
    fn from(mapping: &JobReferencePathMapping) -> Self {
        ReferencePathMapping::new(mapping.from.clone(), mapping.to.clone())
    }
}

/// Serializable conversion settings used by queued and retried jobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobConversionSettings {
    pub write_metadata: bool,
    pub optimize_mesh: bool,
    pub include_normals: bool,
    pub max_triangles: Option<u64>,
    pub quantize_step: Option<f32>,
    pub max_lod_error: f32,
    pub resolve_dirs: Vec<PathBuf>,
    pub reference_path_mappings: Vec<JobReferencePathMapping>,
    pub limits: JobImportLimits,
}

impl Default for JobConversionSettings {
    fn default() -> Self {
        Self {
            write_metadata: true,
            optimize_mesh: true,
            include_normals: true,
            max_triangles: None,
            quantize_step: None,
            max_lod_error: 0.0,
            resolve_dirs: Vec::new(),
            reference_path_mappings: Vec::new(),
            limits: JobImportLimits::default(),
        }
    }
}

impl JobConversionSettings {
    /// Builds production conversion options for this persisted job request.
    pub fn to_conversion_options(&self, metadata_path: Option<PathBuf>) -> ConversionOptions {
        let mut options = ConversionOptions {
            write_metadata: self.write_metadata,
            metadata_path,
            ..ConversionOptions::default()
        };
        if self.optimize_mesh {
            options.mesh.position_quantization_step = self.quantize_step;
        } else {
            options.mesh.weld_vertices = false;
            options.mesh.rebuild_missing_normals = false;
            options.mesh.position_quantization_step = None;
        }
        options.mesh.max_triangles = self.max_triangles;
        options.import.max_lod_error = self.max_lod_error;
        options.import.resolve_dirs = self.resolve_dirs.clone();
        options.import.reference_path_mappings = self
            .reference_path_mappings
            .iter()
            .map(ReferencePathMapping::from)
            .collect();
        options.import.limits = self.limits.into();
        options.export.include_normals = self.include_normals;
        options
    }
}

/// Persisted work request for a business job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobRequest {
    Convert {
        input_path: PathBuf,
        settings: JobConversionSettings,
    },
    Batch {
        input_paths: Vec<PathBuf>,
        check_only: bool,
        settings: JobConversionSettings,
    },
}

/// Stable artifact paths reserved for a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobArtifactPaths {
    pub root_dir: PathBuf,
    pub model_path: Option<PathBuf>,
    pub metadata_path: Option<PathBuf>,
    pub manifest_path: Option<PathBuf>,
    pub batch_output_dir: Option<PathBuf>,
    pub source_info_path: PathBuf,
}

/// Structured failure persisted with a failed job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobFailure {
    pub stage: String,
    pub category: String,
    pub message: String,
    pub retryable: bool,
}

/// Structured result persisted with a completed job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobResult {
    Conversion {
        source_format: String,
        output_path: PathBuf,
        metadata_path: Option<PathBuf>,
        node_count: usize,
        mesh_count: usize,
        primitive_count: usize,
        vertex_count: usize,
        triangle_count: u64,
    },
    Batch {
        manifest_path: PathBuf,
        input_count: usize,
        converted_count: usize,
        checked_count: usize,
        failed_count: usize,
    },
}

/// Persistent record stored in `job.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRecord {
    pub contract_version: String,
    pub job_id: String,
    pub status: JobStatus,
    pub stage: JobStage,
    pub request: JobRequest,
    pub artifacts: JobArtifactPaths,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub started_at_unix_ms: Option<u64>,
    pub finished_at_unix_ms: Option<u64>,
    pub failure: Option<JobFailure>,
    pub result: Option<JobResult>,
}

impl JobRecord {
    /// Serializes the record to pretty JSON for API or CLI responses.
    pub fn to_json_string(&self) -> String {
        let mut json =
            serde_json::to_string_pretty(self).expect("job record JSON serialization should work");
        json.push('\n');
        json
    }
}

/// One source path captured in a job artifact package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSourceInputInfo {
    pub path: PathBuf,
    pub size_bytes: Option<u64>,
}

/// Source package metadata written beside job artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSourceInfo {
    pub contract_version: String,
    pub job_id: String,
    pub kind: String,
    pub inputs: Vec<JobSourceInputInfo>,
    pub created_at_unix_ms: u64,
}

/// Error returned by the local job store.
#[derive(Debug)]
pub enum JobError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    InvalidJobId(String),
    JobNotFound(String),
    InvalidState {
        job_id: String,
        status: JobStatus,
        operation: &'static str,
    },
}

impl fmt::Display for JobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "failed to access `{}`: {source}", path.display())
            }
            Self::Json { path, source } => {
                write!(formatter, "failed to parse `{}`: {source}", path.display())
            }
            Self::InvalidJobId(job_id) => write!(formatter, "invalid job id `{job_id}`"),
            Self::JobNotFound(job_id) => write!(formatter, "job `{job_id}` was not found"),
            Self::InvalidState {
                job_id,
                status,
                operation,
            } => write!(
                formatter,
                "job `{job_id}` is {} and cannot be {operation}",
                status.as_str()
            ),
        }
    }
}

impl Error for JobError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::InvalidJobId(_) | Self::JobNotFound(_) | Self::InvalidState { .. } => None,
        }
    }
}

/// File-backed local job store suitable for CLIs and lightweight services.
#[derive(Debug, Clone)]
pub struct LocalJobStore {
    root_dir: PathBuf,
}

impl LocalJobStore {
    /// Creates a local job store rooted at `root_dir`.
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
        }
    }

    /// Creates a queued single-file conversion job and persists its record.
    pub fn create_conversion_job(
        &self,
        input_path: impl Into<PathBuf>,
        settings: JobConversionSettings,
    ) -> Result<JobRecord, JobError> {
        let job_id = self.reserve_job_id()?;
        let job_dir = self.job_dir_unchecked(&job_id);
        let artifacts = conversion_artifacts(job_dir.join(ARTIFACTS_DIR_NAME), &settings);
        fs::create_dir_all(&artifacts.root_dir).map_err(|source| JobError::Io {
            path: artifacts.root_dir.clone(),
            source,
        })?;

        let now = unix_timestamp_millis();
        let record = JobRecord {
            contract_version: JOB_RECORD_CONTRACT_VERSION.to_string(),
            job_id,
            status: JobStatus::Queued,
            stage: JobStage::Queued,
            request: JobRequest::Convert {
                input_path: input_path.into(),
                settings,
            },
            artifacts,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
            failure: None,
            result: None,
        };
        self.write_job_record(&record)?;
        Ok(record)
    }

    /// Creates a queued batch conversion or preflight job and persists its record.
    pub fn create_batch_job(
        &self,
        input_paths: Vec<PathBuf>,
        check_only: bool,
        settings: JobConversionSettings,
    ) -> Result<JobRecord, JobError> {
        let job_id = self.reserve_job_id()?;
        let job_dir = self.job_dir_unchecked(&job_id);
        let artifacts = batch_artifacts(job_dir.join(ARTIFACTS_DIR_NAME));
        fs::create_dir_all(&artifacts.root_dir).map_err(|source| JobError::Io {
            path: artifacts.root_dir.clone(),
            source,
        })?;

        let now = unix_timestamp_millis();
        let record = JobRecord {
            contract_version: JOB_RECORD_CONTRACT_VERSION.to_string(),
            job_id,
            status: JobStatus::Queued,
            stage: JobStage::Queued,
            request: JobRequest::Batch {
                input_paths,
                check_only,
                settings,
            },
            artifacts,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
            failure: None,
            result: None,
        };
        self.write_job_record(&record)?;
        Ok(record)
    }

    /// Loads a previously persisted job record.
    pub fn load_job(&self, job_id: &str) -> Result<JobRecord, JobError> {
        let path = self.job_record_path(job_id)?;
        if !path.exists() {
            return Err(JobError::JobNotFound(job_id.to_string()));
        }
        let bytes = fs::read(&path).map_err(|source| JobError::Io {
            path: path.clone(),
            source,
        })?;
        serde_json::from_slice(&bytes).map_err(|source| JobError::Json { path, source })
    }

    /// Runs a queued job synchronously and persists the final record.
    pub fn run_job(&self, job_id: &str) -> Result<JobRecord, JobError> {
        let record = self.load_job(job_id)?;
        if record.status != JobStatus::Queued {
            return Err(JobError::InvalidState {
                job_id: record.job_id,
                status: record.status,
                operation: "run",
            });
        }
        self.execute_record(record)
    }

    /// Re-runs a failed job with the same persisted request and artifact paths.
    pub fn retry_job(&self, job_id: &str) -> Result<JobRecord, JobError> {
        let record = self.load_job(job_id)?;
        if record.status != JobStatus::Failed {
            return Err(JobError::InvalidState {
                job_id: record.job_id,
                status: record.status,
                operation: "retried",
            });
        }
        self.execute_record(record)
    }

    fn execute_record(&self, mut record: JobRecord) -> Result<JobRecord, JobError> {
        let now = unix_timestamp_millis();
        record.status = JobStatus::Running;
        record.stage = JobStage::Running;
        record.started_at_unix_ms = Some(now);
        record.finished_at_unix_ms = None;
        record.updated_at_unix_ms = now;
        record.failure = None;
        record.result = None;
        self.write_job_record(&record)?;

        fs::create_dir_all(&record.artifacts.root_dir).map_err(|source| JobError::Io {
            path: record.artifacts.root_dir.clone(),
            source,
        })?;

        let run_result = match record.request.clone() {
            JobRequest::Convert {
                input_path,
                settings,
            } => self.execute_conversion_job(&mut record, &input_path, &settings),
            JobRequest::Batch {
                input_paths,
                check_only,
                settings,
            } => self.execute_batch_job(&mut record, &input_paths, check_only, &settings),
        };

        let finished_at = unix_timestamp_millis();
        record.updated_at_unix_ms = finished_at;
        record.finished_at_unix_ms = Some(finished_at);
        if let Err(failure) = run_result {
            record.status = JobStatus::Failed;
            record.stage = JobStage::from_conversion_stage(&failure.stage);
            record.failure = Some(failure);
        }
        self.write_job_record(&record)?;
        Ok(record)
    }

    fn execute_conversion_job(
        &self,
        record: &mut JobRecord,
        input_path: &Path,
        settings: &JobConversionSettings,
    ) -> Result<(), JobFailure> {
        let output_path = record
            .artifacts
            .model_path
            .clone()
            .expect("conversion jobs reserve model output path");
        write_source_info(&record.job_id, &record.request, &record.artifacts)?;
        let metadata_path = settings.write_metadata.then(|| {
            record
                .artifacts
                .metadata_path
                .clone()
                .expect("metadata-enabled conversion jobs reserve metadata path")
        });
        let options = settings.to_conversion_options(metadata_path);

        match convert_path_to_glb(input_path, &output_path, &options) {
            Ok(summary) => {
                record.status = JobStatus::Succeeded;
                record.stage = JobStage::Succeeded;
                record.result = Some(JobResult::from(summary));
                Ok(())
            }
            Err(error) => Err(JobFailure::from_conversion_error(&error)),
        }
    }

    fn execute_batch_job(
        &self,
        record: &mut JobRecord,
        input_paths: &[PathBuf],
        check_only: bool,
        settings: &JobConversionSettings,
    ) -> Result<(), JobFailure> {
        write_source_info(&record.job_id, &record.request, &record.artifacts)?;
        let output_dir = record
            .artifacts
            .batch_output_dir
            .clone()
            .expect("batch jobs reserve output directory");
        let manifest_path = record
            .artifacts
            .manifest_path
            .clone()
            .expect("batch jobs reserve manifest path");
        let run = run_batch_conversion(
            input_paths,
            &BatchConversionOptions {
                output_dir,
                manifest_path: Some(manifest_path),
                check_only,
                conversion: settings.to_conversion_options(None),
            },
        );

        match run {
            Ok(report) => {
                let failed_count = report.report.failed_count();
                record.result = Some(JobResult::from(report));
                if failed_count == 0 {
                    record.status = JobStatus::Succeeded;
                    record.stage = JobStage::Succeeded;
                    Ok(())
                } else {
                    Err(JobFailure {
                        stage: "batch".to_string(),
                        category: "batch_item_failed".to_string(),
                        message: format!("batch completed with {failed_count} failed items"),
                        retryable: true,
                    })
                }
            }
            Err(error) => Err(JobFailure::from_batch_error(&error)),
        }
    }

    fn write_job_record(&self, record: &JobRecord) -> Result<(), JobError> {
        let path = self.job_record_path(&record.job_id)?;
        let mut bytes = serde_json::to_vec_pretty(record).map_err(|source| JobError::Json {
            path: path.clone(),
            source,
        })?;
        bytes.push(b'\n');
        write_atomic(&path, bytes).map_err(|source| JobError::Io { path, source })
    }

    fn reserve_job_id(&self) -> Result<String, JobError> {
        fs::create_dir_all(self.jobs_dir()).map_err(|source| JobError::Io {
            path: self.jobs_dir(),
            source,
        })?;

        for attempt in 0..32 {
            let job_id = format!(
                "job-{}-{}-{attempt}",
                unix_timestamp_millis(),
                std::process::id()
            );
            let job_dir = self.job_dir_unchecked(&job_id);
            match fs::create_dir(&job_dir) {
                Ok(()) => return Ok(job_id),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(JobError::Io {
                        path: job_dir,
                        source,
                    });
                }
            }
        }

        Err(JobError::Io {
            path: self.jobs_dir(),
            source: io::Error::new(
                io::ErrorKind::AlreadyExists,
                "failed to reserve unique job id",
            ),
        })
    }

    fn job_record_path(&self, job_id: &str) -> Result<PathBuf, JobError> {
        Ok(self.job_dir(job_id)?.join(JOB_RECORD_FILE_NAME))
    }

    fn job_dir(&self, job_id: &str) -> Result<PathBuf, JobError> {
        if is_valid_job_id(job_id) {
            Ok(self.job_dir_unchecked(job_id))
        } else {
            Err(JobError::InvalidJobId(job_id.to_string()))
        }
    }

    fn job_dir_unchecked(&self, job_id: &str) -> PathBuf {
        self.jobs_dir().join(job_id)
    }

    fn jobs_dir(&self) -> PathBuf {
        self.root_dir.join(JOBS_DIR_NAME)
    }
}

impl From<ConversionSummary> for JobResult {
    fn from(summary: ConversionSummary) -> Self {
        Self::Conversion {
            source_format: summary.source_format,
            output_path: summary.output_path,
            metadata_path: summary.metadata_path,
            node_count: summary.node_count,
            mesh_count: summary.mesh_count,
            primitive_count: summary.primitive_count,
            vertex_count: summary.vertex_count,
            triangle_count: summary.triangle_count,
        }
    }
}

impl From<BatchConversionReport> for JobResult {
    fn from(report: BatchConversionReport) -> Self {
        Self::Batch {
            manifest_path: report.manifest_path,
            input_count: report.report.input_count(),
            converted_count: report.report.converted_count(),
            checked_count: report.report.checked_count(),
            failed_count: report.report.failed_count(),
        }
    }
}

impl JobFailure {
    fn from_conversion_error(error: &ConversionError) -> Self {
        let stage = conversion_error_stage(error);
        let message = error.to_string();
        let category = batch_failure_category(stage, &message);
        Self {
            stage: stage.to_string(),
            category: category.to_string(),
            message,
            retryable: retryable_failure(category),
        }
    }

    fn from_batch_error(error: &BatchConversionError) -> Self {
        let stage = match error {
            BatchConversionError::CreateOutputDir { .. }
            | BatchConversionError::WriteManifest { .. } => "io",
            BatchConversionError::CollectInputs(_) | BatchConversionError::EmptyInputSet => {
                "import"
            }
        };
        let message = error.to_string();
        let category = batch_failure_category(stage, &message);
        Self {
            stage: stage.to_string(),
            category: category.to_string(),
            message,
            retryable: retryable_failure(category),
        }
    }
}

fn conversion_artifacts(root_dir: PathBuf, settings: &JobConversionSettings) -> JobArtifactPaths {
    JobArtifactPaths {
        model_path: Some(root_dir.join("model.glb")),
        metadata_path: settings
            .write_metadata
            .then(|| root_dir.join("metadata.json")),
        manifest_path: None,
        batch_output_dir: None,
        source_info_path: root_dir.join("source-info.json"),
        root_dir,
    }
}

fn batch_artifacts(root_dir: PathBuf) -> JobArtifactPaths {
    JobArtifactPaths {
        model_path: None,
        metadata_path: None,
        manifest_path: Some(root_dir.join("manifest.json")),
        batch_output_dir: Some(root_dir.join("outputs")),
        source_info_path: root_dir.join("source-info.json"),
        root_dir,
    }
}

fn write_source_info(
    job_id: &str,
    request: &JobRequest,
    artifacts: &JobArtifactPaths,
) -> Result<(), JobFailure> {
    let (kind, inputs) = match request {
        JobRequest::Convert { input_path, .. } => (
            "convert",
            vec![JobSourceInputInfo {
                path: input_path.clone(),
                size_bytes: file_size(input_path),
            }],
        ),
        JobRequest::Batch { input_paths, .. } => (
            "batch",
            input_paths
                .iter()
                .map(|path| JobSourceInputInfo {
                    path: path.clone(),
                    size_bytes: file_size(path),
                })
                .collect(),
        ),
    };
    let source_info = JobSourceInfo {
        contract_version: JOB_RECORD_CONTRACT_VERSION.to_string(),
        job_id: job_id.to_string(),
        kind: kind.to_string(),
        inputs,
        created_at_unix_ms: unix_timestamp_millis(),
    };
    let mut bytes = serde_json::to_vec_pretty(&source_info).map_err(|error| JobFailure {
        stage: "io".to_string(),
        category: "io".to_string(),
        message: format!("failed to serialize source info: {error}"),
        retryable: true,
    })?;
    bytes.push(b'\n');
    write_atomic(&artifacts.source_info_path, bytes).map_err(|error| JobFailure {
        stage: "io".to_string(),
        category: "io".to_string(),
        message: format!(
            "failed to write `{}`: {error}",
            artifacts.source_info_path.display()
        ),
        retryable: true,
    })
}

fn file_size(path: &Path) -> Option<u64> {
    fs::metadata(path).map(|metadata| metadata.len()).ok()
}

fn retryable_failure(category: &str) -> bool {
    matches!(
        category,
        "io" | "missing_external_reference" | "resource_limit_exceeded" | "batch_item_failed"
    )
}

fn is_valid_job_id(job_id: &str) -> bool {
    !job_id.is_empty()
        && job_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn unix_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0)
}
