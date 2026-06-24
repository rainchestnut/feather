use feather_lite::{
    FORMAT_CAPABILITIES_CONTRACT_VERSION, FileFormat, format_capabilities,
    format_capabilities_json, format_capability,
};

#[test]
fn format_capabilities_expose_machine_readable_contracts() {
    assert_eq!(
        FileFormat::from_label("CATIA_CATPart"),
        Some(FileFormat::CatiaCatPart)
    );
    assert_eq!(FileFormat::from_label("not-a-format"), None);

    let catpart = format_capability(FileFormat::CatiaCatPart)
        .expect("CATPart capability should be published");
    assert!(!catpart.is_available());
    assert_eq!(catpart.status, "partial");
    assert!(catpart.requires_visual_payload);
    assert!(catpart.supports_embedded_assets);
    assert!(!catpart.supports_external_references);
    assert!(!catpart.supports_native_tessellation);
    assert_eq!(catpart.native_brep_tessellation, "not_decoded");

    let catproduct = format_capability(FileFormat::CatiaCatProduct)
        .expect("CATProduct capability should be published");
    assert!(catproduct.supports_external_references);

    let step = format_capability(FileFormat::Step).expect("STEP capability should be published");
    assert_eq!(step.status, "partial");
    assert!(!step.requires_visual_payload);
    assert!(step.supports_embedded_assets);
    assert!(step.supports_native_tessellation);
    assert_eq!(step.native_brep_tessellation, "partial");

    assert_eq!(format_capabilities().len(), 13);
    assert!(format_capability(FileFormat::Unknown).is_none());
}

#[test]
fn format_capabilities_json_is_valid_and_stable() {
    let json = format_capabilities_json();
    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("capabilities JSON should be valid");
    assert_eq!(
        parsed["contract_version"],
        FORMAT_CAPABILITIES_CONTRACT_VERSION
    );
    let formats = parsed["formats"]
        .as_array()
        .expect("formats should be an array");

    let catpart = formats
        .iter()
        .find(|format| format["format"] == "CATIA_CATPart")
        .expect("CATPart JSON capability should exist");
    assert_eq!(
        catpart["extensions"],
        serde_json::json!([".CATPart", ".CADPart"])
    );
    assert_eq!(catpart["available"], false);
    assert_eq!(catpart["status"], "partial");
    assert_eq!(catpart["requires_visual_payload"], true);
    assert_eq!(catpart["supports_external_references"], false);

    let step = formats
        .iter()
        .find(|format| format["format"] == "STEP")
        .expect("STEP JSON capability should exist");
    assert_eq!(step["status"], "partial");
    assert_eq!(step["native_brep_tessellation"], "partial");
    assert_eq!(step["supports_native_tessellation"], true);
}
