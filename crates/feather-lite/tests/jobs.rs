use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use feather_lite::{
    JOB_RECORD_CONTRACT_VERSION, JobConversionSettings, JobResult, JobStatus, LocalJobStore,
};

const SAMPLE_CACHE: &str = "\
FEATHER_CAD_LITE_CACHE_V1
material Default 0.8 0.8 0.82 1.0
mesh Tri
primitive 0
v 0 0 0
v 1 0 0
v 0 1 0
tri 0 1 2
endprimitive
endmesh
node Tri 0 root
END_FEATHER_CAD_LITE_CACHE
";

#[test]
fn conversion_job_writes_artifact_package_and_record() {
    let temp_dir = unique_temp_dir("job-convert");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let store = LocalJobStore::new(temp_dir.join("store"));
    let queued = store
        .create_conversion_job(&input_path, JobConversionSettings::default())
        .expect("job should be created");
    assert_eq!(queued.status, JobStatus::Queued);

    let completed = store.run_job(&queued.job_id).expect("job should run");
    assert_eq!(completed.status, JobStatus::Succeeded);
    assert!(completed.artifacts.source_info_path.is_file());
    assert!(
        completed
            .artifacts
            .model_path
            .as_ref()
            .expect("model path should be reserved")
            .is_file()
    );
    assert!(
        completed
            .artifacts
            .metadata_path
            .as_ref()
            .expect("metadata path should be reserved")
            .is_file()
    );
    assert!(matches!(
        completed.result,
        Some(JobResult::Conversion {
            node_count: 1,
            mesh_count: 1,
            primitive_count: 1,
            vertex_count: 3,
            triangle_count: 1,
            ..
        })
    ));

    let loaded = store
        .load_job(&completed.job_id)
        .expect("job should load from disk");
    assert_eq!(loaded, completed);
    let parsed: serde_json::Value =
        serde_json::from_str(&loaded.to_json_string()).expect("job JSON should parse");
    assert_eq!(parsed["contract_version"], JOB_RECORD_CONTRACT_VERSION);
    assert_eq!(parsed["status"], "succeeded");

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn failed_conversion_job_can_be_retried_after_source_is_fixed() {
    let temp_dir = unique_temp_dir("job-retry");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    fs::write(&input_path, "CATPart private payload without cache")
        .expect("broken fixture should be written");

    let store = LocalJobStore::new(temp_dir.join("store"));
    let queued = store
        .create_conversion_job(&input_path, JobConversionSettings::default())
        .expect("job should be created");
    let failed = store
        .run_job(&queued.job_id)
        .expect("conversion failure should persist as a failed job");
    assert_eq!(failed.status, JobStatus::Failed);
    assert_eq!(
        failed
            .failure
            .as_ref()
            .expect("failure should be recorded")
            .category,
        "no_readable_lightweight_cache"
    );

    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixed fixture should be written");
    let retried = store.retry_job(&queued.job_id).expect("job should retry");

    assert_eq!(retried.status, JobStatus::Succeeded);
    assert!(retried.failure.is_none());
    assert!(matches!(retried.result, Some(JobResult::Conversion { .. })));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn batch_job_writes_manifest_artifact_package() {
    let temp_dir = unique_temp_dir("job-batch");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let store = LocalJobStore::new(temp_dir.join("store"));
    let queued = store
        .create_batch_job(
            vec![input_path],
            false,
            JobConversionSettings {
                write_metadata: true,
                ..JobConversionSettings::default()
            },
        )
        .expect("batch job should be created");
    let completed = store.run_job(&queued.job_id).expect("batch job should run");

    assert_eq!(completed.status, JobStatus::Succeeded);
    assert!(
        completed
            .artifacts
            .manifest_path
            .as_ref()
            .expect("manifest path should be reserved")
            .is_file()
    );
    assert!(matches!(
        completed.result,
        Some(JobResult::Batch {
            input_count: 1,
            converted_count: 1,
            failed_count: 0,
            ..
        })
    ));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

fn unique_temp_dir(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after UNIX epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("feather-lite-{label}-{suffix}"))
}
