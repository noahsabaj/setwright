use serde_json::{Value, json};
use setwright_lib::core::{
    ArxivPreflightReportV1, LatexEngine, PaperSettingsV1, ReviewBundleV1, RuntimeManifestV1,
    TemplateId, validate_review_bundle,
};
use std::path::PathBuf;

fn example(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("schemas")
            .join("examples")
            .join(name),
    )
    .unwrap()
}

#[test]
fn canonical_v1_examples_deserialize_into_the_exact_named_contracts() {
    let settings: PaperSettingsV1 =
        serde_json::from_slice(&example("paper-settings.v1.json")).unwrap();
    assert!(settings.is_valid());
    assert_eq!(settings.engine, LatexEngine::PdfLatex);
    assert_eq!(settings.template_id, TemplateId::GenericArticle);

    let review: ReviewBundleV1 = serde_json::from_slice(&example("review-bundle.v1.json")).unwrap();
    validate_review_bundle(&review).unwrap();
    assert_eq!(review.base_files[0].path, "main.tex");

    let runtime: RuntimeManifestV1 =
        serde_json::from_slice(&example("runtime-manifest.v1.example.json")).unwrap();
    assert_eq!(runtime.tex_live_snapshot.version, 2025);
    assert_eq!(
        runtime.engines,
        [LatexEngine::PdfLatex, LatexEngine::XeLatex]
    );

    let preflight: ArxivPreflightReportV1 =
        serde_json::from_slice(&example("arxiv-preflight-report.v1.json")).unwrap();
    assert!(!preflight.ready);
    assert!(preflight.readiness_is_consistent());
}

#[test]
fn canonical_contracts_reject_unknown_properties() {
    let mut settings: Value = serde_json::from_slice(&example("paper-settings.v1.json")).unwrap();
    settings["unexpected"] = json!(true);
    assert!(serde_json::from_value::<PaperSettingsV1>(settings).is_err());

    let mut review: Value = serde_json::from_slice(&example("review-bundle.v1.json")).unwrap();
    review["baseFiles"][0]["unexpected"] = json!(true);
    assert!(serde_json::from_value::<ReviewBundleV1>(review).is_err());

    let mut runtime: Value =
        serde_json::from_slice(&example("runtime-manifest.v1.example.json")).unwrap();
    runtime["archive"]["unexpected"] = json!(true);
    assert!(serde_json::from_value::<RuntimeManifestV1>(runtime).is_err());

    let mut preflight: Value =
        serde_json::from_slice(&example("arxiv-preflight-report.v1.json")).unwrap();
    preflight["source"]["unexpected"] = json!(true);
    assert!(serde_json::from_value::<ArxivPreflightReportV1>(preflight).is_err());
}

#[test]
fn engine_and_template_wire_values_are_schema_values() {
    assert_eq!(
        serde_json::to_value(LatexEngine::PdfLatex).unwrap(),
        "pdflatex"
    );
    assert_eq!(
        serde_json::to_value(LatexEngine::XeLatex).unwrap(),
        "xelatex"
    );
    assert_eq!(
        serde_json::to_value(TemplateId::GenericArticle).unwrap(),
        "generic-article"
    );
    assert_eq!(
        serde_json::to_value(TemplateId::AcmAcMart).unwrap(),
        "acm-acmart"
    );
    assert_eq!(
        serde_json::to_value(TemplateId::IeeeTran).unwrap(),
        "ieee-ieeetran"
    );
}
