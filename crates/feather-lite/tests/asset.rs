use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use feather_lite::{
    ASSET_PACKAGE_CONTRACT_VERSION, AssetConversionError, AssetConversionProfile,
    AssetConversionRequest, AssetFailure, AssetFailureAction, AssetPackageFreshnessReason,
    AssetPackageStatus, AssetPackageSummaryOperation, AssetPreflightDecision,
    AssetPreflightRequest, AssetPreviewStatus, AssetQualityLevel, BatchAssetConversionRequest,
    BatchItemStatus, JobConversionSettings, asset_conversion_identity,
    batch_asset_conversion_identity, convert_asset, convert_batch_assets, ensure_asset_package,
    ensure_batch_asset_package, explain_asset_package_freshness,
    explain_batch_asset_package_freshness, inspect_asset_package, is_asset_package_current,
    is_batch_asset_package_current, load_current_asset_package, load_current_batch_asset_package,
    preflight_asset, read_asset_package_summary,
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
    assert!(result.quality.previewable);
    assert!(result.quality.has_visual_geometry);
    assert_eq!(result.quality.preview_status, AssetPreviewStatus::Ready);
    assert_eq!(result.quality.preview_status.as_str(), "ready");
    assert_eq!(result.quality.quality_level, AssetQualityLevel::Light);
    assert_eq!(result.quality.quality_level.as_str(), "light");
    assert_eq!(result.quality.input_count, 1);
    assert_eq!(result.quality.successful_count, 1);
    assert_eq!(result.quality.converted_count, 1);
    assert_eq!(result.quality.triangle_count, 1);
    assert!(result.quality.input_size_bytes > 0);
    assert!(result.quality.output_size_bytes > 0);
    assert!(result.quality.metadata_size_bytes > 0);
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
    assert_eq!(diagnostics["quality"]["previewable"], true);
    assert_eq!(diagnostics["quality"]["preview_status"], "ready");
    assert_eq!(diagnostics["quality"]["quality_level"], "light");
    assert_eq!(diagnostics["quality"]["triangle_count"], 1);
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
fn asset_conversion_identity_matches_conversion_and_effective_settings() {
    let temp_dir = unique_temp_dir("asset-identity-single");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let request = AssetConversionRequest::new(&input_path, &output_dir);
    let planned = asset_conversion_identity(&request).expect("identity should be computable");
    let repeated = asset_conversion_identity(&request).expect("identity should be stable");
    assert_eq!(planned, repeated);

    let converted = convert_asset(&request).expect("asset conversion should succeed");
    assert_eq!(planned.asset_id, converted.asset_id);
    assert_eq!(planned.source_sha256, converted.source_sha256);
    assert_eq!(planned.source_size_bytes, converted.source_size_bytes);
    assert_eq!(planned.settings_fingerprint, converted.settings_fingerprint);

    let custom_settings = JobConversionSettings {
        write_metadata: false,
        max_triangles: Some(42),
        ..JobConversionSettings::default()
    };
    let mut metadata_disabled = request.clone();
    metadata_disabled.profile = AssetConversionProfile::Custom(custom_settings.clone());
    let mut metadata_enabled = request.clone();
    metadata_enabled.profile = AssetConversionProfile::Custom(JobConversionSettings {
        write_metadata: true,
        ..custom_settings
    });
    assert_eq!(
        asset_conversion_identity(&metadata_disabled)
            .expect("custom identity should be computable"),
        asset_conversion_identity(&metadata_enabled)
            .expect("custom identity should normalize effective settings")
    );

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
    assert_eq!(second.asset.quality, first.asset.quality);
    assert_eq!(
        second.asset.settings_fingerprint,
        first.asset.settings_fingerprint
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn load_current_asset_package_reads_only_current_package() {
    let temp_dir = unique_temp_dir("asset-load-current");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let request = AssetConversionRequest::new(&input_path, &output_dir);
    assert!(
        load_current_asset_package(&request)
            .expect("current package load should run")
            .is_none()
    );
    assert!(!output_dir.exists());

    let converted = convert_asset(&request).expect("asset conversion should succeed");
    let current = load_current_asset_package(&request)
        .expect("current package load should run")
        .expect("current package should be returned");
    assert_eq!(current.asset_id, converted.asset_id);
    assert_eq!(current.quality, converted.quality);
    assert_eq!(current.triangle_count, converted.triangle_count);

    fs::write(
        &input_path,
        format!("CATPart private payload changed\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be changed");
    assert!(
        load_current_asset_package(&request)
            .expect("stale package load should run")
            .is_none()
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn inspect_asset_package_audits_single_package_without_request() {
    let temp_dir = unique_temp_dir("asset-inspect-package-single");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let missing = inspect_asset_package(&output_dir).expect("package audit should run");
    assert!(!missing.usable);
    assert_eq!(
        missing.reason,
        AssetPackageFreshnessReason::MissingSourceInfo
    );
    assert!(missing.kind.is_none());
    assert!(missing.identity.is_none());

    let converted = convert_asset(&AssetConversionRequest::new(&input_path, &output_dir))
        .expect("asset conversion should succeed");
    let audit = inspect_asset_package(&output_dir).expect("package audit should run");
    assert!(audit.usable);
    assert!(audit.previewable());
    assert_eq!(audit.reason, AssetPackageFreshnessReason::Current);
    assert_eq!(audit.kind.as_deref(), Some("conversion"));
    assert_eq!(audit.status.as_deref(), Some("succeeded"));
    assert_eq!(
        audit
            .identity
            .as_ref()
            .expect("identity should exist")
            .asset_id,
        converted.asset_id
    );
    assert_eq!(audit.input_count, 1);
    assert_eq!(
        audit.quality.as_ref().expect("quality should exist"),
        &converted.quality
    );
    let summary = read_asset_package_summary(&output_dir).expect("package summary should read");
    assert!(summary.audit.usable);
    assert_eq!(summary.items.len(), 1);
    assert_eq!(
        summary.output_size_bytes,
        converted.quality.output_size_bytes
    );
    assert_eq!(
        summary.metadata_size_bytes,
        converted.quality.metadata_size_bytes
    );
    let item = &summary.items[0];
    assert!(item.previewable());
    assert_eq!(item.operation, AssetPackageSummaryOperation::Converted);
    assert_eq!(item.output_path, converted.package.model_path);
    assert_eq!(item.metadata_path, converted.package.metadata_path);
    assert_eq!(item.source_format.as_deref(), Some("CATIA_CATPart"));
    assert_eq!(item.triangle_count, Some(1));
    let metadata = item
        .metadata
        .as_ref()
        .expect("metadata summary should exist");
    assert_eq!(metadata.source_format, "CATIA_CATPart");
    assert_eq!(metadata.triangle_count, 1);
    assert!(!metadata.mode.is_empty());

    fs::remove_file(
        converted
            .package
            .metadata_path
            .as_ref()
            .expect("metadata path should be reserved"),
    )
    .expect("metadata should be removable");
    let incomplete = inspect_asset_package(&output_dir).expect("package audit should run");
    assert!(!incomplete.usable);
    assert_eq!(
        incomplete.reason,
        AssetPackageFreshnessReason::MissingMetadata
    );
    let incomplete_summary =
        read_asset_package_summary(&output_dir).expect("package summary should read");
    assert!(!incomplete_summary.audit.usable);
    assert!(incomplete_summary.items.is_empty());

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
        result.decision,
        AssetPreflightDecision::NeedsReadableVisualization
    );
    assert_eq!(result.decision.as_str(), "needs_readable_visualization");
    assert!(result.quality.is_none());
    assert!(
        result
            .required_condition
            .as_ref()
            .expect("required condition should be returned")
            .contains("readable lightweight visualization payload")
    );
    assert_eq!(
        result
            .failure
            .as_ref()
            .expect("failure should be returned")
            .category,
        "no_readable_lightweight_cache"
    );
    let failure = result.failure.as_ref().expect("failure should be returned");
    assert_eq!(
        failure.decision(),
        AssetPreflightDecision::NeedsReadableVisualization
    );
    assert_eq!(
        failure.action(),
        AssetFailureAction::ProvideReadableVisualization
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn asset_failure_exposes_business_decision_and_action() {
    let references = AssetFailure {
        stage: "import".to_string(),
        category: "missing_external_reference".to_string(),
        message: "missing part".to_string(),
        retryable: true,
    };
    assert_eq!(
        references.decision(),
        AssetPreflightDecision::NeedsExternalReferences
    );
    assert_eq!(
        references.action(),
        AssetFailureAction::ResolveExternalReferences
    );
    assert_eq!(
        references.required_condition(),
        Some("all external references resolved through resolve_dirs or reference mappings")
    );
    assert_eq!(references.action().as_str(), "resolve_external_references");

    let limits = AssetFailure {
        stage: "import".to_string(),
        category: "resource_limit_exceeded".to_string(),
        message: "too large".to_string(),
        retryable: true,
    };
    assert_eq!(
        limits.decision(),
        AssetPreflightDecision::ResourceLimitExceeded
    );
    assert_eq!(limits.action(), AssetFailureAction::IncreaseResourceLimits);

    let batch = AssetFailure {
        stage: "batch".to_string(),
        category: "batch_item_failed".to_string(),
        message: "batch completed with failed items".to_string(),
        retryable: true,
    };
    assert_eq!(batch.decision(), AssetPreflightDecision::Failed);
    assert_eq!(batch.action(), AssetFailureAction::ReviewBatchFailures);
    assert_eq!(
        batch.required_condition(),
        Some("all failed batch items corrected or removed")
    );
}

#[test]
fn preflight_asset_returns_business_ready_decision_and_quality() {
    let temp_dir = unique_temp_dir("asset-preflight-ready");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let result =
        preflight_asset(&AssetPreflightRequest::new(&input_path)).expect("preflight should run");

    assert_eq!(result.source_format, "CATIA_CATPart");
    assert!(result.importable);
    assert_eq!(result.decision, AssetPreflightDecision::Ready);
    assert_eq!(result.decision.as_str(), "ready");
    assert!(result.required_condition.is_none());
    assert!(result.failure.is_none());
    assert_eq!(result.node_count, Some(1));
    assert_eq!(result.mesh_count, Some(1));
    assert_eq!(result.primitive_count, Some(1));
    assert_eq!(result.vertex_count, Some(3));
    assert_eq!(result.triangle_count, Some(1));
    let quality = result.quality.expect("quality should be returned");
    assert!(!quality.previewable);
    assert!(quality.has_visual_geometry);
    assert_eq!(quality.preview_status, AssetPreviewStatus::NoPreviewOutput);
    assert_eq!(quality.quality_level, AssetQualityLevel::Light);
    assert_eq!(quality.checked_count, 1);
    assert_eq!(quality.converted_count, 0);
    assert_eq!(quality.triangle_count, 1);
    assert!(quality.input_size_bytes > 0);
    assert_eq!(quality.output_size_bytes, 0);

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
    assert!(result.quality.previewable);
    assert_eq!(result.quality.preview_status, AssetPreviewStatus::Ready);
    assert_eq!(result.quality.quality_level, AssetQualityLevel::Light);
    assert_eq!(result.quality.input_count, 1);
    assert_eq!(result.quality.successful_count, 1);
    assert_eq!(result.quality.converted_count, 1);
    assert_eq!(result.quality.checked_count, 0);
    assert_eq!(result.quality.failed_count, 0);
    assert_eq!(result.quality.triangle_count, 1);
    assert!(result.quality.input_size_bytes > 0);
    assert!(result.quality.output_size_bytes > 0);
    assert!(result.quality.metadata_size_bytes > 0);
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
    assert_eq!(diagnostics["quality"]["preview_status"], "ready");
    assert_eq!(diagnostics["quality"]["quality_level"], "light");
    assert_eq!(diagnostics["quality"]["converted_count"], 1);
    assert_eq!(diagnostics["quality"]["triangle_count"], 1);

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
fn batch_asset_conversion_identity_matches_conversion_and_mode() {
    let temp_dir = unique_temp_dir("asset-identity-batch");
    let source_dir = temp_dir.join("incoming");
    fs::create_dir_all(&source_dir).expect("source dir should be created");
    let input_b = source_dir.join("b.CATPart");
    let input_a = source_dir.join("a.CATPart");
    let output_dir = source_dir.join("batch-asset");
    fs::write(
        &input_b,
        format!("CATPart private payload B\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture B should be written");
    fs::write(
        &input_a,
        format!("CATPart private payload A\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture A should be written");

    let request = BatchAssetConversionRequest::new(vec![source_dir.clone()], &output_dir);
    assert!(!output_dir.exists());
    let planned = batch_asset_conversion_identity(&request).expect("identity should be computable");

    let converted = convert_batch_assets(&request).expect("batch conversion should succeed");
    assert_eq!(planned.asset_id, converted.asset_id);
    assert_eq!(planned.source_sha256, converted.source_sha256);
    assert_eq!(planned.source_size_bytes, converted.source_size_bytes);
    assert_eq!(planned.settings_fingerprint, converted.settings_fingerprint);

    let repeated =
        batch_asset_conversion_identity(&request).expect("identity should remain stable");
    assert_eq!(planned, repeated);

    let mut check_request = request.clone();
    check_request.check_only = true;
    let check_only =
        batch_asset_conversion_identity(&check_request).expect("check identity should compute");
    assert_eq!(check_only.source_sha256, planned.source_sha256);
    assert_eq!(check_only.source_size_bytes, planned.source_size_bytes);
    assert_ne!(
        check_only.settings_fingerprint,
        planned.settings_fingerprint
    );
    assert_ne!(check_only.asset_id, planned.asset_id);

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
fn load_current_batch_asset_package_reads_only_current_package() {
    let temp_dir = unique_temp_dir("asset-batch-load-current");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("batch-asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let request = BatchAssetConversionRequest::new(vec![input_path], &output_dir);
    assert!(
        load_current_batch_asset_package(&request)
            .expect("current batch load should run")
            .is_none()
    );
    assert!(!output_dir.exists());

    let converted = convert_batch_assets(&request).expect("batch conversion should succeed");
    let current = load_current_batch_asset_package(&request)
        .expect("current batch load should run")
        .expect("current batch package should be returned");
    assert_eq!(current.asset_id, converted.asset_id);
    assert_eq!(current.quality, converted.quality);
    assert_eq!(current.report.input_count(), converted.report.input_count());

    let mut check_request = request.clone();
    check_request.check_only = true;
    assert!(
        load_current_batch_asset_package(&check_request)
            .expect("stale batch load should run")
            .is_none()
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removable");
}

#[test]
fn inspect_asset_package_audits_batch_package_without_request() {
    let temp_dir = unique_temp_dir("asset-inspect-package-batch");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let input_path = temp_dir.join("fixture.CATPart");
    let output_dir = temp_dir.join("batch-asset");
    fs::write(
        &input_path,
        format!("CATPart private payload prefix\n{SAMPLE_CACHE}\nprivate suffix"),
    )
    .expect("fixture should be written");

    let request = BatchAssetConversionRequest::new(vec![input_path.clone()], &output_dir);
    let converted = convert_batch_assets(&request).expect("batch conversion should succeed");
    let audit = inspect_asset_package(&output_dir).expect("package audit should run");
    assert!(audit.usable);
    assert!(audit.previewable());
    assert_eq!(audit.reason, AssetPackageFreshnessReason::Current);
    assert_eq!(audit.kind.as_deref(), Some("batch_conversion"));
    assert_eq!(audit.status.as_deref(), Some("succeeded"));
    assert_eq!(
        audit
            .identity
            .as_ref()
            .expect("identity should exist")
            .asset_id,
        converted.asset_id
    );
    assert_eq!(audit.input_count, 1);
    assert_eq!(
        audit.quality.as_ref().expect("quality should exist"),
        &converted.quality
    );
    let summary = read_asset_package_summary(&output_dir).expect("package summary should read");
    assert!(summary.audit.usable);
    assert_eq!(summary.items.len(), 1);
    assert_eq!(
        summary.output_size_bytes,
        converted.quality.output_size_bytes
    );
    assert_eq!(
        summary.metadata_size_bytes,
        converted.quality.metadata_size_bytes
    );
    let item = &summary.items[0];
    assert!(item.previewable());
    assert_eq!(item.operation, AssetPackageSummaryOperation::Converted);
    assert_eq!(item.source_format.as_deref(), Some("CATIA_CATPart"));
    assert_eq!(item.triangle_count, Some(1));
    assert!(item.output_path.is_some());
    assert!(item.metadata_path.is_some());
    assert_eq!(
        item.metadata
            .as_ref()
            .expect("metadata summary should exist")
            .source_format,
        "CATIA_CATPart"
    );

    let output_path = match &converted.report.items[0].status {
        BatchItemStatus::Ok { output_path, .. } => std::path::PathBuf::from(output_path),
        _ => panic!("batch item should be converted"),
    };
    fs::remove_file(output_path).expect("batch output should be removable");
    let missing_output = inspect_asset_package(&output_dir).expect("package audit should run");
    assert!(!missing_output.usable);
    assert_eq!(
        missing_output.reason,
        AssetPackageFreshnessReason::OutputArtifactMissing
    );

    let check_output_dir = temp_dir.join("check-batch-asset");
    let mut check_request = BatchAssetConversionRequest::new(vec![input_path], &check_output_dir);
    check_request.check_only = true;
    let checked = convert_batch_assets(&check_request).expect("batch check should succeed");
    let check_audit = inspect_asset_package(&check_output_dir).expect("package audit should run");
    assert!(check_audit.usable);
    assert!(!check_audit.previewable());
    assert_eq!(check_audit.reason, AssetPackageFreshnessReason::Current);
    assert_eq!(
        check_audit.quality.as_ref().expect("quality should exist"),
        &checked.quality
    );
    let check_summary =
        read_asset_package_summary(&check_output_dir).expect("package summary should read");
    assert_eq!(check_summary.items.len(), 1);
    assert_eq!(
        check_summary.items[0].operation,
        AssetPackageSummaryOperation::Checked
    );
    assert!(!check_summary.items[0].previewable());
    assert!(check_summary.items[0].output_path.is_none());
    assert!(check_summary.items[0].metadata.is_none());
    assert_eq!(check_summary.items[0].triangle_count, Some(1));

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
    assert!(!changed_mode.asset.quality.previewable);
    assert_eq!(
        changed_mode.asset.quality.preview_status,
        AssetPreviewStatus::NoPreviewOutput
    );
    assert_eq!(changed_mode.asset.quality.checked_count, 1);
    assert_eq!(changed_mode.asset.quality.output_size_bytes, 0);
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
    assert_eq!(
        failure.decision(),
        AssetPreflightDecision::NeedsReadableVisualization
    );
    assert_eq!(
        failure.action(),
        AssetFailureAction::ProvideReadableVisualization
    );
    assert!(package.source_info_path.is_file());
    assert!(package.diagnostics_path.is_file());
    let diagnostics = parse_json(&fs::read_to_string(&package.diagnostics_path).unwrap());
    assert_eq!(diagnostics["status"], "failed");
    assert_eq!(
        diagnostics["failure"]["category"],
        "no_readable_lightweight_cache"
    );
    assert_eq!(
        diagnostics["failure_decision"],
        "needs_readable_visualization"
    );
    assert_eq!(
        diagnostics["failure_action"],
        "provide_readable_visualization"
    );
    assert_eq!(
        diagnostics["failure_required_condition"],
        "readable lightweight visualization payload"
    );
    let audit = inspect_asset_package(&output_dir).expect("package audit should run");
    assert!(!audit.usable);
    assert_eq!(audit.reason, AssetPackageFreshnessReason::DiagnosticsFailed);
    assert_eq!(audit.status.as_deref(), Some("failed"));
    assert_eq!(
        audit
            .failure
            .as_ref()
            .expect("failure should be included")
            .action(),
        AssetFailureAction::ProvideReadableVisualization
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
