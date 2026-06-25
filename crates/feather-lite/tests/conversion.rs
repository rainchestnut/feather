use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use feather_lite::{
    FileFormat, GlbExportOptions, INSPECT_REPORT_CONTRACT_VERSION, ImportError, ImportLimits,
    ImportOptions, ImporterRegistry, InputFile, InspectOptions, LiteDocument, LiteMaterial,
    LiteMesh, LiteNode, LitePrimitive, MeshOptions, ProbeConfidence, ReferencePathMapping,
    detect_format, discover_embedded_visual_assets, discover_embedded_visual_assets_with_limits,
    export_glb, export_metadata_json, import_3dxml_rep_document, inspect_bytes, optimize_document,
    validate_document, validate_glb_payload,
};
use miniz_oxide::deflate::compress_to_vec;

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
fn imports_embedded_catpart_cache_and_exports_glb() {
    let bytes = format!("CATPart-private-prefix\n{SAMPLE_CACHE}\nprivate-suffix");
    let path = std::path::Path::new("fixture.CATPart");
    let input = InputFile::new(Some(path), bytes.as_bytes());
    let registry = ImporterRegistry::default();

    let mut document = registry
        .import(&input, &ImportOptions::default())
        .expect("embedded cache should import");
    optimize_document(&mut document, &MeshOptions::default());
    validate_document(&document).expect("document should validate after optimization");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 1);

    let glb = export_glb(&document, &GlbExportOptions::default()).expect("GLB should export");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());
    assert_eq!(&glb[4..8], &2_u32.to_le_bytes());
}

#[test]
fn imports_standalone_feather_cache() {
    let input = InputFile::new(
        Some(std::path::Path::new("fixture.flite")),
        SAMPLE_CACHE.as_bytes(),
    );
    let registry = ImporterRegistry::default();

    let probe = registry.probe(&input);
    assert_eq!(probe.format, FileFormat::FeatherCache);

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("standalone Feather Lite cache should import");

    assert_eq!(document.metadata.source_format, "FeatherLiteCache");
    assert_eq!(document.metadata.mode, "standalone-cache");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 1);
}

#[test]
fn inspect_api_reports_probe_assets_and_import_check() {
    let bytes = format!("CATPart-private-prefix\n{SAMPLE_CACHE}\nprivate-suffix");
    let path = std::path::Path::new("fixture.CATPart");
    let report = inspect_bytes(
        Some(path),
        bytes.as_bytes(),
        &InspectOptions {
            check_import: true,
            ..InspectOptions::default()
        },
    )
    .expect("inspect API should return a report");

    assert_eq!(report.probe.format, FileFormat::CatiaCatPart);
    assert!(report.probe.has_embedded_cache);
    assert_eq!(report.visual_assets.len(), 1);
    let capability = report
        .capability()
        .expect("CATPart capability should be available");
    assert!(capability.requires_visual_payload);
    assert!(capability.supports_embedded_assets);
    assert!(!capability.supports_external_references);
    let json = report.to_json_string();
    let parsed_json: serde_json::Value =
        serde_json::from_str(&json).expect("inspect JSON should be valid");
    assert_eq!(
        parsed_json["contract_version"],
        INSPECT_REPORT_CONTRACT_VERSION
    );
    assert_eq!(parsed_json["format"], "CATIA_CATPart");
    assert_eq!(parsed_json["capability"]["status"], "partial");
    assert_eq!(parsed_json["capability"]["available"], false);
    assert_eq!(parsed_json["capability"]["requires_visual_payload"], true);
    assert_eq!(
        parsed_json["capability"]["native_brep_tessellation"],
        "not_decoded"
    );
    assert_eq!(parsed_json["import_check"]["importable"], true);
    assert_eq!(
        parsed_json["import_check"]["failure_category"],
        serde_json::Value::Null
    );
    assert_eq!(parsed_json["visual_asset_count"], 1);
    assert_eq!(parsed_json["visual_assets"][0]["kind"], "feather-cache");
    assert!(json.contains("\"format\": \"CATIA_CATPart\""));
    assert!(json.contains("\"import_check\":"));
    assert!(json.contains("\"importable\": true"));
    assert!(json.contains("\"visual_asset_count\": 1"));
    assert!(json.contains("\"kind\": \"feather-cache\""));
    let import_check = report
        .import_check
        .expect("check_import should produce import check");
    assert!(import_check.importable);
    assert_eq!(import_check.mesh_count, Some(1));
    assert_eq!(import_check.triangle_count, Some(1));
    assert_eq!(import_check.failure_category, None);
    assert_eq!(import_check.error, None);
}

#[test]
fn inspect_api_reports_capability_failure_diagnostics() {
    let bytes = b"CATPart private payload without readable preview";
    let report = inspect_bytes(
        Some(std::path::Path::new("fixture.CATPart")),
        bytes,
        &InspectOptions {
            check_import: true,
            ..InspectOptions::default()
        },
    )
    .expect("inspect API should return a failure report");

    let import_check = report
        .import_check
        .as_ref()
        .expect("check_import should produce diagnostics");
    assert!(!import_check.importable);
    assert_eq!(import_check.failure_stage, Some("import"));
    assert_eq!(
        import_check.failure_category,
        Some("no_readable_lightweight_cache")
    );
    assert!(
        import_check
            .required_condition
            .expect("required condition should be explained")
            .contains("readable lightweight visualization payload")
    );

    let parsed_json: serde_json::Value =
        serde_json::from_str(&report.to_json_string()).expect("inspect JSON should be valid");
    assert_eq!(
        parsed_json["import_check"]["failure_category"],
        "no_readable_lightweight_cache"
    );
    assert_eq!(parsed_json["import_check"]["failure_stage"], "import");
    assert!(
        parsed_json["import_check"]["required_condition"]
            .as_str()
            .expect("required condition should be a string")
            .contains("readable lightweight visualization payload")
    );
}

#[test]
fn imports_embedded_nx_cache() {
    let bytes = format!("Unigraphics-private-prefix\n{SAMPLE_CACHE}");
    let path = std::path::Path::new("fixture.prt");
    let input = InputFile::new(Some(path), bytes.as_bytes());
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("embedded NX cache should import");

    assert_eq!(document.metadata.source_format, "NX_PRT");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 1);
}

#[test]
fn imports_standalone_binary_stl() {
    let stl = sample_binary_stl();
    let input = InputFile::new(Some(std::path::Path::new("fixture.stl")), &stl);
    let registry = ImporterRegistry::default();

    let mut document = registry
        .import(&input, &ImportOptions::default())
        .expect("standalone binary STL should import");
    optimize_document(&mut document, &MeshOptions::default());

    assert_eq!(document.metadata.source_format, "STL");
    assert_eq!(document.metadata.mode, "stl-binary");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
}

#[test]
fn imports_standalone_ascii_stl() {
    let stl = sample_ascii_stl();
    let input = InputFile::new(Some(std::path::Path::new("fixture.stl")), stl.as_bytes());
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("standalone ASCII STL should import");

    assert_eq!(document.metadata.source_format, "STL");
    assert_eq!(document.metadata.mode, "stl-ascii");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
}

#[test]
fn imports_standalone_obj() {
    let obj = sample_obj();
    let input = InputFile::new(Some(std::path::Path::new("fixture.obj")), obj.as_bytes());
    let registry = ImporterRegistry::default();

    let mut document = registry
        .import(&input, &ImportOptions::default())
        .expect("standalone OBJ should import");
    optimize_document(&mut document, &MeshOptions::default());

    assert_eq!(document.metadata.source_format, "OBJ");
    assert_eq!(document.metadata.mode, "obj-ascii");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
}

#[test]
fn imports_standalone_obj_usemtl_groups() {
    let obj = sample_obj_with_materials();
    let input = InputFile::new(Some(std::path::Path::new("fixture.obj")), obj.as_bytes());
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("standalone OBJ usemtl groups should import");

    assert_eq!(document.metadata.source_format, "OBJ");
    assert_eq!(document.materials.len(), 2);
    assert_eq!(document.meshes[0].primitives.len(), 2);
    assert_eq!(document.meshes[0].primitives[0].material, Some(0));
    assert_eq!(document.meshes[0].primitives[1].material, Some(1));
}

#[test]
fn mesh_triangle_budget_reduces_preview_geometry() {
    let mut document = sample_lite_document();
    let options = MeshOptions {
        max_triangles: Some(1),
        ..MeshOptions::default()
    };

    optimize_document(&mut document, &options);
    validate_document(&document).expect("LOD document should remain valid");

    assert_eq!(document.metadata.triangle_count, 1);
    assert_eq!(document.meshes[0].primitives[0].indices.len(), 3);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("applied triangle budget LOD"))
    );
}

#[test]
fn mesh_position_quantization_snaps_preview_geometry() {
    let mut document = sample_lite_document();
    document.meshes[0].primitives[0].positions[1] = [1.26, 0.24, -0.26];
    let options = MeshOptions {
        weld_vertices: false,
        position_quantization_step: Some(0.5),
        ..MeshOptions::default()
    };

    optimize_document(&mut document, &options);
    validate_document(&document).expect("quantized document should remain valid");

    assert_eq!(
        document.meshes[0].primitives[0].positions[1],
        [1.5, 0.0, -0.5]
    );
    assert_eq!(document.meshes[0].bbox.max[0], 2.0);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("quantized mesh positions to grid step 0.5000000"))
    );
}

#[test]
fn mesh_cleanup_removes_degenerate_triangles_after_quantization() {
    let mut document = sample_lite_document();
    document.meshes[0].primitives[0].positions[2] = [2.0, 0.4, 0.0];
    document.meshes[0].primitives[0].positions[3] = [0.0, 2.0, 0.0];
    let options = MeshOptions {
        position_quantization_step: Some(1.0),
        ..MeshOptions::default()
    };

    optimize_document(&mut document, &options);
    validate_document(&document).expect("degenerate-pruned document should remain valid");

    assert_eq!(document.metadata.triangle_count, 1);
    assert_eq!(document.meshes[0].primitives[0].indices.len(), 3);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("removed 1 degenerate triangles"))
    );
}

#[test]
fn metadata_exports_scene_bbox_with_node_transforms() {
    let mut document = sample_lite_document();
    document.nodes[0].transform[3][0] = 10.0;
    document.nodes[0].transform[3][1] = -2.0;

    let metadata = export_metadata_json(&document);
    let parsed_metadata: serde_json::Value =
        serde_json::from_str(&metadata).expect("metadata JSON should be valid");

    assert_eq!(
        parsed_metadata["bbox"]["min"],
        serde_json::json!([10, -2, 0])
    );
    assert_eq!(
        parsed_metadata["bbox"]["max"],
        serde_json::json!([12, -1, 0])
    );
    assert!(metadata.contains("\"bbox\": {\"min\": [10, -2, 0], \"max\": [12, -1, 0]}"));
}

#[test]
fn imports_standalone_glb() {
    let glb = sample_glb();
    let input = InputFile::new(Some(std::path::Path::new("fixture.glb")), &glb);
    let registry = ImporterRegistry::default();

    let mut document = registry
        .import(&input, &ImportOptions::default())
        .expect("standalone GLB should import");
    optimize_document(&mut document, &MeshOptions::default());

    assert_eq!(document.metadata.source_format, "GLB");
    assert_eq!(document.metadata.mode, "glb-binary");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert_eq!(document.materials.len(), 1);
}

#[test]
fn imports_embedded_binary_stl_from_cadpart() {
    let mut bytes = b"CATPart proprietary container prefix".to_vec();
    bytes.extend_from_slice(&sample_binary_stl());
    bytes.extend_from_slice(b"CATPart proprietary container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CADPart")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("embedded binary STL should import from CADPart-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("embedded binary STL"))
    );
}

#[test]
fn imports_embedded_ascii_stl_from_cadpart() {
    let bytes = format!(
        "CATPart proprietary container prefix\n{}\nCATPart proprietary container suffix",
        sample_ascii_stl()
    );
    let input = InputFile::new(
        Some(std::path::Path::new("fixture.CATPart")),
        bytes.as_bytes(),
    );
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("embedded ASCII STL should import from CATPart-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("embedded ASCII STL"))
    );
}

#[test]
fn imports_embedded_obj_from_cadpart() {
    let bytes = format!(
        "CATPart proprietary container prefix\n{}\nCATPart proprietary container suffix",
        sample_obj()
    );
    let input = InputFile::new(
        Some(std::path::Path::new("fixture.CATPart")),
        bytes.as_bytes(),
    );
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("embedded OBJ should import from CATPart-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("embedded OBJ"))
    );
}

#[test]
fn imports_embedded_glb_from_cadpart() {
    let mut bytes = b"CATPart proprietary container prefix".to_vec();
    bytes.extend_from_slice(&sample_glb());
    bytes.extend_from_slice(b"CATPart proprietary container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CATPart")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("embedded GLB should import from CATPart-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("embedded GLB"))
    );
}

#[test]
fn imports_embedded_glb_from_cgr() {
    let mut bytes = b"CGR proprietary visualization container prefix".to_vec();
    bytes.extend_from_slice(&sample_glb());
    bytes.extend_from_slice(b"CGR proprietary visualization container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CGR")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("embedded GLB should import from CGR-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CGR");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("embedded GLB"))
    );
}

#[test]
fn imports_stored_zip_obj_from_cadpart() {
    let mut bytes = b"CATPart proprietary container prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/model.obj",
        sample_obj().as_bytes(),
    ));
    bytes.extend_from_slice(b"CATPart proprietary container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CADPart")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("stored ZIP OBJ should import from CADPart-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("ZIP stored entry"))
    );
}

#[test]
fn imports_zip_obj_with_mtl_materials_from_cadpart() {
    let mut bytes = b"CATPart proprietary container prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/model.obj",
        sample_obj_with_materials().as_bytes(),
    ));
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/model.mtl",
        sample_mtl().as_bytes(),
    ));
    bytes.extend_from_slice(b"CATPart proprietary container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CATPart")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("ZIP OBJ+MTL should import from CADPart-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert_eq!(document.materials.len(), 2);
    assert_eq!(document.materials[0].name, "RedPaint");
    assert_eq!(document.materials[0].base_color, [1.0, 0.0, 0.0, 0.75]);
    assert_eq!(document.materials[1].name, "BluePaint");
    assert_eq!(document.materials[1].base_color, [0.0, 0.0, 1.0, 0.8]);
    assert_eq!(document.meshes[0].primitives.len(), 2);
    assert_eq!(document.meshes[0].primitives[0].material, Some(0));
    assert_eq!(document.meshes[0].primitives[1].material, Some(1));
}

#[test]
fn imports_stored_zip_glb_from_cadpart() {
    let glb = sample_glb();
    let mut bytes = b"CATPart proprietary container prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry("preview/model.glb", &glb));
    bytes.extend_from_slice(b"CATPart proprietary container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CADPart")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("stored ZIP GLB should import from CADPart-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("ZIP stored entry"))
    );
}

#[test]
fn imports_zip_gltf_with_external_bin_from_cadpart() {
    let (gltf, bin) = sample_gltf_with_bin();
    let mut bytes = b"CATPart proprietary container prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry("preview/model.gltf", gltf.as_bytes()));
    bytes.extend_from_slice(&stored_zip_entry("preview/model.bin", &bin));
    bytes.extend_from_slice(b"CATPart proprietary container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CATPart")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("ZIP glTF+BIN should import from CATPart-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert_eq!(document.materials[0].name, "Default");
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("ZIP stored entry `preview/model.gltf`"))
    );
}

#[test]
fn imports_zip_gltf_node_trs_transform_from_cadpart() {
    let bin = sample_gltf_bin();
    let gltf = sample_gltf_json_with_node_properties(
        "model.bin",
        bin.len(),
        ",\"translation\":[10,20,30],\"rotation\":[0,0,0.70710678,0.70710678],\"scale\":[2,3,4]",
    );
    let mut bytes = b"CATPart proprietary container prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry("preview/model.gltf", gltf.as_bytes()));
    bytes.extend_from_slice(&stored_zip_entry("preview/model.bin", &bin));
    bytes.extend_from_slice(b"CATPart proprietary container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CATPart")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("ZIP glTF TRS transform should import from CATPart-like private file");

    let node = document
        .nodes
        .iter()
        .find(|node| node.name == "Plate")
        .expect("glTF node should be imported");
    assert_close(node.transform[0][0], 0.0);
    assert_close(node.transform[0][1], 2.0);
    assert_close(node.transform[1][0], -3.0);
    assert_close(node.transform[1][1], 0.0);
    assert_close(node.transform[2][2], 4.0);
    assert_close(node.transform[3][0], 10.0);
    assert_close(node.transform[3][1], 20.0);
    assert_close(node.transform[3][2], 30.0);
}

#[test]
fn imports_zip_gltf_data_uri_from_cadpart() {
    let gltf = sample_gltf_with_data_uri();
    let mut bytes = b"CATPart proprietary container prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry("preview/model.gltf", gltf.as_bytes()));
    bytes.extend_from_slice(b"CATPart proprietary container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CATPart")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("ZIP glTF data URI should import from CATPart-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("ZIP stored entry `preview/model.gltf`"))
    );
}

#[test]
fn imports_zip_gltf_with_interleaved_offset_buffer_from_cadpart() {
    let (gltf, bin) = sample_interleaved_gltf_with_bin();
    let mut bytes = b"CATPart proprietary container prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/interleaved.gltf",
        gltf.as_bytes(),
    ));
    bytes.extend_from_slice(&stored_zip_entry("preview/interleaved.bin", &bin));
    bytes.extend_from_slice(b"CATPart proprietary container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CATPart")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("ZIP interleaved glTF+BIN should import from CATPart-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    let primitive = &document.meshes[0].primitives[0];
    assert_eq!(
        primitive.positions,
        vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ]
    );
    assert_eq!(primitive.normals, vec![[0.0, 0.0, 1.0]; 4]);
    assert_eq!(primitive.indices, vec![0, 1, 2, 0, 2, 3]);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("ZIP stored entry `preview/interleaved.gltf`"))
    );
}

#[test]
fn imports_deflated_zip_glb_from_cadpart() {
    let glb = sample_glb();
    let mut bytes = b"CATPart proprietary container prefix".to_vec();
    bytes.extend_from_slice(&deflated_zip_entry("preview/model.glb", &glb));
    bytes.extend_from_slice(b"CATPart proprietary container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CATPart")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("deflated ZIP GLB should import from CATPart-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("ZIP deflated entry"))
    );
}

#[test]
fn rejects_zip_entry_before_decompression_when_import_limit_is_exceeded() {
    let glb = sample_glb();
    let mut bytes = b"CATPart proprietary container prefix".to_vec();
    bytes.extend_from_slice(&deflated_zip_entry("preview/model.glb", &glb));
    let input = InputFile::new(Some(std::path::Path::new("fixture.CATPart")), &bytes);
    let options = ImportOptions {
        limits: ImportLimits {
            max_archive_entry_uncompressed_bytes: glb.len() - 1,
            ..ImportLimits::default()
        },
        ..ImportOptions::default()
    };

    let error = ImporterRegistry::default()
        .import(&input, &options)
        .expect_err("oversized ZIP entry should be rejected before import");
    assert!(matches!(
        error,
        ImportError::ResourceLimitExceeded {
            resource: "ZIP entry uncompressed bytes",
            limit,
            actual,
        } if limit == glb.len() - 1 && actual == glb.len()
    ));
}

#[test]
fn input_byte_limit_is_enforced_by_import_and_inspect_apis() {
    let bytes = format!("CATPart private payload prefix\n{SAMPLE_CACHE}");
    let path = std::path::Path::new("limited.CATPart");
    let input = InputFile::new(Some(path), bytes.as_bytes());
    let options = ImportOptions {
        limits: ImportLimits {
            max_input_bytes: bytes.len() - 1,
            ..ImportLimits::default()
        },
        ..ImportOptions::default()
    };

    let import_error = ImporterRegistry::default()
        .import(&input, &options)
        .expect_err("oversized byte input should be rejected before import");
    assert!(matches!(
        import_error,
        ImportError::ResourceLimitExceeded {
            resource: "input bytes",
            limit,
            actual,
        } if limit == bytes.len() - 1 && actual == bytes.len()
    ));

    let inspect_error = inspect_bytes(
        Some(path),
        bytes.as_bytes(),
        &InspectOptions {
            import: options,
            check_import: true,
        },
    )
    .expect_err("oversized byte input should be rejected before inspection");
    assert!(
        inspect_error
            .to_string()
            .contains("resource limit exceeded for input bytes")
    );
}

#[test]
fn imports_data_descriptor_zip_glb_from_cadpart() {
    let glb = sample_glb();
    let mut bytes = b"CATPart proprietary container prefix".to_vec();
    bytes.extend_from_slice(&deflated_zip_entry_with_data_descriptor(
        "preview/model.glb",
        &glb,
    ));
    bytes.extend_from_slice(b"CATPart proprietary container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CATPart")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("data-descriptor ZIP GLB should import from CATPart-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("ZIP deflated entry"))
    );
}

#[test]
fn imports_multiple_stored_zip_assets_from_cadpart() {
    let glb = sample_glb();
    let mut bytes = b"CATProduct private payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/part-a.obj",
        sample_obj().as_bytes(),
    ));
    bytes.extend_from_slice(&stored_zip_entry("preview/part-b.glb", &glb));
    bytes.extend_from_slice(b"CATProduct private payload suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CATProduct")), &bytes);
    let registry = ImporterRegistry::default();

    let mut document = registry
        .import(&input, &ImportOptions::default())
        .expect("multiple stored ZIP visual assets should merge into one document");
    validate_document(&document).expect("merged document should validate");
    optimize_document(&mut document, &MeshOptions::default());

    assert_eq!(document.metadata.source_format, "CATIA_CATProduct");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 2);
    assert_eq!(document.metadata.triangle_count, 4);
    assert!(
        document
            .nodes
            .iter()
            .any(|node| node.name == "preview/part-a.obj")
    );
    assert!(
        document
            .nodes
            .iter()
            .any(|node| node.name == "preview/part-b.glb")
    );
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("merged 2 ZIP visual assets"))
    );

    let glb = export_glb(&document, &GlbExportOptions::default()).expect("GLB should export");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());
}

#[test]
fn imports_catproduct_cache_external_references_from_resolve_dir() {
    let temp_dir = unique_temp_dir("catproduct-references");
    let parts_dir = temp_dir.join("parts");
    fs::create_dir_all(&parts_dir).expect("parts dir should be created");
    fs::write(parts_dir.join("part-a.CATPart"), SAMPLE_CACHE)
        .expect("part A cache should be written");
    fs::write(parts_dir.join("part-b.CATPart"), SAMPLE_CACHE)
        .expect("part B cache should be written");

    let assembly_path = temp_dir.join("assembly.CATProduct");
    let assembly = "\
CATProduct private payload prefix
FEATHER_CAD_LITE_CACHE_V1
document Assembly
reference PartA part-a.CATPart root
reference PartB part-b.CATPart root
END_FEATHER_CAD_LITE_CACHE
";
    let input = InputFile::new(Some(&assembly_path), assembly.as_bytes());
    let options = ImportOptions {
        resolve_dirs: vec![parts_dir],
        ..ImportOptions::default()
    };
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &options)
        .expect("CATProduct external references should import");
    validate_document(&document).expect("resolved assembly should validate");

    assert_eq!(document.metadata.source_format, "CATIA_CATProduct");
    assert_eq!(document.metadata.mode, "catia-cache-only");
    assert_eq!(document.metadata.mesh_count, 2);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(document.nodes.iter().any(|node| node.name == "PartA"));
    assert!(document.nodes.iter().any(|node| node.name == "PartB"));
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("resolved external reference `part-a.CATPart`"))
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn imports_catproduct_zip_manifest_external_references_from_resolve_dir() {
    let temp_dir = unique_temp_dir("catproduct-zip-external-references");
    let parts_dir = temp_dir.join("parts");
    fs::create_dir_all(&parts_dir).expect("parts dir should be created");
    fs::write(parts_dir.join("part-a.CATPart"), SAMPLE_CACHE)
        .expect("part A cache should be written");
    fs::write(parts_dir.join("part-b.CATPart"), SAMPLE_CACHE)
        .expect("part B cache should be written");

    let assembly_path = temp_dir.join("assembly.CATProduct");
    let manifest = r#"
<Assembly name="ZipExternalAssembly">
  <Component name="PartA" href="part-a.CATPart"/>
  <Component name="PartB" file="legacy\released\part-b.CATPart" translation="0 5 0"/>
</Assembly>
"#;
    let mut bytes = b"CATProduct private ZIP assembly prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry("assembly.xml", manifest.as_bytes()));
    let input = InputFile::new(Some(&assembly_path), &bytes);
    let options = ImportOptions {
        resolve_dirs: vec![parts_dir],
        ..ImportOptions::default()
    };
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &options)
        .expect("CATProduct ZIP manifest external references should import");

    validate_document(&document).expect("resolved ZIP assembly should validate");
    assert_eq!(document.metadata.source_format, "CATIA_CATProduct");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 2);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document.metadata.warnings.iter().any(|warning| {
            warning.contains(
                "applied ZIP assembly manifest `assembly.xml` with 0 visual references and 2 external references",
            )
        })
    );
    assert!(document.metadata.warnings.iter().any(|warning| {
        warning.contains("resolved external reference `legacy/released/part-b.CATPart`")
    }));
    let part_b = document
        .nodes
        .iter()
        .find(|node| node.name == "PartB")
        .expect("ZIP manifest external node should exist");
    assert_eq!(part_b.transform[3][1], 5.0);
    assert!(!part_b.children.is_empty());

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn resolves_assembly_references_with_windows_paths() {
    let temp_dir = unique_temp_dir("catproduct-windows-references");
    let parts_dir = temp_dir.join("parts");
    fs::create_dir_all(&parts_dir).expect("parts dir should be created");
    fs::write(parts_dir.join("part-a.CATPart"), SAMPLE_CACHE)
        .expect("part A cache should be written");
    fs::write(parts_dir.join("part-b.CATPart"), SAMPLE_CACHE)
        .expect("part B cache should be written");

    let assembly_path = temp_dir.join("assembly.CATProduct");
    let assembly = "\
CATProduct private payload prefix
FEATHER_CAD_LITE_CACHE_V1
document Assembly
reference PartA parts\\part-a.CATPart root
reference PartB C:\\legacy\\released\\part-b.CATPart root
END_FEATHER_CAD_LITE_CACHE
";
    let input = InputFile::new(Some(&assembly_path), assembly.as_bytes());
    let options = ImportOptions {
        resolve_dirs: vec![parts_dir],
        ..ImportOptions::default()
    };
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &options)
        .expect("Windows-style assembly references should resolve");

    validate_document(&document).expect("resolved assembly should validate");
    assert_eq!(document.metadata.source_format, "CATIA_CATProduct");
    assert_eq!(document.metadata.mesh_count, 2);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("resolved external reference `parts\\part-a.CATPart`"))
    );
    assert!(document.metadata.warnings.iter().any(|warning| {
        warning.contains("resolved external reference `C:\\legacy\\released\\part-b.CATPart`")
    }));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn resolves_assembly_references_with_mapped_legacy_roots() {
    let temp_dir = unique_temp_dir("catproduct-reference-map-root");
    let migrated_root = temp_dir.join("released-package");
    let migrated_parts_dir = migrated_root.join("released");
    fs::create_dir_all(&migrated_parts_dir).expect("migrated parts dir should be created");
    fs::write(migrated_parts_dir.join("part-b.CATPart"), SAMPLE_CACHE)
        .expect("migrated part cache should be written");

    let assembly_path = temp_dir.join("assembly.CATProduct");
    let assembly = "\
CATProduct private payload prefix
FEATHER_CAD_LITE_CACHE_V1
document Assembly
reference PartB C:\\vault\\legacy\\released\\part-b.CATPart root
END_FEATHER_CAD_LITE_CACHE
";
    let input = InputFile::new(Some(&assembly_path), assembly.as_bytes());
    let options = ImportOptions {
        reference_path_mappings: vec![ReferencePathMapping::new(
            "C:\\vault\\legacy",
            migrated_root,
        )],
        ..ImportOptions::default()
    };
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &options)
        .expect("legacy root mapping should resolve external reference");

    validate_document(&document).expect("resolved assembly should validate");
    assert_eq!(document.metadata.source_format, "CATIA_CATProduct");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 1);
    assert!(document.metadata.warnings.iter().any(|warning| {
        warning
            .contains("resolved external reference `C:\\vault\\legacy\\released\\part-b.CATPart`")
    }));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn resolves_assembly_references_with_quoted_space_paths() {
    let temp_dir = unique_temp_dir("catproduct-space-references");
    let parts_dir = temp_dir.join("parts with spaces");
    fs::create_dir_all(&parts_dir).expect("parts dir should be created");
    fs::write(parts_dir.join("Part A.CATPart"), SAMPLE_CACHE)
        .expect("part cache should be written");

    let assembly_path = temp_dir.join("assembly.CATProduct");
    let assembly = "\
CATProduct private payload prefix
FEATHER_CAD_LITE_CACHE_V1
document \"Assembly With Spaces\"
reference \"Part A Instance\" \"Part A.CATPart\" root
END_FEATHER_CAD_LITE_CACHE
";
    let input = InputFile::new(Some(&assembly_path), assembly.as_bytes());
    let options = ImportOptions {
        resolve_dirs: vec![parts_dir],
        ..ImportOptions::default()
    };
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &options)
        .expect("quoted assembly references should resolve");

    validate_document(&document).expect("resolved assembly should validate");
    assert_eq!(document.metadata.source_format, "CATIA_CATProduct");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 1);
    assert!(
        document
            .nodes
            .iter()
            .any(|node| node.name == "Part A Instance")
    );
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("document name: Assembly With Spaces"))
    );
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("resolved external reference `Part A.CATPart`"))
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn discovers_visual_assets_for_cache_dumping() {
    let glb = sample_glb();
    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/part-a.obj",
        sample_obj().as_bytes(),
    ));
    bytes.extend_from_slice(&stored_zip_entry("preview/part-b.glb", &glb));
    bytes.extend_from_slice(b"CATPart private payload suffix");

    let assets = discover_embedded_visual_assets(&bytes).expect("assets should be discoverable");

    assert_eq!(assets.len(), 2);
    assert_eq!(assets[0].kind.label(), "obj");
    assert_eq!(assets[0].source.label(), "zip-entry");
    assert_eq!(assets[0].name.as_deref(), Some("preview/part-a.obj"));
    assert_eq!(assets[1].kind.label(), "glb");
    assert_eq!(assets[1].source.label(), "zip-entry");
    assert_eq!(assets[1].name.as_deref(), Some("preview/part-b.glb"));
}

#[test]
fn discovers_3dxml_rep_assets_for_cache_dumping() {
    let mut bytes = b"3DXML private payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/bracket.3DRep",
        sample_3dxml_rep().as_bytes(),
    ));
    bytes.extend_from_slice(b"3DXML private payload suffix");

    let assets =
        discover_embedded_visual_assets(&bytes).expect("3DRep ZIP asset should be discoverable");

    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].kind.label(), "3dxml-rep");
    assert_eq!(assets[0].source.label(), "zip-entry");
    assert_eq!(assets[0].name.as_deref(), Some("preview/bracket.3DRep"));
}

#[test]
fn discovers_deflated_zip_assets_for_cache_dumping() {
    let glb = sample_glb();
    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&deflated_zip_entry("preview/model.glb", &glb));
    bytes.extend_from_slice(b"CATPart private payload suffix");

    let assets =
        discover_embedded_visual_assets(&bytes).expect("deflated ZIP asset should be discoverable");

    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].kind.label(), "glb");
    assert_eq!(assets[0].source.label(), "zip-entry");
    assert_eq!(assets[0].name.as_deref(), Some("preview/model.glb"));
}

#[test]
fn archive_discovery_enforces_entry_count_and_total_uncompressed_limits() {
    let payload = sample_obj().as_bytes();
    let mut bytes = stored_zip_entry("preview/one.obj", payload);
    bytes.extend_from_slice(&stored_zip_entry("preview/two.obj", payload));

    let count_error = discover_embedded_visual_assets_with_limits(
        &bytes,
        &ImportLimits {
            max_archive_entries: 1,
            ..ImportLimits::default()
        },
    )
    .expect_err("archive entry count should be limited");
    assert!(matches!(
        count_error,
        ImportError::ResourceLimitExceeded {
            resource: "ZIP entry count",
            limit: 1,
            actual: 2,
        }
    ));

    let total_limit = payload.len() * 2 - 1;
    let total_error = discover_embedded_visual_assets_with_limits(
        &bytes,
        &ImportLimits {
            max_archive_entry_uncompressed_bytes: payload.len(),
            max_archive_total_uncompressed_bytes: total_limit,
            ..ImportLimits::default()
        },
    )
    .expect_err("archive cumulative uncompressed size should be limited");
    assert!(matches!(
        total_error,
        ImportError::ResourceLimitExceeded {
            resource: "ZIP total uncompressed bytes",
            limit,
            actual,
        } if limit == total_limit && actual == payload.len() * 2
    ));
}

#[test]
fn discovers_data_descriptor_zip_assets_for_cache_dumping() {
    let glb = sample_glb();
    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&deflated_zip_entry_with_data_descriptor(
        "preview/model.glb",
        &glb,
    ));
    bytes.extend_from_slice(b"CATPart private payload suffix");

    let assets = discover_embedded_visual_assets(&bytes)
        .expect("data-descriptor ZIP asset should be discoverable");

    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].kind.label(), "glb");
    assert_eq!(assets[0].source.label(), "zip-entry");
    assert_eq!(assets[0].name.as_deref(), Some("preview/model.glb"));
}

#[test]
fn discovers_zip_gltf_data_uri_assets_for_cache_dumping() {
    let gltf = sample_gltf_with_data_uri();
    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry("preview/model.gltf", gltf.as_bytes()));
    bytes.extend_from_slice(b"CATPart private payload suffix");

    let assets =
        discover_embedded_visual_assets(&bytes).expect("ZIP glTF asset should be discoverable");

    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].kind.label(), "gltf");
    assert_eq!(assets[0].source.label(), "zip-entry");
    assert_eq!(assets[0].name.as_deref(), Some("preview/model.gltf"));
}

#[test]
fn discovers_zip_gltf_and_bin_assets_for_cache_dumping() {
    let (gltf, bin) = sample_gltf_with_bin();
    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry("preview/model.gltf", gltf.as_bytes()));
    bytes.extend_from_slice(&stored_zip_entry("preview/model.bin", &bin));
    bytes.extend_from_slice(b"CATPart private payload suffix");

    let assets = discover_embedded_visual_assets(&bytes)
        .expect("ZIP glTF+BIN assets should be discoverable");

    assert_eq!(assets.len(), 2);
    assert_eq!(assets[0].kind.label(), "gltf");
    assert_eq!(assets[0].source.label(), "zip-entry");
    assert_eq!(assets[0].name.as_deref(), Some("preview/model.gltf"));
    assert_eq!(assets[1].kind.label(), "gltf-bin");
    assert_eq!(assets[1].source.label(), "zip-entry");
    assert_eq!(assets[1].name.as_deref(), Some("preview/model.bin"));
}

#[test]
fn imports_zip64_deflated_zip_glb_from_cadpart() {
    let glb = sample_glb();
    let mut bytes = b"CATPart proprietary container prefix".to_vec();
    bytes.extend_from_slice(&zip64_deflated_zip_entry("preview/model.glb", &glb));
    bytes.extend_from_slice(b"CATPart proprietary container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.CATPart")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("ZIP64 deflated ZIP GLB should import from CATPart-like private file");

    assert_eq!(document.metadata.source_format, "CATIA_CATPart");
    assert_eq!(document.metadata.mode, "catia-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("ZIP deflated entry"))
    );
}

#[test]
fn discovers_zip64_zip_assets_for_cache_dumping() {
    let glb = sample_glb();
    let mut bytes = b"CATPart private payload prefix".to_vec();
    bytes.extend_from_slice(&zip64_deflated_zip_entry("preview/model.glb", &glb));
    bytes.extend_from_slice(b"CATPart private payload suffix");

    let assets =
        discover_embedded_visual_assets(&bytes).expect("ZIP64 ZIP asset should be discoverable");

    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].kind.label(), "glb");
    assert_eq!(assets[0].source.label(), "zip-entry");
    assert_eq!(assets[0].name.as_deref(), Some("preview/model.glb"));
}

#[test]
fn imports_visual_asset_from_ole_stream_private_cad() {
    let ole = sample_ole_with_stream("PreviewGLB", &sample_ole_stream_payload());
    let input = InputFile::new(Some(std::path::Path::new("fixture.ipt")), &ole);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("private CAD OLE stream with embedded GLB should import");

    assert_eq!(document.metadata.source_format, "PRIVATE_CAD");
    assert_eq!(document.metadata.mode, "private-cad-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("OLE stream `PreviewGLB`"))
    );
}

#[test]
fn discovers_visual_asset_inside_ole_stream() {
    let ole = sample_ole_with_stream("PreviewGLB", &sample_ole_stream_payload());

    let assets =
        discover_embedded_visual_assets(&ole).expect("OLE stream asset should be discoverable");

    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].kind.label(), "glb");
    assert_eq!(assets[0].source.label(), "ole-stream");
    assert_eq!(assets[0].name.as_deref(), Some("PreviewGLB"));
}

#[test]
fn ole_discovery_enforces_stream_count_size_and_total_limits() {
    let payload = sample_ole_stream_payload();
    let ole = sample_ole_with_stream("PreviewGLB", &payload);

    let count_error = discover_embedded_visual_assets_with_limits(
        &ole,
        &ImportLimits {
            max_ole_streams: 0,
            ..ImportLimits::default()
        },
    )
    .expect_err("OLE stream count should be limited");
    assert!(matches!(
        count_error,
        ImportError::ResourceLimitExceeded {
            resource: "OLE stream count",
            limit: 0,
            actual: 1,
        }
    ));

    let byte_limit = payload.len() - 1;
    let stream_error = discover_embedded_visual_assets_with_limits(
        &ole,
        &ImportLimits {
            max_ole_stream_bytes: byte_limit,
            ..ImportLimits::default()
        },
    )
    .expect_err("one oversized OLE stream should be limited");
    assert!(matches!(
        stream_error,
        ImportError::ResourceLimitExceeded {
            resource: "OLE stream bytes",
            limit,
            actual,
        } if limit == byte_limit && actual == payload.len()
    ));

    let total_error = discover_embedded_visual_assets_with_limits(
        &ole,
        &ImportLimits {
            max_ole_total_stream_bytes: byte_limit,
            ..ImportLimits::default()
        },
    )
    .expect_err("cumulative OLE stream bytes should be limited");
    assert!(matches!(
        total_error,
        ImportError::ResourceLimitExceeded {
            resource: "OLE total stream bytes",
            limit,
            actual,
        } if limit == byte_limit && actual == payload.len()
    ));
}

#[test]
fn ole_rejects_sector_table_counts_larger_than_the_container() {
    let mut oversized_fat = sample_ole_with_stream("PreviewGLB", &sample_ole_stream_payload());
    write_u32(&mut oversized_fat, 0x2C, u32::MAX);
    let fat_error = discover_embedded_visual_assets(&oversized_fat)
        .expect_err("impossible OLE FAT sector count should be rejected");
    assert!(
        fat_error
            .to_string()
            .contains("OLE FAT sector count 4294967295 exceeds container sector count")
    );

    let mut oversized_difat = sample_ole_with_stream("PreviewGLB", &sample_ole_stream_payload());
    write_u32(&mut oversized_difat, 0x48, u32::MAX);
    let difat_error = discover_embedded_visual_assets(&oversized_difat)
        .expect_err("impossible OLE DIFAT sector count should be rejected");
    assert!(
        difat_error
            .to_string()
            .contains("OLE DIFAT sector count 4294967295 exceeds container sector count")
    );
}

#[test]
fn imports_visual_asset_from_ole_mini_stream_private_cad() {
    let ole = sample_ole_with_mini_stream("SmallPreviewGLB", &sample_glb());
    let input = InputFile::new(Some(std::path::Path::new("fixture.ipt")), &ole);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("private CAD OLE mini stream with GLB should import");

    assert_eq!(document.metadata.source_format, "PRIVATE_CAD");
    assert_eq!(document.metadata.mode, "private-cad-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("OLE stream `SmallPreviewGLB`"))
    );
}

#[test]
fn discovers_visual_asset_inside_ole_mini_stream() {
    let ole = sample_ole_with_mini_stream("SmallPreviewGLB", &sample_glb());

    let assets = discover_embedded_visual_assets(&ole)
        .expect("OLE mini stream asset should be discoverable");

    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].kind.label(), "glb");
    assert_eq!(assets[0].source.label(), "ole-stream");
    assert_eq!(assets[0].name.as_deref(), Some("SmallPreviewGLB"));
}

#[test]
fn imports_embedded_binary_stl_from_nx_prt() {
    let mut bytes = b"Unigraphics NX private container prefix".to_vec();
    bytes.extend_from_slice(&sample_binary_stl());
    let input = InputFile::new(Some(std::path::Path::new("fixture.prt")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("embedded binary STL should import from NX-like private file");

    assert_eq!(document.metadata.source_format, "NX_PRT");
    assert_eq!(document.metadata.mode, "nx-embedded-visual-asset");
    assert_eq!(document.metadata.triangle_count, 2);
}

#[test]
fn imports_embedded_glb_from_solidworks_part() {
    let mut bytes = b"SolidWorks SLDPRT private container prefix".to_vec();
    bytes.extend_from_slice(&sample_glb());
    bytes.extend_from_slice(b"SolidWorks private container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.SLDPRT")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("embedded GLB should import from SolidWorks-like private file");

    assert_eq!(document.metadata.source_format, "SOLIDWORKS_SLDPRT");
    assert_eq!(document.metadata.mode, "solidworks-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
}

#[test]
fn imports_multiple_stored_zip_assets_from_solidworks_assembly() {
    let glb = sample_glb();
    let mut bytes = b"SolidWorks SLDASM private payload prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/component-a.obj",
        sample_obj().as_bytes(),
    ));
    bytes.extend_from_slice(&stored_zip_entry("preview/component-b.glb", &glb));
    let input = InputFile::new(Some(std::path::Path::new("fixture.SLDASM")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("multiple stored ZIP visual assets should import from SolidWorks assembly");

    assert_eq!(document.metadata.source_format, "SOLIDWORKS_SLDASM");
    assert_eq!(document.metadata.mode, "solidworks-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 2);
    assert_eq!(document.metadata.triangle_count, 4);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("merged 2 ZIP visual assets"))
    );
}

#[test]
fn imports_creo_style_prt_without_mislabeling_as_nx() {
    let mut bytes = b"Creo Parametric private part prefix".to_vec();
    bytes.extend_from_slice(&sample_glb());
    let input = InputFile::new(Some(std::path::Path::new("fixture.prt")), &bytes);
    let registry = ImporterRegistry::default();

    let probe = registry.probe(&input);
    assert_eq!(probe.format.label(), "PRIVATE_CAD");

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("generic private .prt with embedded GLB should import");

    assert_eq!(document.metadata.source_format, "PRIVATE_CAD");
    assert_eq!(document.metadata.mode, "private-cad-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
}

#[test]
fn imports_jt_with_embedded_glb_as_private_cad() {
    let mut bytes = b"JT lightweight private container prefix".to_vec();
    bytes.extend_from_slice(&sample_glb());
    bytes.extend_from_slice(b"JT lightweight private container suffix");
    let input = InputFile::new(Some(std::path::Path::new("fixture.jt")), &bytes);
    let registry = ImporterRegistry::default();

    let probe = registry.probe(&input);
    assert_eq!(probe.format.label(), "PRIVATE_CAD");

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("JT-like private file with embedded GLB should import");

    assert_eq!(document.metadata.source_format, "PRIVATE_CAD");
    assert_eq!(document.metadata.mode, "private-cad-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
}

#[test]
fn imports_acis_sat_with_embedded_obj_as_private_cad() {
    let bytes = format!(
        "ACIS private payload prefix\n{}\nACIS private payload suffix",
        sample_obj()
    );
    let input = InputFile::new(Some(std::path::Path::new("fixture.sat")), bytes.as_bytes());
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("ACIS SAT-like private file with embedded OBJ should import");

    assert_eq!(document.metadata.source_format, "PRIVATE_CAD");
    assert_eq!(document.metadata.mode, "private-cad-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
}

#[test]
fn imports_iges_with_embedded_obj_as_private_cad() {
    let bytes = format!(
        "IGES private payload prefix\n{}\nIGES private payload suffix",
        sample_obj()
    );
    let input = InputFile::new(Some(std::path::Path::new("fixture.igs")), bytes.as_bytes());
    let registry = ImporterRegistry::default();

    let probe = registry.probe(&input);
    assert_eq!(probe.format, FileFormat::PrivateCad);

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("IGES-like private file with embedded OBJ should import");

    assert_eq!(document.metadata.source_format, "PRIVATE_CAD");
    assert_eq!(document.metadata.mode, "private-cad-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
}

#[test]
fn imports_inventor_style_assembly_stored_zip_assets() {
    let glb = sample_glb();
    let mut bytes = b"Autodesk Inventor private assembly prefix".to_vec();
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/component-a.obj",
        sample_obj().as_bytes(),
    ));
    bytes.extend_from_slice(&stored_zip_entry("preview/component-b.glb", &glb));
    let input = InputFile::new(Some(std::path::Path::new("fixture.iam")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("generic private assembly should import stored visual assets");

    assert_eq!(document.metadata.source_format, "PRIVATE_CAD");
    assert_eq!(document.metadata.mode, "private-cad-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 2);
    assert_eq!(document.metadata.triangle_count, 4);
}

#[test]
fn imports_xml_3drep_polygon_faces() {
    let document = import_3dxml_rep_document(
        sample_3dxml_rep_with_strips_and_fans().as_bytes(),
        "DASSAULT_3DXML",
        "3dxml-embedded-visual-asset",
        None,
    )
    .expect("readable XML 3DRep should import");

    validate_document(&document).expect("3DRep document should validate");
    assert_eq!(document.metadata.source_format, "DASSAULT_3DXML");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 5);
    assert_eq!(document.meshes[0].primitives[0].positions.len(), 5);
    assert_eq!(document.meshes[0].primitives[0].normals.len(), 5);
}

#[test]
fn imports_3dxml_zip_xml_assembly_manifest() {
    let glb = sample_glb();
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
    bytes.extend_from_slice(&stored_zip_entry("preview/bracket.glb", &glb));
    bytes.extend_from_slice(&stored_zip_entry(
        "preview/cover.obj",
        sample_obj().as_bytes(),
    ));
    let input = InputFile::new(Some(std::path::Path::new("fixture.3dxml")), &bytes);
    let registry = ImporterRegistry::default();

    let probe = registry.probe(&input);
    assert_eq!(probe.format, FileFormat::Dassault3dxml);

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("3DXML ZIP XML assembly manifest should import previews");

    validate_document(&document).expect("manifest assembly document should validate");
    assert_eq!(document.metadata.source_format, "DASSAULT_3DXML");
    assert_eq!(document.metadata.mode, "3dxml-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 2);
    assert_eq!(document.metadata.triangle_count, 4);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("applied ZIP assembly manifest `Manifest.xml`"))
    );
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .all(|warning| !warning.contains("merged 2 ZIP visual assets"))
    );

    let bracket = document
        .nodes
        .iter()
        .find(|node| node.name == "Bracket")
        .expect("Bracket assembly node should exist");
    assert_eq!(bracket.source_id.as_deref(), Some("preview/bracket.glb"));
    assert_eq!(bracket.transform[3][0], 10.0);

    let cover = document
        .nodes
        .iter()
        .find(|node| node.name == "Cover")
        .expect("Cover assembly node should exist");
    assert_eq!(cover.source_id.as_deref(), Some("preview/cover.obj"));
    assert_eq!(cover.transform[3][1], 20.0);
}

#[test]
fn imports_3dxml_product_structure_id_relationships() {
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
    let input = InputFile::new(Some(std::path::Path::new("fixture.3dxml")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("3DXML ProductStructure relationships should import previews");

    validate_document(&document).expect("3DXML relationship document should validate");
    assert_eq!(document.metadata.source_format, "DASSAULT_3DXML");
    assert_eq!(document.metadata.mode, "3dxml-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("applied ZIP assembly manifest `Root.3dxml`"))
    );

    let node_names = document
        .nodes
        .iter()
        .map(|node| node.name.clone())
        .collect::<Vec<_>>();
    let instance = document
        .nodes
        .iter()
        .find(|node| node.name == "BracketInstance")
        .unwrap_or_else(|| panic!("Instance3D relationship node should exist: {node_names:?}"));
    assert_eq!(instance.transform[3][0], 30.0);
    let rep = document
        .nodes
        .iter()
        .find(|node| node.name == "BracketRep")
        .expect("InstanceRep visual node should exist");
    assert_eq!(rep.source_id.as_deref(), Some("preview/bracket.glb"));
}

#[test]
fn imports_3dxml_product_structure_with_xml_3drep() {
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
    let input = InputFile::new(Some(std::path::Path::new("fixture.3dxml")), &bytes);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("3DXML ProductStructure should import readable XML 3DRep previews");

    validate_document(&document).expect("3DXML 3DRep relationship document should validate");
    assert_eq!(document.metadata.source_format, "DASSAULT_3DXML");
    assert_eq!(document.metadata.mode, "3dxml-embedded-visual-asset");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("applied ZIP assembly manifest `Root.3dxml`"))
    );
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("ZIP stored entry `preview/bracket.3DRep`"))
    );

    let instance = document
        .nodes
        .iter()
        .find(|node| node.name == "BracketInstance")
        .expect("Instance3D relationship node should exist");
    assert_eq!(instance.transform[3][0], 30.0);
    let rep = document
        .nodes
        .iter()
        .find(|node| node.name == "BracketRep")
        .expect("InstanceRep visual node should exist");
    assert_eq!(rep.source_id.as_deref(), Some("preview/bracket.3DRep"));
}

#[test]
fn private_cad_without_visual_payload_fails_explicitly() {
    let input = InputFile::new(
        Some(std::path::Path::new("fixture.ipt")),
        b"Autodesk Inventor private payload without a readable preview cache",
    );
    let registry = ImporterRegistry::default();
    let error = registry
        .import(&input, &ImportOptions::default())
        .expect_err("private CAD without visual payload should fail");

    assert!(matches!(
        error,
        ImportError::NoLightweightCache { ref format } if format == "PRIVATE_CAD"
    ));
}

#[test]
fn imports_step_with_embedded_tessellation_cache() {
    let bytes = format!(
        "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n{SAMPLE_CACHE}\nENDSEC;\nEND-ISO-10303-21;"
    );
    let path = std::path::Path::new("fixture.step");
    let input = InputFile::new(Some(path), bytes.as_bytes());
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("STEP with embedded tessellation cache should import");

    assert_eq!(document.metadata.source_format, "STEP");
    assert_eq!(document.metadata.mode, "step-embedded-tessellation");
    assert!(document.metadata.has_brep);
    assert!(!document.metadata.brep_preserved);
}

#[test]
fn imports_native_ap242_triangulated_face() {
    let step = b"ISO-10303-21;
HEADER;
FILE_DESCRIPTION(('AP242 tessellated test'),'2;1');
FILE_NAME('triangulated','2026-06-07T00:00:00',('feather'),('feather'),'','','');
FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF'));
ENDSEC;
DATA;
#10=COORDINATES_LIST('',4,((0.,0.,0.),(2.,0.,0.),(2.,1.,0.),(0.,1.,0.)));
#20=TRIANGULATED_FACE('',#10,4,$,$,(),((1,2,3),(1,3,4)));
#30=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));
#31=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#30);
#32=(CONVERSION_BASED_UNIT('INCH',#31) LENGTH_UNIT() NAMED_UNIT(*));
#33=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#32)) REPRESENTATION_CONTEXT('',''));
ENDSEC;
END-ISO-10303-21;";
    let input = InputFile::new(Some(std::path::Path::new("fixture.step")), step);
    let registry = ImporterRegistry::default();

    let mut document = registry
        .import(&input, &ImportOptions::default())
        .expect("AP242 tessellated STEP should import natively");
    optimize_document(&mut document, &MeshOptions::default());

    assert_eq!(document.metadata.source_format, "STEP");
    assert_eq!(document.metadata.mode, "step-ap242-tessellated");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 2);
    assert_close(document.meshes[0].bbox.max[0], 0.0508);
    assert_close(document.meshes[0].bbox.max[1], 0.0254);
    assert_close(document.meshes[0].bbox.max[2], 0.0);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("inch") && warning.contains("metres"))
    );

    let glb = export_glb(&document, &GlbExportOptions::default()).expect("GLB should export");
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());
}

#[test]
fn imports_ap242_tessellated_assembly_hierarchy() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap242_tessellated_assembly.step");
    let input = InputFile::new(Some(std::path::Path::new("assembly-ap242.step")), step);

    let document = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect("AP242 tessellated assembly should import");

    validate_document(&document).expect("AP242 assembly document should validate");
    assert_eq!(document.metadata.mode, "step-ap242-assembly-tessellated");
    assert_eq!(document.metadata.mesh_count, 2);
    assert_eq!(document.metadata.triangle_count, 3);
    assert_eq!(document.nodes.len(), 7);
    let root = document
        .nodes
        .iter()
        .find(|node| node.name == "RootAssembly")
        .expect("AP242 root assembly node should exist");
    assert_eq!(root.children.len(), 2);
    assert_eq!(document.nodes[root.children[0]].name, "SubOne");
    assert_eq!(document.nodes[root.children[1]].name, "SubTwo");
}

#[test]
fn rejects_conflicting_step_length_unit_contexts() {
    let step = b"ISO-10303-21;
HEADER;
FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF'));
ENDSEC;
DATA;
#10=COORDINATES_LIST('',3,((0.,0.,0.),(1.,0.,0.),(0.,1.,0.)));
#20=TRIANGULATED_FACE('',#10,3,$,$,(),((1,2,3)));
#30=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));
#31=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT($,.METRE.));
#40=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#30)) REPRESENTATION_CONTEXT('',''));
#41=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#31)) REPRESENTATION_CONTEXT('',''));
ENDSEC;
END-ISO-10303-21;";
    let input = InputFile::new(Some(std::path::Path::new("conflicting-units.step")), step);

    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("conflicting STEP length units must be rejected");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason) if reason.contains("conflicting length units")
    ));
}

#[test]
fn imports_native_ap214_planar_brep_box() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_planar_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("planar-box.step")), step);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("planar ADVANCED_FACE B-Rep should tessellate natively");

    validate_document(&document).expect("planar STEP B-Rep document should validate");
    assert_eq!(document.metadata.source_format, "STEP");
    assert_eq!(document.metadata.mode, "step-brep-tessellated");
    assert_eq!(document.metadata.mesh_count, 1);
    assert_eq!(document.metadata.triangle_count, 12);
    assert!(document.metadata.has_brep);
    assert!(!document.metadata.brep_preserved);
    assert_eq!(document.materials.len(), 1);
    assert_eq!(document.materials[0].base_color, [0.2, 0.4, 0.6, 1.0]);
    assert_eq!(document.meshes[0].primitives.len(), 2);
    assert_eq!(document.meshes[0].bbox.min, [0.0, 0.0, 0.0]);
    assert_eq!(document.meshes[0].bbox.max, [0.002, 0.001, 0.001]);
    assert!(
        document
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("millimetre") && warning.contains("metres"))
    );

    let glb = export_glb(&document, &GlbExportOptions::default())
        .expect("planar STEP B-Rep should export to GLB");
    let summary = validate_glb_payload(&glb).expect("planar STEP GLB should validate");
    assert_eq!(summary.mesh_count, 1);
    assert_eq!(summary.triangle_count, 12);
}

#[test]
fn imports_ap214_brep_assembly_hierarchy_and_reuses_meshes() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_brep_assembly.step");
    let input = InputFile::new(Some(std::path::Path::new("assembly.step")), step);

    let document = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect("AP214 B-Rep assembly should import");

    validate_document(&document).expect("STEP assembly document should validate");
    assert_eq!(document.metadata.mode, "step-brep-assembly-tessellated");
    assert_eq!(document.metadata.mesh_count, 2);
    assert_eq!(document.metadata.triangle_count, 3);
    assert_eq!(document.nodes.len(), 7);

    let root = document
        .nodes
        .iter()
        .find(|node| node.name == "RootAssembly")
        .expect("root assembly node should exist");
    assert_eq!(root.children.len(), 2);
    let first_subassembly = &document.nodes[root.children[0]];
    let second_subassembly = &document.nodes[root.children[1]];
    assert_eq!(first_subassembly.name, "SubOne");
    assert_eq!(second_subassembly.name, "SubTwo");
    assert_close(first_subassembly.transform[3][0], 0.01);
    assert_close(second_subassembly.transform[3][0], 0.02);
    assert_eq!(first_subassembly.children.len(), 2);
    assert_eq!(second_subassembly.children.len(), 2);
    assert_eq!(
        document.nodes[first_subassembly.children[0]].mesh,
        document.nodes[second_subassembly.children[0]].mesh
    );
    assert_eq!(
        document.nodes[first_subassembly.children[1]].mesh,
        document.nodes[second_subassembly.children[1]].mesh
    );

    let metadata = export_metadata_json(&document);
    let parsed: serde_json::Value =
        serde_json::from_str(&metadata).expect("assembly metadata should be valid JSON");
    let bbox_min = parsed["bbox"]["min"]
        .as_array()
        .expect("assembly bbox min should be an array");
    let bbox_max = parsed["bbox"]["max"]
        .as_array()
        .expect("assembly bbox max should be an array");
    assert_close(bbox_min[0].as_f64().unwrap() as f32, 0.01);
    assert_close(bbox_min[1].as_f64().unwrap() as f32, 0.002);
    assert_close(bbox_min[2].as_f64().unwrap() as f32, 0.0);
    assert_close(bbox_max[0].as_f64().unwrap() as f32, 0.021);
    assert_close(bbox_max[1].as_f64().unwrap() as f32, 0.005);
    assert_close(bbox_max[2].as_f64().unwrap() as f32, 0.0);
}

#[test]
fn rejects_invalid_or_oversized_step_assembly_graphs() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_brep_assembly.step").to_vec(),
    )
    .expect("fixture should be UTF-8");
    let registry = ImporterRegistry::default();

    let cyclic = fixture.replace(
        "REPRESENTATION_RELATIONSHIP('PartBOccurrence','',#122,#121)",
        "REPRESENTATION_RELATIONSHIP('PartBOccurrence','',#122,#123)",
    );
    let error = registry
        .import(
            &InputFile::new(
                Some(std::path::Path::new("cyclic-assembly.step")),
                cyclic.as_bytes(),
            ),
            &ImportOptions::default(),
        )
        .expect_err("cyclic STEP assembly must be rejected");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason) if reason.contains("assembly relationship cycle")
    ));

    let missing_transform = fixture.replace(
        "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#110)",
        "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#999)",
    );
    let error = registry
        .import(
            &InputFile::new(
                Some(std::path::Path::new("missing-transform.step")),
                missing_transform.as_bytes(),
            ),
            &ImportOptions::default(),
        )
        .expect_err("missing STEP assembly transform must be rejected");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason) if reason.contains("missing entity #999")
    ));

    let error = registry
        .import(
            &InputFile::new(
                Some(std::path::Path::new("assembly.step")),
                fixture.as_bytes(),
            ),
            &ImportOptions {
                limits: ImportLimits {
                    max_step_assembly_nodes: 6,
                    ..ImportLimits::default()
                },
                ..ImportOptions::default()
            },
        )
        .expect_err("STEP assembly node limit must be enforced");
    assert!(matches!(
        error,
        ImportError::ResourceLimitExceeded {
            resource: "STEP assembly nodes",
            limit: 6,
            actual: 7
        }
    ));
}

#[test]
fn applies_rotated_step_assembly_placement_from_local_source_frame() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_brep_assembly.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#100=AXIS2_PLACEMENT_3D('',#10,#40,#41);",
        "#43=DIRECTION('',(0.,1.,0.));\n#94=CARTESIAN_POINT('',(1.,0.,0.));\n#95=CARTESIAN_POINT('',(1.,2.,0.));\n#100=AXIS2_PLACEMENT_3D('',#10,#40,#41);\n#105=AXIS2_PLACEMENT_3D('',#94,#40,#41);\n#106=AXIS2_PLACEMENT_3D('',#95,#40,#43);",
    )
    .replace(
        "#112=ITEM_DEFINED_TRANSFORMATION('PartAOccurrence','',#103,#100);",
        "#112=ITEM_DEFINED_TRANSFORMATION('PartAOccurrence','',#106,#105);",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("rotated-assembly.step")),
        fixture.as_bytes(),
    );

    let document = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect("rotated STEP assembly should import");
    let occurrence = document
        .nodes
        .iter()
        .find(|node| node.name == "PartAOccurrence")
        .expect("rotated part occurrence should exist");
    assert_close(occurrence.transform[0][0], 0.0);
    assert_close(occurrence.transform[0][1], 1.0);
    assert_close(occurrence.transform[1][0], -1.0);
    assert_close(occurrence.transform[1][1], 0.0);
    assert_close(occurrence.transform[3][0], 0.001);
    assert_close(occurrence.transform[3][1], 0.001);
}

#[test]
fn resolves_step_product_and_occurrence_names() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_brep_assembly.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#120=ADVANCED_BREP_SHAPE_REPRESENTATION('PartA',(#80),#200);",
        "#300=PRODUCT('A-ID','Product Named A','',());\n#301=PRODUCT_DEFINITION_FORMATION('','',#300);\n#302=PRODUCT_DEFINITION('','',#301,#200);\n#303=PRODUCT_DEFINITION_SHAPE('','',#302);\n#304=SHAPE_DEFINITION_REPRESENTATION(#303,#120);\n#305=PRODUCT('B-ID','Product Named B','',());\n#306=PRODUCT_DEFINITION_FORMATION('','',#305);\n#307=PRODUCT_DEFINITION('','',#306,#200);\n#308=PRODUCT_DEFINITION_SHAPE('','',#307);\n#309=SHAPE_DEFINITION_REPRESENTATION(#308,#121);\n#310=NEXT_ASSEMBLY_USAGE_OCCURRENCE('A-OCC','Named Part A Occurrence','',#302,#302,'A-01');\n#311=PRODUCT_DEFINITION_SHAPE('','',#310);\n#312=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#132,#311);\n#120=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#80),#200);",
    )
    .replace(
        "#121=ADVANCED_BREP_SHAPE_REPRESENTATION('PartB',(#81),#200);",
        "#121=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#81),#200);",
    )
    .replace(
        "REPRESENTATION_RELATIONSHIP('PartAOccurrence','',#122,#120)",
        "REPRESENTATION_RELATIONSHIP('','',#122,#120)",
    )
    .replace(
        "REPRESENTATION_RELATIONSHIP('PartBOccurrence','',#122,#121)",
        "REPRESENTATION_RELATIONSHIP('','',#122,#121)",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("named-assembly.step")),
        fixture.as_bytes(),
    );

    let document = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect("named STEP assembly should import");

    assert!(
        document
            .nodes
            .iter()
            .any(|node| node.name == "Named Part A Occurrence")
    );
    assert!(
        document
            .nodes
            .iter()
            .any(|node| node.name == "Product Named B")
    );
}

#[test]
fn tessellates_planar_step_face_with_inner_bound() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_planar_hole_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("planar-hole.step")), step);

    let document = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect("planar ADVANCED_FACE with a hole should tessellate");

    validate_document(&document).expect("planar STEP face with a hole should validate");
    assert_eq!(document.metadata.triangle_count, 8);
    assert_eq!(document.meshes[0].bbox.min, [0.0, 0.0, 0.0]);
    assert_eq!(document.meshes[0].bbox.max, [0.004, 0.004, 0.0]);
}

#[test]
fn tessellates_planar_step_face_with_multiple_inner_bounds() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_planar_hole_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace("(1.,1.,0.)", "(0.5,0.5,0.)")
    .replace("(1.,3.,0.)", "(0.5,1.5,0.)")
    .replace("(3.,3.,0.)", "(1.5,1.5,0.)")
    .replace("(3.,1.,0.)", "(1.5,0.5,0.)")
    .replace(
        "#20=POLY_LOOP('',(#10,#11,#12,#13));",
        "#80=CARTESIAN_POINT('',(2.5,0.5,0.));\n#81=CARTESIAN_POINT('',(2.5,1.5,0.));\n#82=CARTESIAN_POINT('',(3.5,1.5,0.));\n#83=CARTESIAN_POINT('',(3.5,0.5,0.));\n#20=POLY_LOOP('',(#10,#11,#12,#13));",
    )
    .replace(
        "#31=FACE_BOUND('',#21,.T.);",
        "#31=FACE_BOUND('',#21,.T.);\n#84=POLY_LOOP('',(#80,#81,#82,#83));\n#85=FACE_BOUND('',#84,.T.);",
    )
    .replace(
        "#60=ADVANCED_FACE('',(#30,#31),#50,.T.);",
        "#60=ADVANCED_FACE('',(#30,#31,#85),#50,.T.);",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("planar-multiple-holes.step")),
        fixture.as_bytes(),
    );

    let document = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect("planar ADVANCED_FACE with multiple holes should tessellate");

    validate_document(&document).expect("planar STEP face with multiple holes should validate");
    assert_eq!(document.metadata.triangle_count, 12);
}

#[test]
fn enforces_step_face_loop_and_vertex_limits() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_planar_hole_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("planar-hole.step")), step);
    let registry = ImporterRegistry::default();

    let loop_error = registry
        .import(
            &input,
            &ImportOptions {
                limits: ImportLimits {
                    max_step_face_loops: 1,
                    ..ImportLimits::default()
                },
                ..ImportOptions::default()
            },
        )
        .expect_err("STEP face loop limit must be enforced");
    assert!(matches!(
        loop_error,
        ImportError::ResourceLimitExceeded {
            resource: "STEP face loops",
            limit: 1,
            actual: 2
        }
    ));

    let vertex_error = registry
        .import(
            &input,
            &ImportOptions {
                limits: ImportLimits {
                    max_step_face_vertices: 7,
                    ..ImportLimits::default()
                },
                ..ImportOptions::default()
            },
        )
        .expect_err("STEP face vertex limit must be enforced");
    assert!(matches!(
        vertex_error,
        ImportError::ResourceLimitExceeded {
            resource: "STEP face vertices",
            limit: 7,
            actual: 8
        }
    ));
}

#[test]
fn rejects_invalid_step_inner_bound_topology() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_planar_hole_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8");
    let registry = ImporterRegistry::default();

    let self_intersecting = fixture.replace(
        "#21=POLY_LOOP('',(#14,#15,#16,#17));",
        "#21=POLY_LOOP('',(#14,#16,#15,#17));",
    );
    let error = registry
        .import(
            &InputFile::new(
                Some(std::path::Path::new("self-intersecting-hole.step")),
                self_intersecting.as_bytes(),
            ),
            &ImportOptions::default(),
        )
        .expect_err("self-intersecting STEP hole must be rejected");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason) if reason.contains("self-intersecting")
    ));

    let touching = fixture.replace(
        "#14=CARTESIAN_POINT('',(1.,1.,0.));",
        "#14=CARTESIAN_POINT('',(0.,1.,0.));",
    );
    let error = registry
        .import(
            &InputFile::new(
                Some(std::path::Path::new("touching-hole.step")),
                touching.as_bytes(),
            ),
            &ImportOptions::default(),
        )
        .expect_err("STEP hole touching its outer loop must be rejected");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason) if reason.contains("intersect or touch")
    ));

    let nested = fixture
        .replace(
            "#20=POLY_LOOP('',(#10,#11,#12,#13));",
            "#80=CARTESIAN_POINT('',(1.5,1.5,0.));\n#81=CARTESIAN_POINT('',(1.5,2.5,0.));\n#82=CARTESIAN_POINT('',(2.5,2.5,0.));\n#83=CARTESIAN_POINT('',(2.5,1.5,0.));\n#20=POLY_LOOP('',(#10,#11,#12,#13));",
        )
        .replace(
            "#31=FACE_BOUND('',#21,.T.);",
            "#31=FACE_BOUND('',#21,.T.);\n#84=POLY_LOOP('',(#80,#81,#82,#83));\n#85=FACE_BOUND('',#84,.T.);",
        )
        .replace(
            "#60=ADVANCED_FACE('',(#30,#31),#50,.T.);",
            "#60=ADVANCED_FACE('',(#30,#31,#85),#50,.T.);",
        );
    let error = registry
        .import(
            &InputFile::new(
                Some(std::path::Path::new("nested-holes.step")),
                nested.as_bytes(),
            ),
            &ImportOptions::default(),
        )
        .expect_err("nested STEP holes must be rejected");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason) if reason.contains("overlap or are nested")
    ));
}

#[test]
fn tessellates_cylindrical_step_face_with_configurable_chord_error() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_cylindrical_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("cylinder.step")), step);
    let registry = ImporterRegistry::default();
    let coarse_options = ImportOptions {
        max_lod_error: 0.1,
        ..ImportOptions::default()
    };

    let coarse = registry
        .import(&input, &coarse_options)
        .expect("cylindrical ADVANCED_FACE should tessellate");

    validate_document(&coarse).expect("cylindrical STEP document should validate");
    assert_eq!(coarse.metadata.mode, "step-brep-tessellated");
    assert_eq!(coarse.metadata.triangle_count, 8);
    assert_eq!(coarse.meshes[0].bbox.min, [-0.001, 0.0, 0.0]);
    assert_eq!(coarse.meshes[0].bbox.max, [0.001, 0.001, 0.002]);

    let fine = registry
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.01,
                ..ImportOptions::default()
            },
        )
        .expect("smaller chord error should remain importable");
    assert!(fine.metadata.triangle_count > coarse.metadata.triangle_count);
}

#[test]
fn tessellates_cylindrical_step_face_with_inner_bound() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_cylindrical_hole_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("cylinder-hole.step")), step);

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.1,
                ..ImportOptions::default()
            },
        )
        .expect("cylindrical ADVANCED_FACE with a hole should tessellate");

    validate_document(&document).expect("cylindrical STEP face with a hole should validate");
    assert_eq!(document.metadata.triangle_count, 16);
    assert_eq!(document.meshes[0].bbox.min, [-0.001, 0.0, 0.0]);
    assert_eq!(document.meshes[0].bbox.max, [0.001, 0.001, 0.002]);
}

#[test]
fn aligns_cylindrical_inner_bound_across_parameter_seam() {
    let step =
        include_bytes!("../../../tests/fixtures/sample_ap214_cylindrical_seam_hole_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("cylinder-seam-hole.step")), step);

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.1,
                ..ImportOptions::default()
            },
        )
        .expect("cylindrical hole should align across the angular parameter seam");

    validate_document(&document).expect("seam-crossing cylindrical hole should validate");
    assert_eq!(document.metadata.triangle_count, 10);
    assert_eq!(document.meshes[0].bbox.min[0], -0.001);
    assert!(document.meshes[0].bbox.min[1] < -0.0007);
    assert!(document.meshes[0].bbox.max[1] > 0.0007);
}

#[test]
fn tessellates_conical_step_face_with_degree_angle_unit() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_conical_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("cone.step")), step);
    let registry = ImporterRegistry::default();
    let coarse = registry
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.1,
                ..ImportOptions::default()
            },
        )
        .expect("conical ADVANCED_FACE should tessellate");

    validate_document(&coarse).expect("conical STEP document should validate");
    assert_eq!(coarse.metadata.mode, "step-brep-tessellated");
    assert_eq!(coarse.metadata.triangle_count, 9);
    assert_eq!(coarse.meshes[0].bbox.min[0], -0.002);
    assert_eq!(coarse.meshes[0].bbox.max[0], 0.002);
    assert_eq!(coarse.meshes[0].bbox.max[2], 0.002);
    assert!(coarse.meshes[0].bbox.max[1] > 0.0019);
    assert!(
        coarse
            .metadata
            .warnings
            .iter()
            .any(|warning| warning.contains("degree") && warning.contains("radians"))
    );

    let fine = registry
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.01,
                ..ImportOptions::default()
            },
        )
        .expect("smaller cone chord error should remain importable");
    assert!(fine.metadata.triangle_count > coarse.metadata.triangle_count);
}

#[test]
fn tessellates_conical_step_face_with_inner_bound() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_conical_hole_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("cone-hole.step")), step);

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.1,
                ..ImportOptions::default()
            },
        )
        .expect("conical ADVANCED_FACE with a hole should tessellate");

    validate_document(&document).expect("conical STEP face with a hole should validate");
    assert_eq!(document.metadata.triangle_count, 18);
    assert_eq!(document.meshes[0].bbox.min, [-0.002, 0.0, 0.0]);
    assert_eq!(document.meshes[0].bbox.max[0], 0.002);
    assert!(document.meshes[0].bbox.max[1] > 0.0019);
    assert_eq!(document.meshes[0].bbox.max[2], 0.002);
}

#[test]
fn tessellates_single_edge_closed_circle_step_face() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_closed_circle_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("disk.step")), step);
    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.1,
                ..ImportOptions::default()
            },
        )
        .expect("single-edge closed STEP circle should tessellate");

    validate_document(&document).expect("closed-circle STEP document should validate");
    assert_eq!(document.metadata.triangle_count, 5);
    assert_eq!(document.meshes[0].bbox.max[0], 0.001);
    assert!(document.meshes[0].bbox.min[0] < -0.0009);
    assert!(document.meshes[0].bbox.max[1] > 0.0009);
}

#[test]
fn tessellates_single_edge_closed_ellipse_step_face() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_closed_ellipse_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("ellipse.step")), step);
    let registry = ImporterRegistry::default();
    let coarse = registry
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.1,
                ..ImportOptions::default()
            },
        )
        .expect("single-edge closed STEP ellipse should tessellate");

    validate_document(&coarse).expect("closed-ellipse STEP document should validate");
    assert!(coarse.metadata.triangle_count >= 6);
    assert_eq!(coarse.meshes[0].bbox.min, [-0.002, -0.001, 0.0]);
    assert_eq!(coarse.meshes[0].bbox.max, [0.002, 0.001, 0.0]);

    let fine = registry
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.01,
                ..ImportOptions::default()
            },
        )
        .expect("smaller ellipse chord error should remain importable");
    assert!(fine.metadata.triangle_count > coarse.metadata.triangle_count);
}

#[test]
fn tessellates_planar_step_face_with_bspline_edge() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_planar_bspline_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("planar-bspline.step")), step);

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.05,
                ..ImportOptions::default()
            },
        )
        .expect("planar ADVANCED_FACE with a B-Spline boundary should tessellate");

    validate_document(&document).expect("planar B-Spline STEP document should validate");
    assert_eq!(document.metadata.mode, "step-brep-tessellated");
    assert!(document.metadata.triangle_count > 2);
    assert_eq!(document.meshes[0].bbox.min, [0.0, 0.0, 0.0]);
    assert_eq!(document.meshes[0].bbox.max[0], 0.001);
    assert!(document.meshes[0].bbox.max[1] > 0.0011);
}

#[test]
fn tessellates_complex_rational_step_bspline_edge() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_planar_bspline_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#52=B_SPLINE_CURVE_WITH_KNOTS('',2,(#12,#14,#13),.UNSPECIFIED.,.F.,.F.,(3,3),(0.,1.),.UNSPECIFIED.);",
        "#52=(BOUNDED_CURVE() B_SPLINE_CURVE(2,(#12,#14,#13),.UNSPECIFIED.,.F.,.F.) B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.) CURVE() GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_CURVE((1.,0.6,1.)) REPRESENTATION_ITEM(''));",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("planar-rational-bspline.step")),
        fixture.as_bytes(),
    );

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.02,
                ..ImportOptions::default()
            },
        )
        .expect("complex rational B-Spline boundary should tessellate");

    validate_document(&document).expect("rational B-Spline STEP document should validate");
    assert!(document.metadata.triangle_count > 2);
    assert_eq!(document.meshes[0].bbox.max[0], 0.001);
    assert!(document.meshes[0].bbox.max[1] > 0.00105);
}

#[test]
fn tessellates_planar_step_face_with_trimmed_bspline_edge() {
    let step =
        include_bytes!("../../../tests/fixtures/sample_ap214_planar_trimmed_bspline_brep.step");
    let input = InputFile::new(
        Some(std::path::Path::new("planar-trimmed-bspline.step")),
        step,
    );

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.02,
                ..ImportOptions::default()
            },
        )
        .expect("planar ADVANCED_FACE with a trimmed B-Spline boundary should tessellate");

    validate_document(&document).expect("trimmed B-Spline STEP document should validate");
    assert_eq!(document.metadata.mode, "step-brep-tessellated");
    assert!(document.metadata.triangle_count > 1);
    assert_eq!(document.meshes[0].bbox.min, [0.0, 0.0, 0.0]);
    assert_eq!(document.meshes[0].bbox.max[0], 0.001);
    assert_eq!(document.meshes[0].bbox.max[1], 0.001175);
}

#[test]
fn tessellates_reversed_trimmed_bspline_edge_sense() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_planar_trimmed_bspline_brep.step")
            .to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#52=TRIMMED_CURVE('',#54,(PARAMETER_VALUE(0.25)),(PARAMETER_VALUE(0.75)),.T.,.PARAMETER.);",
        "#52=TRIMMED_CURVE('',#54,(PARAMETER_VALUE(0.25)),(PARAMETER_VALUE(0.75)),.F.,.PARAMETER.);",
    )
    .replace("#62=EDGE_CURVE('',#22,#23,#52,.T.);", "#62=EDGE_CURVE('',#22,#23,#52,.F.);");
    let input = InputFile::new(
        Some(std::path::Path::new("planar-reversed-trimmed-bspline.step")),
        fixture.as_bytes(),
    );

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.02,
                ..ImportOptions::default()
            },
        )
        .expect("opposite trimmed curve and edge senses should still tessellate");

    validate_document(&document).expect("reversed trimmed B-Spline document should validate");
    assert_eq!(document.meshes[0].bbox.max[1], 0.001175);
}

#[test]
fn tessellates_planar_step_face_with_trimmed_line_edge() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_planar_trimmed_line_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("trimmed-line.step")), step);

    let document = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect("planar ADVANCED_FACE with a trimmed LINE boundary should tessellate");

    validate_document(&document).expect("trimmed LINE STEP document should validate");
    assert_eq!(document.metadata.triangle_count, 2);
    assert_eq!(document.meshes[0].bbox.min, [0.0, 0.0, 0.0]);
    assert_eq!(document.meshes[0].bbox.max, [0.001, 0.001, 0.0]);
}

#[test]
fn tessellates_planar_step_face_with_trimmed_circle_edge() {
    let step =
        include_bytes!("../../../tests/fixtures/sample_ap214_planar_trimmed_circle_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("trimmed-circle.step")), step);

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.05,
                ..ImportOptions::default()
            },
        )
        .expect("planar ADVANCED_FACE with a trimmed CIRCLE boundary should tessellate");

    validate_document(&document).expect("trimmed CIRCLE STEP document should validate");
    assert!(document.metadata.triangle_count > 1);
    assert_eq!(document.meshes[0].bbox.min, [0.0, 0.0, 0.0]);
    assert_eq!(document.meshes[0].bbox.max, [0.001, 0.001, 0.0]);
}

#[test]
fn tessellates_planar_step_face_with_trimmed_ellipse_edge() {
    let step =
        include_bytes!("../../../tests/fixtures/sample_ap214_planar_trimmed_ellipse_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("trimmed-ellipse.step")), step);

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.05,
                ..ImportOptions::default()
            },
        )
        .expect("planar ADVANCED_FACE with a trimmed ELLIPSE boundary should tessellate");

    validate_document(&document).expect("trimmed ELLIPSE STEP document should validate");
    assert!(document.metadata.triangle_count > 1);
    assert_eq!(document.meshes[0].bbox.min, [0.0, 0.0, 0.0]);
    assert_eq!(document.meshes[0].bbox.max, [0.002, 0.001, 0.0]);
}

#[test]
fn tessellates_cylindrical_step_face_with_trimmed_circle_edge() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_cylindrical_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#61=CIRCLE('',#51,1.);",
        "#61=CIRCLE('',#51,1.);\n#64=TRIMMED_CURVE('',#60,(PARAMETER_VALUE(0.)),(PARAMETER_VALUE(3.141592653589793)),.T.,.PARAMETER.);",
    )
    .replace("#70=EDGE_CURVE('',#20,#21,#60,.T.);", "#70=EDGE_CURVE('',#20,#21,#64,.T.);");
    let input = InputFile::new(
        Some(std::path::Path::new("cylindrical-trimmed-circle.step")),
        fixture.as_bytes(),
    );

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.1,
                ..ImportOptions::default()
            },
        )
        .expect("cylindrical ADVANCED_FACE with trimmed CIRCLE boundary should tessellate");

    validate_document(&document).expect("trimmed CIRCLE cylinder STEP document should validate");
    assert_eq!(document.metadata.triangle_count, 8);
    assert_eq!(document.meshes[0].bbox.min[0], -0.001);
    assert_eq!(document.meshes[0].bbox.max[0], 0.001);
}

#[test]
fn tessellates_cylindrical_step_face_with_bspline_edge() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_cylindrical_bspline_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("cylindrical-bspline.step")), step);

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.02,
                ..ImportOptions::default()
            },
        )
        .expect("cylindrical ADVANCED_FACE with a rational B-Spline boundary should tessellate");

    validate_document(&document).expect("cylindrical B-Spline STEP document should validate");
    assert_eq!(document.metadata.mode, "step-brep-tessellated");
    assert!(document.metadata.triangle_count > 2);
    assert_eq!(document.meshes[0].bbox.min, [0.0, 0.0, 0.0]);
    assert_eq!(document.meshes[0].bbox.max, [0.001, 0.001, 0.002]);
}

#[test]
fn tessellates_conical_step_face_with_bspline_edge() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_conical_bspline_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("conical-bspline.step")), step);

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.02,
                ..ImportOptions::default()
            },
        )
        .expect("conical ADVANCED_FACE with a rational B-Spline boundary should tessellate");

    validate_document(&document).expect("conical B-Spline STEP document should validate");
    assert_eq!(document.metadata.mode, "step-brep-tessellated");
    assert!(document.metadata.triangle_count > 2);
    assert_eq!(document.meshes[0].bbox.min, [0.0, 0.0, 0.0]);
    assert_eq!(document.meshes[0].bbox.max, [0.002, 0.002, 0.002]);
}

#[test]
fn tessellates_cylindrical_step_face_with_trimmed_bspline_edge() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_cylindrical_bspline_brep.step")
            .to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#62=(BOUNDED_CURVE() B_SPLINE_CURVE(2,(#12,#15,#13),.UNSPECIFIED.,.F.,.F.) B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.) CURVE() GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_CURVE((1.,0.7071067811865476,1.)) REPRESENTATION_ITEM(''));",
        "#62=(BOUNDED_CURVE() B_SPLINE_CURVE(2,(#12,#15,#13),.UNSPECIFIED.,.F.,.F.) B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.) CURVE() GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_CURVE((1.,0.7071067811865476,1.)) REPRESENTATION_ITEM(''));\n#64=TRIMMED_CURVE('',#62,(PARAMETER_VALUE(0.)),(PARAMETER_VALUE(1.)),.T.,.PARAMETER.);",
    )
    .replace("#72=EDGE_CURVE('',#22,#23,#62,.T.);", "#72=EDGE_CURVE('',#22,#23,#64,.T.);");
    let input = InputFile::new(
        Some(std::path::Path::new("cylindrical-trimmed-bspline.step")),
        fixture.as_bytes(),
    );

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.02,
                ..ImportOptions::default()
            },
        )
        .expect("cylindrical ADVANCED_FACE with a trimmed B-Spline boundary should tessellate");

    validate_document(&document).expect("trimmed B-Spline cylinder STEP document should validate");
    assert!(document.metadata.triangle_count > 2);
    assert_eq!(document.meshes[0].bbox.max, [0.001, 0.001, 0.002]);
}

#[test]
fn rejects_trimmed_analytic_curve_when_vertices_do_not_match_trim_parameters() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_planar_trimmed_circle_brep.step")
            .to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#51=TRIMMED_CURVE('',#54,(PARAMETER_VALUE(0.)),(PARAMETER_VALUE(1.5707963267948966)),.T.,.PARAMETER.);",
        "#51=TRIMMED_CURVE('',#54,(PARAMETER_VALUE(0.2)),(PARAMETER_VALUE(1.5707963267948966)),.T.,.PARAMETER.);",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("trimmed-circle-bad-parameter.step")),
        fixture.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("trimmed analytic edge parameters must match topological vertices");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason)
            if reason.contains("TRIMMED_CURVE start vertex does not match")
    ));
}

#[test]
fn rejects_cartesian_only_trimmed_bspline_edge() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_planar_trimmed_bspline_brep.step")
            .to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#52=TRIMMED_CURVE('',#54,(PARAMETER_VALUE(0.25)),(PARAMETER_VALUE(0.75)),.T.,.PARAMETER.);",
        "#52=TRIMMED_CURVE('',#54,(#15),(#16),.T.,.CARTESIAN.);",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("cartesian-trimmed-bspline.step")),
        fixture.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("Cartesian-only trimmed B-Spline parameters must fail explicitly");
    assert!(matches!(
        error,
        ImportError::TessellationUnsupported { ref reason, .. }
            if reason.contains("TRIMMED_CURVE trim_1 requires a PARAMETER_VALUE trim")
    ));
}

#[test]
fn enforces_step_bspline_limits() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_planar_bspline_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("planar-bspline.step")), step);

    let degree_error = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                limits: ImportLimits {
                    max_step_spline_degree: 1,
                    ..ImportLimits::default()
                },
                ..ImportOptions::default()
            },
        )
        .expect_err("B-Spline degree limit must be enforced");
    assert!(matches!(
        degree_error,
        ImportError::ResourceLimitExceeded {
            resource: "STEP spline degree",
            limit: 1,
            actual: 2,
        }
    ));

    let control_point_error = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                limits: ImportLimits {
                    max_step_spline_control_points: 2,
                    ..ImportLimits::default()
                },
                ..ImportOptions::default()
            },
        )
        .expect_err("B-Spline control point limit must be enforced");
    assert!(matches!(
        control_point_error,
        ImportError::ResourceLimitExceeded {
            resource: "STEP spline control points",
            limit: 2,
            actual: 3,
        }
    ));

    let segment_error = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.01,
                limits: ImportLimits {
                    max_step_curve_segments: 1,
                    ..ImportLimits::default()
                },
                ..ImportOptions::default()
            },
        )
        .expect_err("B-Spline adaptive sampling must respect the segment limit");
    assert!(matches!(
        segment_error,
        ImportError::ResourceLimitExceeded {
            resource: "STEP curve segments",
            limit: 1,
            ..
        }
    ));
}

#[test]
fn enforces_step_ellipse_segment_limit() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_closed_ellipse_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("ellipse.step")), step);
    let options = ImportOptions {
        max_lod_error: 0.1,
        limits: ImportLimits {
            max_step_curve_segments: 4,
            ..ImportLimits::default()
        },
        ..ImportOptions::default()
    };

    let error = ImporterRegistry::default()
        .import(&input, &options)
        .expect_err("ellipse tessellation must respect its segment limit");
    assert!(matches!(
        error,
        ImportError::ResourceLimitExceeded {
            resource: "STEP curve segments",
            limit: 4,
            ..
        }
    ));
}

#[test]
fn rejects_step_ellipse_vertex_outside_curve() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_closed_ellipse_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#10=CARTESIAN_POINT('',(2.,0.,0.));",
        "#10=CARTESIAN_POINT('',(3.,0.,0.));",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("invalid-ellipse.step")),
        fixture.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("ellipse vertex outside the analytic curve must fail");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason) if reason.contains("does not lie on ELLIPSE")
    ));
}

#[test]
fn tessellates_linear_extrusion_plane_step_face() {
    let step =
        include_bytes!("../../../tests/fixtures/sample_ap214_linear_extrusion_plane_brep.step");
    let input = InputFile::new(
        Some(std::path::Path::new("linear-extrusion-plane.step")),
        step,
    );

    let document = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect("LINE SURFACE_OF_LINEAR_EXTRUSION should tessellate as a plane");

    validate_document(&document).expect("linear-extrusion plane STEP document should validate");
    assert_eq!(document.metadata.mode, "step-brep-tessellated");
    assert_eq!(document.metadata.triangle_count, 2);
    assert_eq!(document.meshes[0].bbox.min, [0.0, 0.0, 0.0]);
    assert_eq!(document.meshes[0].bbox.max, [0.002, 0.001, 0.0]);
}

#[test]
fn tessellates_linear_extrusion_cylinder_step_face() {
    let step =
        include_bytes!("../../../tests/fixtures/sample_ap214_linear_extrusion_cylinder_brep.step");
    let input = InputFile::new(
        Some(std::path::Path::new("linear-extrusion-cylinder.step")),
        step,
    );

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.1,
                ..ImportOptions::default()
            },
        )
        .expect("CIRCLE SURFACE_OF_LINEAR_EXTRUSION should tessellate as a cylinder");

    validate_document(&document).expect("linear-extrusion cylinder STEP document should validate");
    assert_eq!(document.metadata.mode, "step-brep-tessellated");
    assert_eq!(document.metadata.triangle_count, 8);
    assert_eq!(document.meshes[0].bbox.min[0], -0.001);
    assert_eq!(document.meshes[0].bbox.max[0], 0.001);
    assert_eq!(document.meshes[0].bbox.max[2], 0.002);
}

#[test]
fn rejects_skew_circle_linear_extrusion_surface() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_linear_extrusion_cylinder_brep.step")
            .to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#32=DIRECTION('',(0.,0.,-1.));",
        "#32=DIRECTION('',(0.,0.,-1.));\n#33=DIRECTION('',(1.,0.,1.));\n#42=VECTOR('',#33,1.);",
    )
    .replace(
        "#100=SURFACE_OF_LINEAR_EXTRUSION('',#60,#40);",
        "#100=SURFACE_OF_LINEAR_EXTRUSION('',#60,#42);",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("skew-circle-extrusion.step")),
        fixture.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("skew circle linear extrusion must not be treated as a right cylinder");
    assert!(matches!(
        error,
        ImportError::TessellationUnsupported { ref reason, .. }
            if reason.contains("CIRCLE sweep axis must be parallel")
    ));
}

#[test]
fn tessellates_spherical_step_face_with_circular_boundaries() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_spherical_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("sphere.step")), step);
    let registry = ImporterRegistry::default();
    let coarse = registry
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.05,
                ..ImportOptions::default()
            },
        )
        .expect("spherical ADVANCED_FACE should tessellate");

    validate_document(&coarse).expect("spherical STEP document should validate");
    assert_eq!(coarse.metadata.triangle_count, 8);
    assert_close(coarse.meshes[0].bbox.min[0], 0.0);
    assert_close(coarse.meshes[0].bbox.min[1], 0.0);
    assert_close(coarse.meshes[0].bbox.min[2], 0.0);
    assert_eq!(coarse.meshes[0].bbox.max[0], 0.001);
    assert_eq!(coarse.meshes[0].bbox.max[1], 0.001);
    assert_close(coarse.meshes[0].bbox.max[2], 0.0007071068);

    let fine = registry
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.005,
                ..ImportOptions::default()
            },
        )
        .expect("smaller sphere chord error should remain importable");
    assert!(fine.metadata.triangle_count > coarse.metadata.triangle_count);
}

#[test]
fn tessellates_spherical_step_face_with_inner_bound() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_spherical_hole_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("sphere-hole.step")), step);

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.05,
                ..ImportOptions::default()
            },
        )
        .expect("spherical ADVANCED_FACE with a hole should tessellate");

    validate_document(&document).expect("spherical STEP face with a hole should validate");
    assert_eq!(document.metadata.triangle_count, 16);
    assert_eq!(document.meshes[0].bbox.max[0], 0.001);
    assert_eq!(document.meshes[0].bbox.max[1], 0.001);
}

#[test]
fn tessellates_spherical_step_face_with_bspline_edge() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_spherical_bspline_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("sphere-bspline.step")), step);

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.02,
                ..ImportOptions::default()
            },
        )
        .expect("spherical ADVANCED_FACE with a rational B-Spline boundary should tessellate");

    validate_document(&document).expect("spherical B-Spline STEP document should validate");
    assert_eq!(document.metadata.mode, "step-brep-tessellated");
    assert!(document.metadata.triangle_count > 8);
    assert_close(document.meshes[0].bbox.min[0], 0.0);
    assert_close(document.meshes[0].bbox.min[1], 0.0);
    assert_close(document.meshes[0].bbox.min[2], 0.0);
    assert_eq!(document.meshes[0].bbox.max[0], 0.001);
    assert_eq!(document.meshes[0].bbox.max[1], 0.001);
    assert_close(document.meshes[0].bbox.max[2], 0.0007071068);
}

#[test]
fn tessellates_spherical_step_face_with_trimmed_bspline_edge() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_spherical_bspline_brep.step")
            .to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#62=(BOUNDED_CURVE() B_SPLINE_CURVE(2,(#12,#16,#13),.UNSPECIFIED.,.F.,.F.) B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.) CURVE() GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_CURVE((1.,0.7071067811865476,1.)) REPRESENTATION_ITEM(''));",
        "#62=(BOUNDED_CURVE() B_SPLINE_CURVE(2,(#12,#16,#13),.UNSPECIFIED.,.F.,.F.) B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.) CURVE() GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_CURVE((1.,0.7071067811865476,1.)) REPRESENTATION_ITEM(''));\n#64=TRIMMED_CURVE('',#62,(PARAMETER_VALUE(0.)),(PARAMETER_VALUE(1.)),.T.,.PARAMETER.);",
    )
    .replace("#72=EDGE_CURVE('',#22,#23,#62,.T.);", "#72=EDGE_CURVE('',#22,#23,#64,.T.);");
    let input = InputFile::new(
        Some(std::path::Path::new("sphere-trimmed-bspline.step")),
        fixture.as_bytes(),
    );

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.02,
                ..ImportOptions::default()
            },
        )
        .expect("spherical ADVANCED_FACE with a trimmed B-Spline boundary should tessellate");

    validate_document(&document).expect("trimmed B-Spline sphere STEP document should validate");
    assert!(document.metadata.triangle_count > 8);
    assert_eq!(document.meshes[0].bbox.max[0], 0.001);
    assert_eq!(document.meshes[0].bbox.max[1], 0.001);
    assert_close(document.meshes[0].bbox.max[2], 0.0007071068);
}

#[test]
fn rejects_spherical_bspline_boundary_that_leaves_surface() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_spherical_bspline_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#16=CARTESIAN_POINT('',(0.707106781,0.707106781,0.707106781));",
        "#16=CARTESIAN_POINT('',(0.707106781,0.9,0.707106781));",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("sphere-off-surface-bspline.step")),
        fixture.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.002,
                ..ImportOptions::default()
            },
        )
        .expect_err("off-surface B-Spline boundary must fail surface validation");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason)
            if reason.contains("boundary does not lie on SPHERICAL_SURFACE")
    ));
}

#[test]
fn rejects_line_boundary_on_spherical_surface() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_spherical_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#60=CIRCLE('',#50,1.);",
        "#34=DIRECTION('',(-1.,1.,0.));\n#40=VECTOR('',#34,1.);\n#60=LINE('',#10,#40);",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("sphere-chord.step")),
        fixture.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("a straight chord must not be projected onto a sphere");
    assert!(matches!(
        error,
        ImportError::TessellationUnsupported { ref reason, .. }
            if reason.contains("SPHERICAL_SURFACE") && reason.contains("LINE")
    ));
}

#[test]
fn rejects_spherical_face_boundary_touching_parameterization_pole() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_spherical_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#12=CARTESIAN_POINT('',(0.,0.707106781,0.707106781));",
        "#12=CARTESIAN_POINT('',(0.,0.,1.));",
    )
    .replace(
        "#72=EDGE_CURVE('',#22,#23,#62,.F.);",
        "#72=EDGE_CURVE('',#22,#20,#63,.F.);",
    )
    .replace(
        "#90=EDGE_LOOP('',(#80,#81,#82,#83));",
        "#90=EDGE_LOOP('',(#80,#81,#82));",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("sphere-pole.step")),
        fixture.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("sphere pole singularity must fail explicitly");
    assert!(matches!(
        error,
        ImportError::TessellationUnsupported { ref reason, .. }
            if reason.contains("parameterization pole")
    ));
}

#[test]
fn tessellates_toroidal_step_face_with_meridian_and_parallel_boundaries() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_toroidal_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("torus.step")), step);
    let registry = ImporterRegistry::default();
    let coarse = registry
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.1,
                ..ImportOptions::default()
            },
        )
        .expect("ring-torus ADVANCED_FACE should tessellate");

    validate_document(&coarse).expect("toroidal STEP document should validate");
    assert_eq!(coarse.metadata.triangle_count, 9);
    assert_close(coarse.meshes[0].bbox.min[0], 0.0);
    assert_close(coarse.meshes[0].bbox.min[1], 0.0);
    assert_close(coarse.meshes[0].bbox.min[2], 0.0);
    assert_eq!(coarse.meshes[0].bbox.max, [0.003, 0.003, 0.001]);

    let fine = registry
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.01,
                ..ImportOptions::default()
            },
        )
        .expect("smaller torus chord error should remain importable");
    assert!(fine.metadata.triangle_count > coarse.metadata.triangle_count);
}

#[test]
fn tessellates_toroidal_step_face_with_inner_bound() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_toroidal_hole_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("torus-hole.step")), step);

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.1,
                ..ImportOptions::default()
            },
        )
        .expect("toroidal ADVANCED_FACE with a hole should tessellate");

    validate_document(&document).expect("toroidal STEP face with a hole should validate");
    assert_eq!(document.metadata.triangle_count, 17);
    assert_eq!(document.meshes[0].bbox.max, [0.003, 0.003, 0.001]);
}

#[test]
fn tessellates_toroidal_step_face_with_bspline_edge() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_toroidal_bspline_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("torus-bspline.step")), step);

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.02,
                ..ImportOptions::default()
            },
        )
        .expect("toroidal ADVANCED_FACE with a rational B-Spline boundary should tessellate");

    validate_document(&document).expect("toroidal B-Spline STEP document should validate");
    assert_eq!(document.metadata.mode, "step-brep-tessellated");
    assert!(document.metadata.triangle_count > 9);
    assert_close(document.meshes[0].bbox.min[0], 0.0);
    assert_close(document.meshes[0].bbox.min[1], 0.0);
    assert_close(document.meshes[0].bbox.min[2], 0.0);
    assert_eq!(document.meshes[0].bbox.max, [0.003, 0.003, 0.001]);
}

#[test]
fn tessellates_toroidal_step_face_with_trimmed_bspline_edge() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_toroidal_bspline_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#62=(BOUNDED_CURVE() B_SPLINE_CURVE(2,(#12,#18,#13),.UNSPECIFIED.,.F.,.F.) B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.) CURVE() GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_CURVE((1.,0.7071067811865476,1.)) REPRESENTATION_ITEM(''));",
        "#62=(BOUNDED_CURVE() B_SPLINE_CURVE(2,(#12,#18,#13),.UNSPECIFIED.,.F.,.F.) B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.) CURVE() GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_CURVE((1.,0.7071067811865476,1.)) REPRESENTATION_ITEM(''));\n#64=TRIMMED_CURVE('',#62,(PARAMETER_VALUE(0.)),(PARAMETER_VALUE(1.)),.T.,.PARAMETER.);",
    )
    .replace("#72=EDGE_CURVE('',#22,#23,#62,.T.);", "#72=EDGE_CURVE('',#22,#23,#64,.T.);");
    let input = InputFile::new(
        Some(std::path::Path::new("torus-trimmed-bspline.step")),
        fixture.as_bytes(),
    );

    let document = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.02,
                ..ImportOptions::default()
            },
        )
        .expect("toroidal ADVANCED_FACE with a trimmed B-Spline boundary should tessellate");

    validate_document(&document).expect("trimmed B-Spline torus STEP document should validate");
    assert!(document.metadata.triangle_count > 9);
    assert_eq!(document.meshes[0].bbox.max, [0.003, 0.003, 0.001]);
}

#[test]
fn tessellates_toroidal_face_across_both_parameter_seams() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_toroidal_seam_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("torus-seams.step")), step);
    let registry = ImporterRegistry::default();
    let coarse = registry
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.1,
                ..ImportOptions::default()
            },
        )
        .expect("toroidal face crossing both parameter seams should tessellate");

    validate_document(&coarse).expect("seam-crossing toroidal document should validate");
    assert_eq!(coarse.metadata.triangle_count, 6);

    let fine = registry
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 0.01,
                ..ImportOptions::default()
            },
        )
        .expect("seam-crossing toroidal face should support finer tessellation");
    assert!(fine.metadata.triangle_count > coarse.metadata.triangle_count);
}

#[test]
fn rejects_line_boundary_on_toroidal_surface() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_toroidal_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#60=CIRCLE('',#50,3.);",
        "#34=DIRECTION('',(-3.,3.,0.));\n#40=VECTOR('',#34,1.);\n#60=LINE('',#10,#40);",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("torus-chord.step")),
        fixture.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("a straight chord must not be projected onto a torus");
    assert!(matches!(
        error,
        ImportError::TessellationUnsupported { ref reason, .. }
            if reason.contains("TOROIDAL_SURFACE") && reason.contains("LINE")
    ));
}

#[test]
fn rejects_non_meridian_or_parallel_circle_on_toroidal_surface() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_toroidal_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#20=VERTEX_POINT('',#10);",
        "#18=CARTESIAN_POINT('',(1.5,1.5,0.));\n#20=VERTEX_POINT('',#10);",
    )
    .replace(
        "#60=CIRCLE('',#50,3.);",
        "#54=AXIS2_PLACEMENT_3D('',#18,#30,#31);\n#60=CIRCLE('',#54,2.121320344);",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("torus-arbitrary-circle.step")),
        fixture.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("an arbitrary circle must not be projected onto a torus");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason)
            if reason.contains("neither a meridian nor a parallel")
    ));
}

#[test]
fn rejects_horn_or_spindle_toroidal_surface() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_toroidal_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#100=TOROIDAL_SURFACE('',#50,2.,1.);",
        "#100=TOROIDAL_SURFACE('',#50,0.5,1.);",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("spindle-torus.step")),
        fixture.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("horn and spindle tori must fail explicitly");
    assert!(matches!(
        error,
        ImportError::TessellationUnsupported { ref reason, .. }
            if reason.contains("ring torus")
    ));
}

#[test]
fn rejects_conical_surface_without_plane_angle_unit() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_conical_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(",#124", "")
    .replace(
        "#122=(NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.));",
        "",
    )
    .replace(
        "#123=PLANE_ANGLE_MEASURE_WITH_UNIT(PLANE_ANGLE_MEASURE(0.0174532925199433),#122);",
        "",
    )
    .replace(
        "#124=(CONVERSION_BASED_UNIT('DEGREE',#123) NAMED_UNIT(*) PLANE_ANGLE_UNIT());",
        "",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("cone-without-angle-unit.step")),
        fixture.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("cone without an explicit plane-angle unit must fail");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason) if reason.contains("plane-angle unit")
    ));
}

#[test]
fn rejects_straight_boundary_that_is_not_a_cone_generator() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_conical_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#12=CARTESIAN_POINT('',(-2.,0.,2.));",
        "#12=CARTESIAN_POINT('',(0.,2.,2.));",
    )
    .replace(
        "#40=VECTOR('',#32,1.);",
        "#34=DIRECTION('',(1.,2.,2.));\n#40=VECTOR('',#34,1.);",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("cone-chord.step")),
        fixture.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("a straight cone chord must not be projected onto the surface");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason)
            if reason.contains("LINE boundary is not a generator")
    ));
}

#[test]
fn enforces_step_curve_segment_limit() {
    let step = include_bytes!("../../../tests/fixtures/sample_ap214_cylindrical_brep.step");
    let input = InputFile::new(Some(std::path::Path::new("cylinder.step")), step);
    let options = ImportOptions {
        max_lod_error: 0.1,
        limits: ImportLimits {
            max_step_curve_segments: 3,
            ..ImportLimits::default()
        },
        ..ImportOptions::default()
    };

    let error = ImporterRegistry::default()
        .import(&input, &options)
        .expect_err("curve tessellation must respect its configured segment limit");
    assert!(matches!(
        error,
        ImportError::ResourceLimitExceeded {
            resource: "STEP curve segments",
            limit: 3,
            actual: 4,
        }
    ));
}

#[test]
fn triangulates_concave_planar_step_face() {
    let step = b"ISO-10303-21;
HEADER;
FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));
ENDSEC;
DATA;
#10=CARTESIAN_POINT('',(0.,0.,0.));
#11=CARTESIAN_POINT('',(2.,0.,0.));
#12=CARTESIAN_POINT('',(1.,0.5,0.));
#13=CARTESIAN_POINT('',(2.,1.,0.));
#14=CARTESIAN_POINT('',(0.,1.,0.));
#20=DIRECTION('',(0.,0.,1.));
#30=POLY_LOOP('',(#10,#11,#12,#13,#14));
#31=FACE_OUTER_BOUND('',#30,.T.);
#40=AXIS2_PLACEMENT_3D('',#10,#20,$);
#41=PLANE('',#40);
#50=ADVANCED_FACE('',(#31),#41,.T.);
ENDSEC;
END-ISO-10303-21;";
    let input = InputFile::new(Some(std::path::Path::new("concave.step")), step);

    let document = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect("concave planar STEP face should tessellate");

    validate_document(&document).expect("concave STEP document should validate");
    assert_eq!(document.metadata.mode, "step-brep-tessellated");
    assert_eq!(document.metadata.triangle_count, 3);
}

#[test]
fn rejects_incompatible_cylindrical_face_boundaries() {
    let step = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_planar_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#160=PLANE('',#150);",
        "#160=CYLINDRICAL_SURFACE('',#150,1.);",
    );
    let input = InputFile::new(
        Some(std::path::Path::new("curved-surface.step")),
        step.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("off-surface STEP boundaries must not produce a partial mesh");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason)
            if reason.contains("LINE boundary is not parallel")
    ));
}

#[test]
fn rejects_straight_boundary_that_only_touches_cylinder_at_endpoints() {
    let step = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_cylindrical_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8")
    .replace(
        "#12=CARTESIAN_POINT('',(-1.,0.,2.));",
        "#12=CARTESIAN_POINT('',(0.,1.,2.));",
    )
    .replace(
        "#40=VECTOR('',#30,1.);",
        "#40=VECTOR('',#30,1.);\n#33=DIRECTION('',(1.,1.,2.));\n#42=VECTOR('',#33,1.);",
    )
    .replace("#62=LINE('',#11,#40);", "#62=LINE('',#11,#42);");
    let input = InputFile::new(
        Some(std::path::Path::new("cylinder-chord.step")),
        step.as_bytes(),
    );

    let error = ImporterRegistry::default()
        .import(
            &input,
            &ImportOptions {
                max_lod_error: 10.0,
                ..ImportOptions::default()
            },
        )
        .expect_err("a straight chord must not be projected onto a cylinder");
    assert!(matches!(
        error,
        ImportError::InvalidData(ref reason)
            if reason.contains("LINE boundary is not parallel")
    ));
}

#[test]
fn rejects_invalid_circle_geometry_and_out_of_bounds_hole() {
    let fixture = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_planar_brep.step").to_vec(),
    )
    .expect("fixture should be UTF-8");
    let curved_edge = fixture.replace("#50=LINE('',#10,#40);", "#50=CIRCLE('',#150,1.);");
    let curved_input = InputFile::new(
        Some(std::path::Path::new("curved-edge.step")),
        curved_edge.as_bytes(),
    );
    let curved_error = ImporterRegistry::default()
        .import(&curved_input, &ImportOptions::default())
        .expect_err("invalid STEP circle geometry must not be tessellated");
    assert!(matches!(
        curved_error,
        ImportError::InvalidData(ref reason) if reason.contains("does not lie on CIRCLE")
    ));

    let inner_bound = String::from_utf8(
        include_bytes!("../../../tests/fixtures/sample_ap214_planar_hole_brep.step").to_vec(),
    )
    .expect("hole fixture should be UTF-8")
    .replace(
        "#14=CARTESIAN_POINT('',(1.,1.,0.));",
        "#14=CARTESIAN_POINT('',(5.,5.,0.));",
    )
    .replace(
        "#15=CARTESIAN_POINT('',(1.,3.,0.));",
        "#15=CARTESIAN_POINT('',(5.,7.,0.));",
    )
    .replace(
        "#16=CARTESIAN_POINT('',(3.,3.,0.));",
        "#16=CARTESIAN_POINT('',(7.,7.,0.));",
    )
    .replace(
        "#17=CARTESIAN_POINT('',(3.,1.,0.));",
        "#17=CARTESIAN_POINT('',(7.,5.,0.));",
    );
    let inner_input = InputFile::new(
        Some(std::path::Path::new("inner-bound.step")),
        inner_bound.as_bytes(),
    );
    let inner_error = ImporterRegistry::default()
        .import(&inner_input, &ImportOptions::default())
        .expect_err("STEP hole outside its outer boundary must be rejected");
    assert!(
        matches!(
            inner_error,
            ImportError::InvalidData(ref reason) if reason.contains("outside the outer boundary")
        ),
        "unexpected error: {inner_error:?}"
    );

    let invalid_line = fixture.replace("#50=LINE('',#10,#40);", "#50=LINE('',#13,#40);");
    let invalid_line_input = InputFile::new(
        Some(std::path::Path::new("invalid-line.step")),
        invalid_line.as_bytes(),
    );
    let invalid_line_error = ImporterRegistry::default()
        .import(&invalid_line_input, &ImportOptions::default())
        .expect_err("EDGE_CURVE vertices outside its LINE should be rejected");
    assert!(matches!(
        invalid_line_error,
        ImportError::InvalidData(ref reason) if reason.contains("do not lie on LINE")
    ));
}

#[test]
fn export_glb_uses_u16_indices_for_small_meshes() {
    let document = sample_lite_document();
    let glb = export_glb(&document, &GlbExportOptions::default()).expect("GLB should export");
    let json = glb_json_chunk(&glb);

    assert!(json.contains("\"byteLength\":12,\"target\":34963"));
    assert!(json.contains("\"componentType\":5123,\"count\":6,\"type\":\"SCALAR\""));
}

#[test]
fn export_glb_keeps_u32_indices_when_required() {
    let document = large_index_lite_document();
    let glb = export_glb(&document, &GlbExportOptions::default()).expect("GLB should export");
    let json = glb_json_chunk(&glb);

    assert!(json.contains("\"byteLength\":12,\"target\":34963"));
    assert!(json.contains("\"componentType\":5125,\"count\":3,\"type\":\"SCALAR\""));
}

#[test]
fn validates_generated_glb_payload_before_accepting_output() {
    let document = sample_lite_document();
    let glb = export_glb(&document, &GlbExportOptions::default()).expect("GLB should export");
    let summary = validate_glb_payload(&glb).expect("exported GLB should validate");

    assert_eq!(summary.mesh_count, 1);
    assert_eq!(summary.triangle_count, 2);

    let mut trailing_bytes = glb;
    trailing_bytes.push(0);
    let error = validate_glb_payload(&trailing_bytes).expect_err("trailing bytes should fail");
    assert!(error.to_string().contains("invalid GLB output"));
    assert!(
        error
            .to_string()
            .contains("GLB payload length does not match its header")
    );
}

#[test]
fn imports_native_ap242_triangulated_face_colors() {
    let step = b"ISO-10303-21;
HEADER;
FILE_DESCRIPTION(('AP242 tessellated color test'),'2;1');
FILE_NAME('colored','2026-06-08T00:00:00',('feather'),('feather'),'','','');
FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF'));
ENDSEC;
DATA;
#10=COORDINATES_LIST('',4,((0.,0.,0.),(2.,0.,0.),(2.,1.,0.),(0.,1.,0.)));
#20=TRIANGULATED_FACE('',#10,4,$,$,(),((1,2,3)));
#21=TRIANGULATED_FACE('',#10,4,$,$,(),((1,3,4)));
#30=COLOUR_RGB('',0.2,0.4,0.6);
#31=COLOUR_RGB('',0.8,0.1,0.1);
#40=FILL_AREA_STYLE_COLOUR('',#30);
#41=FILL_AREA_STYLE('',(#40));
#42=SURFACE_STYLE_FILL_AREA(#41);
#43=SURFACE_SIDE_STYLE('',(#42));
#44=SURFACE_STYLE_USAGE(.BOTH.,#43);
#45=PRESENTATION_STYLE_ASSIGNMENT((#44));
#46=STYLED_ITEM('',(#45),#20);
#50=FILL_AREA_STYLE_COLOUR('',#31);
#51=FILL_AREA_STYLE('',(#50));
#52=SURFACE_STYLE_FILL_AREA(#51);
#53=SURFACE_SIDE_STYLE('',(#52));
#54=SURFACE_STYLE_USAGE(.BOTH.,#53);
#55=PRESENTATION_STYLE_ASSIGNMENT((#54));
#56=STYLED_ITEM('',(#55),#21);
ENDSEC;
END-ISO-10303-21;";
    let input = InputFile::new(Some(std::path::Path::new("colored.step")), step);
    let registry = ImporterRegistry::default();

    let document = registry
        .import(&input, &ImportOptions::default())
        .expect("colored AP242 tessellated STEP should import natively");

    validate_document(&document).expect("colored STEP document should validate");
    assert_eq!(document.metadata.source_format, "STEP");
    assert_eq!(document.metadata.mode, "step-ap242-tessellated");
    assert_eq!(document.materials.len(), 2);
    assert_eq!(document.meshes[0].primitives.len(), 2);
    assert_eq!(document.meshes[0].primitives[0].material, Some(0));
    assert_eq!(document.meshes[0].primitives[1].material, Some(1));
    assert_eq!(document.materials[0].base_color, [0.2, 0.4, 0.6, 1.0]);
    assert_eq!(document.materials[1].base_color, [0.8, 0.1, 0.1, 1.0]);

    let glb = export_glb(&document, &GlbExportOptions::default()).expect("GLB should export");
    let glb_text = String::from_utf8_lossy(&glb);
    assert!(glb_text.contains("\"name\":\"STEP_Color_1\""));
    assert!(glb_text.contains("\"baseColorFactor\":[0.2000000,0.4000000,0.6000000,1.0000000]"));
}

#[test]
fn catpart_without_cache_fails_explicitly() {
    let input = InputFile::new(
        Some(std::path::Path::new("fixture.CATPart")),
        b"CATPart-private-data-without-cache",
    );
    let registry = ImporterRegistry::default();
    let error = registry
        .import(&input, &ImportOptions::default())
        .expect_err("CATPart without visual cache should fail");

    assert!(matches!(
        error,
        ImportError::NoLightweightCache { ref format } if format == "CATIA_CATPart"
    ));
}

#[test]
fn catpart_solid_metadata_is_not_misclassified_as_embedded_binary_stl() {
    let mut bytes = b"V5_CFV2 CATIA metadata prefix".to_vec();
    let candidate_start = bytes.len();
    bytes.resize(candidate_start + 80, 0);
    let header = b"AbsoluteAxisSystem Solid metadata";
    bytes[candidate_start..candidate_start + header.len()].copy_from_slice(header);
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&f32::NAN.to_le_bytes());
    bytes.resize(bytes.len() + 46, 0);

    let input = InputFile::new(Some(std::path::Path::new("fixture.CATPart")), &bytes);
    let registry = ImporterRegistry::default();
    let error = registry
        .import(&input, &ImportOptions::default())
        .expect_err("CATIA metadata must not be treated as an embedded STL");

    assert!(
        matches!(
            error,
            ImportError::NoLightweightCache { ref format } if format == "CATIA_CATPart"
        ),
        "unexpected import error: {error}"
    );
}

#[test]
fn probes_catia_v5_cfv2_native_visualization_without_claiming_import_support() {
    let bytes = sample_catia_v5_cfv2("V5R30SP4HF0");
    let path = std::path::Path::new("fixture.CATPart");
    let probe = detect_format(Some(path), &bytes);

    assert_eq!(probe.confidence, ProbeConfidence::Certain);
    assert_eq!(probe.container_kind, Some("catia-v5-cfv2"));
    assert_eq!(probe.source_version.as_deref(), Some("V5R30SP4HF0"));
    assert_eq!(
        probe.native_visualization,
        Some("catia-native-cgr-container")
    );

    let input = InputFile::new(Some(path), &bytes);
    let error = ImporterRegistry::default()
        .import(&input, &ImportOptions::default())
        .expect_err("native CATCGRCont should not be reported as decoded");
    assert!(matches!(
        error,
        ImportError::NativeVisualizationUnsupported {
            ref format,
            representation: "CATCGRCont"
        } if format == "CATIA_CATPart"
    ));

    let report = inspect_bytes(
        Some(path),
        &bytes,
        &InspectOptions {
            check_import: true,
            ..InspectOptions::default()
        },
    )
    .expect("CFV2 inspection should succeed");
    let check = report
        .import_check
        .as_ref()
        .expect("inspection should include import validation");
    assert_eq!(
        check.failure_category,
        Some("native_visualization_not_decoded")
    );

    let json: serde_json::Value =
        serde_json::from_str(&report.to_json_string()).expect("inspect JSON should be valid");
    assert_eq!(json["container_kind"], "catia-v5-cfv2");
    assert_eq!(json["source_version"], "V5R30SP4HF0");
    assert_eq!(json["native_visualization"], "catia-native-cgr-container");
}

#[test]
fn plain_step_reports_missing_open_source_tessellation() {
    let input = InputFile::new(
        Some(std::path::Path::new("fixture.step")),
        b"ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;",
    );
    let registry = ImporterRegistry::default();
    let error = registry
        .import(&input, &ImportOptions::default())
        .expect_err("plain STEP should require native tessellation");

    assert!(matches!(
        error,
        ImportError::TessellationUnsupported { ref format, .. } if format == "STEP"
    ));
}

#[test]
fn detects_cache_marker_inside_unknown_binary() {
    let bytes = "prefix\nFEATHER_CAD_LITE_CACHE_V1\nEND_FEATHER_CAD_LITE_CACHE\nsuffix";
    let probe = detect_format(None, bytes.as_bytes());

    assert_eq!(probe.confidence, ProbeConfidence::High);
    assert!(probe.has_embedded_cache);
}

fn unique_temp_dir(label: &str) -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "feather-lite-test-{label}-{}-{stamp}",
        std::process::id()
    ))
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

fn sample_ascii_stl() -> &'static str {
    "solid visual
facet normal 0 0 1
outer loop
vertex 0 0 0
vertex 2 0 0
vertex 2 1 0
endloop
endfacet
facet normal 0 0 1
outer loop
vertex 0 0 0
vertex 2 1 0
vertex 0 1 0
endloop
endfacet
endsolid visual"
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

fn sample_obj_with_materials() -> &'static str {
    "# Wavefront OBJ visual cache
mtllib model.mtl
o PaintedPlate
v 0 0 0
v 2 0 0
v 2 1 0
v 0 1 0
vn 0 0 1
usemtl RedPaint
f 1//1 2//1 3//1
usemtl BluePaint
f 1//1 3//1 4//1"
}

fn sample_mtl() -> &'static str {
    "newmtl RedPaint
Kd 1.0 0.0 0.0
d 0.75
newmtl BluePaint
Kd 0.0 0.0 1.0
Tr 0.2"
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

fn sample_3dxml_rep_with_strips_and_fans() -> &'static str {
    r#"
<Root>
  <Rep>
    <VertexBuffer>
      <Positions>0 0 0 2 0 0 2 1 0 0 1 0 1 2 0</Positions>
      <Normals>0 0 1 0 0 1 0 0 1 0 0 1 0 0 1</Normals>
    </VertexBuffer>
    <Faces>
      <Face triangles="0 1 2"/>
      <Face strips="0 1 2 3"/>
      <Face fans="0 2 3 4"/>
    </Faces>
  </Rep>
</Root>
"#
}

fn sample_glb() -> Vec<u8> {
    export_glb(&sample_lite_document(), &GlbExportOptions::default()).expect("sample GLB exports")
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

fn sample_interleaved_gltf_with_bin() -> (String, Vec<u8>) {
    let mut bin = vec![0xAA; 8];
    for position in [
        [0.0_f32, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [2.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ] {
        for value in position {
            bin.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0.0_f32, 0.0, 1.0] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    bin.extend_from_slice(&[0xBB; 4]);
    for index in [0_u16, 1, 2, 0, 2, 3] {
        bin.extend_from_slice(&index.to_le_bytes());
    }
    let gltf = sample_interleaved_gltf_json("interleaved.bin", bin.len());
    (gltf, bin)
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
    sample_gltf_json_with_node_properties(uri, byte_length, "")
}

fn sample_interleaved_gltf_json(uri: &str, byte_length: usize) -> String {
    format!(
        "{{\"asset\":{{\"version\":\"2.0\"}},\"scene\":0,\"scenes\":[{{\"nodes\":[0]}}],\"nodes\":[{{\"name\":\"Plate\",\"mesh\":0}}],\"materials\":[{{\"name\":\"Default\",\"pbrMetallicRoughness\":{{\"baseColorFactor\":[0.8,0.8,0.82,1.0]}}}}],\"meshes\":[{{\"name\":\"Plate\",\"primitives\":[{{\"attributes\":{{\"POSITION\":0,\"NORMAL\":1}},\"indices\":2,\"mode\":4,\"material\":0}}]}}],\"buffers\":[{{\"uri\":\"{}\",\"byteLength\":{}}}],\"bufferViews\":[{{\"buffer\":0,\"byteOffset\":8,\"byteLength\":96,\"byteStride\":24,\"target\":34962}},{{\"buffer\":0,\"byteOffset\":108,\"byteLength\":12,\"target\":34963}}],\"accessors\":[{{\"bufferView\":0,\"byteOffset\":0,\"componentType\":5126,\"count\":4,\"type\":\"VEC3\"}},{{\"bufferView\":0,\"byteOffset\":12,\"componentType\":5126,\"count\":4,\"type\":\"VEC3\"}},{{\"bufferView\":1,\"byteOffset\":0,\"componentType\":5123,\"count\":6,\"type\":\"SCALAR\"}}]}}",
        escape_json_string(uri),
        byte_length
    )
}

fn sample_gltf_json_with_node_properties(
    uri: &str,
    byte_length: usize,
    node_properties: &str,
) -> String {
    format!(
        "{{\"asset\":{{\"version\":\"2.0\"}},\"scene\":0,\"scenes\":[{{\"nodes\":[0]}}],\"nodes\":[{{\"name\":\"Plate\",\"mesh\":0{}}}],\"materials\":[{{\"name\":\"Default\",\"pbrMetallicRoughness\":{{\"baseColorFactor\":[0.8,0.8,0.82,1.0]}}}}],\"meshes\":[{{\"name\":\"Plate\",\"primitives\":[{{\"attributes\":{{\"POSITION\":0}},\"indices\":1,\"mode\":4,\"material\":0}}]}}],\"buffers\":[{{\"uri\":\"{}\",\"byteLength\":{}}}],\"bufferViews\":[{{\"buffer\":0,\"byteOffset\":0,\"byteLength\":48,\"target\":34962}},{{\"buffer\":0,\"byteOffset\":48,\"byteLength\":24,\"target\":34963}}],\"accessors\":[{{\"bufferView\":0,\"componentType\":5126,\"count\":4,\"type\":\"VEC3\"}},{{\"bufferView\":1,\"componentType\":5125,\"count\":6,\"type\":\"SCALAR\"}}]}}",
        node_properties,
        escape_json_string(uri),
        byte_length
    )
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.0001,
        "expected {actual} to be close to {expected}"
    );
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

fn glb_json_chunk(glb: &[u8]) -> String {
    assert_eq!(&glb[0..4], &0x4654_6C67_u32.to_le_bytes());
    let json_len = u32::from_le_bytes(glb[12..16].try_into().expect("JSON length header")) as usize;
    assert_eq!(&glb[16..20], &0x4E4F_534A_u32.to_le_bytes());
    String::from_utf8(glb[20..20 + json_len].to_vec()).expect("GLB JSON should be UTF-8")
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

fn large_index_lite_document() -> LiteDocument {
    let mut document = LiteDocument::new("Fixture", "fixture");
    let overflow_index = u32::from(u16::MAX) + 1;
    let mut primitive = LitePrimitive::new(None);
    primitive.positions = (0..=overflow_index)
        .map(|index| [index as f32, 0.0, 0.0])
        .collect();
    primitive.indices = vec![0, 1, overflow_index];
    let mut mesh = LiteMesh::new("LargeIndex");
    mesh.primitives.push(primitive);
    mesh.recompute_bbox();
    document.meshes.push(mesh);
    document.nodes.push(LiteNode::new("LargeIndex", Some(0)));
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

fn sample_ole_stream_payload() -> Vec<u8> {
    let mut payload = b"private-preview-stream-prefix".to_vec();
    payload.extend_from_slice(&sample_glb());
    payload.resize(4600, 0);
    payload
}

fn sample_ole_with_stream(name: &str, payload: &[u8]) -> Vec<u8> {
    const SECTOR_SIZE: usize = 512;
    const FATSECT: u32 = 0xFFFF_FFFD;
    const FREESECT: u32 = 0xFFFF_FFFF;
    const ENDOFCHAIN: u32 = 0xFFFF_FFFE;

    let stream_sector_count = payload.len().div_ceil(SECTOR_SIZE);
    let total_sectors = 2 + stream_sector_count;
    assert!(total_sectors <= SECTOR_SIZE / 4);

    let mut header = vec![0_u8; SECTOR_SIZE];
    header[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    write_u16(&mut header, 0x18, 0x003E);
    write_u16(&mut header, 0x1A, 0x0003);
    write_u16(&mut header, 0x1C, 0xFFFE);
    write_u16(&mut header, 0x1E, 9);
    write_u16(&mut header, 0x20, 6);
    write_u32(&mut header, 0x2C, 1);
    write_u32(&mut header, 0x30, 1);
    write_u32(&mut header, 0x38, 4096);
    write_u32(&mut header, 0x3C, ENDOFCHAIN);
    write_u32(&mut header, 0x44, ENDOFCHAIN);
    for offset in (0x4C..0x200).step_by(4) {
        write_u32(&mut header, offset, FREESECT);
    }
    write_u32(&mut header, 0x4C, 0);

    let mut sectors = vec![vec![0_u8; SECTOR_SIZE]; total_sectors];
    let mut fat = vec![FREESECT; SECTOR_SIZE / 4];
    fat[0] = FATSECT;
    fat[1] = ENDOFCHAIN;
    for index in 0..stream_sector_count {
        let sector_id = 2 + index;
        fat[sector_id] = if index + 1 == stream_sector_count {
            ENDOFCHAIN
        } else {
            (sector_id + 1) as u32
        };
    }
    for (index, value) in fat.iter().enumerate() {
        write_u32(&mut sectors[0], index * 4, *value);
    }

    write_directory_entry(&mut sectors[1][0..128], "Root Entry", 5, ENDOFCHAIN, 0);
    write_directory_entry(&mut sectors[1][128..256], name, 2, 2, payload.len() as u64);

    for (sector_index, chunk) in payload.chunks(SECTOR_SIZE).enumerate() {
        sectors[2 + sector_index][..chunk.len()].copy_from_slice(chunk);
    }

    let mut bytes = header;
    for sector in sectors {
        bytes.extend_from_slice(&sector);
    }
    bytes
}

fn sample_ole_with_mini_stream(name: &str, payload: &[u8]) -> Vec<u8> {
    const SECTOR_SIZE: usize = 512;
    const MINI_SECTOR_SIZE: usize = 64;
    const FATSECT: u32 = 0xFFFF_FFFD;
    const FREESECT: u32 = 0xFFFF_FFFF;
    const ENDOFCHAIN: u32 = 0xFFFF_FFFE;

    assert!(payload.len() < 4096);
    let mini_sector_count = payload.len().div_ceil(MINI_SECTOR_SIZE);
    let mini_stream_len = mini_sector_count * MINI_SECTOR_SIZE;
    let mini_stream_sector_count = mini_stream_len.div_ceil(SECTOR_SIZE);
    let mini_stream_start_sector = 3_u32;
    let total_sectors = 3 + mini_stream_sector_count;
    assert!(total_sectors <= SECTOR_SIZE / 4);

    let mut header = vec![0_u8; SECTOR_SIZE];
    header[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    write_u16(&mut header, 0x18, 0x003E);
    write_u16(&mut header, 0x1A, 0x0003);
    write_u16(&mut header, 0x1C, 0xFFFE);
    write_u16(&mut header, 0x1E, 9);
    write_u16(&mut header, 0x20, 6);
    write_u32(&mut header, 0x2C, 1);
    write_u32(&mut header, 0x30, 1);
    write_u32(&mut header, 0x38, 4096);
    write_u32(&mut header, 0x3C, 2);
    write_u32(&mut header, 0x40, 1);
    write_u32(&mut header, 0x44, ENDOFCHAIN);
    for offset in (0x4C..0x200).step_by(4) {
        write_u32(&mut header, offset, FREESECT);
    }
    write_u32(&mut header, 0x4C, 0);

    let mut sectors = vec![vec![0_u8; SECTOR_SIZE]; total_sectors];
    let mut fat = vec![FREESECT; SECTOR_SIZE / 4];
    fat[0] = FATSECT;
    fat[1] = ENDOFCHAIN;
    fat[2] = ENDOFCHAIN;
    for index in 0..mini_stream_sector_count {
        let sector_id = 3 + index;
        fat[sector_id] = if index + 1 == mini_stream_sector_count {
            ENDOFCHAIN
        } else {
            (sector_id + 1) as u32
        };
    }
    for (index, value) in fat.iter().enumerate() {
        write_u32(&mut sectors[0], index * 4, *value);
    }

    let mut mini_fat = vec![FREESECT; SECTOR_SIZE / 4];
    for index in 0..mini_sector_count {
        mini_fat[index] = if index + 1 == mini_sector_count {
            ENDOFCHAIN
        } else {
            (index + 1) as u32
        };
    }
    for (index, value) in mini_fat.iter().enumerate() {
        write_u32(&mut sectors[2], index * 4, *value);
    }

    write_directory_entry(
        &mut sectors[1][0..128],
        "Root Entry",
        5,
        mini_stream_start_sector,
        mini_stream_len as u64,
    );
    write_directory_entry(&mut sectors[1][128..256], name, 2, 0, payload.len() as u64);

    let mut mini_stream = vec![0_u8; mini_stream_len];
    mini_stream[..payload.len()].copy_from_slice(payload);
    for (sector_index, chunk) in mini_stream.chunks(SECTOR_SIZE).enumerate() {
        sectors[3 + sector_index][..chunk.len()].copy_from_slice(chunk);
    }

    let mut bytes = header;
    for sector in sectors {
        bytes.extend_from_slice(&sector);
    }
    bytes
}

fn write_directory_entry(
    entry: &mut [u8],
    name: &str,
    object_type: u8,
    start_sector: u32,
    stream_size: u64,
) {
    const FREESECT: u32 = 0xFFFF_FFFF;

    let mut utf16 = name.encode_utf16().collect::<Vec<_>>();
    utf16.push(0);
    for (index, value) in utf16.iter().enumerate() {
        write_u16(entry, index * 2, *value);
    }
    write_u16(entry, 64, (utf16.len() * 2) as u16);
    entry[66] = object_type;
    entry[67] = 1;
    write_u32(entry, 68, FREESECT);
    write_u32(entry, 72, FREESECT);
    write_u32(entry, 76, FREESECT);
    write_u32(entry, 116, start_sector);
    write_u64(entry, 120, stream_size);
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
