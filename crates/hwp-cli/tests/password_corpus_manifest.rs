//! Synthetic contract coverage for the private Phase 8 password corpus manifest.
//!
//! Genuine document paths and credentials remain outside the repository. This
//! suite exercises only opaque identifiers and synthetic absolute paths.

#[path = "common/password_corpus_manifest.rs"]
mod password_corpus_manifest;

use password_corpus_manifest::{
    discovery_would_write_evidence, load_manifest_for_test, manifest_fixture, repo_root,
};

#[test]
fn manifest_contract_accepts_a_redacted_synthetic_matrix() {
    let manifest = manifest_fixture();
    let descriptors = load_manifest_for_test(&manifest, &repo_root(), |reference| {
        Some(format!("secret-for-{reference}"))
    })
    .expect("the synthetic manifest should satisfy the closed contract");

    assert_eq!(descriptors.len(), 3);
    assert!(descriptors.iter().all(|entry| entry.source_path.is_none()));
    assert!(descriptors.iter().all(|entry| entry.credential.is_some()));
}

#[test]
fn manifest_contract_rejects_closed_schema_and_role_violations() {
    for (label, manifest) in password_corpus_manifest::invalid_manifest_cases() {
        assert!(
            load_manifest_for_test(&manifest, &repo_root(), |_| Some("secret".to_owned())).is_err(),
            "{label} must fail closed"
        );
    }
}

#[test]
fn manifest_contract_without_a_manifest_does_not_publish_evidence() {
    assert!(
        !discovery_would_write_evidence(None),
        "missing owner configuration must stop before evidence publication"
    );
}
