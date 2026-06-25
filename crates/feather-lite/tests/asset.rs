use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use feather_lite::{
    ASSET_PACKAGE_CONTRACT_VERSION, AssetConversionError, AssetConversionProfile,
    AssetConversionRequest, AssetPackageFreshnessReason, AssetPackageStatus, AssetPreflightRequest,
    BatchAssetConversionRequest, BatchItemStatus, convert_asset, convert_batch_assets,
    ensure_asset_package, ensure_batch_asset_package, explain_asset_package_freshness,
    explain_batch_asset_package_freshness, is_asset_package_current,
    is_batch_asset_package_current, preflight_asset,
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
    assert_eq!(
        result.asset_id,
        source_info["asset_id"]
            .as_str()
            .expect("asset id should be a string")
    );
    assert_eq!(
        result.source_sha256,
        source_info["source_sha256"]
            .as_str()
            .expect("source hash should be a string")
    );
    assert_eq!(
        result.source_size_bytes,
        source_info["source_size_bytes"]
            .as_u64()
            .expect("source size should be an integer")
    );
    assert_eq!(
        result.settings_fingerprint,
        source_info["settings_fingerprint"]
            .as_str()
            .expect("settings fingerprint should be a string")
    );
    assert_eq!(
        result.source_size_bytes,
        diagnostics["source_size_bytes"]
            .as_u64()
            .expect("diagnostic source size should be an integer")
    );
    assert!(is_asset_package_current(&request).expect("freshness check should run"));

    fs::write(
        &input_path,
        format!("CATPart private payload changed\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be changed");
    assert!(!is_asset_package_current(&request).expect("freshness check should run"));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn ensure_asset_package_reuses_current_package() {
    let temp_dir = unique_temp_dir("asset-ensure-reuse");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let request = AssetConversionRequest::new(&input_path, &output_dir);
    let first = ensure_asset_package(&request).expect("first ensure should convert");
    assert_eq!(first.status, AssetPackageStatus::Converted);
    assert_eq!(first.status.as_str(), "converted");

    let second = ensure_asset_package(&request).expect("second ensure should reuse");
    assert_eq!(second.status, AssetPackageStatus::Reused);
    assert_eq!(second.status.as_str(), "reused");
    assert_eq!(second.asset.asset_id, first.asset.asset_id);
    assert_eq!(second.asset.source_sha256, first.asset.source_sha256);
    assert_eq!(
        second.asset.settings_fingerprint,
        first.asset.settings_fingerprint
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn ensure_asset_package_rebuilds_when_source_or_profile_changes() {
    let temp_dir = unique_temp_dir("asset-ensure-stale");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let mut request = AssetConversionRequest::new(&input_path, &output_dir);
    let first = ensure_asset_package(&request).expect("first ensure should convert");
    assert_eq!(first.status, AssetPackageStatus::Converted);

    fs::write(
        &input_path,
        format!("CATPart private payload changed\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be changed");
    let changed_source = ensure_asset_package(&request).expect("source change should convert");
    assert_eq!(changed_source.status, AssetPackageStatus::Converted);
    assert_ne!(changed_source.asset.asset_id, first.asset.asset_id);
    assert_ne!(
        changed_source.asset.source_sha256,
        first.asset.source_sha256
    );

    request.profile = AssetConversionProfile::HighQuality;
    let changed_profile = ensure_asset_package(&request).expect("profile change should convert");
    assert_eq!(changed_profile.status, AssetPackageStatus::Converted);
    assert_ne!(
        changed_profile.asset.settings_fingerprint,
        changed_source.asset.settings_fingerprint
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn explain_asset_package_freshness_reports_single_reuse_reasons() {
    let temp_dir = unique_temp_dir("asset-freshness-single");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let request = AssetConversionRequest::new(&input_path, &output_dir);
    let missing_package =
        explain_asset_package_freshness(&request).expect("freshness should explain missing state");
    assert!(!missing_package.current);
    assert_eq!(
        missing_package.reason,
        AssetPackageFreshnessReason::MissingSourceInfo
    );

    let first = ensure_asset_package(&request).expect("first ensure should convert");
    let current =
        explain_asset_package_freshness(&request).expect("freshness should explain current state");
    assert!(current.current);
    assert_eq!(current.reason, AssetPackageFreshnessReason::Current);
    assert_eq!(current.reason.as_str(), "current");

    let metadata_path = first
        .asset
        .package
        .metadata_path
        .as_ref()
        .expect("metadata path should be reserved")
        .clone();
    fs::remove_file(&metadata_path).expect("metadata should be removable");
    let missing_metadata = explain_asset_package_freshness(&request)
        .expect("freshness should detect missing metadata");
    assert!(!missing_metadata.current);
    assert_eq!(
        missing_metadata.reason,
        AssetPackageFreshnessReason::MissingMetadata
    );
    assert_eq!(missing_metadata.reason.as_str(), "missing_metadata");

    let rebuilt = ensure_asset_package(&request).expect("missing metadata should rebuild");
    let diagnostics_path = rebuilt.asset.package.diagnostics_path.clone();
    fs::remove_file(&diagnostics_path).expect("diagnostics should be removable");
    let missing_diagnostics = explain_asset_package_freshness(&request)
        .expect("freshness should detect missing diagnostics");
    assert_eq!(
        missing_diagnostics.reason,
        AssetPackageFreshnessReason::MissingDiagnostics
    );

    let _ = ensure_asset_package(&request).expect("missing diagnostics should rebuild");
    fs::write(
        &input_path,
        format!("CATPart private payload changed\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be changed");
    let changed_source =
        explain_asset_package_freshness(&request).expect("freshness should detect source change");
    assert_eq!(
        changed_source.reason,
        AssetPackageFreshnessReason::SourceChanged
    );

    let changed_result = ensure_asset_package(&request).expect("changed source should rebuild");
    let mut high_quality_request = request.clone();
    high_quality_request.profile = AssetConversionProfile::HighQuality;
    let changed_settings = explain_asset_package_freshness(&high_quality_request)
        .expect("freshness should detect settings change");
    assert_eq!(
        changed_settings.reason,
        AssetPackageFreshnessReason::SettingsChanged
    );

    assert_ne!(
        changed_result.asset.source_sha256,
        first.asset.source_sha256
    );
    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn ensure_asset_package_rebuilds_incomplete_or_failed_package() {
    let temp_dir = unique_temp_dir("asset-ensure-incomplete");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let request = AssetConversionRequest::new(&input_path, &output_dir);
    let first = ensure_asset_package(&request).expect("first ensure should convert");
    let model_path = first
        .asset
        .package
        .model_path
        .as_ref()
        .expect("model path should be reserved")
        .clone();
    fs::remove_file(&model_path).expect("model should be removable");
    assert_eq!(
        explain_asset_package_freshness(&request)
            .expect("freshness should detect missing model")
            .reason,
        AssetPackageFreshnessReason::MissingModel
    );

    let rebuilt = ensure_asset_package(&request).expect("missing model should convert");
    assert_eq!(rebuilt.status, AssetPackageStatus::Converted);
    assert!(model_path.is_file());

    let diagnostics_path = rebuilt.asset.package.diagnostics_path.clone();
    let mut diagnostics = parse_json(&fs::read_to_string(&diagnostics_path).unwrap());
    diagnostics["status"] = serde_json::Value::String("failed".to_string());
    fs::write(
        &diagnostics_path,
        serde_json::to_string_pretty(&diagnostics).expect("diagnostics should serialize"),
    )
    .expect("diagnostics should be writable");
    assert_eq!(
        explain_asset_package_freshness(&request)
            .expect("freshness should detect failed diagnostics")
            .reason,
        AssetPackageFreshnessReason::DiagnosticsFailed
    );

    let after_failed_diagnostics =
        ensure_asset_package(&request).expect("failed diagnostics should convert");
    assert_eq!(
        after_failed_diagnostics.status,
        AssetPackageStatus::Converted
    );
    assert!(is_asset_package_current(&request).expect("freshness check should run"));

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
    assert_eq!(
        diagnostics["asset_id"]
            .as_str()
            .expect("asset id should be a string"),
        result.asset_id
    );
    assert_eq!(
        diagnostics["source_sha256"]
            .as_str()
            .expect("source hash should be a string"),
        result.source_sha256
    );
    assert_eq!(
        diagnostics["source_size_bytes"]
            .as_u64()
            .expect("source size should be an integer"),
        result.source_size_bytes
    );
    assert_eq!(
        diagnostics["settings_fingerprint"]
            .as_str()
            .expect("settings fingerprint should be a string"),
        result.settings_fingerprint
    );
    assert_eq!(diagnostics["converted_count"], 1);

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn convert_batch_assets_expands_directory_inputs_for_package_identity() {
    let temp_dir = unique_temp_dir("asset-batch-directory");
    let source_dir = temp_dir.join("incoming");
    let nested_dir = source_dir.join("nested");
    fs::create_dir_all(&nested_dir).expect("source dirs should be created");
    let input_a = source_dir.join("a.CATPart");
    let input_b = nested_dir.join("b.CATPart");
    let ignored = source_dir.join("ignore.txt");
    let output_dir = temp_dir.join("batch-asset");
    fs::write(
        &input_a,
        format!("CATPart private payload A\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture A should be written");
    fs::write(
        &input_b,
        format!("CATPart private payload B\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture B should be written");
    fs::write(&ignored, "not a supported batch input").expect("ignored file should be written");

    let result = convert_batch_assets(&BatchAssetConversionRequest::new(
        vec![source_dir.clone()],
        &output_dir,
    ))
    .expect("directory batch asset conversion should succeed");

    assert_eq!(result.report.input_count(), 2);
    assert_eq!(result.report.converted_count(), 2);
    let source_info = parse_json(&fs::read_to_string(&result.package.source_info_path).unwrap());
    assert_eq!(source_info["kind"], "batch_conversion");
    assert_eq!(
        source_info["inputs"]
            .as_array()
            .expect("inputs array")
            .len(),
        2
    );
    let source_dir_path = source_dir.display().to_string();
    assert!(
        source_info["inputs"]
            .as_array()
            .expect("inputs array")
            .iter()
            .all(|input| input["path"]
                .as_str()
                .expect("input path should be a string")
                != source_dir_path.as_str())
    );
    assert_eq!(
        source_info["source_size_bytes"]
            .as_u64()
            .expect("source size should be an integer"),
        result.source_size_bytes
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn ensure_batch_asset_package_reuses_current_package() {
    let temp_dir = unique_temp_dir("asset-batch-ensure-reuse");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("batch-asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let request = BatchAssetConversionRequest::new(vec![input_path], &output_dir);
    let first = ensure_batch_asset_package(&request).expect("first ensure should convert");
    assert_eq!(first.status, AssetPackageStatus::Converted);
    assert_eq!(first.asset.report.converted_count(), 1);
    assert!(is_batch_asset_package_current(&request).expect("freshness check should run"));

    let second = ensure_batch_asset_package(&request).expect("second ensure should reuse");
    assert_eq!(second.status, AssetPackageStatus::Reused);
    assert_eq!(second.asset.asset_id, first.asset.asset_id);
    assert_eq!(second.asset.report.converted_count(), 1);

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn ensure_batch_asset_package_rebuilds_when_source_or_mode_changes() {
    let temp_dir = unique_temp_dir("asset-batch-ensure-stale");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("batch-asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let mut request = BatchAssetConversionRequest::new(vec![input_path.clone()], &output_dir);
    let first = ensure_batch_asset_package(&request).expect("first ensure should convert");
    assert_eq!(first.status, AssetPackageStatus::Converted);

    fs::write(
        &input_path,
        format!("CATPart private payload changed\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be changed");
    let changed_source =
        ensure_batch_asset_package(&request).expect("source change should convert");
    assert_eq!(changed_source.status, AssetPackageStatus::Converted);
    assert_ne!(changed_source.asset.asset_id, first.asset.asset_id);

    request.check_only = true;
    let changed_mode = ensure_batch_asset_package(&request).expect("mode change should convert");
    assert_eq!(changed_mode.status, AssetPackageStatus::Converted);
    assert_eq!(changed_mode.asset.report.checked_count(), 1);
    assert_ne!(
        changed_mode.asset.settings_fingerprint,
        changed_source.asset.settings_fingerprint
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn explain_batch_asset_package_freshness_reports_batch_reuse_reasons() {
    let temp_dir = unique_temp_dir("asset-freshness-batch");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("batch-asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let request = BatchAssetConversionRequest::new(vec![input_path], &output_dir);
    let missing_package = explain_batch_asset_package_freshness(&request)
        .expect("freshness should explain missing batch state");
    assert_eq!(
        missing_package.reason,
        AssetPackageFreshnessReason::MissingSourceInfo
    );

    let first = ensure_batch_asset_package(&request).expect("first ensure should convert");
    let current = explain_batch_asset_package_freshness(&request)
        .expect("freshness should explain current batch state");
    assert!(current.current);
    assert_eq!(current.reason, AssetPackageFreshnessReason::Current);

    let mut check_request = request.clone();
    check_request.check_only = true;
    let changed_mode = explain_batch_asset_package_freshness(&check_request)
        .expect("freshness should detect batch mode change");
    assert_eq!(
        changed_mode.reason,
        AssetPackageFreshnessReason::SettingsChanged
    );

    let output_path = match &first.asset.report.items[0].status {
        BatchItemStatus::Ok { output_path, .. } => std::path::PathBuf::from(output_path),
        _ => panic!("batch item should be converted"),
    };
    fs::remove_file(&output_path).expect("batch output should be removable");
    let missing_output = explain_batch_asset_package_freshness(&request)
        .expect("freshness should detect missing batch output");
    assert_eq!(
        missing_output.reason,
        AssetPackageFreshnessReason::OutputArtifactMissing
    );

    let rebuilt =
        ensure_batch_asset_package(&request).expect("missing output should be regenerated");
    let manifest_path = rebuilt
        .asset
        .package
        .manifest_path
        .as_ref()
        .expect("manifest path should be reserved")
        .clone();
    let mut manifest = parse_json(&fs::read_to_string(&manifest_path).unwrap());
    manifest["contract_version"] = serde_json::Value::String("broken-contract".to_string());
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).expect("manifest should serialize"),
    )
    .expect("manifest should be writable");
    let manifest_mismatch = explain_batch_asset_package_freshness(&request)
        .expect("freshness should detect manifest mismatch");
    assert_eq!(
        manifest_mismatch.reason,
        AssetPackageFreshnessReason::ManifestMismatch
    );

    fs::remove_file(&manifest_path).expect("manifest should be removable");
    let missing_manifest = explain_batch_asset_package_freshness(&request)
        .expect("freshness should detect missing manifest");
    assert_eq!(
        missing_manifest.reason,
        AssetPackageFreshnessReason::MissingManifest
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn ensure_batch_asset_package_rebuilds_incomplete_package() {
    let temp_dir = unique_temp_dir("asset-batch-ensure-incomplete");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("batch-asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let request = BatchAssetConversionRequest::new(vec![input_path], &output_dir);
    let first = ensure_batch_asset_package(&request).expect("first ensure should convert");
    let output_path = match &first.asset.report.items[0].status {
        BatchItemStatus::Ok { output_path, .. } => std::path::PathBuf::from(output_path),
        _ => panic!("batch item should be converted"),
    };
    fs::remove_file(&output_path).expect("output should be removable");

    let rebuilt = ensure_batch_asset_package(&request).expect("missing output should convert");
    assert_eq!(rebuilt.status, AssetPackageStatus::Converted);
    assert!(output_path.is_file());

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn ensure_batch_asset_package_reuses_unchanged_items_only() {
    let temp_dir = unique_temp_dir("asset-batch-ensure-partial");
    let source_dir = temp_dir.join("incoming");
    let output_dir = temp_dir.join("batch-asset");
    fs::create_dir_all(&source_dir).expect("source dir should be created");
    let input_a = source_dir.join("a.CATPart");
    let input_b = source_dir.join("b.CATPart");
    let input_c = source_dir.join("c.CATPart");
    let input_d = source_dir.join("d.CATPart");
    fs::write(
        &input_a,
        format!("CATPart private payload A\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture A should be written");
    fs::write(
        &input_b,
        format!("CATPart private payload B\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture B should be written");
    fs::write(
        &input_c,
        format!("CATPart private payload C\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture C should be written");

    let request = BatchAssetConversionRequest::new(vec![source_dir.clone()], &output_dir);
    let first = ensure_batch_asset_package(&request).expect("first ensure should convert");
    assert_eq!(first.status, AssetPackageStatus::Converted);
    assert_eq!(first.asset.report.converted_count(), 3);
    assert_eq!(first.asset.report.reused_count(), 0);

    fs::write(
        &input_b,
        format!("CATPart private payload B changed\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture B should be changed");
    fs::remove_file(&input_c).expect("fixture C should be removable");
    fs::write(
        &input_d,
        format!("CATPart private payload D\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture D should be written");

    let second = ensure_batch_asset_package(&request).expect("second ensure should be incremental");
    assert_eq!(second.status, AssetPackageStatus::Converted);
    assert_eq!(second.asset.report.input_count(), 3);
    assert_eq!(second.asset.report.reused_count(), 1);
    assert_eq!(second.asset.report.converted_count(), 2);
    assert_eq!(second.asset.report.failed_count(), 0);
    assert!(
        second
            .asset
            .report
            .items
            .iter()
            .any(|item| item.input_path == input_a.display().to_string()
                && matches!(item.status, BatchItemStatus::Reused { .. }))
    );
    assert!(
        second
            .asset
            .report
            .items
            .iter()
            .all(|item| item.input_path != input_c.display().to_string())
    );

    let manifest = parse_json(
        &fs::read_to_string(second.asset.package.manifest_path.as_ref().unwrap()).unwrap(),
    );
    assert_eq!(manifest["input_count"], 3);
    assert_eq!(manifest["converted_count"], 2);
    assert_eq!(manifest["reused_count"], 1);
    assert!(
        manifest["items"]
            .as_array()
            .expect("manifest items should be an array")
            .iter()
            .any(|item| item["status"] == "reused" && item["operation"] == "reused")
    );
    assert!(
        manifest["items"]
            .as_array()
            .expect("manifest items should be an array")
            .iter()
            .all(|item| item["input_path"] != input_c.display().to_string())
    );

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
