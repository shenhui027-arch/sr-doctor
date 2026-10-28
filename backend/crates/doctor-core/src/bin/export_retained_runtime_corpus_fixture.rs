use std::{env, fs, path::PathBuf};

use doctor_contracts::RequestedSourceKind;
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, assemble_public_report,
    resolve_version_and_graph, v01_analysis_receipt,
};
use sha2::{Digest, Sha256};

const RAW_PROFILE: &str = include_str!(
    "../../../../../fixtures/runtime/3.5.14/docker_runtime_corpus_complex.profile.txt"
);
const RAW_PROFILE_SHA256: &str = "9538497E592E029E4439F3272A0D05FB81FBE682D488CCD703B3B97D92D1D2CE";

fn rendered_report() -> String {
    let actual_hash = format!("{:X}", Sha256::digest(RAW_PROFILE.as_bytes()));
    assert_eq!(
        actual_hash, RAW_PROFILE_SHA256,
        "retained runtime source SHA-256 drift"
    );

    let context = AdapterContext::new(v01_analysis_receipt()).expect("receipt context");
    let mut output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, RAW_PROFILE.as_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .expect("retained runtime corpus adapts");
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).expect("retained runtime corpus resolves");
    let report = assemble_public_report(output).expect("retained runtime corpus report assembles");

    serde_json::to_string_pretty(&report).expect("report serializes") + "\n"
}

fn target_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .and_then(|path| path.parent())
        .expect("workspace root")
        .join("fixtures/expected/3.5.14/docker_runtime_corpus_complex.report.json")
}

fn main() {
    let rendered = rendered_report();
    let target = target_path();

    if env::args().any(|argument| argument == "--check") {
        let current = fs::read_to_string(&target).expect("checked-in retained runtime report");
        assert_eq!(current, rendered, "retained runtime report fixture drift");
    } else {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("fixture output directory");
        }
        fs::write(&target, rendered).expect("write retained runtime report fixture");
    }

    eprintln!(
        "retained runtime corpus report: {} ({RAW_PROFILE_SHA256})",
        target.display()
    );
}
