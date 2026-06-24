use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use feather_lite::{
    CACHE_DUMP_MANIFEST_CONTRACT_VERSION, CacheDumpError, ImportError, ImportLimits,
    dump_embedded_visual_assets, dump_embedded_visual_assets_with_limits,
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
fn cache_dump_writes_assets_and_manifest_from_core_api() {
    let temp_dir = unique_temp_dir("cache-dump");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("dump");
    fs::write(
        &input,
        format!("CATPart-private-prefix\n{SAMPLE_CACHE}\nprivate-suffix"),
    )
    .expect("fixture should be written");

    let report =
        dump_embedded_visual_assets(&input, &output_dir).expect("cache dump should succeed");

    assert_eq!(report.asset_count(), 1);
    assert_eq!(report.manifest_path, output_dir.join("manifest.json"));
    assert_eq!(report.assets[0].index, 0);
    assert_eq!(report.assets[0].kind, "feather-cache");
    assert_eq!(report.assets[0].source, "embedded-bytes");
    assert_eq!(report.assets[0].file_name, "asset_000.flite");
    assert_eq!(
        report.assets[0].output_path,
        output_dir.join("asset_000.flite")
    );

    let dumped_cache =
        fs::read_to_string(output_dir.join("asset_000.flite")).expect("cache asset should exist");
    assert!(dumped_cache.contains("FEATHER_CAD_LITE_CACHE_V1"));

    let manifest =
        fs::read_to_string(output_dir.join("manifest.json")).expect("manifest should exist");
    let parsed_manifest: serde_json::Value =
        serde_json::from_str(&manifest).expect("cache dump manifest JSON should be valid");
    assert_eq!(
        parsed_manifest["contract_version"],
        CACHE_DUMP_MANIFEST_CONTRACT_VERSION
    );
    assert_eq!(parsed_manifest["asset_count"], 1);
    assert_eq!(parsed_manifest["assets"][0]["kind"], "feather-cache");
    assert_eq!(parsed_manifest["assets"][0]["source"], "embedded-bytes");
    assert_eq!(parsed_manifest["assets"][0]["file"], "asset_000.flite");
    assert!(manifest.contains("\"asset_count\": 1"));
    assert!(manifest.contains("\"kind\": \"feather-cache\""));
    assert!(manifest.contains("\"source\": \"embedded-bytes\""));
    assert!(manifest.contains("\"file\": \"asset_000.flite\""));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn cache_dump_with_limits_rejects_oversized_archive_entry() {
    let temp_dir = unique_temp_dir("cache-dump-limits");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("dump");
    let payload = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    fs::write(&input, stored_zip_entry("preview/model.obj", payload))
        .expect("ZIP fixture should be written");

    let error = dump_embedded_visual_assets_with_limits(
        &input,
        &output_dir,
        &ImportLimits {
            max_archive_entry_uncompressed_bytes: payload.len() - 1,
            ..ImportLimits::default()
        },
    )
    .expect_err("cache dump should enforce archive limits");
    assert!(matches!(
        error,
        CacheDumpError::AssetScan(ImportError::ResourceLimitExceeded {
            resource: "ZIP entry uncompressed bytes",
            limit,
            actual,
        }) if limit == payload.len() - 1 && actual == payload.len()
    ));
    assert!(!output_dir.exists());

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
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

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after UNIX epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("feather-lite-{prefix}-{suffix}"))
}
