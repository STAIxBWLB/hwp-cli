//! Offline JSON Schema validator used by the structured corpus source gate.

use std::io::Read as _;
use std::path::Path;

const MAX_JSON_BYTES: u64 = 2 * 1024 * 1024;

fn read_json(path: &Path) -> anyhow::Result<serde_json::Value> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > MAX_JSON_BYTES
    {
        anyhow::bail!("schema input rejected")
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        anyhow::bail!("schema input rejected")
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn main() -> anyhow::Result<()> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() != 6 {
        anyhow::bail!("expected exactly three schema/document path pairs")
    }
    for pair in arguments.chunks_exact(2) {
        let schema = read_json(Path::new(&pair[0]))?;
        let document = read_json(Path::new(&pair[1]))?;
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&schema)?;
        if !validator.is_valid(&document) {
            anyhow::bail!("structured corpus JSON does not satisfy its closed schema")
        }
    }
    println!("structured corpus schemas: valid");
    Ok(())
}
