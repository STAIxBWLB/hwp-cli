//! `hwp render` raster format integration tests (EDT-03).
//!
//! Drives the built binary over the one committed sample and covers what the non-PNG raster
//! formats actually promise: page dimensions equal to the PNG render at the same dpi and format
//! inference from the output extension. Nothing here asserts font-dependent output (glyphs, page
//! counts) - the expected page count is derived from the PNG run, so the file is CI-safe on a host
//! with no bundled fonts.

use std::path::{Path, PathBuf};
use std::process::Command;

const DPI: &str = "96";

fn sample() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx")
}

/// A fresh, empty output directory under the crate's target dir.
fn out_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/render-raster-formats")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create output dir");
    dir
}

/// Render the sample into `dir/out.<ext>`, returning the page files in page order.
fn render(dir: &Path, ext: &str, format: Option<&str>) -> Vec<PathBuf> {
    let output = dir.join(format!("out.{ext}"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hwp"));
    cmd.arg("render").args(["--dpi", DPI]);
    if let Some(format) = format {
        cmd.args(["--format", format]);
    }
    let done = cmd
        .arg("-o")
        .arg(&output)
        .arg(sample())
        .output()
        .expect("run hwp render");
    assert!(
        done.status.success(),
        "hwp render --format {format:?} -o {}: {}",
        output.display(),
        String::from_utf8_lossy(&done.stderr)
    );
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read output dir")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some(ext))
        .collect();
    // out-1.ext, out-2.ext, ... sort lexically in page order for the page counts we render here;
    // a single-page render writes out.ext with no number.
    files.sort();
    assert!(!files.is_empty(), "no {ext} page written");
    files
}

fn decode(path: &Path) -> image::RgbaImage {
    image::open(path)
        .unwrap_or_else(|error| panic!("decode {}: {error}", path.display()))
        .to_rgba8()
}

#[test]
fn jpeg_pages_match_the_png_render_dimensions() {
    let png = render(&out_dir("jpeg-dims-png"), "png", Some("png"));
    let jpeg = render(&out_dir("jpeg-dims-jpeg"), "jpg", Some("jpeg"));
    assert_eq!(
        png.len(),
        jpeg.len(),
        "jpeg must write one file per page the png render wrote"
    );
    for (png, jpeg) in png.iter().zip(&jpeg) {
        let (png, jpeg) = (decode(png), decode(jpeg));
        // Dimensions only: JPEG is lossy, so pixel equality is not a claim this format supports.
        assert_eq!(
            png.dimensions(),
            jpeg.dimensions(),
            "jpeg page dimensions must equal the png page dimensions at the same dpi"
        );
    }
}

#[test]
fn output_extension_infers_the_format() {
    // Without inference the `_ => RenderFormat::Png` catch-all would write PNG bytes under these
    // names, so the magic bytes are the assertion that matters.
    for (ext, magic) in [("jpg", &b"\xFF\xD8"[..]), ("jpeg", &b"\xFF\xD8"[..])] {
        let files = render(&out_dir(&format!("infer-{ext}")), ext, None);
        for file in &files {
            let bytes = std::fs::read(file).expect("read rendered page");
            assert!(
                bytes.starts_with(magic),
                "{}: expected {ext} magic bytes, got {:02X?}",
                file.display(),
                &bytes[..magic.len().min(bytes.len())]
            );
        }
    }
}
