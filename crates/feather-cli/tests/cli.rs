use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use feather_lite::{
    BATCH_MANIFEST_CONTRACT_VERSION, CACHE_DUMP_MANIFEST_CONTRACT_VERSION,
    FORMAT_CAPABILITIES_CONTRACT_VERSION, GlbExportOptions, INSPECT_REPORT_CONTRACT_VERSION,
    JOB_RECORD_CONTRACT_VERSION, LiteDocument, LiteMaterial, LiteMesh, LiteNode, LitePrimitive,
    export_glb,
};
use miniz_oxide::deflate::compress_to_vec;

#[test]
fn formats_lists_real_support_contract() {
    let output = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("formats")
        .output()
        .expect("formats command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("CATIA_CATPart"));
    assert!(stdout.contains("CATIA_CGR"));
    assert!(stdout.contains("DASSAULT_3DXML"));
    assert!(stdout.contains("NX_PRT"));
    assert!(stdout.contains("SOLIDWORKS_SLDPRT"));
    assert!(stdout.contains("SOLIDWORKS_SLDASM"));
    assert!(stdout.contains("PRIVATE_CAD"));
    assert!(stdout.contains(".jt"));
    assert!(stdout.contains(".sat"));
    assert!(stdout.contains(".iges"));
    assert!(stdout.contains(".model"));
    assert!(stdout.contains("generic private CAD cache-first fallback"));
    assert!(stdout.contains("FeatherLiteCache"));
    assert!(stdout.contains(".flite"));
    assert!(stdout.contains("standalone Feather Lite cache"));
    assert!(stdout.contains("STL"));
    assert!(stdout.contains("OBJ"));
    assert!(stdout.contains("GLB"));
    assert!(stdout.contains("data URI"));
    assert!(stdout.contains("node TRS/matrix transforms"));
    assert!(stdout.contains("interleaved/offset bufferViews"));
    assert!(stdout.contains("AP242 tessellated faces"));
    assert!(stdout.contains(
        "PLANE/CYLINDRICAL_SURFACE/CONICAL_SURFACE/SPHERICAL_SURFACE and regular ring TOROIDAL_SURFACE"
    ));
    assert!(stdout.contains("embedded STL"));
    assert!(stdout.contains("embedded STL/OBJ"));
    assert!(stdout.contains("native binary CATCGRCont sections are detected but not decoded"));
    assert!(stdout.contains("cache-declared external references"));
    assert!(stdout.contains("resolve-dir external references"));
    assert!(stdout.contains("map-root remapped external references"));
    assert!(stdout.contains("ZIP XML assembly manifests"));
    assert!(stdout.contains("ProductStructure ID relationships"));
    assert!(stdout.contains("XML 3DRep polygonal tessellation"));
    assert!(stdout.contains("urn:3DXML references"));
    assert!(stdout.contains("RelativeMatrix transforms"));
    assert!(stdout.contains("mesh cleaning/quantization/LOD"));
    assert!(stdout.contains("proprietary assembly references and transforms are detected"));
    assert!(stdout.contains("B_SPLINE_CURVE_WITH_KNOTS boundaries"));
    assert!(stdout.contains("parameter TRIMMED_CURVE spans"));
    assert!(stdout.contains("Cartesian-only trimmed curves"));
    assert!(stdout.contains("cone faces reaching or crossing the apex"));
    assert!(stdout.contains("sphere faces touching parameterization poles"));
    assert!(stdout.contains("bounded outer/inner loops"));
    assert!(stdout.contains("shape-representation assembly hierarchy"));
    assert!(stdout.contains("ITEM_DEFINED_TRANSFORMATION"));
    assert!(stdout.contains("horn and spindle tori"));
    assert!(stdout.contains("non-meridian/non-parallel torus circles"));
    assert!(stdout.contains("binary/encrypted 3DRep streams"));
}

#[test]
fn formats_json_lists_machine_readable_support_contract() {
    let output = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("formats")
        .arg("--json")
        .output()
        .expect("formats --json command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("formats --json should emit valid JSON");
    assert_eq!(
        parsed["contract_version"],
        FORMAT_CAPABILITIES_CONTRACT_VERSION
    );
    let formats = parsed["formats"]
        .as_array()
        .expect("formats field should be an array");

    let catproduct = formats
        .iter()
        .find(|format| format["format"] == "CATIA_CATProduct")
        .expect("CATProduct contract should be present");
    assert_eq!(catproduct["requires_visual_payload"], true);
    assert_eq!(catproduct["status"], "partial");
    assert_eq!(catproduct["available"], false);
    assert_eq!(catproduct["supports_external_references"], true);
    assert_eq!(catproduct["native_brep_tessellation"], "not_decoded");

    let step = formats
        .iter()
        .find(|format| format["format"] == "STEP")
        .expect("STEP contract should be present");
    assert_eq!(step["status"], "partial");
    assert_eq!(step["supports_native_tessellation"], true);
    assert_eq!(step["native_brep_tessellation"], "partial");
}

#[test]
fn inspect_and_convert_fixture_from_cli() {
    let fixture = workspace_fixture("sample_embedded_cache.CATPart");

    let inspect = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("inspect")
        .arg("--check")
        .arg(&fixture)
        .output()
        .expect("inspect command should run");

    assert!(inspect.status.success());
    let inspect_stdout = String::from_utf8(inspect.stdout).expect("stdout should be UTF-8");
    assert!(inspect_stdout.contains("format: CATIA_CATPart"));
    assert!(inspect_stdout.contains("embedded_cache: true"));
    assert!(inspect_stdout.contains("visual_assets: 1"));
    assert!(inspect_stdout.contains("importable: true"));
    assert!(inspect_stdout.contains("triangles: 2"));
    assert!(inspect_stdout.contains("kind: feather-cache"));

    let temp_dir = unique_temp_dir("cache-fixture");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let output_glb = temp_dir.join("sample.glb");
    let metadata = temp_dir.join("sample.metadata.json");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&fixture)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );
    let convert_stdout = String::from_utf8(convert.stdout).expect("stdout should be UTF-8");
    assert!(convert_stdout.contains("nodes: 1"));
    assert!(convert_stdout.contains("meshes: 1"));
    assert!(convert_stdout.contains("primitives: 1"));
    assert!(convert_stdout.contains("vertices: 4"));
    assert!(convert_stdout.contains("triangles: 2"));

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(metadata_json.contains("\"node_count\": 1"));
    assert!(metadata_json.contains("\"primitive_count\": 1"));
    assert!(metadata_json.contains("\"vertex_count\": 4"));
    assert!(metadata_json.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn job_convert_creates_business_artifact_package_from_cli() {
    let fixture = workspace_fixture("sample_embedded_cache.CATPart");
    let temp_dir = unique_temp_dir("job-convert");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let store_dir = temp_dir.join("jobs");

    let output = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("job")
        .arg("convert")
        .arg(&fixture)
        .arg("--store")
        .arg(&store_dir)
        .arg("--json")
        .output()
        .expect("job convert command should run");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("job convert should emit valid JSON");
    assert_eq!(parsed["contract_version"], JOB_RECORD_CONTRACT_VERSION);
    assert_eq!(parsed["status"], "succeeded");
    assert_eq!(parsed["result"]["kind"], "conversion");
    assert_eq!(parsed["result"]["triangle_count"], 2);
    let model_path = parsed["artifacts"]["model_path"]
        .as_str()
        .expect("model path should be present");
    let metadata_path = parsed["artifacts"]["metadata_path"]
        .as_str()
        .expect("metadata path should be present");
    let source_info_path = parsed["artifacts"]["source_info_path"]
        .as_str()
        .expect("source info path should be present");
    assert!(PathBuf::from(model_path).is_file());
    assert!(PathBuf::from(metadata_path).is_file());
    assert!(PathBuf::from(source_info_path).is_file());

    let job_id = parsed["job_id"].as_str().expect("job id should be present");
    let status = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("job")
        .arg("status")
        .arg(job_id)
        .arg("--store")
        .arg(&store_dir)
        .arg("--json")
        .output()
        .expect("job status command should run");
    assert!(status.status.success());
    let status_stdout = String::from_utf8(status.stdout).expect("stdout should be UTF-8");
    let status_json: serde_json::Value =
        serde_json::from_str(&status_stdout).expect("job status should emit valid JSON");
    assert_eq!(status_json["job_id"], job_id);
    assert_eq!(status_json["status"], "succeeded");

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_native_ap214_planar_brep_from_cli() {
    let fixture = workspace_fixture("sample_ap214_planar_brep.step");
    let temp_dir = unique_temp_dir("step-planar-brep");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let output_glb = temp_dir.join("planar-box.glb");
    let metadata = temp_dir.join("planar-box.metadata.json");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&fixture)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("STEP convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );
    let glb = fs::read(&output_glb).expect("STEP GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());
    let metadata = fs::read_to_string(metadata).expect("STEP metadata should be written");
    assert!(metadata.contains("\"mode\": \"step-brep-tessellated\""));
    assert!(metadata.contains("\"triangle_count\": 12"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cylindrical_step_with_tessellation_controls() {
    let fixture = workspace_fixture("sample_ap214_cylindrical_brep.step");
    let temp_dir = unique_temp_dir("step-cylinder");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let output_glb = temp_dir.join("cylinder.glb");
    let metadata = temp_dir.join("cylinder.metadata.json");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&fixture)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .arg("--chord-error")
        .arg("0.1")
        .arg("--max-step-curve-segments")
        .arg("4")
        .output()
        .expect("STEP cylinder convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );
    let glb = fs::read(&output_glb).expect("STEP cylinder GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());
    let metadata = fs::read_to_string(metadata).expect("STEP metadata should be written");
    assert!(metadata.contains("\"mode\": \"step-brep-tessellated\""));
    assert!(metadata.contains("\"triangle_count\": 8"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_conical_step_with_degree_angle_unit() {
    let fixture = workspace_fixture("sample_ap214_conical_brep.step");
    let temp_dir = unique_temp_dir("step-cone");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let output_glb = temp_dir.join("cone.glb");
    let metadata = temp_dir.join("cone.metadata.json");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&fixture)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .arg("--chord-error")
        .arg("0.1")
        .arg("--max-step-curve-segments")
        .arg("8")
        .output()
        .expect("STEP cone convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );
    let glb = fs::read(&output_glb).expect("STEP cone GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());
    let metadata = fs::read_to_string(metadata).expect("STEP cone metadata should be written");
    assert!(metadata.contains("\"mode\": \"step-brep-tessellated\""));
    assert!(metadata.contains("\"triangle_count\": 9"));
    assert!(metadata.contains("interpreted STEP degree plane angles as radians"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_analytic_step_geometry() {
    let temp_dir = unique_temp_dir("step-analytic");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let cases = [
        (
            "sample_ap214_closed_ellipse_brep.step",
            "ellipse",
            "0.1",
            10_u64,
        ),
        ("sample_ap214_spherical_brep.step", "sphere", "0.05", 8_u64),
        ("sample_ap214_toroidal_brep.step", "torus", "0.1", 9_u64),
        (
            "sample_ap214_planar_hole_brep.step",
            "planar-hole",
            "0.1",
            8_u64,
        ),
        (
            "sample_ap214_cylindrical_hole_brep.step",
            "cylinder-hole",
            "0.1",
            16_u64,
        ),
        (
            "sample_ap214_conical_hole_brep.step",
            "cone-hole",
            "0.1",
            18_u64,
        ),
        (
            "sample_ap214_spherical_hole_brep.step",
            "sphere-hole",
            "0.05",
            16_u64,
        ),
        (
            "sample_ap214_toroidal_hole_brep.step",
            "torus-hole",
            "0.1",
            17_u64,
        ),
        (
            "sample_ap214_planar_bspline_brep.step",
            "planar-bspline",
            "0.05",
            3_u64,
        ),
        (
            "sample_ap214_planar_trimmed_bspline_brep.step",
            "planar-trimmed-bspline",
            "0.02",
            3_u64,
        ),
        (
            "sample_ap214_planar_trimmed_line_brep.step",
            "planar-trimmed-line",
            "0.1",
            2_u64,
        ),
        (
            "sample_ap214_planar_trimmed_circle_brep.step",
            "planar-trimmed-circle",
            "0.05",
            3_u64,
        ),
        (
            "sample_ap214_planar_trimmed_ellipse_brep.step",
            "planar-trimmed-ellipse",
            "0.05",
            4_u64,
        ),
        (
            "sample_ap214_linear_extrusion_plane_brep.step",
            "linear-extrusion-plane",
            "0.1",
            2_u64,
        ),
        (
            "sample_ap214_linear_extrusion_cylinder_brep.step",
            "linear-extrusion-cylinder",
            "0.1",
            8_u64,
        ),
    ];

    for (fixture_name, output_name, chord_error, triangle_count) in cases {
        let fixture = workspace_fixture(fixture_name);
        let output_glb = temp_dir.join(format!("{output_name}.glb"));
        let metadata = temp_dir.join(format!("{output_name}.metadata.json"));
        let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
            .arg("convert")
            .arg(&fixture)
            .arg("-o")
            .arg(&output_glb)
            .arg("--metadata")
            .arg(&metadata)
            .arg("--chord-error")
            .arg(chord_error)
            .arg("--max-step-curve-segments")
            .arg("32")
            .output()
            .expect("STEP analytic geometry convert command should run");

        assert!(
            convert.status.success(),
            "{}",
            String::from_utf8_lossy(&convert.stderr)
        );
        let glb = fs::read(&output_glb).expect("STEP GLB should be written");
        assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());
        let metadata = fs::read_to_string(metadata).expect("STEP metadata should be written");
        assert!(metadata.contains("\"mode\": \"step-brep-tessellated\""));
        assert!(metadata.contains(&format!("\"triangle_count\": {triangle_count}")));
    }

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn enforces_step_face_limits_from_cli() {
    let fixture = workspace_fixture("sample_ap214_planar_hole_brep.step");
    let temp_dir = unique_temp_dir("step-face-limits");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");

    for (option, limit, expected_resource) in [
        ("--max-step-face-loops", "1", "STEP face loops"),
        ("--max-step-face-vertices", "7", "STEP face vertices"),
    ] {
        let output_glb = temp_dir.join(format!("{}.glb", &option[2..]));
        let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
            .arg("convert")
            .arg(&fixture)
            .arg("-o")
            .arg(&output_glb)
            .arg(option)
            .arg(limit)
            .output()
            .expect("STEP limited convert command should run");

        assert!(!convert.status.success());
        assert!(
            String::from_utf8_lossy(&convert.stderr).contains(expected_resource),
            "{}",
            String::from_utf8_lossy(&convert.stderr)
        );
        assert!(!output_glb.exists());
    }

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn enforces_step_bspline_limits_from_cli() {
    let fixture = workspace_fixture("sample_ap214_planar_bspline_brep.step");
    let temp_dir = unique_temp_dir("step-bspline-limits");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");

    for (option, limit, expected_resource) in [
        ("--max-step-spline-degree", "1", "STEP spline degree"),
        (
            "--max-step-spline-control-points",
            "2",
            "STEP spline control points",
        ),
    ] {
        let output_glb = temp_dir.join(format!("{}.glb", &option[2..]));
        let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
            .arg("convert")
            .arg(&fixture)
            .arg("-o")
            .arg(&output_glb)
            .arg(option)
            .arg(limit)
            .output()
            .expect("STEP B-Spline limited convert command should run");

        assert!(!convert.status.success());
        assert!(
            String::from_utf8_lossy(&convert.stderr).contains(expected_resource),
            "{}",
            String::from_utf8_lossy(&convert.stderr)
        );
    }

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_step_assemblies_and_enforces_node_limit() {
    let temp_dir = unique_temp_dir("step-assemblies");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let cases = [
        (
            "sample_ap214_brep_assembly.step",
            "ap214",
            "step-brep-assembly-tessellated",
        ),
        (
            "sample_ap242_tessellated_assembly.step",
            "ap242",
            "step-ap242-assembly-tessellated",
        ),
    ];

    for (fixture_name, output_name, mode) in cases {
        let fixture = workspace_fixture(fixture_name);
        let output_glb = temp_dir.join(format!("{output_name}.glb"));
        let metadata = temp_dir.join(format!("{output_name}.metadata.json"));
        let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
            .arg("convert")
            .arg(&fixture)
            .arg("-o")
            .arg(&output_glb)
            .arg("--metadata")
            .arg(&metadata)
            .arg("--max-step-assembly-nodes")
            .arg("16")
            .output()
            .expect("STEP assembly convert command should run");

        assert!(
            convert.status.success(),
            "{}",
            String::from_utf8_lossy(&convert.stderr)
        );
        let glb = fs::read(&output_glb).expect("STEP assembly GLB should be written");
        assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());
        let glb_text = String::from_utf8_lossy(&glb);
        assert!(glb_text.contains("RootAssembly"));
        assert!(glb_text.contains("SubOne"));
        assert!(glb_text.contains("\"children\""));
        assert!(glb_text.contains("\"matrix\""));
        let metadata = fs::read_to_string(metadata).expect("assembly metadata should be written");
        assert!(metadata.contains(&format!("\"mode\": \"{mode}\"")));
        assert!(metadata.contains("\"mesh_count\": 2"));
        assert!(metadata.contains("\"triangle_count\": 3"));
    }

    let fixture = workspace_fixture("sample_ap214_brep_assembly.step");
    let output_glb = temp_dir.join("limited.glb");
    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&fixture)
        .arg("-o")
        .arg(&output_glb)
        .arg("--max-step-assembly-nodes")
        .arg("6")
        .output()
        .expect("limited STEP assembly convert command should run");
    assert!(!convert.status.success());
    assert!(String::from_utf8_lossy(&convert.stderr).contains("STEP assembly nodes"));
    assert!(!output_glb.exists());

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn inspect_json_reports_embedded_visual_assets() {
    let temp_dir = unique_temp_dir("inspect-json-assets");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("inspectable.CATPart");
    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/part-a.obj",
        sample_obj().as_bytes(),
    ));
    bytes.extend_from_slice(&stored_zip_entry("preview/part-b.glb", &sample_glb()));
    fs::write(&input, bytes).expect("test CATPart should be written");

    let inspect = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("inspect")
        .arg("--json")
        .arg("--check")
        .arg(&input)
        .output()
        .expect("inspect command should run");

    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );

    let stdout = String::from_utf8(inspect.stdout).expect("stdout should be UTF-8");
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("inspect JSON should be valid");
    assert_eq!(parsed["contract_version"], INSPECT_REPORT_CONTRACT_VERSION);
    assert_eq!(parsed["capability"]["requires_visual_payload"], true);
    assert_eq!(parsed["capability"]["supports_embedded_assets"], true);
    assert_eq!(
        parsed["import_check"]["failure_category"],
        serde_json::Value::Null
    );
    assert!(stdout.contains("\"format\": \"CATIA_CATPart\""));
    assert!(stdout.contains("\"embedded_cache\": false"));
    assert!(stdout.contains("\"import_check\":"));
    assert!(stdout.contains("\"importable\": true"));
    assert!(stdout.contains("\"triangle_count\": 4"));
    assert!(stdout.contains("\"visual_asset_count\": 2"));
    assert!(stdout.contains("\"kind\": \"obj\""));
    assert!(stdout.contains("\"kind\": \"glb\""));
    assert!(stdout.contains("\"source\": \"zip-entry\""));
    assert!(stdout.contains("\"name\": \"preview/part-a.obj\""));
    assert!(stdout.contains("\"name\": \"preview/part-b.glb\""));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn inspect_check_reports_unimportable_private_cad() {
    let temp_dir = unique_temp_dir("inspect-check-failure");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("no_preview.CATPart");
    fs::write(&input, "CATPart private payload without readable preview")
        .expect("test CATPart should be written");

    let inspect = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("inspect")
        .arg("--json")
        .arg("--check")
        .arg(&input)
        .output()
        .expect("inspect command should run");

    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );

    let stdout = String::from_utf8(inspect.stdout).expect("stdout should be UTF-8");
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("inspect JSON should be valid");
    assert_eq!(
        parsed["import_check"]["failure_category"],
        "no_readable_lightweight_cache"
    );
    assert_eq!(parsed["import_check"]["failure_stage"], "import");
    assert!(
        parsed["import_check"]["required_condition"]
            .as_str()
            .expect("required condition should be a string")
            .contains("readable lightweight visualization payload")
    );
    assert!(stdout.contains("\"format\": \"CATIA_CATPart\""));
    assert!(stdout.contains("\"importable\": false"));
    assert!(stdout.contains("has no readable lightweight visualization cache"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn archive_limits_are_enforced_by_inspect_batch_and_dump_cache_cli() {
    let temp_dir = unique_temp_dir("archive-limits");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("limited.CATPart");
    let payload = sample_obj().as_bytes();
    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry("preview/model.obj", payload));
    fs::write(&input, bytes).expect("ZIP-backed CATPart fixture should be written");
    let limit = (payload.len() - 1).to_string();

    let inspect = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("inspect")
        .arg(&input)
        .arg("--max-archive-entry-bytes")
        .arg(&limit)
        .output()
        .expect("inspect command should run");
    assert!(!inspect.status.success());
    assert!(String::from_utf8_lossy(&inspect.stderr).contains("resource limit exceeded"));

    let batch_output = temp_dir.join("batch");
    let batch = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("batch")
        .arg(&input)
        .arg("--out")
        .arg(&batch_output)
        .arg("--check-only")
        .arg("--max-archive-entry-bytes")
        .arg(&limit)
        .output()
        .expect("batch command should run");
    assert!(!batch.status.success());
    let manifest = fs::read_to_string(batch_output.join("manifest.json"))
        .expect("batch should write a failure manifest");
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest).expect("batch manifest should be valid JSON");
    assert_eq!(
        manifest["items"][0]["error_category"],
        "resource_limit_exceeded"
    );

    let dump_output = temp_dir.join("dump");
    let dump = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("dump-cache")
        .arg(&input)
        .arg("--out")
        .arg(&dump_output)
        .arg("--max-archive-entry-bytes")
        .arg(&limit)
        .output()
        .expect("dump-cache command should run");
    assert!(!dump.status.success());
    assert!(String::from_utf8_lossy(&dump.stderr).contains("resource limit exceeded"));
    assert!(!dump_output.exists());

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn input_limits_are_enforced_by_every_file_processing_command() {
    let temp_dir = unique_temp_dir("input-limits");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("limited.CATPart");
    let bytes = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    fs::write(&input, &bytes).expect("CATPart fixture should be written");
    let limit = (bytes.len() - 1).to_string();

    let inspect = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("inspect")
        .arg(&input)
        .arg("--max-input-bytes")
        .arg(&limit)
        .output()
        .expect("inspect command should run");
    assert!(!inspect.status.success());
    assert!(String::from_utf8_lossy(&inspect.stderr).contains("input bytes"));

    let converted = temp_dir.join("limited.glb");
    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&converted)
        .arg("--max-input-bytes")
        .arg(&limit)
        .output()
        .expect("convert command should run");
    assert!(!convert.status.success());
    assert!(String::from_utf8_lossy(&convert.stderr).contains("input bytes"));
    assert!(!converted.exists());

    let batch_output = temp_dir.join("batch");
    let batch = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("batch")
        .arg(&input)
        .arg("--out")
        .arg(&batch_output)
        .arg("--check-only")
        .arg("--max-input-bytes")
        .arg(&limit)
        .output()
        .expect("batch command should run");
    assert!(!batch.status.success());
    let manifest = fs::read_to_string(batch_output.join("manifest.json"))
        .expect("batch should write a failure manifest");
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest).expect("batch manifest should be valid JSON");
    assert_eq!(
        manifest["items"][0]["error_category"],
        "resource_limit_exceeded"
    );

    let dump_output = temp_dir.join("dump");
    let dump = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("dump-cache")
        .arg(&input)
        .arg("--out")
        .arg(&dump_output)
        .arg("--max-input-bytes")
        .arg(&limit)
        .output()
        .expect("dump-cache command should run");
    assert!(!dump.status.success());
    assert!(String::from_utf8_lossy(&dump.stderr).contains("input bytes"));
    assert!(!dump_output.exists());

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn inspect_reports_catia_v5_cfv2_native_visualization_from_cli() {
    let temp_dir = unique_temp_dir("inspect-catia-v5-profile");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("native.CATPart");
    fs::write(&input, sample_catia_v5_cfv2("V5R30SP4HF0")).expect("CFV2 fixture should be written");

    let inspect = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("inspect")
        .arg("--json")
        .arg("--check")
        .arg(&input)
        .output()
        .expect("inspect command should run");
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );

    let stdout = String::from_utf8(inspect.stdout).expect("stdout should be UTF-8");
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("inspect JSON should be valid");
    assert_eq!(parsed["confidence"], "Certain");
    assert_eq!(parsed["container_kind"], "catia-v5-cfv2");
    assert_eq!(parsed["source_version"], "V5R30SP4HF0");
    assert_eq!(parsed["native_visualization"], "catia-native-cgr-container");
    assert_eq!(
        parsed["import_check"]["failure_category"],
        "native_visualization_not_decoded"
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn inspect_check_resolves_assembly_references_from_cli() {
    let temp_dir = unique_temp_dir("inspect-check-resolve-dir");
    let parts_dir = temp_dir.join("parts");
    fs::create_dir_all(&parts_dir).expect("parts dir should be created");
    let input = temp_dir.join("assembly.CATProduct");

    let part = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    fs::write(parts_dir.join("part-a.CATPart"), part).expect("part should be written");

    let assembly = "\
CATProduct private payload prefix
FEATHER_CAD_LITE_CACHE_V1
document Assembly
reference PartA part-a.CATPart root
END_FEATHER_CAD_LITE_CACHE
";
    fs::write(&input, assembly).expect("assembly should be written");

    let inspect = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("inspect")
        .arg("--json")
        .arg("--check")
        .arg("--resolve-dir")
        .arg(&parts_dir)
        .arg(&input)
        .output()
        .expect("inspect command should run");

    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );

    let stdout = String::from_utf8(inspect.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("\"format\": \"CATIA_CATProduct\""));
    assert!(stdout.contains("\"importable\": true"));
    assert!(stdout.contains("\"mesh_count\": 1"));
    assert!(stdout.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn inspect_check_resolves_assembly_references_with_map_root_from_cli() {
    let temp_dir = unique_temp_dir("inspect-check-map-root");
    let migrated_root = temp_dir.join("released-package");
    let migrated_parts_dir = migrated_root.join("released");
    fs::create_dir_all(&migrated_parts_dir).expect("migrated parts dir should be created");
    let input = temp_dir.join("assembly.CATProduct");

    let part = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    fs::write(migrated_parts_dir.join("part-b.CATPart"), part)
        .expect("migrated part should be written");

    let assembly = r#"
CATProduct private payload prefix
FEATHER_CAD_LITE_CACHE_V1
document Assembly
reference PartB C:\vault\legacy\released\part-b.CATPart root
END_FEATHER_CAD_LITE_CACHE
"#;
    fs::write(&input, assembly).expect("assembly should be written");

    let inspect = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("inspect")
        .arg("--json")
        .arg("--check")
        .arg("--map-root")
        .arg(format!(r"C:\vault\legacy={}", migrated_root.display()))
        .arg(&input)
        .output()
        .expect("inspect command should run");

    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );

    let stdout = String::from_utf8(inspect.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("\"format\": \"CATIA_CATProduct\""));
    assert!(stdout.contains("\"importable\": true"));
    assert!(stdout.contains("\"mesh_count\": 1"));
    assert!(stdout.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cadpart_with_embedded_binary_stl_from_cli() {
    let temp_dir = unique_temp_dir("embedded-stl");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("embedded_stl.CADPart");
    let output_glb = temp_dir.join("embedded_stl.glb");
    let metadata = temp_dir.join("embedded_stl.metadata.json");

    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&sample_binary_stl());
    bytes.extend_from_slice(b"CATPart private payload suffix");
    fs::write(&input, bytes).expect("test CADPart should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(metadata_json.contains("\"mode\": \"catia-embedded-visual-asset\""));
    assert!(metadata_json.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cadpart_with_embedded_obj_from_cli() {
    let temp_dir = unique_temp_dir("embedded-obj");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("embedded_obj.CADPart");
    let output_glb = temp_dir.join("embedded_obj.glb");
    let metadata = temp_dir.join("embedded_obj.metadata.json");

    let bytes = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    fs::write(&input, bytes).expect("test CADPart should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(metadata_json.contains("\"mode\": \"catia-embedded-visual-asset\""));
    assert!(metadata_json.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cadpart_with_max_triangles_lod_from_cli() {
    let temp_dir = unique_temp_dir("lod-obj");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("lod_obj.CADPart");
    let output_glb = temp_dir.join("lod_obj.glb");
    let metadata = temp_dir.join("lod_obj.metadata.json");

    let bytes = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    fs::write(&input, bytes).expect("test CADPart should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .arg("--max-triangles")
        .arg("1")
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(metadata_json.contains("\"triangle_count\": 1"));
    assert!(metadata_json.contains("applied triangle budget LOD"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cadpart_with_mesh_quantization_from_cli() {
    let temp_dir = unique_temp_dir("quantize-obj");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("quantized_obj.CATPart");
    let output_glb = temp_dir.join("quantized_obj.glb");
    let metadata = temp_dir.join("quantized_obj.metadata.json");

    let obj = "# Wavefront OBJ visual cache
o Plate
v 0.02 0 0
v 1.26 0.24 -0.26
v 2.01 1.02 0
vn 0 0 1
f 1//1 2//1 3//1";
    let bytes = format!("CATPart private payload prefix\n{obj}\nCATPart private payload suffix");
    fs::write(&input, bytes).expect("test CADPart should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .arg("--quantize")
        .arg("0.5")
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(metadata_json.contains("\"triangle_count\": 1"));
    assert!(metadata_json.contains("quantized mesh positions to grid step 0.5000000"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cadpart_and_prunes_quantized_degenerate_triangles_from_cli() {
    let temp_dir = unique_temp_dir("prune-degenerate");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("degenerate_after_quantize.CATPart");
    let output_glb = temp_dir.join("degenerate_after_quantize.glb");
    let metadata = temp_dir.join("degenerate_after_quantize.metadata.json");

    let obj = "# Wavefront OBJ visual cache
o Plate
v 0 0 0
v 2 0 0
v 2 0.4 0
v 0 2 0
f 1 2 3
f 1 3 4";
    let bytes = format!("CATPart private payload prefix\n{obj}\nCATPart private payload suffix");
    fs::write(&input, bytes).expect("test CADPart should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .arg("--quantize")
        .arg("1.0")
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"triangle_count\": 1"));
    assert!(metadata_json.contains("removed 1 degenerate triangles after mesh cleanup"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cadpart_without_normals_from_cli() {
    let temp_dir = unique_temp_dir("no-normals");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("no_normals.CATPart");
    let output_glb = temp_dir.join("no_normals.glb");
    let metadata = temp_dir.join("no_normals.metadata.json");

    let bytes = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    fs::write(&input, bytes).expect("test CATPart should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .arg("--no-normals")
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    let json = glb_json_chunk(&glb);
    assert!(json.contains("\"POSITION\""));
    assert!(!json.contains("\"NORMAL\""));

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("omitted normals from GLB export"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_catproduct_external_references_from_cli() {
    let temp_dir = unique_temp_dir("catproduct-references");
    let parts_dir = temp_dir.join("parts");
    fs::create_dir_all(&parts_dir).expect("parts dir should be created");
    let input = temp_dir.join("assembly.CATProduct");
    let output_glb = temp_dir.join("assembly.glb");
    let metadata = temp_dir.join("assembly.metadata.json");

    let part_a = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    let part_b = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    fs::write(parts_dir.join("Part A.CATPart"), part_a).expect("part A should be written");
    fs::write(parts_dir.join("part-b.CATPart"), part_b).expect("part B should be written");

    let assembly = r#"
CATProduct private payload prefix
FEATHER_CAD_LITE_CACHE_V1
document Assembly
reference "Part A Instance" "parts\Part A.CATPart" root
reference PartB C:\legacy\released\part-b.CATPart root
END_FEATHER_CAD_LITE_CACHE
"#;
    fs::write(&input, assembly).expect("assembly should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .arg("--resolve-dir")
        .arg(&parts_dir)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATProduct\""));
    assert!(metadata_json.contains("\"triangle_count\": 4"));
    assert!(metadata_json.contains("resolved external reference `parts\\\\Part A.CATPart`"));
    assert!(
        metadata_json
            .contains("resolved external reference `C:\\\\legacy\\\\released\\\\part-b.CATPart`")
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_catproduct_external_references_with_map_root_from_cli() {
    let temp_dir = unique_temp_dir("catproduct-map-root");
    let migrated_root = temp_dir.join("released-package");
    let migrated_parts_dir = migrated_root.join("released");
    fs::create_dir_all(&migrated_parts_dir).expect("migrated parts dir should be created");
    let input = temp_dir.join("assembly.CATProduct");
    let output_glb = temp_dir.join("assembly.glb");
    let metadata = temp_dir.join("assembly.metadata.json");

    let part_b = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    fs::write(migrated_parts_dir.join("part-b.CATPart"), part_b)
        .expect("migrated part should be written");

    let assembly = r#"
CATProduct private payload prefix
FEATHER_CAD_LITE_CACHE_V1
document Assembly
reference PartB C:\vault\legacy\released\part-b.CATPart root
END_FEATHER_CAD_LITE_CACHE
"#;
    fs::write(&input, assembly).expect("assembly should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .arg("--map-root")
        .arg(format!(r"C:\vault\legacy={}", migrated_root.display()))
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATProduct\""));
    assert!(metadata_json.contains("\"mesh_count\": 1"));
    assert!(metadata_json.contains("\"triangle_count\": 2"));
    assert!(metadata_json.contains(
        "resolved external reference `C:\\\\vault\\\\legacy\\\\released\\\\part-b.CATPart`"
    ));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_catproduct_zip_manifest_external_references_from_cli() {
    let temp_dir = unique_temp_dir("catproduct-zip-external-references");
    let parts_dir = temp_dir.join("parts");
    fs::create_dir_all(&parts_dir).expect("parts dir should be created");
    let input = temp_dir.join("zip_assembly.CATProduct");
    let output_glb = temp_dir.join("zip_assembly.glb");
    let metadata = temp_dir.join("zip_assembly.metadata.json");

    let part_a = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    let part_b = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    fs::write(parts_dir.join("part-a.CATPart"), part_a).expect("part A should be written");
    fs::write(parts_dir.join("part-b.CATPart"), part_b).expect("part B should be written");

    let manifest = r#"
<Assembly name="ZipExternalAssembly">
  <Component name="PartA" href="part-a.CATPart"/>
  <Component name="PartB" file="legacy\released\part-b.CATPart" translation="0 5 0"/>
</Assembly>
"#;
    let mut bytes = b"CATProduct private ZIP assembly prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry("assembly.xml", manifest.as_bytes()));
    fs::write(&input, bytes).expect("ZIP assembly should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .arg("--resolve-dir")
        .arg(&parts_dir)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATProduct\""));
    assert!(metadata_json.contains("applied ZIP assembly manifest `assembly.xml`"));
    assert!(metadata_json.contains("0 visual references and 2 external references"));
    assert!(metadata_json.contains("resolved external reference `part-a.CATPart`"));
    assert!(metadata_json.contains("resolved external reference `legacy/released/part-b.CATPart`"));
    assert!(metadata_json.contains("\"mesh_count\": 2"));
    assert!(metadata_json.contains("\"triangle_count\": 4"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn batch_converts_directory_and_records_failures_from_cli() {
    let temp_dir = unique_temp_dir("batch-directory");
    let source_dir = temp_dir.join("sources");
    let nested_dir = source_dir.join("nested");
    let output_dir = temp_dir.join("out");
    fs::create_dir_all(&nested_dir).expect("source dirs should be created");

    let good_part = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    fs::write(source_dir.join("part-a.CATPart"), &good_part).expect("part A should be written");
    fs::write(nested_dir.join("part-b.CADPart"), &good_part).expect("part B should be written");
    fs::write(
        source_dir.join("bad.CATPart"),
        "CATPart private payload without readable visual data",
    )
    .expect("bad part should be written");
    fs::write(source_dir.join("notes.txt"), "not a CAD file").expect("note should be written");

    let batch = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("batch")
        .arg(&source_dir)
        .arg("--out")
        .arg(&output_dir)
        .arg("--no-normals")
        .output()
        .expect("batch command should run");

    assert!(
        !batch.status.success(),
        "mixed batch should report failure after processing every input"
    );

    let stdout = String::from_utf8(batch.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("converted: 2"));
    assert!(stdout.contains("failed: 1"));
    assert!(stdout.contains("ok\t"));
    assert!(stdout.contains("error\t"));

    let manifest = fs::read_to_string(output_dir.join("manifest.json"))
        .expect("batch manifest should be written");
    assert!(manifest.contains("\"input_count\": 3"));
    assert!(manifest.contains("\"success_count\": 2"));
    assert!(manifest.contains("\"failed_count\": 1"));
    assert!(manifest.contains("\"total_input_bytes\":"));
    assert!(manifest.contains("\"total_output_bytes\":"));
    assert!(manifest.contains("\"total_metadata_bytes\":"));
    assert!(manifest.contains("\"total_duration_ms\":"));
    assert!(manifest.contains("\"total_node_count\": 2"));
    assert!(manifest.contains("\"total_mesh_count\": 2"));
    assert!(manifest.contains("\"total_primitive_count\": 2"));
    assert!(manifest.contains("\"total_vertex_count\": 8"));
    assert!(manifest.contains("\"total_triangle_count\": 4"));
    assert!(manifest.contains("\"input_size_bytes\":"));
    assert!(manifest.contains("\"duration_ms\":"));
    assert!(manifest.contains("\"output_size_bytes\":"));
    assert!(manifest.contains("\"metadata_size_bytes\":"));
    assert!(manifest.contains("\"status\": \"ok\""));
    assert!(manifest.contains("\"status\": \"error\""));
    assert!(manifest.contains("\"importable\": true"));
    assert!(manifest.contains("\"importable\": false"));
    assert!(manifest.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(manifest.contains("\"probe_confidence\": \"High\""));
    assert!(manifest.contains("\"embedded_cache\": false"));
    assert!(manifest.contains("\"error_stage\": \"import\""));
    assert!(manifest.contains("has no readable lightweight visualization cache"));
    assert!(manifest.contains("bad.CATPart"));
    assert!(!manifest.contains("notes.txt"));
    let manifest_json: serde_json::Value =
        serde_json::from_str(&manifest).expect("batch manifest should be valid JSON");
    assert_eq!(
        manifest_json["contract_version"],
        BATCH_MANIFEST_CONTRACT_VERSION
    );
    assert_eq!(manifest_json["summary"]["total_node_count"], 2);
    assert_eq!(manifest_json["summary"]["total_mesh_count"], 2);
    assert_eq!(manifest_json["summary"]["total_primitive_count"], 2);
    assert_eq!(manifest_json["summary"]["total_vertex_count"], 8);
    assert_eq!(manifest_json["summary"]["total_triangle_count"], 4);
    for ok_item in manifest_json["items"]
        .as_array()
        .expect("batch items should be an array")
        .iter()
        .filter(|item| item["status"] == "ok")
    {
        assert_eq!(ok_item["node_count"], 1);
        assert_eq!(ok_item["mesh_count"], 1);
        assert_eq!(ok_item["primitive_count"], 1);
        assert_eq!(ok_item["vertex_count"], 4);
        assert_eq!(ok_item["triangle_count"], 2);
    }
    let failed_item = manifest_json["items"]
        .as_array()
        .expect("batch items should be an array")
        .iter()
        .find(|item| item["status"] == "error")
        .expect("one batch item should fail");
    assert_eq!(failed_item["capability"]["format"], "CATIA_CATPart");
    assert_eq!(
        failed_item["required_condition"],
        "provide a readable lightweight visualization payload: Feather cache, embedded mesh/GLB/glTF/STL/OBJ, ZIP/OLE preview, or resolvable cache-declared reference"
    );

    let glb_paths = fs::read_dir(&output_dir)
        .expect("output dir should exist")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .map(|extension| extension == "glb")
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    assert_eq!(glb_paths.len(), 2);
    for glb_path in glb_paths {
        let glb = fs::read(glb_path).expect("batch GLB should be readable");
        assert!(!glb_json_chunk(&glb).contains("\"NORMAL\""));
    }

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn batch_check_only_validates_without_exporting_glbs_from_cli() {
    let temp_dir = unique_temp_dir("batch-check-only");
    let source_dir = temp_dir.join("sources");
    let output_dir = temp_dir.join("out");
    fs::create_dir_all(&source_dir).expect("source dir should be created");

    let good_part = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    fs::write(source_dir.join("good.CATPart"), good_part).expect("good part should be written");
    fs::write(
        source_dir.join("bad.CATPart"),
        "CATPart private payload without readable visual data",
    )
    .expect("bad part should be written");
    let missing_reference = "\
CATProduct private payload prefix
FEATHER_CAD_LITE_CACHE_V1
document MissingReferenceAssembly
reference MissingPart missing-part.CATPart root
END_FEATHER_CAD_LITE_CACHE
";
    fs::write(source_dir.join("missing_ref.CATProduct"), missing_reference)
        .expect("missing reference assembly should be written");

    let batch = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("batch")
        .arg(&source_dir)
        .arg("--out")
        .arg(&output_dir)
        .arg("--check-only")
        .output()
        .expect("batch command should run");

    assert!(
        !batch.status.success(),
        "mixed preflight should report failure after checking every input"
    );

    let stdout = String::from_utf8(batch.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("converted: 0"));
    assert!(stdout.contains("checked: 1"));
    assert!(stdout.contains("failed: 2"));
    assert!(stdout.contains("checked\t"));
    assert!(stdout.contains("error\t"));

    let manifest = fs::read_to_string(output_dir.join("manifest.json"))
        .expect("batch manifest should be written");
    assert!(manifest.contains("\"input_count\": 3"));
    assert!(manifest.contains("\"success_count\": 1"));
    assert!(manifest.contains("\"converted_count\": 0"));
    assert!(manifest.contains("\"checked_count\": 1"));
    assert!(manifest.contains("\"failed_count\": 2"));
    assert!(manifest.contains("\"summary\":"));
    assert!(manifest.contains("\"total_node_count\": 1"));
    assert!(manifest.contains("\"total_mesh_count\": 1"));
    assert!(manifest.contains("\"total_primitive_count\": 1"));
    assert!(manifest.contains("\"total_vertex_count\": 6"));
    assert!(manifest.contains("\"total_triangle_count\": 2"));
    assert!(manifest.contains("\"total_input_bytes\":"));
    assert!(manifest.contains("\"total_output_bytes\": 0"));
    assert!(manifest.contains("\"total_metadata_bytes\": 0"));
    assert!(manifest.contains("\"total_duration_ms\":"));
    assert!(manifest.contains("\"input_size_bytes\":"));
    assert!(manifest.contains("\"duration_ms\":"));
    assert!(manifest.contains("\"formats\":"));
    assert!(manifest.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(manifest.contains("\"source_format\": \"CATIA_CATProduct\""));
    assert!(manifest.contains("\"failure_stages\":"));
    assert!(manifest.contains("\"stage\": \"import\""));
    assert!(manifest.contains("\"count\": 2"));
    assert!(manifest.contains("\"failure_categories\":"));
    assert!(manifest.contains("\"category\": \"missing_external_reference\""));
    assert!(manifest.contains("\"category\": \"no_readable_lightweight_cache\""));
    assert!(manifest.contains("\"status\": \"checked\""));
    assert!(manifest.contains("\"status\": \"error\""));
    assert!(manifest.contains("\"output_path\": null"));
    assert!(manifest.contains("\"metadata_path\": null"));
    assert!(manifest.contains("\"output_size_bytes\": null"));
    assert!(manifest.contains("\"metadata_size_bytes\": null"));
    assert!(manifest.contains("\"importable\": true"));
    assert!(manifest.contains("\"importable\": false"));
    assert!(manifest.contains("\"node_count\": 1"));
    assert!(manifest.contains("\"mesh_count\": 1"));
    assert!(manifest.contains("\"primitive_count\": 1"));
    assert!(manifest.contains("\"vertex_count\": 6"));
    assert!(manifest.contains("\"triangle_count\": 2"));
    assert!(manifest.contains("\"error_stage\": \"import\""));
    assert!(manifest.contains("\"error_category\": \"missing_external_reference\""));
    assert!(manifest.contains("\"error_category\": \"no_readable_lightweight_cache\""));
    assert!(manifest.contains("external reference `missing-part.CATPart` could not be resolved"));
    assert!(manifest.contains("has no readable lightweight visualization cache"));
    let manifest_json: serde_json::Value =
        serde_json::from_str(&manifest).expect("batch check-only manifest should be valid JSON");
    assert_eq!(manifest_json["summary"]["total_node_count"], 1);
    assert_eq!(manifest_json["summary"]["total_mesh_count"], 1);
    assert_eq!(manifest_json["summary"]["total_primitive_count"], 1);
    assert_eq!(manifest_json["summary"]["total_vertex_count"], 6);
    assert_eq!(manifest_json["summary"]["total_triangle_count"], 2);
    let checked_item = manifest_json["items"]
        .as_array()
        .expect("batch items should be an array")
        .iter()
        .find(|item| item["status"] == "checked")
        .expect("one batch item should be checked");
    assert_eq!(checked_item["node_count"], 1);
    assert_eq!(checked_item["mesh_count"], 1);
    assert_eq!(checked_item["primitive_count"], 1);
    assert_eq!(checked_item["vertex_count"], 6);
    assert_eq!(checked_item["triangle_count"], 2);
    assert!(
        manifest_json["items"]
            .as_array()
            .expect("batch items should be an array")
            .iter()
            .any(|item| item["required_condition"]
                .as_str()
                .unwrap_or_default()
                .contains("--resolve-dir"))
    );

    let glb_count = fs::read_dir(&output_dir)
        .expect("output dir should exist")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .map(|extension| extension == "glb")
                .unwrap_or(false)
        })
        .count();
    assert_eq!(glb_count, 0);

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn batch_directory_uses_extensions_and_header_probe_for_candidates() {
    let temp_dir = unique_temp_dir("batch-header-probe");
    let source_dir = temp_dir.join("sources");
    let output_dir = temp_dir.join("out");
    fs::create_dir_all(&source_dir).expect("source dir should be created");

    let good_part = format!(
        "CATPart private payload prefix\n{}\nCATPart private payload suffix",
        sample_obj()
    );
    fs::write(source_dir.join("extension.CATPart"), &good_part)
        .expect("extension candidate should be written");
    fs::write(source_dir.join("header.bin"), &good_part)
        .expect("header signature candidate should be written");

    let mut late_unknown = vec![b'x'; 9000];
    late_unknown.extend_from_slice(good_part.as_bytes());
    fs::write(source_dir.join("late-cache.bin"), late_unknown)
        .expect("late unknown file should be written");

    let batch = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("batch")
        .arg(&source_dir)
        .arg("--out")
        .arg(&output_dir)
        .output()
        .expect("batch command should run");

    assert!(
        batch.status.success(),
        "{}",
        String::from_utf8_lossy(&batch.stderr)
    );

    let stdout = String::from_utf8(batch.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("converted: 2"));
    assert!(stdout.contains("failed: 0"));

    let manifest = fs::read_to_string(output_dir.join("manifest.json"))
        .expect("batch manifest should be written");
    assert!(manifest.contains("\"input_count\": 2"));
    assert!(manifest.contains("extension.CATPart"));
    assert!(manifest.contains("header.bin"));
    assert!(!manifest.contains("late-cache.bin"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cadpart_with_embedded_glb_from_cli() {
    let temp_dir = unique_temp_dir("embedded-glb");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("embedded_glb.CADPart");
    let output_glb = temp_dir.join("embedded_glb.glb");
    let metadata = temp_dir.join("embedded_glb.metadata.json");

    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&sample_glb());
    bytes.extend_from_slice(b"CATPart private payload suffix");
    fs::write(&input, bytes).expect("test CADPart should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(metadata_json.contains("embedded GLB"));
    assert!(metadata_json.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_jt_with_embedded_glb_from_cli() {
    let temp_dir = unique_temp_dir("jt-embedded-glb");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("preview.jt");
    let output_glb = temp_dir.join("preview.glb");
    let metadata = temp_dir.join("preview.metadata.json");

    let mut bytes = b"JT lightweight private payload prefix".to_vec();
    bytes.extend_from_slice(&sample_glb());
    bytes.extend_from_slice(b"JT lightweight private payload suffix");
    fs::write(&input, bytes).expect("test JT-like file should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"PRIVATE_CAD\""));
    assert!(metadata_json.contains("\"mode\": \"private-cad-embedded-visual-asset\""));
    assert!(metadata_json.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cadpart_with_stored_zip_obj_from_cli() {
    let temp_dir = unique_temp_dir("stored-zip-obj");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("stored_zip_obj.CADPart");
    let output_glb = temp_dir.join("stored_zip_obj.glb");
    let metadata = temp_dir.join("stored_zip_obj.metadata.json");

    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/model.obj",
        sample_obj().as_bytes(),
    ));
    bytes.extend_from_slice(b"CATPart private payload suffix");
    fs::write(&input, bytes).expect("test CADPart should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(metadata_json.contains("ZIP stored entry"));
    assert!(metadata_json.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cadpart_with_zip_gltf_and_bin_from_cli() {
    let temp_dir = unique_temp_dir("zip-gltf-bin");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("zip_gltf_bin.CATPart");
    let output_glb = temp_dir.join("zip_gltf_bin.glb");
    let metadata = temp_dir.join("zip_gltf_bin.metadata.json");
    let (gltf, bin) = sample_gltf_with_bin();

    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry("preview/model.gltf", gltf.as_bytes()));
    bytes.extend_from_slice(&stored_zip_entry("preview/model.bin", &bin));
    bytes.extend_from_slice(b"CATPart private payload suffix");
    fs::write(&input, bytes).expect("test CADPart should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(metadata_json.contains("ZIP stored entry `preview/model.gltf`"));
    assert!(metadata_json.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cadpart_with_zip_gltf_data_uri_from_cli() {
    let temp_dir = unique_temp_dir("zip-gltf-data-uri");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("zip_gltf_data_uri.CATPart");
    let output_glb = temp_dir.join("zip_gltf_data_uri.glb");
    let metadata = temp_dir.join("zip_gltf_data_uri.metadata.json");
    let gltf = sample_gltf_with_data_uri();

    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry("preview/model.gltf", gltf.as_bytes()));
    bytes.extend_from_slice(b"CATPart private payload suffix");
    fs::write(&input, bytes).expect("test CATPart should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(metadata_json.contains("ZIP stored entry `preview/model.gltf`"));
    assert!(metadata_json.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_3dxml_zip_xml_assembly_from_cli() {
    let temp_dir = unique_temp_dir("zip-xml-assembly");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("assembly.3dxml");
    let output_glb = temp_dir.join("assembly.glb");
    let metadata = temp_dir.join("assembly.metadata.json");
    let assembly = r#"
<ProductStructure name="RootAssembly">
  <Instance3D name="Bracket" associatedFile="urn:3DXML:preview/bracket.glb">
    <RelativeMatrix>1 0 0 0 1 0 0 0 1 10 0 0</RelativeMatrix>
  </Instance3D>
  <Component name="Cover" href="urn:3DXML:preview/cover.obj"
    transform="1 0 0 0 0 1 0 0 0 0 1 0 0 20 0 1"/>
</ProductStructure>
"#;

    let mut bytes = b"3DXML private assembly payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry("Manifest.xml", assembly.as_bytes()));
    bytes.extend_from_slice(&stored_zip_entry("preview/bracket.glb", &sample_glb()));
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/cover.obj",
        sample_obj().as_bytes(),
    ));
    fs::write(&input, bytes).expect("test 3DXML-like assembly should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"DASSAULT_3DXML\""));
    assert!(metadata_json.contains("\"mode\": \"3dxml-embedded-visual-asset\""));
    assert!(metadata_json.contains("applied ZIP assembly manifest `Manifest.xml`"));
    assert!(metadata_json.contains("ZIP stored entry `preview/bracket.glb`"));
    assert!(metadata_json.contains("ZIP stored entry `preview/cover.obj`"));
    assert!(metadata_json.contains("\"mesh_count\": 2"));
    assert!(metadata_json.contains("\"triangle_count\": 4"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_3dxml_product_structure_relationships_from_cli() {
    let temp_dir = unique_temp_dir("3dxml-product-structure");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("product_structure.3dxml");
    let output_glb = temp_dir.join("product_structure.glb");
    let metadata = temp_dir.join("product_structure.metadata.json");
    let root = r#"
<Model_3dxml>
  <ProductStructure>
    <Reference3D id="R_ROOT" name="RootAssembly"/>
    <Reference3D id="R_BRACKET" name="BracketReference"/>
    <Instance3D id="I_BRACKET" name="BracketInstance">
      <IsAggregatedBy>R_ROOT</IsAggregatedBy>
      <IsInstanceOf>R_BRACKET</IsInstanceOf>
      <RelativeMatrix>1 0 0 0 1 0 0 0 1 30 0 0</RelativeMatrix>
    </Instance3D>
    <ReferenceRep id="REP_BRACKET" name="BracketPreview" associatedFile="urn:3DXML:preview/bracket.glb"/>
    <InstanceRep id="IR_BRACKET" name="BracketRep">
      <IsAggregatedBy>R_BRACKET</IsAggregatedBy>
      <IsInstanceOf>REP_BRACKET</IsInstanceOf>
    </InstanceRep>
  </ProductStructure>
</Model_3dxml>
"#;

    let mut bytes = b"3DXML realistic product structure package".to_vec();
    bytes.extend_from_slice(&stored_zip_entry(
        "Manifest.xml",
        b"<Manifest><Root>Root.3dxml</Root></Manifest>",
    ));
    bytes.extend_from_slice(&stored_zip_entry("Root.3dxml", root.as_bytes()));
    bytes.extend_from_slice(&stored_zip_entry("preview/bracket.glb", &sample_glb()));
    fs::write(&input, bytes).expect("test 3DXML ProductStructure should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"DASSAULT_3DXML\""));
    assert!(metadata_json.contains("applied ZIP assembly manifest `Root.3dxml`"));
    assert!(metadata_json.contains("\"mesh_count\": 1"));
    assert!(metadata_json.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_3dxml_product_structure_with_xml_3drep_from_cli() {
    let temp_dir = unique_temp_dir("3dxml-product-structure-3drep");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("product_structure_3drep.3dxml");
    let output_glb = temp_dir.join("product_structure_3drep.glb");
    let metadata = temp_dir.join("product_structure_3drep.metadata.json");
    let root = r#"
<Model_3dxml>
  <ProductStructure>
    <Reference3D id="R_ROOT" name="RootAssembly"/>
    <Reference3D id="R_BRACKET" name="BracketReference"/>
    <Instance3D id="I_BRACKET" name="BracketInstance">
      <IsAggregatedBy>R_ROOT</IsAggregatedBy>
      <IsInstanceOf>R_BRACKET</IsInstanceOf>
      <RelativeMatrix>1 0 0 0 1 0 0 0 1 30 0 0</RelativeMatrix>
    </Instance3D>
    <ReferenceRep id="REP_BRACKET" name="BracketPreview" format="TESSELLATED" associatedFile="urn:3DXML:preview/bracket.3DRep"/>
    <InstanceRep id="IR_BRACKET" name="BracketRep">
      <IsAggregatedBy>R_BRACKET</IsAggregatedBy>
      <IsInstanceOf>REP_BRACKET</IsInstanceOf>
    </InstanceRep>
  </ProductStructure>
</Model_3dxml>
"#;

    let mut bytes = b"3DXML ProductStructure with XML 3DRep".to_vec();
    bytes.extend_from_slice(&stored_zip_entry(
        "Manifest.xml",
        b"<Manifest><Root>Root.3dxml</Root></Manifest>",
    ));
    bytes.extend_from_slice(&stored_zip_entry("Root.3dxml", root.as_bytes()));
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/bracket.3DRep",
        sample_3dxml_rep().as_bytes(),
    ));
    fs::write(&input, bytes).expect("test 3DXML ProductStructure should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"DASSAULT_3DXML\""));
    assert!(metadata_json.contains("applied ZIP assembly manifest `Root.3dxml`"));
    assert!(metadata_json.contains("ZIP stored entry `preview/bracket.3DRep`"));
    assert!(metadata_json.contains("readable XML 3DRep polygonal tessellation"));
    assert!(metadata_json.contains("\"mesh_count\": 1"));
    assert!(metadata_json.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cadpart_with_deflated_zip_glb_from_cli() {
    let temp_dir = unique_temp_dir("deflated-zip-glb");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("deflated_zip_glb.CATPart");
    let output_glb = temp_dir.join("deflated_zip_glb.glb");
    let metadata = temp_dir.join("deflated_zip_glb.metadata.json");

    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&deflated_zip_entry("preview/model.glb", &sample_glb()));
    bytes.extend_from_slice(b"CATPart private payload suffix");
    fs::write(&input, bytes).expect("test CADPart should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(metadata_json.contains("ZIP deflated entry"));
    assert!(metadata_json.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cadpart_with_data_descriptor_zip_glb_from_cli() {
    let temp_dir = unique_temp_dir("data-descriptor-zip-glb");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("data_descriptor_zip_glb.CATPart");
    let output_glb = temp_dir.join("data_descriptor_zip_glb.glb");
    let metadata = temp_dir.join("data_descriptor_zip_glb.metadata.json");

    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&deflated_zip_entry_with_data_descriptor(
        "preview/model.glb",
        &sample_glb(),
    ));
    bytes.extend_from_slice(b"CATPart private payload suffix");
    fs::write(&input, bytes).expect("test CADPart should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(metadata_json.contains("ZIP deflated entry"));
    assert!(metadata_json.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn converts_cadpart_with_zip64_zip_glb_from_cli() {
    let temp_dir = unique_temp_dir("zip64-zip-glb");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("zip64_zip_glb.CATPart");
    let output_glb = temp_dir.join("zip64_zip_glb.glb");
    let metadata = temp_dir.join("zip64_zip_glb.metadata.json");

    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&zip64_deflated_zip_entry(
        "preview/model.glb",
        &sample_glb(),
    ));
    bytes.extend_from_slice(b"CATPart private payload suffix");
    fs::write(&input, bytes).expect("test CATPart should be written");

    let convert = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output_glb)
        .arg("--metadata")
        .arg(&metadata)
        .output()
        .expect("convert command should run");

    assert!(
        convert.status.success(),
        "{}",
        String::from_utf8_lossy(&convert.stderr)
    );

    let glb = fs::read(&output_glb).expect("GLB should be written");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    let metadata_json = fs::read_to_string(&metadata).expect("metadata should be written");
    assert!(metadata_json.contains("\"source_format\": \"CATIA_CATPart\""));
    assert!(metadata_json.contains("ZIP deflated entry"));
    assert!(metadata_json.contains("\"triangle_count\": 2"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn dumps_multiple_visual_assets_from_private_container() {
    let temp_dir = unique_temp_dir("dump-cache");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("assembly.CATProduct");
    let dump_dir = temp_dir.join("dump");

    let glb = sample_glb();
    let mut bytes = b"CATProduct private payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/part-a.obj",
        sample_obj().as_bytes(),
    ));
    bytes.extend_from_slice(&stored_zip_entry("preview/part-b.glb", &glb));
    bytes.extend_from_slice(b"CATProduct private payload suffix");
    fs::write(&input, bytes).expect("test CATProduct should be written");

    let dump = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("dump-cache")
        .arg(&input)
        .arg("--out")
        .arg(&dump_dir)
        .output()
        .expect("dump-cache command should run");

    assert!(
        dump.status.success(),
        "{}",
        String::from_utf8_lossy(&dump.stderr)
    );

    let stdout = String::from_utf8(dump.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("assets: 2"));

    let manifest = fs::read_to_string(dump_dir.join("manifest.json"))
        .expect("dump manifest should be written");
    let manifest_json: serde_json::Value =
        serde_json::from_str(&manifest).expect("dump manifest should be valid JSON");
    assert_eq!(
        manifest_json["contract_version"],
        CACHE_DUMP_MANIFEST_CONTRACT_VERSION
    );
    assert!(manifest.contains("\"asset_count\": 2"));
    assert!(manifest.contains("\"source\": \"zip-entry\""));
    assert!(manifest.contains("\"entry_name\": \"preview/part-a.obj\""));
    assert!(manifest.contains("\"entry_name\": \"preview/part-b.glb\""));

    let dumped_obj =
        fs::read_to_string(dump_dir.join("asset_000.obj")).expect("OBJ asset should be dumped");
    assert!(dumped_obj.contains("# Wavefront OBJ visual cache"));
    let dumped_glb = fs::read(dump_dir.join("asset_001.glb")).expect("GLB asset should be dumped");
    assert_eq!(&dumped_glb[0..4], &0x4654_6C67_u32.to_le_bytes());

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn dumps_zip_gltf_assets_from_private_container() {
    let temp_dir = unique_temp_dir("dump-cache-gltf");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("preview.CATPart");
    let dump_dir = temp_dir.join("dump");
    let (gltf, bin) = sample_gltf_with_bin();

    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry("preview/model.gltf", gltf.as_bytes()));
    bytes.extend_from_slice(&stored_zip_entry("preview/model.bin", &bin));
    bytes.extend_from_slice(b"CATPart private payload suffix");
    fs::write(&input, bytes).expect("test CATPart should be written");

    let dump = Command::new(env!("CARGO_BIN_EXE_feather"))
        .arg("dump-cache")
        .arg(&input)
        .arg("--out")
        .arg(&dump_dir)
        .output()
        .expect("dump-cache command should run");

    assert!(
        dump.status.success(),
        "{}",
        String::from_utf8_lossy(&dump.stderr)
    );

    let stdout = String::from_utf8(dump.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("assets: 2"));

    let manifest = fs::read_to_string(dump_dir.join("manifest.json"))
        .expect("dump manifest should be written");
    assert!(manifest.contains("\"asset_count\": 2"));
    assert!(manifest.contains("\"kind\": \"gltf\""));
    assert!(manifest.contains("\"kind\": \"gltf-bin\""));
    assert!(manifest.contains("\"entry_name\": \"preview/model.gltf\""));
    assert!(manifest.contains("\"entry_name\": \"preview/model.bin\""));

    let dumped_gltf =
        fs::read_to_string(dump_dir.join("asset_000.gltf")).expect("glTF asset should be dumped");
    assert!(dumped_gltf.contains("\"uri\":\"model.bin\""));
    let dumped_bin = fs::read(dump_dir.join("asset_001.bin")).expect("BIN asset should be dumped");
    assert_eq!(dumped_bin, bin);

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

fn workspace_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn sample_binary_stl() -> Vec<u8> {
    let mut bytes = vec![0_u8; 80];
    let header = b"BINARY STL VISUAL MESH CACHE";
    bytes[..header.len()].copy_from_slice(header);
    bytes.extend_from_slice(&2_u32.to_le_bytes());

    push_stl_triangle(
        &mut bytes,
        [0.0, 0.0, 1.0],
        [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 1.0, 0.0]],
    );
    push_stl_triangle(
        &mut bytes,
        [0.0, 0.0, 1.0],
        [[0.0, 0.0, 0.0], [2.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
    );
    bytes
}

fn sample_obj() -> &'static str {
    "# Wavefront OBJ visual cache
o Plate
v 0 0 0
v 2 0 0
v 2 1 0
v 0 1 0
vn 0 0 1
f 1//1 2//1 3//1 4//1"
}

fn sample_glb() -> Vec<u8> {
    export_glb(&sample_lite_document(), &GlbExportOptions::default()).expect("sample GLB exports")
}

fn glb_json_chunk(glb: &[u8]) -> String {
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());
    let json_len = u32::from_le_bytes(glb[12..16].try_into().expect("JSON length header")) as usize;
    assert_eq!(&glb[16..20], &0x4E4F_534A_u32.to_le_bytes());
    String::from_utf8(glb[20..20 + json_len].to_vec()).expect("GLB JSON should be UTF-8")
}

fn sample_3dxml_rep() -> &'static str {
    r#"
<Root>
  <Rep>
    <VertexBuffer>
      <Positions>0 0 0 2 0 0 2 1 0 0 1 0</Positions>
      <Normals>0 0 1 0 0 1 0 0 1 0 0 1</Normals>
    </VertexBuffer>
    <Faces>
      <Face triangles="0 1 2 0 2 3"/>
    </Faces>
  </Rep>
</Root>
"#
}

fn sample_gltf_with_bin() -> (String, Vec<u8>) {
    let bin = sample_gltf_bin();
    let gltf = sample_gltf_json("model.bin", bin.len());
    (gltf, bin)
}

fn sample_gltf_with_data_uri() -> String {
    let bin = sample_gltf_bin();
    let uri = format!(
        "data:application/octet-stream;base64,{}",
        encode_base64(&bin)
    );
    sample_gltf_json(&uri, bin.len())
}

fn sample_gltf_bin() -> Vec<u8> {
    let mut bin = Vec::new();
    for position in [
        [0.0_f32, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [2.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ] {
        for value in position {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    for index in [0_u32, 1, 2, 0, 2, 3] {
        bin.extend_from_slice(&index.to_le_bytes());
    }
    bin
}

fn sample_gltf_json(uri: &str, byte_length: usize) -> String {
    format!(
        "{{\"asset\":{{\"version\":\"2.0\"}},\"scene\":0,\"scenes\":[{{\"nodes\":[0]}}],\"nodes\":[{{\"name\":\"Plate\",\"mesh\":0}}],\"materials\":[{{\"name\":\"Default\",\"pbrMetallicRoughness\":{{\"baseColorFactor\":[0.8,0.8,0.82,1.0]}}}}],\"meshes\":[{{\"name\":\"Plate\",\"primitives\":[{{\"attributes\":{{\"POSITION\":0}},\"indices\":1,\"mode\":4,\"material\":0}}]}}],\"buffers\":[{{\"uri\":\"{}\",\"byteLength\":{}}}],\"bufferViews\":[{{\"buffer\":0,\"byteOffset\":0,\"byteLength\":48,\"target\":34962}},{{\"buffer\":0,\"byteOffset\":48,\"byteLength\":24,\"target\":34963}}],\"accessors\":[{{\"bufferView\":0,\"componentType\":5126,\"count\":4,\"type\":\"VEC3\"}},{{\"bufferView\":1,\"componentType\":5125,\"count\":6,\"type\":\"SCALAR\"}}]}}",
        escape_json_string(uri),
        byte_length
    )
}

fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        encoded.push(ALPHABET[(first >> 2) as usize] as char);
        encoded.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(ALPHABET[(((second & 0x0F) << 2) | (third >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[(third & 0x3F) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

fn escape_json_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn sample_lite_document() -> LiteDocument {
    let mut document = LiteDocument::new("Fixture", "fixture");
    document
        .materials
        .push(LiteMaterial::new("Default", [0.8, 0.8, 0.82, 1.0]));
    let mut primitive = LitePrimitive::new(Some(0));
    primitive.positions = vec![
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [2.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    primitive.normals = vec![[0.0, 0.0, 1.0]; 4];
    primitive.indices = vec![0, 1, 2, 0, 2, 3];
    let mut mesh = LiteMesh::new("Plate");
    mesh.primitives.push(primitive);
    mesh.recompute_bbox();
    document.meshes.push(mesh);
    document.nodes.push(LiteNode::new("Plate", Some(0)));
    document.refresh_metadata();
    document
}

fn push_stl_triangle(bytes: &mut Vec<u8>, normal: [f32; 3], vertices: [[f32; 3]; 3]) {
    for value in normal {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for vertex in vertices {
        for value in vertex {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&0_u16.to_le_bytes());
}

fn stored_zip_entry(name: &str, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0x0403_4B50_u32.to_le_bytes());
    bytes.extend_from_slice(&20_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(name.as_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

fn deflated_zip_entry(name: &str, payload: &[u8]) -> Vec<u8> {
    let compressed = compress_to_vec(payload, 6);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0x0403_4B50_u32.to_le_bytes());
    bytes.extend_from_slice(&20_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&8_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(name.as_bytes());
    bytes.extend_from_slice(&compressed);
    bytes
}

fn deflated_zip_entry_with_data_descriptor(name: &str, payload: &[u8]) -> Vec<u8> {
    let compressed = compress_to_vec(payload, 6);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0x0403_4B50_u32.to_le_bytes());
    bytes.extend_from_slice(&20_u16.to_le_bytes());
    bytes.extend_from_slice(&0x0008_u16.to_le_bytes());
    bytes.extend_from_slice(&8_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(name.as_bytes());
    bytes.extend_from_slice(&compressed);

    bytes.extend_from_slice(&0x0807_4B50_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());

    let central_directory_offset = bytes.len() as u32;
    bytes.extend_from_slice(&0x0201_4B50_u32.to_le_bytes());
    bytes.extend_from_slice(&20_u16.to_le_bytes());
    bytes.extend_from_slice(&20_u16.to_le_bytes());
    bytes.extend_from_slice(&0x0008_u16.to_le_bytes());
    bytes.extend_from_slice(&8_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(name.as_bytes());

    let central_directory_size = bytes.len() as u32 - central_directory_offset;
    bytes.extend_from_slice(&0x0605_4B50_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&central_directory_size.to_le_bytes());
    bytes.extend_from_slice(&central_directory_offset.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes
}

fn zip64_deflated_zip_entry(name: &str, payload: &[u8]) -> Vec<u8> {
    let compressed = compress_to_vec(payload, 6);
    let zip64_extra = zip64_size_extra(payload.len(), compressed.len());
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0x0403_4B50_u32.to_le_bytes());
    bytes.extend_from_slice(&45_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&8_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&(zip64_extra.len() as u16).to_le_bytes());
    bytes.extend_from_slice(name.as_bytes());
    bytes.extend_from_slice(&zip64_extra);
    bytes.extend_from_slice(&compressed);

    let central_directory_offset = bytes.len() as u32;
    bytes.extend_from_slice(&0x0201_4B50_u32.to_le_bytes());
    bytes.extend_from_slice(&45_u16.to_le_bytes());
    bytes.extend_from_slice(&45_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&8_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&(zip64_extra.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(name.as_bytes());
    bytes.extend_from_slice(&zip64_extra);

    let central_directory_size = bytes.len() as u32 - central_directory_offset;
    bytes.extend_from_slice(&0x0605_4B50_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&central_directory_size.to_le_bytes());
    bytes.extend_from_slice(&central_directory_offset.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes
}

fn zip64_size_extra(uncompressed_size: usize, compressed_size: usize) -> Vec<u8> {
    let mut extra = Vec::new();
    extra.extend_from_slice(&0x0001_u16.to_le_bytes());
    extra.extend_from_slice(&16_u16.to_le_bytes());
    extra.extend_from_slice(&(uncompressed_size as u64).to_le_bytes());
    extra.extend_from_slice(&(compressed_size as u64).to_le_bytes());
    extra
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

fn unique_temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "feather-cli-test-{label}-{}-{stamp}",
        std::process::id()
    ))
}
