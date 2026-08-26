//! Owner-controlled genuine password corpus coverage.

#[path = "common/password_corpus.rs"]
mod password_corpus;

#[test]
fn password_corpus_is_cleanly_skipped_without_a_manifest() {
    assert_eq!(
        password_corpus::run_password_corpus(None, None).unwrap(),
        password_corpus::CorpusRun::Skipped
    );
}

#[test]
fn present_manifest_requires_an_empty_external_receipt_directory() {
    let root = std::env::temp_dir().join(format!("hwp-cli-password-corpus-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("manifest")).unwrap();
    std::fs::create_dir_all(root.join("receipts")).unwrap();
    let manifest = root.join("manifest/manifest.json");
    let receipt_dir = root.join("receipts");
    std::fs::write(&manifest, "{}").unwrap();
    std::fs::write(receipt_dir.join("old.json"), "{}").unwrap();
    let result = password_corpus::run_password_corpus(Some(manifest), Some(receipt_dir));
    let _ = std::fs::remove_dir_all(root);
    assert!(result.is_err());
}

#[test]
fn genuine_owner_password_corpus() {
    password_corpus::run_password_corpus_from_env().unwrap();
}
