//! Reverse resolver for addressed edit-ops targets (EDT-05, Phase 7 plan 07-01).
//!
//! This is the reverse of [`crate::markdown`]'s forward walk: that module descends the IR
//! assigning each paragraph/table/generic-control a [`SegmentPath`] as it emits markdown; this
//! module takes an [`Address`] (an `id` checksum-plus-path or a raw `at` coordinate) and walks
//! back down to the IR value it names.
//!
//! The checksum an `id` address carries is **drift detection, not tamper evidence** — the same
//! statement `segment_id.rs`'s module doc makes about the id itself. A mismatch means the
//! caller's view of the document is stale (something moved since it read the id), not that the
//! content was maliciously altered; a truncated hash over public content is not a MAC, and
//! anyone who can write an ops file can recompute it.
//!
//! `at` addresses a **top-level paragraph only** (`{"section": 0, "paragraph": 3}` names
//! `doc.sections[0].paragraphs[3]`, with an optional `run` for run granularity). A paragraph
//! nested inside a table cell or a generic control has no `at` spelling — its path already
//! carries the full index chain, so it is addressed by `id`.

use hwp_model::control::GenericControl;
use hwp_model::{Control, Document, Paragraph};

use crate::segment_id::{SegmentPath, canonical_char_shape_runs, paragraph_id, run_id};

/// Maximum index-chain depth (T-07-03): documents nest far shallower than this, and the
/// resolver iterates rather than recurses over the chain, so this bounds parse-time and
/// walk-time work, not stack depth. The `id` string parser in `edit_ops.rs` enforces the same
/// cap before an `Address` is ever built.
pub const MAX_ADDRESS_INDICES: usize = 32;

/// An address into a document: either a checksummed `id` path or a raw `at` coordinate, with an
/// optional WCHAR sub-range narrowing a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// Section index in `Document::sections`.
    pub section: usize,
    /// The chain below the section, matching [`SegmentPath::indices`]. `indices[0]` is the
    /// top-level paragraph. At [`Granularity::Run`] the LAST entry is the run index (mirroring
    /// `segment_id.rs`'s `run_id` id shape, which appends the run index after the paragraph's
    /// own path); at [`Granularity::Paragraph`] every entry is a nesting step down to the
    /// paragraph itself.
    pub indices: Vec<usize>,
    /// Present for an `id` address (drift detection); absent for an `at` address, which carries
    /// no checksum to check.
    pub checksum: Option<String>,
    /// WCHAR `[start, end)` narrowing a run, in the source paragraph's own coordinate space
    /// (absolute UTF-16 code units — A1). Only meaningful at `Granularity::Run`; `None` there
    /// means the whole run (D-01).
    pub chars: Option<(u32, u32)>,
}

/// The addressing depth an op declares it accepts. A paragraph-level op rejects a run-range
/// address; the schema enforces this shape-wise via `paragraphAddress`/`runAddress` (D-11), and
/// `resolve` enforces it against the document's own structure (a run index or `chars` makes no
/// sense without a run).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    Paragraph,
    Run,
}

/// What an address resolved to, once `resolve` has walked the document and validated it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetKind {
    Paragraph,
    Run {
        /// Index into `canonical_char_shape_runs(paragraph)` — the run the address names.
        index: usize,
        /// The resolved WCHAR range to act on: the full run when the address carried no
        /// `chars`, or the validated sub-range when it did.
        w_start: u32,
        w_end: u32,
    },
}

/// The resolved target: the paragraph's own path (never including a trailing run index — see
/// [`Address::indices`]), what kind of target it is, and the id at the target *before* any edit
/// applies (for staleness re-derivation and, later, the EDT-06 report).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub path: SegmentPath,
    pub kind: TargetKind,
    pub before_id: String,
}

/// Why an address failed to resolve. Every message names only the address and, for a checksum
/// mismatch, the two checksums — never document text (T-07-04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// No paragraph, table cell, or generic-control paragraph exists at the given path.
    NoSuchPath,
    /// The path resolved to a paragraph, but it has no run at the given index.
    NoSuchRun,
    /// The `id` checksum no longer matches the content at the resolved path (D-03).
    StaleChecksum { expected: String, found: String },
    /// `chars` is not a sub-range of the named run's own `[start, end)` boundary (A5).
    CharsOutOfRange {
        requested: (u32, u32),
        run_bounds: (u32, u32),
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchPath => write!(f, "주소가 가리키는 위치를 문서에서 찾을 수 없습니다"),
            Self::NoSuchRun => write!(f, "주소가 가리키는 run이 문단에 없습니다"),
            Self::StaleChecksum { expected, found } => {
                write!(
                    f,
                    "체크섬이 일치하지 않습니다 (기대 {expected}, 실제 {found})"
                )
            }
            Self::CharsOutOfRange {
                requested: (rs, re),
                run_bounds: (bs, be),
            } => write!(
                f,
                "chars 범위 [{rs}, {re})가 run 경계 [{bs}, {be})를 벗어났습니다"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Resolves `addr` against `doc` at the declared `want` granularity: walks the section and
/// paragraph chain (never slicing a `Vec` directly — every step is `.get()`), then either
/// derives the paragraph id or locates the named run via [`canonical_char_shape_runs`] and
/// derives the run id. When `addr.checksum` is `Some`, the derived id's checksum prefix must
/// match it (D-03). When `want` is [`Granularity::Run`] and `addr.chars` is `Some`, the range
/// must be a sub-range of the run's own boundary (A5); `None` means the whole run (D-01).
pub fn resolve(
    doc: &Document,
    addr: &Address,
    want: Granularity,
) -> Result<ResolvedTarget, ResolveError> {
    if addr.indices.is_empty() {
        return Err(ResolveError::NoSuchPath);
    }
    let para_depth = match want {
        Granularity::Paragraph => addr.indices.len(),
        Granularity::Run => addr.indices.len().saturating_sub(1),
    };
    if para_depth == 0 {
        return Err(ResolveError::NoSuchPath);
    }
    let para_indices = &addr.indices[..para_depth];
    let paragraph = descend(doc, addr.section, para_indices)?;
    let path = SegmentPath {
        section: addr.section,
        indices: para_indices.to_vec(),
    };

    match want {
        Granularity::Paragraph => {
            let found = paragraph_id(&path, paragraph);
            check_checksum(&addr.checksum, &found)?;
            Ok(ResolvedTarget {
                path,
                kind: TargetKind::Paragraph,
                before_id: found,
            })
        }
        Granularity::Run => {
            let run_index = addr.indices[addr.indices.len() - 1];
            let runs = canonical_char_shape_runs(paragraph);
            let Some(&(start, _)) = runs.get(run_index) else {
                return Err(ResolveError::NoSuchRun);
            };
            let end = runs
                .get(run_index + 1)
                .map_or_else(|| paragraph.wchar_len(), |&(next, _)| next);
            let found = run_id(&path, paragraph, run_index);
            check_checksum(&addr.checksum, &found)?;
            let (w_start, w_end) = match addr.chars {
                Some((s, e)) => {
                    if s < start || e > end || s >= e {
                        return Err(ResolveError::CharsOutOfRange {
                            requested: (s, e),
                            run_bounds: (start, end),
                        });
                    }
                    (s, e)
                }
                None => (start, end),
            };
            Ok(ResolvedTarget {
                path,
                kind: TargetKind::Run {
                    index: run_index,
                    w_start,
                    w_end,
                },
                before_id: found,
            })
        }
    }
}

fn check_checksum(expected: &Option<String>, found_id: &str) -> Result<(), ResolveError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let found = found_id.split('.').next().unwrap_or("");
    if found != expected {
        return Err(ResolveError::StaleChecksum {
            expected: expected.clone(),
            found: found.to_string(),
        });
    }
    Ok(())
}

/// Walks from `section`/`indices[0]` (the top-level paragraph) through `indices[1..]`, matching
/// the control kind each depth implies — `Control::Table` steps into its cell list,
/// `Control::Generic` (with no `raw_children`) into its paragraph lists — mirroring
/// `structure::walk_para_lists`'s descent and its skip of `raw_children`-backed objects (whose
/// inner IR edits the hwp5 writer would discard anyway).
fn descend<'d>(
    doc: &'d Document,
    section: usize,
    indices: &[usize],
) -> Result<&'d Paragraph, ResolveError> {
    let section_ref = doc.sections.get(section).ok_or(ResolveError::NoSuchPath)?;
    let mut para = section_ref
        .paragraphs
        .get(indices[0])
        .ok_or(ResolveError::NoSuchPath)?;
    let mut i = 1;
    while i < indices.len() {
        let control = para
            .controls
            .get(indices[i])
            .ok_or(ResolveError::NoSuchPath)?;
        match control {
            Control::Table(table) => {
                let cell_index = *indices.get(i + 1).ok_or(ResolveError::NoSuchPath)?;
                let cell = table
                    .cells
                    .get(cell_index)
                    .ok_or(ResolveError::NoSuchPath)?;
                let p_index = *indices.get(i + 2).ok_or(ResolveError::NoSuchPath)?;
                para = cell
                    .paragraphs
                    .get(p_index)
                    .ok_or(ResolveError::NoSuchPath)?;
                i += 3;
            }
            Control::Generic(generic) if generic.raw_children.is_empty() => {
                let seq = *indices.get(i + 1).ok_or(ResolveError::NoSuchPath)?;
                para = flat_paragraph(generic, seq).ok_or(ResolveError::NoSuchPath)?;
                i += 2;
            }
            _ => return Err(ResolveError::NoSuchPath),
        }
    }
    Ok(para)
}

/// The paragraph at flat index `seq` across `control.paragraph_lists`, concatenated in order —
/// mirroring how `markdown.rs`'s forward walk numbers a generic control's paragraphs (one
/// sequential index space across every `ParagraphList`, not per-list).
fn flat_paragraph(control: &GenericControl, seq: usize) -> Option<&Paragraph> {
    let mut remaining = seq;
    for list in &control.paragraph_lists {
        if remaining < list.paragraphs.len() {
            return Some(&list.paragraphs[remaining]);
        }
        remaining -= list.paragraphs.len();
    }
    None
}

fn flat_paragraph_mut(control: &mut GenericControl, seq: usize) -> Option<&mut Paragraph> {
    let mut remaining = seq;
    for list in &mut control.paragraph_lists {
        if remaining < list.paragraphs.len() {
            return Some(&mut list.paragraphs[remaining]);
        }
        remaining -= list.paragraphs.len();
    }
    None
}

/// The `&mut` counterpart of `descend`, for apply-time mutation. `path` always names a
/// paragraph (never a trailing run index — see [`Address::indices`] / [`ResolvedTarget::path`]).
/// Returns `None` under the same conditions `resolve` would return [`ResolveError::NoSuchPath`];
/// a caller that already resolved successfully should not normally see `None` here unless the
/// document changed shape since (an earlier op in the same batch), which is exactly what an
/// address-driven apply function must re-check before mutating (planner decision 5).
pub fn paragraph_at_mut<'d>(
    doc: &'d mut Document,
    path: &SegmentPath,
) -> Option<&'d mut Paragraph> {
    let section = doc.sections.get_mut(path.section)?;
    let indices = &path.indices;
    if indices.is_empty() {
        return None;
    }
    let mut para = section.paragraphs.get_mut(indices[0])?;
    let mut i = 1;
    while i < indices.len() {
        let control = para.controls.get_mut(indices[i])?;
        match control {
            Control::Table(table) => {
                let cell_index = *indices.get(i + 1)?;
                let cell = table.cells.get_mut(cell_index)?;
                let p_index = *indices.get(i + 2)?;
                para = cell.paragraphs.get_mut(p_index)?;
                i += 3;
            }
            Control::Generic(generic) if generic.raw_children.is_empty() => {
                let seq = *indices.get(i + 1)?;
                para = flat_paragraph_mut(generic, seq)?;
                i += 2;
            }
            _ => return None,
        }
    }
    Some(para)
}

/// Clears `hwpx_raw_xml` on every `Control::Generic` ancestor along `path`, mirroring the
/// `if inner { g.hwpx_raw_xml = None; }` tail `structure.rs`'s and `format.rs`'s own per-kind
/// recursive walks (`restyle_para`, `align_para`, `props_para`) already apply whenever a
/// descendant paragraph changes. A pure top-level-paragraph path (no ancestors) is a no-op.
pub(crate) fn invalidate_ancestors(doc: &mut Document, path: &SegmentPath) {
    let Some(section) = doc.sections.get_mut(path.section) else {
        return;
    };
    let indices = &path.indices;
    if indices.is_empty() {
        return;
    }
    let Some(mut para) = section.paragraphs.get_mut(indices[0]) else {
        return;
    };
    let mut i = 1;
    while i < indices.len() {
        let Some(control) = para.controls.get_mut(indices[i]) else {
            return;
        };
        match control {
            Control::Table(table) => {
                let Some(&cell_index) = indices.get(i + 1) else {
                    return;
                };
                let Some(cell) = table.cells.get_mut(cell_index) else {
                    return;
                };
                let Some(&p_index) = indices.get(i + 2) else {
                    return;
                };
                let Some(next) = cell.paragraphs.get_mut(p_index) else {
                    return;
                };
                para = next;
                i += 3;
            }
            Control::Generic(generic) => {
                generic.hwpx_raw_xml = None;
                if !generic.raw_children.is_empty() {
                    return;
                }
                let Some(&seq) = indices.get(i + 1) else {
                    return;
                };
                let Some(next) = flat_paragraph_mut(generic, seq) else {
                    return;
                };
                para = next;
                i += 2;
            }
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_markdown::from_markdown;

    fn sample_doc() -> Document {
        from_markdown("첫 문단\n\n둘째 문단\n\nplain **bold** tail\n")
    }

    #[test]
    fn resolves_a_known_paragraph_path() {
        let doc = sample_doc();
        let addr = Address {
            section: 0,
            indices: vec![0],
            checksum: None,
            chars: None,
        };
        let resolved = resolve(&doc, &addr, Granularity::Paragraph).unwrap();
        assert_eq!(resolved.kind, TargetKind::Paragraph);
        assert_eq!(
            resolved.path,
            SegmentPath {
                section: 0,
                indices: vec![0]
            }
        );
    }

    #[test]
    fn rejects_a_wrong_checksum() {
        let doc = sample_doc();
        let addr = Address {
            section: 0,
            indices: vec![0],
            checksum: Some("0000000000000000".to_string()),
            chars: None,
        };
        let err = resolve(&doc, &addr, Granularity::Paragraph).unwrap_err();
        assert!(matches!(err, ResolveError::StaleChecksum { .. }));
    }

    #[test]
    fn accepts_the_correct_checksum() {
        let doc = sample_doc();
        let path = SegmentPath {
            section: 0,
            indices: vec![0],
        };
        let checksum = paragraph_id(&path, &doc.sections[0].paragraphs[0])
            .split('.')
            .next()
            .unwrap()
            .to_string();
        let addr = Address {
            section: 0,
            indices: vec![0],
            checksum: Some(checksum),
            chars: None,
        };
        assert!(resolve(&doc, &addr, Granularity::Paragraph).is_ok());
    }

    #[test]
    fn rejects_an_out_of_bounds_index() {
        let doc = sample_doc();
        let addr = Address {
            section: 0,
            indices: vec![99],
            checksum: None,
            chars: None,
        };
        let err = resolve(&doc, &addr, Granularity::Paragraph).unwrap_err();
        assert_eq!(err, ResolveError::NoSuchPath);
    }

    #[test]
    fn resolves_a_run_with_no_chars_to_the_whole_run() {
        let doc = sample_doc();
        let addr = Address {
            section: 0,
            indices: vec![2, 1],
            checksum: None,
            chars: None,
        };
        let resolved = resolve(&doc, &addr, Granularity::Run).unwrap();
        let runs = canonical_char_shape_runs(&doc.sections[0].paragraphs[2]);
        assert_eq!(
            resolved.kind,
            TargetKind::Run {
                index: 1,
                w_start: runs[1].0,
                w_end: runs[2].0,
            }
        );
    }

    #[test]
    fn rejects_chars_outside_the_named_runs_boundary() {
        let doc = sample_doc();
        let runs = canonical_char_shape_runs(&doc.sections[0].paragraphs[2]);
        let addr = Address {
            section: 0,
            indices: vec![2, 1],
            checksum: None,
            chars: Some((runs[1].0, runs[2].0 + 1)),
        };
        let err = resolve(&doc, &addr, Granularity::Run).unwrap_err();
        assert!(matches!(err, ResolveError::CharsOutOfRange { .. }));
    }

    #[test]
    fn rejects_a_run_index_past_the_paragraphs_run_count() {
        let doc = sample_doc();
        let addr = Address {
            section: 0,
            indices: vec![0, 99],
            checksum: None,
            chars: None,
        };
        let err = resolve(&doc, &addr, Granularity::Run).unwrap_err();
        assert_eq!(err, ResolveError::NoSuchRun);
    }
}
