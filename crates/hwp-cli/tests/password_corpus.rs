//! Owner-controlled genuine password corpus coverage.

#[path = "common/password_corpus.rs"]
mod password_corpus;

#[test]
fn password_corpus_is_cleanly_skipped_without_a_manifest() {
    let receipt_dir = std::env::temp_dir().join(format!(
        "hwp-cli-password-corpus-skip-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&receipt_dir);
    std::fs::create_dir_all(&receipt_dir).unwrap();
    assert_eq!(
        password_corpus::run_password_corpus(None, Some(receipt_dir.clone())).unwrap(),
        password_corpus::CorpusRun::Skipped
    );
    assert!(std::fs::read_dir(&receipt_dir).unwrap().next().is_none());
    let _ = std::fs::remove_dir_all(receipt_dir);
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

#[cfg(unix)]
#[test]
fn receipt_directory_symlink_into_repository_is_rejected() {
    use std::os::unix::fs::symlink;

    let repository = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let inside = repository
        .join("target")
        .join(format!("password-receipts-{}", std::process::id()));
    let outside = std::env::temp_dir().join(format!(
        "hwp-cli-password-receipt-link-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&outside);
    let _ = std::fs::remove_dir_all(&inside);
    std::fs::create_dir_all(&inside).unwrap();
    symlink(&inside, &outside).unwrap();

    assert!(password_corpus::require_external_receipt_dir(&outside, &repository).is_err());

    std::fs::remove_file(outside).unwrap();
    std::fs::remove_dir_all(inside).unwrap();
}

#[cfg(unix)]
#[test]
fn corpus_source_symlink_into_repository_is_rejected() {
    use std::os::unix::fs::symlink;

    let repository = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let outside = std::env::temp_dir().join(format!(
        "hwp-cli-password-source-link-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&outside);
    symlink(repository.join("Cargo.toml"), &outside).unwrap();

    assert!(password_corpus::require_external_corpus_source(&outside, &repository).is_err());

    std::fs::remove_file(outside).unwrap();
}

#[test]
fn genuine_owner_password_corpus() {
    password_corpus::run_password_corpus_from_env().unwrap();
}

#[test]
#[ignore = "release-only revalidation after the genuine receipt run"]
fn validate_existing_complete_run() {
    password_corpus::validate_existing_complete_run_from_env().unwrap();
}
