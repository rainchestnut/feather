use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use feather_lite::{
    BATCH_MANIFEST_CONTRACT_VERSION, BatchConversionError, BatchConversionOptions,
    BatchInputDiagnostic, BatchItem, BatchItemStatus, BatchReport, ConversionOptions,
    ImportOptions, batch_failure_category, batch_input_diagnostic, batch_output_file_name,
    collect_batch_input_paths, is_supported_batch_candidate, run_batch_conversion,
    validate_batch_input_path,
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
fn batch_report_serializes_manifest_summary_and_items() {
    let report = BatchReport::new(vec![
        BatchItem {
            index: 0,
            input_path: "input/part.CATPart".to_string(),
            input_size_bytes: Some(120),
            duration_ms: 7,
            status: BatchItemStatus::Ok {
                source_format: "CATIA_CATPart".to_string(),
                output_path: "out/asset_000_part.glb".to_string(),
                metadata_path: Some("out/asset_000_part.metadata.json".to_string()),
                output_size_bytes: Some(80),
                metadata_size_bytes: Some(24),
                node_count: 3,
                mesh_count: 1,
                primitive_count: 2,
                vertex_count: 8,
                triangle_count: 12,
            },
        },
        BatchItem {
            index: 1,
            input_path: "input/reused.CATPart".to_string(),
            input_size_bytes: Some(96),
            duration_ms: 1,
            status: BatchItemStatus::Reused {
                source_format: "CATIA_CATPart".to_string(),
                output_path: "out/asset_001_reused.glb".to_string(),
                metadata_path: Some("out/asset_001_reused.metadata.json".to_string()),
                output_size_bytes: Some(64),
                metadata_size_bytes: Some(16),
                node_count: 2,
                mesh_count: 1,
                primitive_count: 1,
                vertex_count: 3,
                triangle_count: 1,
            },
        },
        BatchItem {
            index: 2,
            input_path: "input/missing.CATProduct".to_string(),
            input_size_bytes: Some(40),
            duration_ms: 3,
            status: BatchItemStatus::Error {
                diagnostic: BatchInputDiagnostic {
                    source_format: Some("CATIA_CATProduct".to_string()),
                    probe_confidence: Some("High".to_string()),
                    embedded_cache: Some(false),
                    probe_reason: Some("extension matched CATProduct".to_string()),
                    ..BatchInputDiagnostic::default()
                },
                stage: "import",
                message: "external reference foo.CATPart could not be resolved".to_string(),
            },
        },
    ]);

    assert_eq!(report.input_count(), 3);
    assert_eq!(report.success_count(), 2);
    assert_eq!(report.converted_count(), 1);
    assert_eq!(report.reused_count(), 1);
    assert_eq!(report.checked_count(), 0);
    assert_eq!(report.failed_count(), 1);

    let summary = report.summary();
    assert_eq!(summary.total_input_bytes, 256);
    assert_eq!(summary.total_output_bytes, 144);
    assert_eq!(summary.total_metadata_bytes, 40);
    assert_eq!(summary.total_node_count, 5);
    assert_eq!(summary.total_mesh_count, 2);
    assert_eq!(summary.total_primitive_count, 3);
    assert_eq!(summary.total_vertex_count, 11);
    assert_eq!(summary.total_triangle_count, 13);
    assert_eq!(
        summary.failure_categories[0].name,
        "missing_external_reference"
    );
    assert_eq!(summary.failure_categories[0].count, 1);

    let json = report.to_manifest_json();
    let parsed_json: serde_json::Value =
        serde_json::from_str(&json).expect("batch manifest JSON should be valid");
    assert_eq!(
        parsed_json["contract_version"],
        BATCH_MANIFEST_CONTRACT_VERSION
    );
    assert_eq!(parsed_json["input_count"], 3);
    assert_eq!(parsed_json["converted_count"], 1);
    assert_eq!(parsed_json["reused_count"], 1);
    assert_eq!(parsed_json["failed_count"], 1);
    assert_eq!(parsed_json["summary"]["total_node_count"], 5);
    assert_eq!(parsed_json["summary"]["total_mesh_count"], 2);
    assert_eq!(parsed_json["summary"]["total_primitive_count"], 3);
    assert_eq!(parsed_json["summary"]["total_vertex_count"], 11);
    assert_eq!(parsed_json["summary"]["total_triangle_count"], 13);
    assert_eq!(parsed_json["items"][0]["status"], "ok");
    assert_eq!(parsed_json["items"][0]["operation"], "converted");
    assert_eq!(parsed_json["items"][0]["node_count"], 3);
    assert_eq!(parsed_json["items"][0]["mesh_count"], 1);
    assert_eq!(parsed_json["items"][0]["primitive_count"], 2);
    assert_eq!(parsed_json["items"][0]["vertex_count"], 8);
    assert_eq!(parsed_json["items"][0]["triangle_count"], 12);
    assert_eq!(
        parsed_json["items"][0]["capability"]["format"],
        "CATIA_CATPart"
    );
    assert_eq!(
        parsed_json["items"][0]["capability"]["requires_visual_payload"],
        true
    );
    assert_eq!(parsed_json["items"][1]["status"], "reused");
    assert_eq!(parsed_json["items"][1]["operation"], "reused");
    assert_eq!(parsed_json["items"][1]["triangle_count"], 1);
    assert_eq!(parsed_json["items"][2]["status"], "error");
    assert_eq!(parsed_json["items"][2]["operation"], "error");
    assert_eq!(
        parsed_json["items"][2]["capability"]["format"],
        "CATIA_CATProduct"
    );
    assert_eq!(
        parsed_json["items"][2]["error_category"],
        "missing_external_reference"
    );
    assert!(
        parsed_json["items"][2]["required_condition"]
            .as_str()
            .expect("batch error should explain required condition")
            .contains("--resolve-dir")
    );
    assert!(json.contains("\"input_count\": 3"));
    assert!(json.contains("\"converted_count\": 1"));
    assert!(json.contains("\"reused_count\": 1"));
    assert!(json.contains("\"failed_count\": 1"));
    assert!(json.contains("\"status\": \"ok\""));
    assert!(json.contains("\"status\": \"reused\""));
    assert!(json.contains("\"status\": \"error\""));
    assert!(json.contains("\"error_category\": \"missing_external_reference\""));
}

#[test]
fn batch_failure_category_has_stable_public_mapping() {
    assert_eq!(
        batch_failure_category(
            "import",
            "resource limit exceeded for ZIP entry count: 10 exceeds 4"
        ),
        "resource_limit_exceeded"
    );
    assert_eq!(
        batch_failure_category(
            "import",
            "private CAD has no readable lightweight visualization cache"
        ),
        "no_readable_lightweight_cache"
    );
    assert_eq!(
        batch_failure_category("import", "B-Rep surface tessellation is pending"),
        "tessellation_pending"
    );
    assert_eq!(
        batch_failure_category(
            "import",
            "CATIA_CATPart contains native CATCGRCont visualization, but its binary representation is not decoded by the open-source importer"
        ),
        "native_visualization_not_decoded"
    );
    assert_eq!(
        batch_failure_category("export", "GLB validation failed"),
        "export"
    );
}

#[test]
fn batch_preflight_helpers_probe_and_validate_input_paths() {
    let temp_dir = unique_temp_dir("batch-preflight");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("fixture.CATPart");
    fs::write(
        &input,
        format!("CATPart-private-prefix\n{SAMPLE_CACHE}\nprivate-suffix"),
    )
    .expect("fixture should be written");

    assert!(is_supported_batch_candidate(&input));

    let diagnostic = batch_input_diagnostic(&input);
    assert_eq!(diagnostic.source_format.as_deref(), Some("CATIA_CATPart"));
    assert_eq!(diagnostic.embedded_cache, Some(true));

    let summary = validate_batch_input_path(&input, &ImportOptions::default())
        .expect("batch preflight should validate importable input");
    assert_eq!(summary.source_format, "CATIA_CATPart");
    assert_eq!(summary.node_count, 1);
    assert_eq!(summary.mesh_count, 1);
    assert_eq!(summary.primitive_count, 1);
    assert_eq!(summary.vertex_count, 3);
    assert_eq!(summary.triangle_count, 1);

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn batch_manifest_reports_catia_v5_container_profile_for_native_cgr_failure() {
    let temp_dir = unique_temp_dir("batch-catia-v5-profile");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("fixture.CATPart");
    fs::write(&input, sample_catia_v5_cfv2("V5R30SP4HF0")).expect("CFV2 fixture should be written");

    let diagnostic = batch_input_diagnostic(&input);
    assert_eq!(diagnostic.container_kind.as_deref(), Some("catia-v5-cfv2"));
    assert_eq!(diagnostic.source_version.as_deref(), Some("V5R30SP4HF0"));
    assert_eq!(
        diagnostic.native_visualization.as_deref(),
        Some("catia-native-cgr-container")
    );

    let output_dir = temp_dir.join("out");
    let run = run_batch_conversion(
        std::slice::from_ref(&input),
        &BatchConversionOptions {
            output_dir,
            manifest_path: None,
            check_only: true,
            conversion: ConversionOptions::default(),
        },
    )
    .expect("failed imports should still produce a batch report");
    assert_eq!(run.report.failed_count(), 1);

    let manifest: serde_json::Value = serde_json::from_str(&run.report.to_manifest_json())
        .expect("batch manifest should be valid JSON");
    let item = &manifest["items"][0];
    assert_eq!(item["container_kind"], "catia-v5-cfv2");
    assert_eq!(item["source_version"], "V5R30SP4HF0");
    assert_eq!(item["native_visualization"], "catia-native-cgr-container");
    assert_eq!(item["error_category"], "native_visualization_not_decoded");

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn batch_collects_directory_inputs_and_names_outputs_from_core_api() {
    let temp_dir = unique_temp_dir("batch-collect");
    let source_dir = temp_dir.join("sources");
    let output_dir = source_dir.join("out");
    fs::create_dir_all(&output_dir).expect("directories should be created");

    let good_part = format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix");
    let extension_candidate = source_dir.join("extension.CATPart");
    let header_candidate = source_dir.join("header.bin");
    let late_unknown = source_dir.join("late-cache.bin");
    let skipped_output = output_dir.join("skip-me.CATPart");
    fs::write(&extension_candidate, &good_part).expect("extension candidate should be written");
    fs::write(&header_candidate, &good_part).expect("header candidate should be written");
    let mut late_payload = vec![b'x'; 9000];
    late_payload.extend_from_slice(good_part.as_bytes());
    fs::write(&late_unknown, late_payload).expect("late unknown should be written");
    fs::write(&skipped_output, &good_part).expect("output dir file should be written");

    let inputs = collect_batch_input_paths(std::slice::from_ref(&source_dir), &output_dir)
        .expect("batch input collection should succeed");

    assert_eq!(inputs.len(), 2);
    assert!(inputs.contains(&extension_candidate));
    assert!(inputs.contains(&header_candidate));
    assert!(!inputs.contains(&late_unknown));
    assert!(!inputs.contains(&skipped_output));
    assert_eq!(
        batch_output_file_name(4, std::path::Path::new("Part A#.CATPart")),
        "asset_004_Part_A.glb"
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn batch_runner_converts_inputs_and_writes_manifest_from_core_api() {
    let temp_dir = unique_temp_dir("batch-runner-convert");
    let source_dir = temp_dir.join("sources");
    let output_dir = temp_dir.join("out");
    fs::create_dir_all(&source_dir).expect("source dir should be created");

    let good_part = source_dir.join("good.CATPart");
    let broken_part = source_dir.join("broken.CATPart");
    fs::write(
        &good_part,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("good fixture should be written");
    fs::write(&broken_part, "CATPart private payload without cache")
        .expect("broken fixture should be written");

    let run = run_batch_conversion(
        &[good_part, broken_part],
        &BatchConversionOptions {
            output_dir,
            manifest_path: None,
            check_only: false,
            conversion: ConversionOptions {
                write_metadata: true,
                ..ConversionOptions::default()
            },
        },
    )
    .expect("batch runner should write a manifest even when one item fails");

    assert_eq!(run.report.input_count(), 2);
    assert_eq!(run.report.converted_count(), 1);
    assert_eq!(run.report.failed_count(), 1);
    assert!(run.manifest_path.is_file());

    let converted = run
        .report
        .items
        .iter()
        .find(|item| item.status.is_converted())
        .expect("one item should convert");
    let BatchItemStatus::Ok {
        output_path,
        metadata_path,
        node_count,
        mesh_count,
        primitive_count,
        vertex_count,
        triangle_count,
        ..
    } = &converted.status
    else {
        panic!("converted item should have ok status");
    };
    assert_eq!(*node_count, 1);
    assert_eq!(*mesh_count, 1);
    assert_eq!(*primitive_count, 1);
    assert_eq!(*vertex_count, 3);
    assert_eq!(*triangle_count, 1);
    assert!(std::path::Path::new(output_path).is_file());
    assert!(
        std::path::Path::new(
            metadata_path
                .as_deref()
                .expect("metadata should be written for converted item")
        )
        .is_file()
    );

    let manifest = fs::read_to_string(&run.manifest_path).expect("manifest should be readable");
    let parsed_manifest: serde_json::Value =
        serde_json::from_str(&manifest).expect("batch runner manifest JSON should be valid");
    assert_eq!(
        parsed_manifest["contract_version"],
        BATCH_MANIFEST_CONTRACT_VERSION
    );
    assert_eq!(parsed_manifest["converted_count"], 1);
    assert_eq!(parsed_manifest["failed_count"], 1);
    assert_eq!(parsed_manifest["summary"]["total_node_count"], 1);
    assert_eq!(parsed_manifest["summary"]["total_mesh_count"], 1);
    assert_eq!(parsed_manifest["summary"]["total_primitive_count"], 1);
    assert_eq!(parsed_manifest["summary"]["total_vertex_count"], 3);
    assert_eq!(parsed_manifest["summary"]["total_triangle_count"], 1);
    let ok = parsed_manifest["items"]
        .as_array()
        .expect("batch items should be an array")
        .iter()
        .find(|item| item["status"] == "ok")
        .expect("one batch item should convert");
    assert_eq!(ok["node_count"], 1);
    assert_eq!(ok["mesh_count"], 1);
    assert_eq!(ok["primitive_count"], 1);
    assert_eq!(ok["vertex_count"], 3);
    assert_eq!(ok["triangle_count"], 1);
    assert_eq!(
        parsed_manifest["summary"]["failure_categories"][0]["category"],
        "no_readable_lightweight_cache"
    );
    let failed = parsed_manifest["items"]
        .as_array()
        .expect("batch items should be an array")
        .iter()
        .find(|item| item["status"] == "error")
        .expect("one batch item should fail");
    assert_eq!(failed["capability"]["format"], "CATIA_CATPart");
    assert_eq!(
        failed["required_condition"],
        "provide a readable lightweight visualization payload: Feather cache, embedded mesh/GLB/glTF/STL/OBJ, ZIP/OLE preview, or resolvable cache-declared reference"
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn batch_manifest_write_failure_removes_atomic_temp_file() {
    let temp_dir = unique_temp_dir("batch-manifest-write-failure");
    let source_dir = temp_dir.join("sources");
    let output_dir = temp_dir.join("out");
    let manifest_path = temp_dir.join("manifest-target");
    fs::create_dir_all(&source_dir).expect("source dir should be created");
    fs::create_dir_all(&manifest_path).expect("manifest failure directory should be created");

    let input = source_dir.join("fixture.CATPart");
    fs::write(
        &input,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let error = run_batch_conversion(
        &[input],
        &BatchConversionOptions {
            output_dir: output_dir.clone(),
            manifest_path: Some(manifest_path.clone()),
            check_only: false,
            conversion: ConversionOptions {
                write_metadata: true,
                ..ConversionOptions::default()
            },
        },
    )
    .expect_err("directory manifest target should fail manifest writing");

    assert!(matches!(
        error,
        BatchConversionError::WriteManifest { path, .. } if path == manifest_path
    ));
    assert!(
        !fs::read_dir(&temp_dir)
            .expect("temp dir should be readable")
            .any(|entry| entry
                .expect("temp entry should be readable")
                .file_name()
                .to_string_lossy()
                .contains(".manifest-target.tmp-"))
    );
    assert!(
        fs::read_dir(&output_dir)
            .expect("output dir should be readable")
            .next()
            .is_none()
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn batch_runner_check_only_validates_without_exporting_outputs() {
    let temp_dir = unique_temp_dir("batch-runner-check");
    let source_dir = temp_dir.join("sources");
    let output_dir = temp_dir.join("out");
    fs::create_dir_all(&source_dir).expect("source dir should be created");

    let input = source_dir.join("fixture.CATPart");
    fs::write(
        &input,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let run = run_batch_conversion(
        &[input],
        &BatchConversionOptions {
            output_dir: output_dir.clone(),
            manifest_path: None,
            check_only: true,
            conversion: ConversionOptions::default(),
        },
    )
    .expect("check-only batch runner should validate and write manifest");

    assert_eq!(run.report.input_count(), 1);
    assert_eq!(run.report.checked_count(), 1);
    assert_eq!(run.report.converted_count(), 0);
    assert_eq!(run.report.failed_count(), 0);
    assert!(run.manifest_path.is_file());
    assert!(
        fs::read_dir(&output_dir)
            .expect("output dir should exist")
            .all(|entry| entry
                .expect("output entry should be readable")
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("glb"))
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after UNIX epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("feather-lite-{prefix}-{suffix}"))
}

fn sample_catia_v5_cfv2(release: &str) -> Vec<u8> {
    let mut bytes = vec![0_u8; 256];
    bytes[..b"V5_CFV2".len()].copy_from_slice(b"V5_CFV2");
    bytes[8..12].copy_from_slice(&192_u32.to_be_bytes());
    bytes[12..16].copy_from_slice(&64_u32.to_be_bytes());
    bytes[32..32 + release.len()].copy_from_slice(release.as_bytes());
    bytes[64..74].copy_from_slice(b"CATCGRCont");
    bytes[96..103].copy_from_slice(b"CATPart");
    bytes
}
