use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use feather_lite::{
    ASSET_PACKAGE_CONTRACT_VERSION, AssetConversionError, AssetConversionProfile,
    AssetConversionRequest, AssetPreflightRequest, BatchAssetConversionRequest, convert_asset,
    convert_batch_assets, preflight_asset,
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
fn convert_asset_writes_standard_business_package() {
    let temp_dir = unique_temp_dir("asset-convert");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let mut request = AssetConversionRequest::new(&input_path, &output_dir);
    request.profile = AssetConversionProfile::StandardReview;
    let result = convert_asset(&request).expect("asset conversion should succeed");

    assert_eq!(result.source_format, "CATIA_CATPart");
    assert_eq!(result.triangle_count, 1);
    assert!(
        result
            .package
            .model_path
            .as_ref()
            .expect("model path should be reserved")
            .is_file()
    );
    assert!(
        result
            .package
            .metadata_path
            .as_ref()
            .expect("metadata path should be reserved")
            .is_file()
    );
    assert!(result.package.source_info_path.is_file());
    assert!(result.package.diagnostics_path.is_file());

    let source_info = parse_json(&fs::read_to_string(&result.package.source_info_path).unwrap());
    assert_eq!(
        source_info["contract_version"],
        ASSET_PACKAGE_CONTRACT_VERSION
    );
    assert_eq!(source_info["kind"], "conversion");
    assert_eq!(source_info["profile"], "standard_review");

    let diagnostics = parse_json(&fs::read_to_string(&result.package.diagnostics_path).unwrap());
    assert_eq!(
        diagnostics["contract_version"],
        ASSET_PACKAGE_CONTRACT_VERSION
    );
    assert_eq!(diagnostics["status"], "succeeded");
    assert_eq!(diagnostics["triangle_count"], 1);

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn preflight_asset_returns_business_failure_without_writing_package() {
    let temp_dir = unique_temp_dir("asset-preflight");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    fs::write(&input_path, "CATPart private payload without cache")
        .expect("broken fixture should be written");

    let result =
        preflight_asset(&AssetPreflightRequest::new(&input_path)).expect("preflight should run");

    assert_eq!(result.source_format, "CATIA_CATPart");
    assert!(!result.importable);
    assert_eq!(
        result
            .failure
            .as_ref()
            .expect("failure should be returned")
            .category,
        "no_readable_lightweight_cache"
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn convert_batch_assets_writes_manifest_package() {
    let temp_dir = unique_temp_dir("asset-batch");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("batch-asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let mut request = BatchAssetConversionRequest::new(vec![input_path], &output_dir);
    request.profile = AssetConversionProfile::MobilePreview;
    let result = convert_batch_assets(&request).expect("batch asset conversion should succeed");

    assert_eq!(result.report.input_count(), 1);
    assert_eq!(result.report.converted_count(), 1);
    assert!(
        result
            .package
            .manifest_path
            .as_ref()
            .expect("manifest path should be reserved")
            .is_file()
    );
    assert!(result.package.source_info_path.is_file());
    assert!(result.package.diagnostics_path.is_file());

    let diagnostics = parse_json(&fs::read_to_string(&result.package.diagnostics_path).unwrap());
    assert_eq!(diagnostics["status"], "succeeded");
    assert_eq!(diagnostics["profile"], "mobile_preview");
    assert_eq!(diagnostics["converted_count"], 1);

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn convert_asset_failure_writes_diagnostics_and_returns_package() {
    let temp_dir = unique_temp_dir("asset-failure");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("asset");
    fs::write(&input_path, "CATPart private payload without cache")
        .expect("broken fixture should be written");

    let error = convert_asset(&AssetConversionRequest::new(&input_path, &output_dir))
        .expect_err("broken private CAD should fail");
    let AssetConversionError::ConversionFailed { package, failure } = error else {
        panic!("expected conversion failure");
    };

    assert_eq!(failure.category, "no_readable_lightweight_cache");
    assert!(package.source_info_path.is_file());
    assert!(package.diagnostics_path.is_file());
    let diagnostics = parse_json(&fs::read_to_string(&package.diagnostics_path).unwrap());
    assert_eq!(diagnostics["status"], "failed");
    assert_eq!(
        diagnostics["failure"]["category"],
        "no_readable_lightweight_cache"
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

fn parse_json(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("JSON should parse")
}

fn unique_temp_dir(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after UNIX epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("feather-lite-{label}-{suffix}"))
}
