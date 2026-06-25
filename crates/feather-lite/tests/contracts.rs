use std::path::PathBuf;

use feather_lite::{
    BATCH_MANIFEST_CONTRACT_VERSION, BatchReport, CACHE_DUMP_MANIFEST_CONTRACT_VERSION,
    CacheDumpReport, DumpedVisualAsset, FORMAT_CAPABILITIES_CONTRACT_VERSION,
    INSPECT_REPORT_CONTRACT_VERSION, InspectOptions, format_capabilities_json, inspect_bytes,
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
fn exported_contract_versions_match_documented_v1_identifiers() {
    assert_eq!(
        FORMAT_CAPABILITIES_CONTRACT_VERSION,
        "feather.format-capabilities.v1"
    );
    assert_eq!(INSPECT_REPORT_CONTRACT_VERSION, "feather.inspect-report.v1");
    assert_eq!(BATCH_MANIFEST_CONTRACT_VERSION, "feather.batch-manifest.v1");
    assert_eq!(
        CACHE_DUMP_MANIFEST_CONTRACT_VERSION,
        "feather.cache-dump-manifest.v1"
    );
}

#[test]
fn core_json_surfaces_emit_contract_versions_and_required_roots() {
    let capabilities = parse_json(&format_capabilities_json());
    assert_contract_version(&capabilities, FORMAT_CAPABILITIES_CONTRACT_VERSION);
    assert!(
        capabilities["formats"].is_array(),
        "capabilities contract should expose formats array"
    );

    let inspect_report = inspect_bytes(
        Some(std::path::Path::new("fixture.CATPart")),
        format!("CATPart private payload\n{SAMPLE_CACHE}").as_bytes(),
        &InspectOptions::default(),
    )
    .expect("inspect API should produce a report");
    let inspect = parse_json(&inspect_report.to_json_string());
    assert_contract_version(&inspect, INSPECT_REPORT_CONTRACT_VERSION);
    assert!(inspect["format"].is_string());
    assert!(inspect["capability"].is_object());
    assert!(inspect["visual_assets"].is_array());
    let inspect_object = inspect
        .as_object()
        .expect("inspect contract root should be an object");
    for field in ["container_kind", "source_version", "native_visualization"] {
        assert!(
            inspect_object.contains_key(field),
            "inspect contract should expose {field}"
        );
    }

    let batch = parse_json(&BatchReport::new(Vec::new()).to_manifest_json());
    assert_contract_version(&batch, BATCH_MANIFEST_CONTRACT_VERSION);
    assert_eq!(batch["input_count"], 0);
    assert!(batch["summary"].is_object());
    assert_eq!(batch["summary"]["total_node_count"], 0);
    assert_eq!(batch["summary"]["total_primitive_count"], 0);
    assert_eq!(batch["summary"]["total_vertex_count"], 0);
    assert!(batch["items"].is_array());

    let cache_report = CacheDumpReport {
        source_path: "fixture.CATPart".to_string(),
        manifest_path: PathBuf::from("manifest.json"),
        assets: vec![DumpedVisualAsset {
            index: 0,
            kind: "feather-cache".to_string(),
            source: "embedded-bytes".to_string(),
            byte_start: 10,
            byte_end: 20,
            entry_name: None,
            file_name: "asset_000.flite".to_string(),
            output_path: PathBuf::from("asset_000.flite"),
        }],
    };
    let cache = parse_json(&cache_report.to_manifest_json());
    assert_contract_version(&cache, CACHE_DUMP_MANIFEST_CONTRACT_VERSION);
    assert_eq!(cache["asset_count"], 1);
    assert!(cache["assets"].is_array());
}

fn parse_json(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("contract JSON should be valid")
}

fn assert_contract_version(json: &serde_json::Value, expected: &str) {
    assert_eq!(json["contract_version"], expected);
}
