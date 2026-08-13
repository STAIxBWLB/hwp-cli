//! 골든(기준) 렌더 대조 테스트 — 한글이 내보낸 기준 PNG와 픽셀·위치 오차 비교.
//!
//! `HWP_GOLDEN=1`일 때만 실행한다(폰트 가용성·기준 이미지에 의존하므로 기본 CI는
//! `tests/render.rs`의 구조 스모크만). 기준 이미지 만드는 법은 `fixtures/golden/README.md`.
//!
//! 실행: `HWP_FONT_DIR=$PWD/fonts HWP_GOLDEN=1 cargo test -p hwp-render golden -- --nocapture`

use std::path::{Path, PathBuf};

use hwp_render::{RenderOptions, render_document};

/// DPI used to produce reference images; this must match the README guidance.
const GOLDEN_DPI: f32 = 150.0;
/// Temporary upper bound that each parity step is expected to tighten.
/// PR 7 (GG-3/4/18/20 advance batch and re-baseline) tightens 0.60 to 0.30.
/// No Hancom reference image is committed yet, so the first scoreboard must validate or adjust it.
const MAX_BAD_PIXEL_PCT: f32 = 0.30;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn font_dirs() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os("HWP_FONT_DIR") {
        vec![PathBuf::from(dir)]
    } else {
        vec![workspace_root().join("fonts")]
    }
}

fn load_doc(path: &Path) -> Option<hwp_model::Document> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "hwp" => Some(hwp5::read_document(path).ok()?.document),
        "hwpx" => Some(hwpx::read_document(path).ok()?.document),
        _ => None,
    }
}

/// `<name>.p<N>.ref.png` → (name, page).
fn parse_ref_name(file: &str) -> Option<(String, usize)> {
    let stem = file.strip_suffix(".ref.png")?;
    let (name, ppage) = stem.rsplit_once(".p")?;
    Some((name.to_string(), ppage.parse().ok()?))
}

fn find_fixture(root: &Path, name: &str) -> Option<PathBuf> {
    [
        root.join("fixtures/hwp5").join(format!("{name}.hwp")),
        root.join("fixtures/hwpx").join(format!("{name}.hwpx")),
    ]
    .into_iter()
    .find(|cand| cand.exists())
}

#[test]
fn 골든_기준_대조() {
    if std::env::var_os("HWP_GOLDEN").is_none() {
        eprintln!("golden 스킵 (HWP_GOLDEN 미설정)");
        return;
    }
    let root = workspace_root();
    let golden_dir = root.join("fixtures/golden");
    let mut checked = 0;

    let entries = match std::fs::read_dir(&golden_dir) {
        Ok(e) => e,
        Err(_) => {
            eprintln!("golden 디렉터리 없음 — 스킵");
            return;
        }
    };
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some((name, page)) = parse_ref_name(&file) else {
            continue;
        };
        let Some(fixture) = find_fixture(&root, &name) else {
            eprintln!("기준 {file}: fixture '{name}' 없음 — 스킵");
            continue;
        };
        let doc =
            load_doc(&fixture).unwrap_or_else(|| panic!("문서 읽기 실패: {}", fixture.display()));
        let out = render_document(
            &doc,
            &RenderOptions {
                dpi: GOLDEN_DPI,
                font_dirs: font_dirs(),
            },
        )
        .expect("렌더 실패");
        assert!(page >= 1 && page <= out.pages.len(), "{file}: 페이지 범위");

        let reference = hwp_render::load_png(&entry.path()).expect("기준 PNG 로드");
        let (rep, _) = hwp_render::compare(&out.pages[page - 1], &reference, 16)
            .unwrap_or_else(|e| panic!("{file}: {e}"));
        eprintln!(
            "골든 {file}: bad={:.2}% mae={:.1} dx={} dy={}",
            rep.bad_pixel_pct * 100.0,
            rep.mae,
            rep.dx,
            rep.dy
        );
        assert!(
            rep.bad_pixel_pct < MAX_BAD_PIXEL_PCT,
            "{file}: 픽셀 차이율 {:.2}% > 상한 {:.0}%",
            rep.bad_pixel_pct * 100.0,
            MAX_BAD_PIXEL_PCT * 100.0
        );
        checked += 1;
    }
    eprintln!("골든 대조 {checked}건 완료");
}
