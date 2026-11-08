//! `hwp styles` — named-style listing with resolved attributes and the paragraphs that use each.

use std::path::Path;

use hwp_cli::cli::StylesFormat;
use hwp_cli::styles_report::{list, styles_json, styles_text};

use crate::commands::cat::load_document;

pub fn run(file: &Path, format: StylesFormat) -> anyhow::Result<()> {
    let doc = load_document(file)?;
    let styles = list(&doc)?;
    match format {
        StylesFormat::Text => print!("{}", styles_text(&styles)),
        StylesFormat::Json => println!("{}", serde_json::to_string_pretty(&styles_json(&styles))?),
    }
    Ok(())
}
