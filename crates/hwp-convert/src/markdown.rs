//! IR → GFM markdown.
//!
//! Mapping rules:
//! - Outline-style paragraphs ("개요 N") become `#` repeated N times.
//! - Bold/italic character shapes become `**`/`*` spans; strikeout becomes `~~`;
//!   underline/superscript/subscript become `<u>`/`<sup>`/`<sub>` from char_shape_runs.
//! - Hyperlinks (%hlk fields) become `[label](URL)`.
//! - Pictures become `![image]()` or extracted relative references when media_dir is set.
//! - Tables become GFM tables with the first row as the header. Merged cells or nested tables
//!   require HTML `<table>` with colspan/rowspan and inline tags to preserve content.
//! - Bullet/number paragraphs become `- ` / `N. ` lists. Numbered heads always export the
//!   item's ordinal in digit form ("3."), since GFM ordered markers are digits
//!   only — non-digit engine marks ("가.", "㉮") must not harden into literal
//!   text on the md round trip (#134).
//! - Footnotes/endnotes become inline `[^N]`/`[^eN]` markers and trailing GFM definitions.
//! - Equations (eqed) become inline `$script$` or block `$$script$$`, preserving HWP script.
//! - Line breaks (10) become hard breaks; tabs become spaces.

use std::fs::OpenOptions;
use std::io::{Error, ErrorKind, Write};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use hwp_model::list::ListState;
use hwp_model::{
    Cell, CharShape, Control, Document, Equation, GenericControl, HwpChar, Paragraph, Table,
    TextOptions, ctrl_char,
};

use crate::segment::{
    RawSeg, RunControlKind, Segment, SegmentKind, SegmentStyle, always_has_para_segment,
    char_shape_id_at, summarize,
};
use crate::segment_id::{
    SegmentPath, canonical_char_shape_runs, cell_id, control_id, paragraph_id, picture_id, run_id,
    table_id,
};

/// Markdown output options.
#[derive(Default)]
pub struct MarkdownOptions<'a> {
    /// Directory for extracted image binaries. With `Some`, images are written there as
    /// `image1.png`, etc., and referenced as `![image](prefix/image1.png)`. The directory is
    /// created lazily for the first image, so an image-free document creates no directory.
    /// With `None`, the existing empty `![image]()` references are retained.
    pub media_dir: Option<&'a Path>,
    /// Image reference prefix. `None` uses the media_dir directory name, preserving existing behavior.
    /// Set this to keep a user-provided relative path such as CLI `--media-dir figs` in links.
    pub media_prefix: Option<&'a str>,
    /// Text extraction options: headers/footers and hidden comments are excluded by default.
    pub text: TextOptions,
}

/// Source IR coordinates for the character range `[start, end)` in Markdown output.
///
/// Offsets count **Unicode scalars (characters)**, not bytes, matching Python `str` indexing.
/// Consumers can slice `md[start:end]` directly. The section/paragraph indices refer to the
/// IR (`--format json`) and remain stable across decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownSegment {
    /// Index into `doc.sections[]`.
    pub section: usize,
    /// Index into `sections[section].paragraphs[]` (top-level paragraphs).
    pub para: usize,
    /// Inclusive character offset.
    pub start: usize,
    /// Exclusive character offset.
    pub end: usize,
}

/// Serializes the complete IR to GFM Markdown; retains the existing signature without image extraction.
pub fn to_markdown(doc: &Document) -> String {
    // Without media_dir there is no I/O, so this cannot fail.
    to_markdown_with(doc, &MarkdownOptions::default())
        .expect("media_dir 미지정 시 IO가 없어 실패할 수 없다")
}

/// Option-taking variant. Setting media_dir extracts images; extraction I/O failures return `Err`.
///
/// Wrapper that discards the segment map. It shares the emission core ([`emit_markdown`])
/// with [`to_markdown_with_segments`], so Markdown is structurally byte-identical whether
/// or not segments are requested.
pub fn to_markdown_with(doc: &Document, opts: &MarkdownOptions) -> std::io::Result<String> {
    Ok(emit_markdown(doc, opts, false)?.0)
}

/// Emits the same Markdown as [`to_markdown_with`] and returns [`MarkdownSegment`] entries
/// that attribute each output character range to its source paragraph.
///
/// Segments are ordered by start and do not overlap; unattributed output (blank lines or
/// section separators) may leave gaps. Table lines inherit their containing paragraph index;
/// footnote/endnote definitions inherit their referring paragraph. Headers/footers need opts.text.
pub fn to_markdown_with_segments(
    doc: &Document,
    opts: &MarkdownOptions,
) -> std::io::Result<(String, Vec<MarkdownSegment>)> {
    let (markdown, segments, _) = emit_markdown(doc, opts, false)?;
    Ok((markdown, segments))
}

/// Markdown emission core, producing both the string and segment map.
///
/// Instrumentation is always enabled: a few raw-segment Vec pushes have negligible overhead.
/// Emission records out.len() before and after each paragraph in section/paragraph order.
/// Trailing footnote/endnote definitions retain the paragraph coordinates recorded on collection.
/// [`cleanup_with_map`] remaps deleted bytes, then byte offsets are converted to character offsets.
pub(crate) fn emit_markdown(
    doc: &Document,
    opts: &MarkdownOptions,
    v2: bool,
) -> std::io::Result<(String, Vec<MarkdownSegment>, Vec<Segment>)> {
    let mut ctx = Ctx {
        media_dir: opts.media_dir,
        dir_name: opts
            .media_prefix
            .map(|p| p.replace('\\', "/").trim_end_matches('/').to_string())
            .or_else(|| {
                opts.media_dir
                    .and_then(|d| d.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_default(),
        img_no: 0,
        pending_media: Vec::new(),
        include_header_footer: opts.text.include_header_footer,
        include_hidden: opts.text.include_hidden,
        html_mode: false,
        last_was_list: false,
        list_widths: Vec::new(),
        list_keys: Vec::new(),
        notes: Vec::new(),
        foot_n: 0,
        end_n: 0,
        cur_section: 0,
        cur_para: 0,
        v2,
        path: SegmentPath::default(),
        ctrl_index: 0,
        ctrl_style: SegmentStyle::default(),
    };

    let mut out = Body::default();
    // Raw segments: (section, paragraph, byte_start, byte_end), in emission/start order.
    let mut raw: Vec<(usize, usize, usize, usize)> = Vec::new();

    for (section_index, section) in doc.sections.iter().enumerate() {
        if section_index > 0 {
            break_section_list(&mut ctx, &mut out);
        }
        // Reset list numbering per section, matching the renderer.
        let mut list_state = ListState::default();
        for (para_index, para) in section.paragraphs.iter().enumerate() {
            // Update the paragraph coordinates assigned to collected footnotes/endnotes.
            ctx.cur_section = section_index;
            ctx.cur_para = para_index;
            let path = SegmentPath {
                section: section_index,
                indices: vec![para_index],
            };
            if v2 {
                ctx.path = path.clone();
            }
            let start = out.len();
            let own = render_paragraph(doc, para, &mut list_state, &mut ctx, &mut out);
            if out.len() > start {
                raw.push((section_index, para_index, start, out.len()));
            }
            if v2 {
                // A paragraph that emitted nothing of its own - only the blank line closing
                // the list block above it - starts where that separator ends, so it gets no
                // range (the separator belongs to no segment) and at most a point.
                let start = if own { start } else { out.len() };
                if let Some(seg) = para_segment(doc, para, path, start, out.len()) {
                    out.segs.push(seg);
                }
            }
        }
    }
    // Collect footnote/endnote definitions at the document end. Collection follows document order,
    // so their byte ranges naturally follow the body segments in increasing order.
    if !ctx.notes.is_empty() {
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        for note in &ctx.notes {
            let start = out.len();
            emit_note(note, &mut out);
            if out.len() > start {
                raw.push((note.src.0, note.src.1, start, out.len()));
            }
        }
    }
    ctx.persist_media()?;

    // Remap deleted bytes through cleanup, then convert segment offsets to character units.
    let (cleaned, deletions) = cleanup_with_map(&out.s);
    let segments = build_segments(&cleaned, &deletions, &raw);
    let v2_segments = if v2 {
        finalize_v2(&cleaned, &deletions, out.segs)
    } else {
        Vec::new()
    };
    Ok((cleaned, segments, v2_segments))
}

/// A markdown buffer together with the v2 spans recorded into it.
///
/// Spans are byte offsets **into this buffer**. When the buffer's text is copied into a parent
/// buffer - an inline fragment into the output, a block through [`append_block`], a cell into its
/// row - [`Body::absorb`] carries the spans over, so one recording idiom serves every nesting
/// level and no second emitter is needed.
#[derive(Default)]
struct Body {
    s: String,
    segs: Vec<RawSeg>,
    /// The character-shape run being emitted, closed when the next one opens.
    open_run: Option<RawSeg>,
    /// Field spans opened at `FIELD_START` and closed at `FIELD_END`.
    open_fields: Vec<RawSeg>,
    /// The source-paragraph WCHAR offset of the character being emitted, kept by the walk so a
    /// run piece can close at the right source offset wherever the flush happens. One cursor,
    /// assigned once per character, so the segment path and the text path cannot drift.
    source_pos: u32,
    /// A control boundary can precede the Text-driven emphasis transition.
    source_is_control: bool,
}

impl Deref for Body {
    type Target = String;
    fn deref(&self) -> &String {
        &self.s
    }
}

impl DerefMut for Body {
    fn deref_mut(&mut self) -> &mut String {
        &mut self.s
    }
}

impl From<String> for Body {
    fn from(s: String) -> Self {
        Body {
            s,
            ..Default::default()
        }
    }
}

impl Body {
    /// Rebases `segs` from a child buffer onto this one.
    ///
    /// `[lo, hi)` is the child range that actually reached the output - the rest was trimmed, and
    /// an offset outside it clamps to the edge. `anchors` are ascending `(child offset, offset in
    /// this buffer)` pairs, one per contiguous copied run: a single pair for a plain append, one
    /// per line when the copy inserts an indent.
    fn absorb(&mut self, segs: Vec<RawSeg>, lo: usize, hi: usize, anchors: &[(usize, usize)]) {
        if segs.is_empty() || anchors.is_empty() {
            return;
        }
        let map = |off: usize| -> usize {
            let off = off.clamp(lo, hi);
            let i = anchors.partition_point(|&(src, _)| src <= off);
            let (src, dst) = anchors[i.saturating_sub(1)];
            dst + off.saturating_sub(src)
        };
        for mut seg in segs {
            seg.start = map(seg.start);
            seg.end = map(seg.end).max(seg.start);
            // A control whose projected character was trimmed away did not reach this buffer.
            seg.controls = seg
                .controls
                .into_iter()
                .filter(|&(_, off, _)| off >= lo && off < hi)
                .map(|(kind, off, source)| (kind, map(off), source))
                .collect();
            self.segs.push(seg);
        }
    }

    /// Appends `text` verbatim and carries `segs`, whose offsets start at `lo` in the child.
    fn absorb_append(&mut self, text: &str, segs: Vec<RawSeg>, lo: usize) {
        let base = self.s.len();
        self.s.push_str(text);
        self.absorb(segs, lo, lo + text.len(), &[(lo, base)]);
    }

    /// Records a control the walk just projected as the one character ending at the cursor
    /// (#543), on the run piece that is open. Outside v2 there is no open run, so this is free.
    fn note_control(&mut self, kind: RunControlKind, source: u32) {
        let at = self.s.len();
        if let Some(run) = self.open_run.as_mut() {
            // The projection is one ASCII byte, so the character starts one byte back.
            run.controls.push((kind, at - 1, source));
        }
    }

    /// Carries recorded controls and run boundaries through emphasis edits in this buffer.
    fn shift_offsets(&mut self, moves: &[ByteMove]) {
        // A span can outlive a run whose marks matched the next one, so runs already closed
        // into `segs` follow the move too.
        for run in self
            .segs
            .iter_mut()
            .filter(|run| run.kind == SegmentKind::Run)
        {
            for movement in moves {
                if run.start_follows_deletions {
                    run.start = movement.boundary(run.start);
                }
                if run.end_follows_deletions {
                    run.end = movement.boundary(run.end);
                }
            }
        }
        if let Some(run) = self.open_run.as_mut() {
            for movement in moves {
                if run.start_follows_deletions {
                    run.start = movement.boundary(run.start);
                }
            }
            // Its end is an unset sentinel until close_run_at records the current cursor.
        }
        for run in self.open_run.iter_mut().chain(self.segs.iter_mut()) {
            for (_, off, _) in run.controls.iter_mut() {
                for m in moves {
                    *off = m.apply(*off);
                }
            }
        }
    }

    /// Closes the open run at the cursor.
    fn close_run(&mut self) {
        self.close_run_at(self.s.len());
    }

    /// Closes the open run at `at`, which the run boundary logic places between the previous
    /// run's closing emphasis marker and the next one's opening marker.
    ///
    /// The piece's source span ends at the character the walk is on, so a flush triggered BY a
    /// block control closes the piece before that control's own characters: the control is not
    /// this piece's source text, and the next piece resumes after it.
    fn close_run_at(&mut self, at: usize) {
        if let Some(mut seg) = self.open_run.take() {
            seg.end = at.max(seg.start);
            seg.end_follows_deletions = self.source_is_control;
            if let Some((source_start, _)) = seg.source {
                seg.source = Some((source_start, self.source_pos));
            }
            self.segs.push(seg);
        }
    }

    /// Closes the innermost open field at the cursor.
    fn close_field(&mut self) {
        if let Some(mut seg) = self.open_fields.pop() {
            seg.end = self.s.len();
            self.segs.push(seg);
        }
    }

    /// Closes everything still open, because a flush resets the offsets they were recorded in.
    fn close_open(&mut self) {
        self.close_run();
        while !self.open_fields.is_empty() {
            self.close_field();
        }
    }
}

/// The child path one index below `path` - the positional half of a nested segment's id.
fn child(path: &SegmentPath, index: usize) -> SegmentPath {
    let mut indices = path.indices.clone();
    indices.push(index);
    SegmentPath {
        section: path.section,
        indices,
    }
}

/// Carries the v2 spans through the same cleanup remap and byte-to-scalar conversion
/// [`build_segments`] performs for v1, so both vectors index the same string the same way.
fn finalize_v2(cleaned: &str, deletions: &[(usize, usize)], raw: Vec<RawSeg>) -> Vec<Segment> {
    let map = DeletionMap::new(deletions);
    let clen = cleaned.len();
    let at = |off: usize| floor_char_boundary(cleaned, map.remap(off).min(clen));
    let mut bounds: Vec<usize> = raw
        .iter()
        .flat_map(|s| {
            [at(s.start), at(s.end)]
                .into_iter()
                .chain(s.controls.iter().map(|&(_, off, _)| at(off)))
        })
        .collect();
    bounds.sort_unstable();
    bounds.dedup();
    let char_bounds = bytes_to_chars(cleaned, &bounds);
    // A control's projected character survives only if the cleanup did not delete its byte.
    let survives = |off: usize| map.remap(off) != map.remap(off + 1);
    crate::segment::finalize(
        raw,
        |off| {
            let byte = at(off);
            char_bounds[bounds
                .binary_search(&byte)
                .expect("경계는 bounds에 반드시 존재")]
        },
        survives,
    )
}

/// Emits one footnote/endnote definition at the document end in GFM footnote syntax.
/// Definitions are always `[^N]:` even for notes referenced inside HTML tables — the
/// importer (from_markdown) parses them via ENABLE_FOOTNOTES and reattaches them to
/// fnref markers (#47).
fn emit_note(note: &Note, out: &mut String) {
    let mut lines = note.text.lines();
    match lines.next() {
        Some(first) => {
            out.push_str(&format!("[^{}]: {first}\n", note.label));
            // Continuation lines get 4-space indentation (GFM footnote continuation rule).
            for l in lines {
                out.push_str(&format!("    {l}\n"));
            }
        }
        None => out.push_str(&format!("[^{}]:\n", note.label)),
    }
}

struct PendingMedia {
    path: PathBuf,
    data: Vec<u8>,
}

struct Note {
    label: String,
    text: String,
    /// Top-level paragraph referring to this footnote/endnote: (section index, paragraph index).
    src: (usize, usize),
}

/// Rendering state: image extraction, text policy, lists/notes, and HTML-table mode.
struct Ctx<'a> {
    media_dir: Option<&'a Path>,
    /// Reference prefix (media_prefix or directory name).
    dir_name: String,
    /// Next image number (one-based).
    img_no: usize,
    /// Images to write after rendering and collision checks.
    pending_media: Vec<PendingMedia>,
    include_header_footer: bool,
    include_hidden: bool,
    /// Inside an HTML table, emit marks/links/images as HTML tags because block HTML disables Markdown.
    html_mode: bool,
    /// Whether the preceding output was a list item, for a blank line when the list ends.
    last_was_list: bool,
    /// Content-column width and (head kind, definition ID) for each active list level.
    list_widths: Vec<usize>,
    list_keys: Vec<(u8, u16)>,
    /// Footnotes/endnotes for trailing definitions.
    notes: Vec<Note>,
    foot_n: u32,
    end_n: u32,
    /// Current top-level paragraph for segment attribution, updated by the main loop.
    cur_section: usize,
    cur_para: usize,
    /// v2 span recording. Off for the default path, which therefore keeps paying nothing beyond
    /// the v1 `Vec` pushes it already carried.
    v2: bool,
    /// The id path of the paragraph being rendered; saved and restored around every recursion.
    path: SegmentPath,
    /// The index in `Paragraph::controls` of the control [`render_control`] is rendering.
    ctrl_index: usize,
    /// The two style levels at that control's character position, computed by the walk that has
    /// the containing paragraph in hand.
    ctrl_style: SegmentStyle,
}

impl Ctx<'_> {
    /// Queues Picture bytes for writing and builds the Markdown image reference.
    fn image_ref(&mut self, data: &[u8]) -> String {
        let html = self.html_mode;
        let fallback = || {
            if html {
                "<!-- image -->".to_string()
            } else {
                "![image]()".to_string()
            }
        };
        let Some(dir) = self.media_dir else {
            return fallback();
        };
        self.img_no += 1;
        let (ext, _) = crate::image::image_kind(data);
        let file = format!("image{}.{ext}", self.img_no);
        self.pending_media.push(PendingMedia {
            path: dir.join(&file),
            data: data.to_vec(),
        });
        let reference = if self.dir_name.is_empty() {
            file
        } else {
            format!("{}/{}", self.dir_name, file)
        };
        if html {
            format!(
                "<img src=\"{}\" alt=\"image\">",
                escape_html_attr(&reference)
            )
        } else {
            format!("![image]({})", md_link_dest(&reference))
        }
    }

    /// Reuse an existing file only when its bytes match. Check all collisions before writing new
    /// files with create_new; on failure remove only files created by this call.
    fn persist_media(&self) -> std::io::Result<()> {
        if self.pending_media.is_empty() {
            return Ok(());
        }
        for item in &self.pending_media {
            match std::fs::read(&item.path) {
                Ok(existing) if existing == item.data => {}
                Ok(_) => return Err(media_collision(&item.path)),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }

        let dir = self.media_dir.expect("대기 이미지가 있으면 media_dir 존재");
        std::fs::create_dir_all(dir)?;
        let mut created = Vec::new();
        for item in &self.pending_media {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&item.path)
            {
                Ok(mut file) => {
                    created.push(item.path.clone());
                    if let Err(e) = file.write_all(&item.data) {
                        rollback_media(&created);
                        return Err(e);
                    }
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => match std::fs::read(&item.path) {
                    Ok(existing) if existing == item.data => {}
                    Ok(_) => {
                        rollback_media(&created);
                        return Err(media_collision(&item.path));
                    }
                    Err(read_error) => {
                        rollback_media(&created);
                        return Err(read_error);
                    }
                },
                Err(e) => {
                    rollback_media(&created);
                    return Err(e);
                }
            }
        }
        Ok(())
    }
}

fn media_collision(path: &Path) -> Error {
    Error::new(
        ErrorKind::AlreadyExists,
        format!("기존 미디어 파일을 덮어쓸 수 없습니다: {}", path.display()),
    )
}

fn rollback_media(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

/// Collapses excessive blank lines to one and returns the cleaned string with **deleted ranges**:
/// (start, len) in original out bytes, ordered by start and disjoint.
///
/// Cleanup deletes only **whole blank lines**, so each deletion is a contiguous original byte range.
/// If the last line lacks a newline, cleanup appends one; this insertion cannot shift earlier
/// offsets and needs no mapping entry. Generated Markdown contains no carriage returns, so splitting
/// uses only newlines, matching str::lines().
fn cleanup_with_map(out: &str) -> (String, Vec<(usize, usize)>) {
    let mut cleaned = String::with_capacity(out.len());
    let mut deletions: Vec<(usize, usize)> = Vec::new();
    let mut blank_run = 0usize;
    let bytes = out.as_bytes();
    let mut i = 0usize;
    while i < out.len() {
        // Current line content [i, line_end), and the next line start including the newline.
        let (line_end, next) = match bytes[i..].iter().position(|&b| b == b'\n') {
            Some(p) => (i + p, i + p + 1),  // Advance past the newline.
            None => (out.len(), out.len()), // Final line without a newline.
        };
        let line = &out[i..line_end];
        let keep = if line.trim().is_empty() {
            blank_run += 1;
            blank_run <= 1
        } else {
            blank_run = 0;
            true
        };
        if keep {
            cleaned.push_str(line);
            cleaned.push('\n');
        } else {
            // Delete the whole line, including its newline: original range [i, next).
            deletions.push((i, next - i));
        }
        i = next;
    }
    (cleaned, deletions)
}

/// Wraps deleted ranges in an original-to-cleaned offset map using binary search, O(log D).
struct DeletionMap {
    /// (Deletion start, exclusive end, cumulative bytes deleted before this range).
    entries: Vec<(usize, usize, usize)>,
}

impl DeletionMap {
    fn new(deletions: &[(usize, usize)]) -> Self {
        let mut entries = Vec::with_capacity(deletions.len());
        let mut cum = 0usize;
        for &(start, len) in deletions {
            entries.push((start, start + len, cum));
            cum += len;
        }
        Self { entries }
    }

    /// Maps original byte offset old to cleaned bytes. Offsets inside a deletion collapse to its
    /// start, so a deleted line naturally converges on the collapse point.
    fn remap(&self, old: usize) -> usize {
        // Only the last range starting before old can affect it; ranges are ordered and disjoint.
        let idx = self.entries.partition_point(|&(start, _, _)| start < old);
        if idx == 0 {
            return old;
        }
        let (start, end, before) = self.entries[idx - 1];
        let removed = if end <= old {
            before + (end - start) // This entire deletion precedes old.
        } else {
            before + (old - start) // old lies inside this deletion and collapses to its start.
        };
        old - removed
    }
}

/// Converts raw original byte spans to final character spans in the cleaned string.
///
/// First remap/clamp bytes through cleanup; then trim trailing whitespace at character boundaries
/// and discard empty ranges. Convert the remaining boundaries to character offsets in one pass.
/// Line boundaries should already be character boundaries; defensively round down if needed.
/// Debug assertions check ordered, disjoint, in-bounds output ranges.
fn build_segments(
    cleaned: &str,
    deletions: &[(usize, usize)],
    raw: &[(usize, usize, usize, usize)],
) -> Vec<MarkdownSegment> {
    let map = DeletionMap::new(deletions);
    let clen = cleaned.len();

    // 1) Remap, clamp, round down to a character boundary, and trim trailing whitespace.
    let mut byte_segs: Vec<(usize, usize, usize, usize)> = Vec::with_capacity(raw.len());
    for &(section, para, s, e) in raw {
        let ns = floor_char_boundary(cleaned, map.remap(s).min(clen));
        let ne0 = floor_char_boundary(cleaned, map.remap(e).min(clen));
        if ne0 <= ns {
            continue;
        }
        // Trim trailing whitespace/newlines for clean quotations; discard whitespace-only spans.
        let trimmed = cleaned[ns..ne0].trim_end();
        let ne = ns + trimmed.len();
        if ne <= ns {
            continue;
        }
        byte_segs.push((section, para, ns, ne));
    }

    // 2) Sort and deduplicate byte boundaries, then convert them to character offsets together.
    let mut bounds: Vec<usize> = Vec::with_capacity(byte_segs.len() * 2);
    for &(_, _, s, e) in &byte_segs {
        bounds.push(s);
        bounds.push(e);
    }
    bounds.sort_unstable();
    bounds.dedup();
    let char_bounds = bytes_to_chars(cleaned, &bounds);
    let char_at = |b: usize| -> usize {
        let idx = bounds
            .binary_search(&b)
            .expect("경계는 bounds에 반드시 존재");
        char_bounds[idx]
    };

    // 3) Assemble character segments.
    let segments: Vec<MarkdownSegment> = byte_segs
        .iter()
        .map(|&(section, para, s, e)| MarkdownSegment {
            section,
            para,
            start: char_at(s),
            end: char_at(e),
        })
        .collect();

    debug_assert!(
        segments.windows(2).all(|w| w[0].start <= w[1].start),
        "세그먼트는 start 오름차순이어야 함"
    );
    debug_assert!(
        segments.windows(2).all(|w| w[0].end <= w[1].start),
        "세그먼트는 비중첩이어야 함"
    );
    debug_assert!(
        segments
            .iter()
            .all(|seg| seg.start < seg.end && seg.end <= cleaned.chars().count()),
        "세그먼트는 범위 내(0 < 폭, end <= 문자수)여야 함"
    );
    segments
}

/// Nearest character boundary at or before i (replacement for the then-unstable std method).
fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Converts sorted unique byte boundaries to character offsets in one char_indices pass.
/// Returns character offsets in the input order.
fn bytes_to_chars(s: &str, bounds: &[usize]) -> Vec<usize> {
    let mut result = vec![0usize; bounds.len()];
    let mut bi = 0usize;
    let mut char_idx = 0usize;
    for (byte_off, _) in s.char_indices() {
        while bi < bounds.len() && bounds[bi] == byte_off {
            result[bi] = char_idx;
            bi += 1;
        }
        char_idx += 1;
    }
    // Remaining boundaries at s.len() map to the total character count.
    while bi < bounds.len() {
        result[bi] = char_idx;
        bi += 1;
    }
    result
}

/// Inline marks open bold/italic/strike/underline/sup/sub and close in reverse order.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
struct Marks {
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    sup: bool,
    sub: bool,
}

impl Marks {
    fn from_shape(shape: Option<&CharShape>) -> Self {
        shape.map_or(Self::default(), |s| Self {
            bold: s.is_bold(),
            italic: s.is_italic(),
            strike: s.has_strike(),
            underline: s.has_underline(),
            sup: s.is_superscript(),
            sub: s.is_subscript(),
        })
    }

    fn open(self, html: bool) -> String {
        let mut s = String::new();
        if self.bold {
            s.push_str(if html { "<strong>" } else { "**" });
        }
        if self.italic {
            s.push_str(if html { "<em>" } else { "*" });
        }
        if self.strike {
            s.push_str(if html { "<s>" } else { "~~" });
        }
        if self.underline {
            s.push_str("<u>");
        }
        if self.sup {
            s.push_str("<sup>");
        }
        if self.sub {
            s.push_str("<sub>");
        }
        s
    }

    fn close(self, html: bool) -> String {
        let mut s = String::new();
        if self.sub {
            s.push_str("</sub>");
        }
        if self.sup {
            s.push_str("</sup>");
        }
        if self.underline {
            s.push_str("</u>");
        }
        if self.strike {
            s.push_str(if html { "</s>" } else { "~~" });
        }
        if self.italic {
            s.push_str(if html { "</em>" } else { "*" });
        }
        if self.bold {
            s.push_str(if html { "</strong>" } else { "**" });
        }
        s
    }
}

/// Body indices for an open emphasis span: open_at starts its opening marker; content_at follows
/// the marker. They change only when opening the span and drive whitespace relocation on close.
#[derive(Default, Clone, Copy)]
struct Span {
    open_at: usize,
    content_at: usize,
}

/// An actual string edit made by [`close_span_moves`], in execution order.
#[derive(Clone, Copy)]
enum ByteMove {
    Remove {
        start: usize,
        end: usize,
    },
    Insert {
        at: usize,
        len: usize,
    },
    Rotate {
        start: usize,
        middle: usize,
        end: usize,
    },
}

impl ByteMove {
    /// Where an existing byte sits after the edit; controls never occupy removed markers.
    fn apply(self, off: usize) -> usize {
        match self {
            Self::Remove { start, end } => off - off.saturating_sub(start).min(end - start),
            Self::Insert { at, len } => off + if off >= at { len } else { 0 },
            Self::Rotate { start, middle, end } if (middle..end).contains(&off) => {
                start + off - middle
            }
            Self::Rotate { start, middle, end } if (start..middle).contains(&off) => {
                off + end - middle
            }
            Self::Rotate { .. } => off,
        }
    }

    /// Run cuts follow deletions, including cuts at old EOF. Insertion and rotation affect
    /// only whitespace and generated delimiters, so retaining cuts inside those regions cannot
    /// transfer visible non-whitespace text. This affinity preserves existing valid ranges;
    /// surviving control characters constrain their pieces separately during reconciliation.
    fn boundary(self, off: usize) -> usize {
        match self {
            Self::Remove { .. } => self.apply(off),
            Self::Insert { .. } | Self::Rotate { .. } => off,
        }
    }
}

/// Closes an emphasis span and carries recorded run cuts and control anchors through its edits.
fn close_body_span(body: &mut Body, marks: Marks, span: Span, html: bool) {
    let moves = close_span_moves(&mut body.s, marks, span, html);
    body.shift_offsets(&moves);
}

/// Closes every open mark and resets them (a link boundary, a line break), keeping the
/// recorded offsets in step. A later text character reopens what it needs.
fn close_body_marks(body: &mut Body, marks: &mut Marks, span: Span, html: bool) {
    close_body_span(body, *marks, span, html);
    *marks = Marks::default();
}

/// Closes an emphasis span while respecting GFM delimiter rules.
///
/// GFM does not parse `**`/`*`/`~~` as emphasis if whitespace precedes the closing delimiter or
/// follows the opening delimiter; the marker would appear literally.
/// Move leading/trailing whitespace outside bold/italic/strike markers to preserve parsing.
/// A whitespace-only span emits no markers, avoiding `** **`.
/// HTML mode and HTML-only effects such as underline/scripts do not depend on whitespace and
/// close normally. Empty marks after a reset require no action.
///
/// Returns edits affecting recorded offsets, in execution order.
fn close_span_moves(body: &mut String, marks: Marks, span: Span, html: bool) -> Vec<ByteMove> {
    let mut moves = Vec::new();
    if marks == Marks::default() {
        return moves;
    }
    // Without GFM delimiters (HTML mode or underline/scripts only), no whitespace relocation is needed.
    let needs_fix = !html && (marks.bold || marks.italic || marks.strike);
    if !needs_fix {
        body.push_str(&marks.close(html));
        return moves;
    }
    // Trim by character, preserving boundaries for multibyte whitespace such as U+3000.
    let content = &body[span.content_at..];
    let lead_len = content.len() - content.trim_start().len();
    let kept_len = content.trim_end().len();
    if kept_len == 0 {
        // Whitespace-only content: remove opening markers and retain just the whitespace.
        moves.push(ByteMove::Remove {
            start: span.open_at,
            end: span.content_at,
        });
        body.replace_range(span.open_at..span.content_at, "");
        return moves;
    }
    // Detach trailing whitespace, close the trimmed content, then restore the whitespace.
    let trail = body[span.content_at + kept_len..].to_string();
    body.truncate(span.content_at + kept_len);
    let close = marks.close(html);
    body.push_str(&close);
    body.push_str(&trail);
    if !trail.is_empty() {
        moves.push(ByteMove::Insert {
            at: span.content_at + kept_len,
            len: close.len(),
        });
    }
    // Move leading whitespace before the opening markers.
    if lead_len > 0 {
        let lead = body[span.content_at..span.content_at + lead_len].to_string();
        body.replace_range(span.content_at..span.content_at + lead_len, "");
        body.insert_str(span.open_at, &lead);
        moves.push(ByteMove::Rotate {
            start: span.open_at,
            middle: span.content_at,
            end: span.content_at + lead_len,
        });
    }
    moves
}

/// Returns whether the paragraph emitted content of its own, as opposed to nothing or only a
/// list-block separator.
fn render_paragraph(
    doc: &Document,
    para: &Paragraph,
    list_state: &mut ListState,
    ctx: &mut Ctx,
    out: &mut Body,
) -> bool {
    // Update list counters for every paragraph in document order, including empty ones, as rendering does.
    let marker = list_state.marker(doc, para);
    // Outline style becomes a heading.
    let heading = doc
        .header
        .styles
        .get(para.style.0 as usize)
        .and_then(|s| s.name.strip_prefix("개요 "))
        .and_then(|n| n.trim().parse::<usize>().ok())
        .filter(|n| (1..=6).contains(n));

    let fragments = render_fragments(doc, para, ctx);
    let own = fragments.iter().any(Fragment::emits);

    if let Some(level) = heading {
        close_list_block(ctx, &mut out.s);
        emit_paragraph_fragments(fragments, out, Some(&format!("{} ", "#".repeat(level))));
        return own;
    }

    if marker.is_some() {
        let (ty, level, definition_id) = list_head(doc, para).unwrap_or((3, 1, 0));
        let number = list_state.current_number(doc, para);
        emit_list_fragments(fragments, ty, level, definition_id, number, ctx, out);
        return own;
    }

    close_list_block(ctx, &mut out.s);
    emit_paragraph_fragments(fragments, out, None);
    own
}

/// Intermediate inline/block fragments in document order. Each carries v2 spans in its local
/// string coordinates, remapped when copied into the output.
enum Fragment {
    Inline(Body),
    Block(Body),
}

impl Fragment {
    /// Whether the emitters copy any of this fragment to the output: they skip an inline
    /// fragment that trims to nothing, and a block that is only newlines.
    fn emits(&self) -> bool {
        match self {
            Fragment::Inline(body) => !body.s.trim().is_empty(),
            Fragment::Block(body) => !body.s.trim_matches('\n').is_empty(),
        }
    }
}

fn emit_paragraph_fragments(fragments: Vec<Fragment>, out: &mut Body, prefix: Option<&str>) {
    let mut prefix = prefix;
    for fragment in fragments {
        match fragment {
            Fragment::Inline(body) => {
                let Body { s, segs, .. } = body;
                let lo = s.len() - s.trim_start().len();
                let text = s.trim();
                if text.is_empty() {
                    // Even without text to move, retain point segments at the current cursor.
                    out.absorb_append("", segs, lo);
                    continue;
                }
                if let Some(p) = prefix.take() {
                    out.s.push_str(p);
                }
                out.absorb_append(text, segs, lo);
                out.s.push_str("\n\n");
            }
            Fragment::Block(block) => append_block(out, block, 0),
        }
    }
}

fn append_block(out: &mut Body, block: Body, indent: usize) {
    let Body { s, segs, .. } = block;
    let lo = s.len() - s.trim_start_matches('\n').len();
    let text = s.trim_matches('\n');
    if text.is_empty() {
        return;
    }
    if !out.s.is_empty() && !out.s.ends_with("\n\n") {
        out.s.push('\n');
    }
    let pad = " ".repeat(indent);
    let mut anchors = Vec::new();
    let mut src = lo;
    for line in text.lines() {
        if !line.is_empty() {
            out.s.push_str(&pad);
        }
        anchors.push((src, out.s.len()));
        out.s.push_str(line);
        out.s.push('\n');
        src += line.len() + 1;
    }
    out.s.push('\n');
    out.absorb(segs, lo, lo + text.len(), &anchors);
}

fn emit_list_fragments(
    fragments: Vec<Fragment>,
    ty: u8,
    level: u8,
    definition_id: u16,
    number: Option<u64>,
    ctx: &mut Ctx,
    out: &mut Body,
) {
    // GFM ordered markers are digits only, so a numbered head always exports
    // the item's ordinal in digit form ("3."), whatever its engine mark
    // ("가.", "1)", "(가)", "㉮", …): the importer rebuilds the same numbered
    // head from it, while carrying the mark as literal text would harden it
    // into the body on every round trip (#134). Ordinals past the CommonMark
    // 9-digit marker limit degrade to a plain bullet.
    let gfm_marker = if ty == 3 {
        "-".to_string()
    } else {
        number
            .filter(|n| *n <= 999_999_999)
            .map_or_else(|| "-".to_string(), |n| format!("{n}."))
    };
    let indent = prepare_list_level(ctx, ty, level, definition_id, &gfm_marker, &mut out.s);
    let first_prefix = format!("{}{gfm_marker} ", " ".repeat(indent));
    let continuation = " ".repeat(indent + gfm_marker.chars().count() + 1);
    let mut emitted = false;

    for fragment in fragments {
        match fragment {
            Fragment::Inline(body) => {
                let Body { s, segs, .. } = body;
                let lo = s.len() - s.trim_start().len();
                let text = s.trim();
                if text.is_empty() {
                    out.absorb_append("", segs, lo);
                    continue;
                }
                let mut anchors = Vec::new();
                if emitted {
                    out.s.push('\n');
                    push_indented_lines(out, &continuation, &continuation, text, lo, &mut anchors);
                } else {
                    push_indented_lines(out, &first_prefix, &continuation, text, lo, &mut anchors);
                    emitted = true;
                }
                out.absorb(segs, lo, lo + text.len(), &anchors);
            }
            Fragment::Block(block) => {
                if !emitted {
                    out.s.push_str(&first_prefix);
                    out.s.push('\n');
                    emitted = true;
                }
                append_block(out, block, continuation.len());
            }
        }
    }
    if emitted && !out.s.ends_with('\n') {
        out.s.push('\n');
    }
    ctx.last_was_list = emitted;
}

/// Copies lines with prefixes, recording (source, destination) offsets at each line start in anchors.
fn push_indented_lines(
    out: &mut Body,
    first: &str,
    continuation: &str,
    text: &str,
    mut src: usize,
    anchors: &mut Vec<(usize, usize)>,
) {
    for (i, line) in text.lines().enumerate() {
        out.s.push_str(if i == 0 { first } else { continuation });
        anchors.push((src, out.s.len()));
        out.s.push_str(line);
        out.s.push('\n');
        src += line.len() + 1;
    }
}

fn prepare_list_level(
    ctx: &mut Ctx,
    ty: u8,
    level: u8,
    definition_id: u16,
    marker: &str,
    out: &mut String,
) -> usize {
    let index = level.saturating_sub(1) as usize;
    if ctx
        .list_keys
        .get(index)
        .is_some_and(|key| *key != (ty, definition_id))
    {
        if index == 0 {
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
            out.push_str("<!-- list-break -->\n\n");
        } else if !out.ends_with("\n\n") {
            out.push('\n');
        }
        ctx.list_widths.truncate(index);
        ctx.list_keys.truncate(index);
    }
    while ctx.list_widths.len() < index {
        ctx.list_widths.push(2);
        ctx.list_keys.push((ty, definition_id));
    }
    let indent = ctx.list_widths.iter().take(index).sum();
    let width = marker.chars().count() + 1;
    if ctx.list_widths.len() == index {
        ctx.list_widths.push(width);
        ctx.list_keys.push((ty, definition_id));
    } else {
        ctx.list_widths[index] = width;
        ctx.list_keys[index] = (ty, definition_id);
        ctx.list_widths.truncate(index + 1);
        ctx.list_keys.truncate(index + 1);
    }
    indent
}

/// Ends a list block with a blank line before an ordinary paragraph or heading.
fn close_list_block(ctx: &mut Ctx, out: &mut String) {
    if ctx.last_was_list && !out.ends_with("\n\n") {
        out.push('\n');
    }
    ctx.last_was_list = false;
    ctx.list_widths.clear();
    ctx.list_keys.clear();
}

fn break_section_list(ctx: &mut Ctx, out: &mut String) {
    if ctx.last_was_list {
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str("<!-- list-break -->\n\n");
    }
    ctx.last_was_list = false;
    ctx.list_widths.clear();
    ctx.list_keys.clear();
}

/// (Head kind, level): 2 = numbering, 3 = bullet; None for a non-list paragraph.
fn list_head(doc: &Document, para: &Paragraph) -> Option<(u8, u8, u16)> {
    let ps = doc.header.para_shapes.get(para.para_shape.0 as usize)?;
    let ty = ps.head_type();
    (ty == 2 || ty == 3).then(|| (ty, ps.head_level(), ps.numbering_id))
}

/// Renders paragraph content as inline/block fragments in document order.
fn render_fragments(doc: &Document, para: &Paragraph, ctx: &mut Ctx) -> Vec<Fragment> {
    let mut fragments = Vec::new();
    let mut body = Body::default();
    let mut wchar_pos = 0u32;
    let mut marks = Marks::default();
    // Body indices for the current emphasis span, updated only on opening.
    let mut span = Span::default();
    // Open hyperlink target URL, set by FIELD_START and closed by FIELD_END.
    let mut link_url: Option<String> = None;
    // v2 run segments use the **canonical** run list because run_id indexes that list.
    // Walking raw char_shape_runs could give a piece text and ID from different runs;
    // the lists diverge only for redundant hwp5 runs, making the mismatch easy to overlook.
    let runs = if ctx.v2 {
        canonical_char_shape_runs(para)
    } else {
        Vec::new()
    };
    let mut next_run = 0usize;
    // Active run; if a block interrupts emission, reopen its next piece.
    let mut current_run: Option<usize> = None;
    // Candidate run boundary from an emphasis transition, or the current cursor if absent.
    let mut boundary: Option<usize> = None;

    for ch in &para.chars {
        // The source cursor every run piece closes against: the offset of the character about to
        // be consumed. Assigned first, before anything in this iteration can end a piece, so a
        // run that ends because the shape changed here, or because a block flushes the output,
        // ends at THIS character rather than the previous one - and a control's own characters
        // belong to no piece.
        body.source_pos = wchar_pos;
        body.source_is_control = !matches!(ch, HwpChar::Text(_));
        // Transition marks using the character shape at the current position.
        // Close and reopen all marks on change to keep nesting consistent.
        if let HwpChar::Text(_) = ch {
            let mut want = Marks::from_shape(shape_at(doc, para, wchar_pos));
            // Hyperlink syntax already implies underline on the label;
            // avoid redundant `[<u>label</u>](URL)` markup.
            if link_url.is_some() {
                want.underline = false;
            }
            if want != marks {
                close_body_span(&mut body, marks, span, ctx.html_mode);
                span.open_at = body.len();
                // The candidate boundary follows close_span whitespace relocation and precedes the next
                // opening marker. The emitted markers belong to their runs; publication later reconciles
                // boundaries when a shared span relocates characters across several runs.
                boundary = Some(span.open_at);
                body.push_str(&want.open(ctx.html_mode));
                span.content_at = body.len();
                marks = want;
            }
        }
        if next_run < runs.len() && runs[next_run].0 <= wchar_pos {
            let at = boundary.unwrap_or(body.s.len());
            while next_run < runs.len() && runs[next_run].0 <= wchar_pos {
                let shape = runs[next_run].1.0;
                open_run_segment(doc, para, ctx, &mut body, next_run, shape, at, wchar_pos);
                current_run = Some(next_run);
                next_run += 1;
            }
        } else if let Some(index) = current_run
            && body.open_run.is_none()
        {
            // A block (table/block equation) flushed emission and closed this run. The same run remains
            // active, so reopen its next piece in the new coordinate system; pieces share one ID
            // (see the fragmented-run module documentation).
            let shape = runs[index].1.0;
            let at = body.s.len();
            open_run_segment(doc, para, ctx, &mut body, index, shape, at, wchar_pos);
        }
        boundary = None;
        match ch {
            HwpChar::Text(c) => {
                if ctx.html_mode {
                    push_html_escaped(&mut body, *c);
                } else {
                    body.push(*c);
                }
            }
            HwpChar::CharCtrl(code) => match *code {
                ctrl_char::LINE_BREAK => {
                    close_body_marks(&mut body, &mut marks, span, ctx.html_mode);
                    body.push_str(if ctx.html_mode { "<br/>" } else { "  \n" });
                }
                ctrl_char::HYPHEN => {
                    body.push('-');
                    body.note_control(RunControlKind::Hyphen, wchar_pos);
                }
                ctrl_char::NB_SPACE => {
                    body.push(' ');
                    body.note_control(RunControlKind::NbSpace, wchar_pos);
                }
                ctrl_char::FW_SPACE => {
                    body.push(' ');
                    body.note_control(RunControlKind::FwSpace, wchar_pos);
                }
                _ => {}
            },
            HwpChar::InlineCtrl { code, .. } => {
                if *code == ctrl_char::FIELD_END {
                    // End the hyperlink label with `](URL)` or `</a>`.
                    if let Some(url) = link_url.take() {
                        close_body_marks(&mut body, &mut marks, span, ctx.html_mode);
                        if ctx.html_mode {
                            body.push_str("</a>");
                        } else {
                            body.push_str("](");
                            body.push_str(&md_link_dest(&url));
                            body.push(')');
                        }
                    }
                    body.close_field();
                } else if *code == ctrl_char::TAB {
                    body.push(' ');
                    body.note_control(RunControlKind::Tab, wchar_pos);
                }
            }
            HwpChar::ExtCtrl {
                code, ctrl_index, ..
            } => {
                if let Some(idx) = ctrl_index
                    && let Some(control) = para.controls.get(*idx as usize)
                {
                    if ctx.v2 {
                        ctx.ctrl_index = *idx as usize;
                        ctx.ctrl_style = summarize(doc, para, char_shape_id_at(para, wchar_pos));
                        open_control_segment(ctx, &mut body, *code, control);
                    }
                    if *code == ctrl_char::FIELD_START
                        && let Some(url) = crate::field::hyperlink_url(control)
                    {
                        // Start a hyperlink field with `[` or `<a href>` and link the following label text.
                        close_body_marks(&mut body, &mut marks, span, ctx.html_mode);
                        if ctx.html_mode {
                            body.push_str("<a href=\"");
                            for c in url.chars() {
                                match c {
                                    '&' => body.push_str("&amp;"),
                                    '"' => body.push_str("&quot;"),
                                    '<' => body.push_str("&lt;"),
                                    _ => body.push(c),
                                }
                            }
                            body.push_str("\">");
                        } else {
                            body.push('[');
                        }
                        link_url = Some(url);
                    } else {
                        render_control(
                            doc,
                            control,
                            *code,
                            ctx,
                            &mut body,
                            &mut marks,
                            &mut fragments,
                            span,
                        );
                    }
                }
            }
        }
        wchar_pos += ch.wchar_width();
    }
    close_body_span(&mut body, marks, span, ctx.html_mode);
    // The walk has consumed the whole paragraph, so the last piece's source span ends at the
    // paragraph's end — the same bound `run_wchar_bounds` gives the last canonical run.
    body.source_pos = wchar_pos;
    body.source_is_control = false;
    body.close_open();
    flush_inline(&mut body, &mut fragments);
    fragments
}

/// v2 para segments cover both top-level paragraphs and paragraphs inside table cells or Generic
/// paragraph lists, so all seven segment kinds remain addressable in nested contexts.
///
/// `[start, end)` is what the paragraph emitted. Nothing emitted means no segment, unless the
/// paragraph always has one (see [`always_has_para_segment`]): then it is a point at `start`,
/// so the render layout artifact's row for it has an id to join to (#285).
fn para_segment(
    doc: &Document,
    para: &Paragraph,
    path: SegmentPath,
    start: usize,
    end: usize,
) -> Option<RawSeg> {
    let point = end == start;
    if end < start || (point && !always_has_para_segment(para)) {
        return None;
    }
    Some(RawSeg {
        kind: SegmentKind::Para,
        id: paragraph_id(&path, para),
        style: summarize(doc, para, char_shape_id_at(para, 0)),
        path,
        ctrl_id: None,
        name: None,
        start,
        end,
        point,
        source: None,
        controls: Vec::new(),
        start_follows_deletions: false,
        end_follows_deletions: false,
    })
}

/// Closes the current run and opens a new one (v2 only). IDs come exclusively from this crate's
/// segment_id entry points; this module hashes nothing.
///
/// `at` is the emitted-markdown offset the piece starts at and `source_at` the source-paragraph
/// offset it starts at: the two coordinate spaces the walk carries side by side (#517).
#[allow(clippy::too_many_arguments)]
fn open_run_segment(
    doc: &Document,
    para: &Paragraph,
    ctx: &Ctx,
    body: &mut Body,
    index: usize,
    shape: u16,
    at: usize,
    source_at: u32,
) {
    body.close_run_at(at);
    body.open_run = Some(RawSeg {
        kind: SegmentKind::Run,
        id: run_id(&ctx.path, para, index),
        path: child(&ctx.path, index),
        style: summarize(doc, para, Some(shape)),
        ctrl_id: None,
        name: None,
        start: at,
        end: 0,
        point: false,
        // Filled in by `close_run_at`: the end is only known when the piece ends.
        source: Some((source_at, source_at)),
        controls: Vec::new(),
        start_follows_deletions: body.source_is_control,
        end_follows_deletions: false,
    });
}

/// Opens a field/bookmark segment (v2 only).
///
/// These have no dedicated Control variant and **different owners**: field.rs classifies field
/// ctrl IDs, while bookmark.rs owns bookmarks. A bookmark uses bokm, not a %bmk field.
/// Widening is_field_ctrl_id would misclassify a non-field and propagate that error into
/// the Phase 6 typed operations.
fn open_control_segment(ctx: &Ctx, body: &mut Body, code: u16, control: &Control) {
    let Control::Generic(g) = control else {
        return;
    };
    let path = child(&ctx.path, ctx.ctrl_index);
    let ctrl_id = Some(String::from_utf8_lossy(&g.ctrl_id).into_owned());
    if code == ctrl_char::BOOKMARK && g.ctrl_id == *b"bokm" {
        // Bookmarks are non-emitting point markers (GG-25): start == end.
        let at = body.s.len();
        body.segs.push(RawSeg {
            kind: SegmentKind::Bookmark,
            id: control_id(&path, g),
            path,
            style: ctx.ctrl_style.clone(),
            ctrl_id,
            name: crate::bookmark::bookmark_name(control),
            start: at,
            end: at,
            point: false,
            source: None,
            controls: Vec::new(),
            start_follows_deletions: false,
            end_follows_deletions: false,
        });
    } else if code == ctrl_char::FIELD_START && crate::field::is_field_ctrl_id(&g.ctrl_id) {
        body.open_fields.push(RawSeg {
            kind: SegmentKind::Field,
            id: control_id(&path, g),
            path,
            style: ctx.ctrl_style.clone(),
            ctrl_id,
            name: None,
            start: body.s.len(),
            end: 0,
            point: false,
            source: None,
            controls: Vec::new(),
            start_follows_deletions: false,
            end_follows_deletions: false,
        });
    }
}

fn flush_inline(body: &mut Body, fragments: &mut Vec<Fragment>) {
    // Close the open span before flushing resets the coordinate system.
    body.close_open();
    // Flush segments even without text; otherwise bookmark-only paragraphs disappear, although
    // point markers correctly emit no text.
    if !body.s.is_empty() || !body.segs.is_empty() {
        fragments.push(Fragment::Inline(Body {
            s: std::mem::take(&mut body.s),
            segs: std::mem::take(&mut body.segs),
            ..Default::default()
        }));
    }
}

fn push_block(
    body: &mut Body,
    marks: &mut Marks,
    html: bool,
    fragments: &mut Vec<Fragment>,
    block: Body,
    span: Span,
) {
    close_body_marks(body, marks, span, html);
    flush_inline(body, fragments);
    fragments.push(Fragment::Block(block));
}

/// Formats Markdown link targets, wrapping spaces or parentheses in `<...>`.
fn md_link_dest(url: &str) -> String {
    if url
        .chars()
        .any(|c| c.is_whitespace() || c == '(' || c == ')')
    {
        format!("<{}>", url.replace('<', "%3C").replace('>', "%3E"))
    } else {
        url.to_string()
    }
}

#[allow(clippy::too_many_arguments)]
fn render_control(
    doc: &Document,
    control: &Control,
    code: u16,
    ctx: &mut Ctx,
    body: &mut Body,
    marks: &mut Marks,
    fragments: &mut Vec<Fragment>,
    span: Span,
) {
    match control {
        Control::SectionDef(_) => {}
        Control::Picture(pic) => {
            let start = body.s.len();
            let data = doc.resolve_bin(&pic.bin_ref);
            match data {
                Some(data) => {
                    let r = ctx.image_ref(data);
                    body.push_str(&r);
                }
                None => body.push_str(if ctx.html_mode {
                    "<!-- image -->"
                } else {
                    "![image]()"
                }),
            }
            if ctx.v2 {
                let path = child(&ctx.path, ctx.ctrl_index);
                let end = body.s.len();
                body.segs.push(RawSeg {
                    kind: SegmentKind::Image,
                    // The interpreted payload participates in the ID so replacing a same-position, same-size
                    // picture changes its ID.
                    id: picture_id(&path, pic, data),
                    path,
                    style: ctx.ctrl_style.clone(),
                    ctrl_id: None,
                    name: None,
                    start,
                    end,
                    point: false,
                    source: None,
                    controls: Vec::new(),
                    start_follows_deletions: false,
                    end_follows_deletions: false,
                });
            }
        }
        Control::Table(table) => {
            let mut block = Body::default();
            let path = child(&ctx.path, ctx.ctrl_index);
            // Capture this control's style **before recursion**: nested controls such as pictures inside
            // cells overwrite ctx.ctrl_style.
            let style = ctx.ctrl_style.clone();
            let saved = std::mem::replace(&mut ctx.path, path.clone());
            // Merged cells and cell blocks cannot be represented by GFM pipe tables; use HTML.
            if ctx.html_mode || has_span(table) || has_block_content(table, ctx) {
                render_html_table(doc, table, ctx, &mut block);
            } else {
                render_gfm_table(doc, table, ctx, &mut block);
            }
            ctx.path = saved;
            if ctx.v2 {
                let end = block.s.len();
                block.segs.push(RawSeg {
                    kind: SegmentKind::Table,
                    id: table_id(&path, table),
                    path,
                    style,
                    ctrl_id: None,
                    name: None,
                    start: 0,
                    end,
                    point: false,
                    source: None,
                    controls: Vec::new(),
                    start_follows_deletions: false,
                    end_follows_deletions: false,
                });
            }
            push_block(body, marks, ctx.html_mode, fragments, block, span);
        }
        Control::Generic(g) => {
            // Equations preserve their original script between dollar delimiters.
            if let Some(eq) = &g.equation {
                render_equation(eq, ctx, body, marks, fragments, span);
                return;
            }
            // Footnotes/endnotes use inline markers and trailing definitions instead of inline note bodies.
            if code == ctrl_char::FOOTNOTE_ENDNOTE && matches!(&g.ctrl_id, b"fn  " | b"en  ") {
                let label = if g.ctrl_id == *b"fn  " {
                    ctx.foot_n += 1;
                    ctx.foot_n.to_string()
                } else {
                    ctx.end_n += 1;
                    format!("e{}", ctx.end_n)
                };
                // Render the definition body as markdown regardless of position (definitions
                // are always emitted as top-level `[^N]:` at the document end, so they must
                // not contain HTML inline markup — #47).
                let saved_html = ctx.html_mode;
                ctx.html_mode = false;
                let text = note_text(doc, g, ctx);
                ctx.html_mode = saved_html;
                let src = (ctx.cur_section, ctx.cur_para);
                ctx.notes.push(Note {
                    label: label.clone(),
                    text,
                    src,
                });
                if ctx.html_mode {
                    body.push_str(&format!(
                        "<sup id=\"fnref-{label}\"><a href=\"#fn-{label}\">{label}</a></sup>"
                    ));
                } else {
                    body.push_str(&format!("[^{label}]"));
                }
                return;
            }
            // Exclude headers/footers and hidden comments according to the text extraction policy.
            if (code == ctrl_char::HEADER_FOOTER && !ctx.include_header_footer)
                || (code == ctrl_char::HIDDEN_COMMENT && !ctx.include_hidden)
            {
                return;
            }
            let base = child(&ctx.path, ctx.ctrl_index);
            let saved = std::mem::replace(&mut ctx.path, base.clone());
            let mut seq = 0usize;
            for list in &g.paragraph_lists {
                for p in &list.paragraphs {
                    if ctx.v2 {
                        ctx.path = child(&base, seq);
                    }
                    seq += 1;
                    let para_start = body.s.len();
                    let mut interrupted = false;
                    for fragment in render_fragments(doc, p, ctx) {
                        match fragment {
                            Fragment::Inline(inline) => {
                                let Body { s, segs, .. } = inline;
                                let lo = s.len() - s.trim_start().len();
                                let inline = s.trim();
                                if inline.is_empty() {
                                    body.absorb_append("", segs, lo);
                                } else {
                                    if !body.s.is_empty() && !body.s.ends_with([' ', '\n']) {
                                        body.s.push(' ');
                                    }
                                    body.absorb_append(inline, segs, lo);
                                }
                            }
                            Fragment::Block(block) => {
                                // The block flushes `body`, resetting its coordinates, so this
                                // paragraph's contribution is not one range. It is a point at
                                // the start of that contribution instead, recorded while
                                // `para_start` still indexes the buffer it was taken from.
                                if ctx.v2
                                    && !interrupted
                                    && let Some(seg) = para_segment(
                                        doc,
                                        p,
                                        ctx.path.clone(),
                                        para_start,
                                        para_start,
                                    )
                                {
                                    body.segs.push(seg);
                                }
                                interrupted = true;
                                push_block(body, marks, ctx.html_mode, fragments, block, span);
                            }
                        }
                    }
                    if ctx.v2
                        && !interrupted
                        && let Some(seg) =
                            para_segment(doc, p, ctx.path.clone(), para_start, body.s.len())
                    {
                        body.segs.push(seg);
                    }
                }
            }
            ctx.path = saved;
        }
    }
}

/// Equations: inline `$..$`, block `$$..$$`; original HWP script, not LaTeX.
fn render_equation(
    eq: &Equation,
    ctx: &mut Ctx,
    body: &mut Body,
    marks: &mut Marks,
    fragments: &mut Vec<Fragment>,
    span: Span,
) {
    if ctx.html_mode && eq.inline {
        body.push_str("<code>");
        for c in eq.script.chars() {
            push_html_escaped(body, c);
        }
        body.push_str("</code>");
    } else if eq.inline {
        body.push('$');
        body.push_str(&eq.script);
        body.push('$');
    } else if ctx.html_mode {
        let mut block = String::from("<div class=\"hwp-equation\"><code>");
        for c in eq.script.chars() {
            push_html_escaped(&mut block, c);
        }
        block.push_str("</code></div>");
        push_block(body, marks, true, fragments, block.into(), span);
    } else {
        push_block(
            body,
            marks,
            false,
            fragments,
            format!("$$\n{}\n$$", eq.script).into(),
            span,
        );
    }
}

/// Joins footnote/endnote paragraph fragments in document order.
fn note_text(doc: &Document, g: &GenericControl, ctx: &mut Ctx) -> String {
    let mut parts: Vec<String> = Vec::new();
    for list in &g.paragraph_lists {
        for p in &list.paragraphs {
            for fragment in render_fragments(doc, p, ctx) {
                // Definitions collect at the document end, so discard their v2 spans; like v1, definitions
                // are attributed to their referring paragraph.
                let text = match fragment {
                    Fragment::Inline(body) | Fragment::Block(body) => body.s,
                };
                let text = text.trim();
                if !text.is_empty() {
                    parts.push(text.to_string());
                }
            }
        }
    }
    parts.join("\n")
}

/// Any horizontally or vertically merged cell rules out a GFM pipe table.
fn has_span(table: &Table) -> bool {
    table.cells.iter().any(|c| c.col_span > 1 || c.row_span > 1)
}

/// Any cell containing a block fragment (table/block equation) rules out a GFM pipe table.
fn has_block_content(table: &Table, ctx: &Ctx) -> bool {
    table
        .cells
        .iter()
        .any(|c| c.paragraphs.iter().any(|p| paragraph_has_block(p, ctx)))
}

fn paragraph_has_block(para: &Paragraph, ctx: &Ctx) -> bool {
    para.controls.iter().any(|control| match control {
        Control::Table(_) => true,
        Control::Generic(g) => {
            let excluded = ((g.ctrl_id == *b"head" || g.ctrl_id == *b"foot")
                && !ctx.include_header_footer)
                || (g.ctrl_id == *b"tcmt" && !ctx.include_hidden);
            !excluded
                && (g.equation.as_ref().is_some_and(|eq| !eq.inline)
                    || g.paragraph_lists.iter().any(|list| {
                        list.paragraphs
                            .iter()
                            .any(|paragraph| paragraph_has_block(paragraph, ctx))
                    }))
        }
        _ => false,
    })
}

/// GFM pipe table: first row as header, no merged cells.
fn render_gfm_table(doc: &Document, table: &Table, ctx: &mut Ctx, out: &mut Body) {
    let cols = table.cols.max(1) as usize;
    let mut grid: Vec<Vec<Body>> = Vec::new();
    for (cell_index, cell) in table.cells.iter().enumerate() {
        let row = cell.row as usize;
        while grid.len() <= row {
            grid.push((0..cols).map(|_| Body::default()).collect());
        }
        let mut text = Body::default();
        let path = child(&ctx.path, cell_index);
        let saved = std::mem::replace(&mut ctx.path, path.clone());
        for (para_index, p) in cell.paragraphs.iter().enumerate() {
            if ctx.v2 {
                ctx.path = child(&path, para_index);
            }
            let para_start = text.s.len();
            for fragment in render_fragments(doc, p, ctx) {
                let Fragment::Inline(body) = fragment else {
                    debug_assert!(false, "블록 셀은 HTML 표로 선분기되어야 함");
                    continue;
                };
                let Body { s, segs, .. } = body;
                if !text.s.is_empty() && !s.is_empty() {
                    text.s.push(' ');
                }
                let lo = s.len() - s.trim_start().len();
                text.absorb_append(s.trim(), segs, lo);
            }
            if ctx.v2
                && let Some(seg) = para_segment(doc, p, ctx.path.clone(), para_start, text.s.len())
            {
                text.segs.push(seg);
            }
        }
        ctx.path = saved;
        if ctx.v2 {
            let end = text.s.len();
            text.segs.push(RawSeg {
                kind: SegmentKind::Cell,
                id: cell_id(&path, cell),
                style: cell_style(doc, cell),
                path,
                ctrl_id: None,
                name: None,
                start: 0,
                end,
                point: false,
                source: None,
                controls: Vec::new(),
                start_follows_deletions: false,
                end_follows_deletions: false,
            });
        }
        if let Some(slot) = grid[row].get_mut(cell.col as usize) {
            *slot = escape_gfm_cell(text);
        }
    }
    out.s.push('\n');
    for (i, row) in grid.into_iter().enumerate() {
        out.s.push_str("| ");
        for (c, cell) in row.into_iter().enumerate() {
            if c > 0 {
                out.s.push_str(" | ");
            }
            let Body { s, segs, .. } = cell;
            out.absorb_append(&s, segs, 0);
        }
        out.s.push_str(" |\n");
        if i == 0 {
            out.s.push_str(&format!("|{}\n", " --- |".repeat(cols)));
        }
    }
    out.s.push('\n');
}

/// Escapes cell content for pipe tables: escape pipes and turn newlines into spaces. Only pipe
/// escaping changes length, so build per-character anchors only when spans need remapping.
fn escape_gfm_cell(cell: Body) -> Body {
    if cell.segs.is_empty() {
        return cell.s.replace('|', "\\|").replace('\n', " ").into();
    }
    let Body { s, segs, .. } = cell;
    let mut out = Body::default();
    let mut anchors = Vec::with_capacity(s.len());
    for (at, c) in s.char_indices() {
        anchors.push((at, out.s.len()));
        match c {
            '|' => out.s.push_str("\\|"),
            '\n' => out.s.push(' '),
            _ => out.s.push(c),
        }
    }
    let len = s.len();
    // Anchor the end boundary too: a final escaped pipe adds a byte.
    anchors.push((len, out.s.len()));
    out.absorb(segs, 0, len, &anchors);
    out
}

/// The cell's two style layers: first paragraph style and directly referenced shapes.
fn cell_style(doc: &Document, cell: &Cell) -> SegmentStyle {
    cell.paragraphs
        .first()
        .map(|p| summarize(doc, p, char_shape_id_at(p, 0)))
        .unwrap_or_default()
}

/// HTML table preserving merged cells (colspan/rowspan) and blocks, including nested tables.
/// Cell content uses html_mode because block HTML does not render Markdown.
///
/// **One line per table row**: each `<tr>...</tr>` occupies one line. Top-level tables separate
/// rows for readability and line-based quotation. Nested tables in cells (html_mode) serialize
/// without newlines so they cannot split the outer row; all nested rows share that line,
/// while each line still has equal counts of `<tr` and `</tr>`.
fn render_html_table(doc: &Document, table: &Table, ctx: &mut Ctx, out: &mut Body) {
    let rows = table.rows.max(1) as usize;
    let cols = table.cols.max(1) as usize;
    // Nested cell tables enter in html_mode and emit only the single-line form.
    let nested = ctx.html_mode;
    // Grid of positions covered by merged cells.
    let mut covered = vec![vec![false; cols]; rows];
    out.s
        .push_str(if nested { "<table>" } else { "\n<table>\n" });
    for r in 0..rows {
        out.s.push_str("<tr>");
        for c in 0..cols {
            if covered[r][c] {
                continue; // Position covered by a preceding merged cell.
            }
            let Some((cell_index, cell)) = table
                .cells
                .iter()
                .enumerate()
                .find(|(_, cell)| cell.row as usize == r && cell.col as usize == c)
            else {
                out.s.push_str("<td></td>");
                continue;
            };
            for dr in 0..cell.row_span.max(1) as usize {
                for dc in 0..cell.col_span.max(1) as usize {
                    if let Some(slot) = covered.get_mut(r + dr).and_then(|row| row.get_mut(c + dc))
                    {
                        *slot = true;
                    }
                }
            }
            let mut attrs = String::new();
            if cell.col_span > 1 {
                attrs.push_str(&format!(" colspan=\"{}\"", cell.col_span));
            }
            if cell.row_span > 1 {
                attrs.push_str(&format!(" rowspan=\"{}\"", cell.row_span));
            }
            let path = child(&ctx.path, cell_index);
            let saved = std::mem::replace(&mut ctx.path, path.clone());
            let content = render_cell_html(doc, cell, ctx);
            ctx.path = saved;
            let start = out.s.len();
            out.s.push_str(&format!("<td{attrs}>"));
            let Body { s, segs, .. } = content;
            out.absorb_append(&s, segs, 0);
            out.s.push_str("</td>");
            if ctx.v2 {
                let end = out.s.len();
                out.segs.push(RawSeg {
                    kind: SegmentKind::Cell,
                    id: cell_id(&path, cell),
                    style: cell_style(doc, cell),
                    path,
                    ctrl_id: None,
                    name: None,
                    start,
                    end,
                    point: false,
                    source: None,
                    controls: Vec::new(),
                    start_follows_deletions: false,
                    end_follows_deletions: false,
                });
            }
        }
        // Only top-level tables add newlines after rows; nested tables stay on one line.
        out.s.push_str(if nested { "</tr>" } else { "</tr>\n" });
    }
    out.s
        .push_str(if nested { "</table>" } else { "</table>\n\n" });
}

/// Renders cell content as html_mode fragments, preserving original order.
fn render_cell_html(doc: &Document, cell: &Cell, ctx: &mut Ctx) -> Body {
    let saved = ctx.html_mode;
    ctx.html_mode = true;
    let cell_path = ctx.path.clone();
    let mut content = Body::default();
    for (para_index, p) in cell.paragraphs.iter().enumerate() {
        if ctx.v2 {
            ctx.path = child(&cell_path, para_index);
        }
        let para_start = content.s.len();
        for fragment in render_fragments(doc, p, ctx) {
            let Body { s, segs, .. } = match fragment {
                Fragment::Inline(body) | Fragment::Block(body) => body,
            };
            let lo = s.len() - s.trim_start().len();
            let text = s.trim();
            if text.is_empty() {
                content.absorb_append("", segs, lo);
                continue;
            }
            if !content.s.is_empty() {
                content.s.push_str("<br/>");
            }
            content.absorb_append(text, segs, lo);
        }
        if ctx.v2
            && let Some(seg) = para_segment(doc, p, ctx.path.clone(), para_start, content.s.len())
        {
            content.segs.push(seg);
        }
    }
    ctx.path = cell_path;
    ctx.html_mode = saved;
    content
}

/// Escapes HTML text nodes (& < >).
fn push_html_escaped(out: &mut String, c: char) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        _ => out.push(c),
    }
}

fn escape_html_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Character shape at the given WCHAR position.
fn shape_at<'d>(doc: &'d Document, para: &Paragraph, pos: u32) -> Option<&'d CharShape> {
    let id = para
        .char_shape_runs
        .iter()
        .rev()
        .find(|(start, _)| *start <= pos)
        .map(|(_, id)| *id)?;
    doc.header.char_shapes.get(id.0 as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_markdown::from_markdown;
    use hwp_model::{CharShapeId, HwpChar, ParagraphList};

    /// Hyperlinks survive md -> IR -> md as `[label](URL)`.
    /// Link syntax already implies underline, so label formatting emits no redundant `<u>`.
    #[test]
    fn 하이퍼링크_왕복_보존() {
        let doc = from_markdown("자세히는 [여기](https://example.com/path)를 본다\n");
        let md = to_markdown(&doc);
        assert!(
            md.contains("[여기](https://example.com/path)"),
            "링크 왕복: {md}"
        );
    }

    /// Without media_dir, images retain empty references, preserving existing behavior.
    #[test]
    fn 이미지_기본은_빈참조() {
        let mut doc = from_markdown("사진: 여기");
        let png = png_bytes();
        insert_temp_image(&mut doc, "사진:", "md_img_none.png", &png);
        let md = to_markdown(&doc);
        assert!(md.contains("![image]()"), "빈 참조 유지: {md}");
    }

    /// With media_dir, images are extracted and referenced by relative paths.
    #[test]
    fn 이미지_media_dir_추출() {
        let mut doc = from_markdown("사진: 여기");
        let png = png_bytes();
        insert_temp_image(&mut doc, "사진:", "md_img_extract.png", &png);

        let dir = unique_dir("md_media_extract");
        // The directory must not exist before extraction, confirming lazy creation.
        assert!(!dir.exists());
        let md = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                ..Default::default()
            },
        )
        .unwrap();
        let name = dir.file_name().unwrap().to_string_lossy();
        assert!(
            md.contains(&format!("![image]({name}/image1.png)")),
            "상대경로 참조: {md}"
        );
        let extracted = dir.join("image1.png");
        assert!(extracted.exists(), "이미지 파일 추출");
        assert_eq!(std::fs::read(&extracted).unwrap(), png, "추출 바이트 일치");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An image-free document creates no directory even when media_dir is set.
    #[test]
    fn 이미지_없으면_디렉터리_미생성() {
        let doc = from_markdown("본문만 있는 문단\n");
        let dir = unique_dir("md_media_empty");
        let _ = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!dir.exists(), "이미지 없으면 디렉터리 미생성");
    }

    /// Multiple images are numbered image1/image2 in document order.
    #[test]
    fn 이미지_카운터_증가() {
        // The two images receive image1/image2 references in order.
        let mut doc = from_markdown("첫 사진: 여기\n\n둘 사진: 저기");
        let png = png_bytes();
        insert_temp_image(&mut doc, "첫 사진:", "md_cnt1.png", &png);
        insert_temp_image(&mut doc, "둘 사진:", "md_cnt2.png", &png);
        let dir = unique_dir("md_media_counter");
        let md = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                ..Default::default()
            },
        )
        .unwrap();
        let name = dir.file_name().unwrap().to_string_lossy();
        assert!(md.contains(&format!("{name}/image1.png")), "첫 이미지");
        assert!(md.contains(&format!("{name}/image2.png")), "둘째 이미지");
        assert!(dir.join("image2.png").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// media_prefix replaces the directory name in image references.
    #[test]
    fn 이미지_media_prefix() {
        let mut doc = from_markdown("사진: 여기");
        let png = png_bytes();
        insert_temp_image(&mut doc, "사진:", "md_img_prefix.png", &png);
        let dir = unique_dir("md_media_prefix");
        let md = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                media_prefix: Some("figs"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            md.contains("![image](figs/image1.png)"),
            "prefix 참조: {md}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 이미지_경로를_markdown에_안전하게_인코딩() {
        let mut doc = from_markdown("사진: 여기");
        let png = png_bytes();
        insert_temp_image(&mut doc, "사진:", "md_img_escaped.png", &png);
        let dir = unique_dir("md_media_escaped");
        let md = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                media_prefix: Some(r"my figs\(draft)"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            md.contains("![image](<my figs/(draft)/image1.png>)"),
            "공백·괄호·구분자 처리: {md}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 이미지_충돌은_덮어쓰지_않고_동일파일만_재사용() {
        let mut doc = from_markdown("사진: 여기");
        let png = png_bytes();
        insert_temp_image(&mut doc, "사진:", "md_img_collision.png", &png);
        let dir = unique_dir("md_media_collision");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("image1.png");
        std::fs::write(&target, b"existing file").unwrap();

        let error = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&target).unwrap(), b"existing file");

        std::fs::write(&target, &png).unwrap();
        to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                ..Default::default()
            },
        )
        .expect("동일 바이트는 재사용");
        assert_eq!(std::fs::read(&target).unwrap(), png);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 이미지_충돌은_새파일_기록_전에_선검사() {
        let mut doc = from_markdown("첫 사진: 여기\n\n둘 사진: 저기");
        let png = png_bytes();
        insert_temp_image(&mut doc, "첫 사진:", "md_preflight1.png", &png);
        insert_temp_image(&mut doc, "둘 사진:", "md_preflight2.png", &png);
        let dir = unique_dir("md_media_preflight");
        std::fs::create_dir_all(dir.join("image2.png")).unwrap();

        assert!(
            to_markdown_with(
                &doc,
                &MarkdownOptions {
                    media_dir: Some(&dir),
                    ..Default::default()
                },
            )
            .is_err()
        );
        assert!(!dir.join("image1.png").exists(), "선검사 전에는 기록 금지");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Footnotes/endnotes emit inline markers and trailing definitions (GH-3).
    #[test]
    fn 각주_미주_마커와_정의() {
        let mut doc = from_markdown("본문 문단\n");
        let note = |id: &[u8; 4], txt: &str| {
            Control::Generic(GenericControl {
                ctrl_id: *id,
                data: vec![],
                paragraph_lists: vec![ParagraphList {
                    header_data: vec![],
                    paragraphs: vec![Paragraph {
                        chars: txt.chars().map(HwpChar::Text).collect(),
                        ..Paragraph::default()
                    }],
                }],
                extras: vec![],
                raw_children: vec![],
                gso_shapes: vec![],
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
            })
        };
        let anchor = |idx: u32, id: &[u8; 4]| HwpChar::ExtCtrl {
            code: ctrl_char::FOOTNOTE_ENDNOTE,
            ctrl_id: *id,
            payload: vec![],
            ctrl_index: Some(idx),
        };
        let para = &mut doc.sections[0].paragraphs[0];
        // The first paragraph has an injected section definition, so its control count is the index.
        let i0 = para.controls.len() as u32;
        para.controls.push(note(b"fn  ", "각주 내용"));
        let i1 = para.controls.len() as u32;
        para.controls.push(note(b"en  ", "미주 내용"));
        para.chars.push(anchor(i0, b"fn  "));
        para.chars.push(anchor(i1, b"en  "));

        let md = to_markdown(&doc);
        assert!(md.contains("본문 문단[^1][^e1]"), "본문 마커: {md}");
        assert!(md.contains("[^1]: 각주 내용"), "각주 정의: {md}");
        assert!(md.contains("[^e1]: 미주 내용"), "미주 정의: {md}");
        // Definitions collect at the document end.
        assert!(
            md.trim_end().ends_with("[^e1]: 미주 내용"),
            "정의는 문서 끝: {md}"
        );
    }

    #[test]
    fn html_표_각주는_html_마커와_gfm_정의로_연결() {
        let note = Control::Generic(GenericControl {
            ctrl_id: *b"fn  ",
            data: vec![],
            paragraph_lists: vec![ParagraphList {
                header_data: vec![],
                paragraphs: vec![Paragraph {
                    chars: "표 안 각주".chars().map(HwpChar::Text).collect(),
                    ..Paragraph::default()
                }],
            }],
            extras: vec![],
            raw_children: vec![],
            gso_shapes: vec![],
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        });
        let cell_para = Paragraph {
            chars: "셀 본문"
                .chars()
                .map(HwpChar::Text)
                .chain(std::iter::once(HwpChar::ExtCtrl {
                    code: ctrl_char::FOOTNOTE_ENDNOTE,
                    ctrl_id: *b"fn  ",
                    payload: vec![],
                    ctrl_index: Some(0),
                }))
                .collect(),
            controls: vec![note],
            ..Paragraph::default()
        };
        let mut doc = from_markdown("표\n");
        insert_table(
            &mut doc.sections[0].paragraphs[0],
            one_cell_table(cell_para, 2, 2),
        );

        let md = to_markdown(&doc);
        assert!(
            md.contains(r##"<sup id="fnref-1"><a href="#fn-1">1</a></sup>"##),
            "HTML 각주 참조: {md}"
        );
        // Definitions are GFM syntax regardless of reference position — an HTML block would
        // be rejected by the importer contract (#47).
        assert!(md.contains("[^1]: 표 안 각주"), "GFM 각주 정의: {md}");
        assert!(!md.contains("hwp-footnote"), "HTML 정의 블록 금지: {md}");
    }

    /// Equations emit inline `$..$` or block `$$..$$`.
    #[test]
    fn 수식_인라인_블록() {
        let mk = |script: &str, inline: bool| {
            Control::Generic(GenericControl {
                ctrl_id: *b"eqed",
                data: vec![],
                paragraph_lists: vec![],
                extras: vec![],
                raw_children: vec![],
                gso_shapes: vec![],
                equation: Some(Equation {
                    script: script.to_string(),
                    width: 0,
                    height: 0,
                    inline,
                    x: 0,
                    y: 0,
                    ..Equation::default()
                }),
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
            })
        };
        let anchor = |idx: u32| HwpChar::ExtCtrl {
            code: ctrl_char::OBJECT,
            ctrl_id: *b"eqed",
            payload: vec![],
            ctrl_index: Some(idx),
        };
        let mut doc = from_markdown("인라인 수식: \n");
        let p0 = &mut doc.sections[0].paragraphs[0];
        // Use the index after the first paragraph's injected section-definition control.
        let i0 = p0.controls.len() as u32;
        p0.chars.push(anchor(i0));
        p0.controls.push(mk("a+b", true));
        // Add a paragraph containing only a block equation.
        doc.sections[0].paragraphs.push(Paragraph {
            chars: vec![anchor(0)],
            controls: vec![mk("x^2", false)],
            ..Paragraph::default()
        });

        let md = to_markdown(&doc);
        assert!(md.contains("$a+b$"), "인라인 수식: {md}");
        assert!(md.contains("$$\nx^2\n$$"), "블록 수식: {md}");
    }

    #[test]
    fn 블록_수식_앞뒤_순서_보존() {
        let mut doc = from_markdown("앞 뒤\n");
        let para = &mut doc.sections[0].paragraphs[0];
        let index = para.controls.len() as u32;
        para.controls.push(equation_control("x^2", false));
        para.chars = "앞 "
            .chars()
            .map(HwpChar::Text)
            .chain(std::iter::once(control_anchor(index, b"eqed")))
            .chain(" 뒤".chars().map(HwpChar::Text))
            .collect();

        let md = to_markdown(&doc);
        let before = md.find("앞").unwrap();
        let equation = md.find("$$\nx^2\n$$").unwrap();
        let after = md.find("뒤").unwrap();
        assert!(
            before < equation && equation < after,
            "등장 순서 보존: {md}"
        );
    }

    #[test]
    fn 블록_수식_셀은_html_표로_내용_보존() {
        use hwp_model::{BorderFillId, HwpUnit};
        let mut cell_para = Paragraph::default();
        cell_para.controls.push(equation_control("a+b", false));
        cell_para.chars = "앞"
            .chars()
            .map(HwpChar::Text)
            .chain(std::iter::once(control_anchor(0, b"eqed")))
            .chain("뒤".chars().map(HwpChar::Text))
            .collect();
        let table = Table {
            rows: 1,
            cols: 1,
            row_cell_counts: vec![1],
            cells: vec![Cell {
                list_attr: 0,
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1,
                width: HwpUnit(0),
                height: HwpUnit(0),
                margins: [0; 4],
                border_fill: BorderFillId(0),
                header_tail: vec![],
                paragraphs: vec![cell_para],
            }],
            common_data: vec![],
            placement: None,
            attr: 0,
            cell_spacing: 0,
            inner_margins: [0; 4],
            border_fill: BorderFillId(0),
            table_tail: vec![],
            caption: None,
            extras: vec![],
        };
        let mut doc = from_markdown("표\n");
        insert_table(&mut doc.sections[0].paragraphs[0], table);

        let md = to_markdown(&doc);
        assert!(md.contains("<table>"), "블록 셀은 HTML 폴백: {md}");
        let before = md.find("<td>앞").unwrap();
        let equation = md.find("hwp-equation").unwrap();
        let after = md.find("뒤</td>").unwrap();
        assert!(before < equation && equation < after, "셀 순서 보존: {md}");
    }

    /// Bullet/number paragraphs emit GFM lists (GH-6).
    #[test]
    fn 목록_불릿과_번호() {
        use hwp_model::{NumFmt, NumLevel, ParaShape, ParaShapeId};
        let ps = |ty: u32, lv: u32, nid: u16| ParaShape {
            attr1: (ty << 23) | (lv << 25),
            numbering_id: nid,
            ..ParaShape::default()
        };
        // Number-list counters are independent per numbering ID;
        // validate lists with different formats in separate documents.
        let mut doc = from_markdown("불릿 하나\n\n번호 하나\n\n번호 둘\n");
        let base = doc.header.para_shapes.len() as u16;
        doc.header.para_shapes.push(ps(3, 1, 0)); // Bullet.
        doc.header.para_shapes.push(ps(2, 1, 0)); // Numbering (decimal).
        doc.header.bullet_chars = vec!['•'];
        doc.header.numbering_levels = vec![vec![NumLevel::default(); 7]];
        for (i, p) in doc.sections[0].paragraphs.iter_mut().enumerate() {
            p.para_shape = ParaShapeId(if i == 0 { base } else { base + 1 });
        }
        let md = to_markdown(&doc);
        assert!(md.contains("- 불릿 하나\n"), "불릿: {md}");
        assert!(md.contains("1. 번호 하나\n"), "숫자 번호 1: {md}");
        assert!(md.contains("2. 번호 둘\n"), "숫자 번호 2: {md}");

        // GFM accepts only numeric ordered markers, so Korean numbering exports its numeric ordinal (#134).
        let mut doc2 = from_markdown("한글 번호\n");
        let base2 = doc2.header.para_shapes.len() as u16;
        doc2.header.para_shapes.push(ps(2, 1, 1));
        doc2.header.numbering_levels = vec![
            vec![NumLevel::default(); 7],
            vec![
                NumLevel {
                    start: 1,
                    fmt: NumFmt::HangulSyllable,
                    template: String::new(),
                };
                7
            ],
        ];
        doc2.sections[0].paragraphs[0].para_shape = ParaShapeId(base2);
        let md2 = to_markdown(&doc2);
        assert!(md2.contains("1. 한글 번호"), "한글 형식 서수 마커: {md2}");
        assert!(!md2.contains("가."), "마커가 본문으로 굳으면 안 됨: {md2}");
    }

    /// Non-numeric engine-marker ladders also remain stable through md -> IR -> md (#134).
    #[test]
    fn 비숫자_번호마커_왕복_안정() {
        use hwp_model::{NumFmt, NumLevel, ParaShape, ParaShapeId};
        let ps = |level: u32| ParaShape {
            attr1: (2 << 23) | (level << 25),
            numbering_id: 0,
            ..ParaShape::default()
        };
        let lvl = |fmt: NumFmt, template: &str| NumLevel {
            start: 1,
            fmt,
            template: template.to_string(),
        };
        // The statutory ladder's first three rungs: "1." / "가." / "1)".
        let mut doc = from_markdown("상위\n\n중위\n\n하위\n");
        let base = doc.header.para_shapes.len() as u16;
        doc.header.para_shapes.push(ps(1));
        doc.header.para_shapes.push(ps(2));
        doc.header.para_shapes.push(ps(3));
        doc.header.numbering_levels = vec![vec![
            lvl(NumFmt::Digit, "^1."),
            lvl(NumFmt::HangulSyllable, "^2."),
            lvl(NumFmt::Digit, "^3)"),
        ]];
        for (paragraph, shape) in
            doc.sections[0]
                .paragraphs
                .iter_mut()
                .zip([base, base + 1, base + 2])
        {
            paragraph.para_shape = ParaShapeId(shape);
        }

        let md1 = to_markdown(&doc);
        assert!(
            md1.contains("1. 상위\n   1. 중위\n      1. 하위"),
            "모든 수준이 숫자 서수 마커여야 함: {md1}"
        );
        assert!(
            !md1.contains("가.") && !md1.contains("1)"),
            "엔진 마커가 리터럴로 새면 안 됨: {md1}"
        );

        // Re-importing must rebuild the same numbered heads, so a second export
        // is byte-identical (no "가. 가."-style degradation).
        let doc2 = from_markdown(&md1);
        let md2 = to_markdown(&doc2);
        assert_eq!(md1, md2, "md 왕복은 안정적이어야 함");

        // A bullet ladder round-trips through `- ` unchanged.
        let bullet = from_markdown("- 하나\n  - 둘\n");
        let bullet_md = to_markdown(&bullet);
        assert_eq!(bullet_md, to_markdown(&from_markdown(&bullet_md)));
    }

    #[test]
    fn 목록_중첩은_부모_마커_폭으로_들여쓰기() {
        use hwp_model::{NumLevel, ParaShape, ParaShapeId};
        use pulldown_cmark::{Event, Parser, Tag, TagEnd};

        let mut doc = from_markdown("상위\n\n하위\n\n다음\n");
        let base = doc.header.para_shapes.len() as u16;
        let ps = |ty: u32, level: u32| ParaShape {
            attr1: (ty << 23) | (level << 25),
            numbering_id: 0,
            ..ParaShape::default()
        };
        doc.header.para_shapes.push(ps(2, 1));
        doc.header.para_shapes.push(ps(3, 2));
        doc.header.numbering_levels = vec![vec![NumLevel::default(); 7]];
        doc.header.bullet_chars = vec!['•'];
        for (paragraph, shape) in doc.sections[0]
            .paragraphs
            .iter_mut()
            .zip([base, base + 1, base])
        {
            paragraph.para_shape = ParaShapeId(shape);
        }

        let md = to_markdown(&doc);
        assert!(
            md.contains("1. 상위\n   - 하위\n2. 다음"),
            "목록 들여쓰기: {md}"
        );
        let mut depth = 0usize;
        let mut max_depth = 0usize;
        for event in Parser::new(&md) {
            match event {
                Event::Start(Tag::List(_)) => {
                    depth += 1;
                    max_depth = max_depth.max(depth);
                }
                Event::End(TagEnd::List(_)) => depth -= 1,
                _ => {}
            }
        }
        assert_eq!(max_depth, 2, "파서에서도 중첩 목록이어야 함: {md}");
    }

    #[test]
    fn 구역_경계에서_번호목록_재시작() {
        use hwp_model::{NumLevel, ParaShape, ParaShapeId};
        use pulldown_cmark::{Event, Parser, Tag};

        let mut doc = from_markdown("첫 구역\n");
        let shape = doc.header.para_shapes.len() as u16;
        doc.header.para_shapes.push(ParaShape {
            attr1: (2 << 23) | (1 << 25),
            numbering_id: 0,
            ..ParaShape::default()
        });
        doc.header.numbering_levels = vec![vec![NumLevel::default(); 7]];
        doc.sections[0].paragraphs[0].para_shape = ParaShapeId(shape);
        let mut second = doc.sections[0].clone();
        second.paragraphs[0].chars = "둘째 구역".chars().map(HwpChar::Text).collect();
        doc.sections.push(second);

        let md = to_markdown(&doc);
        assert!(md.contains("<!-- list-break -->"), "구역 목록 분리: {md}");
        let starts = Parser::new(&md)
            .filter(|event| matches!(event, Event::Start(Tag::List(_))))
            .count();
        assert_eq!(starts, 2, "서로 다른 최상위 목록이어야 함: {md}");
    }

    /// Merged cells fall back to HTML tables with colspan/rowspan (GH-4).
    #[test]
    fn 병합셀_html_표() {
        use hwp_model::{BorderFillId, Cell, HwpUnit, Table};
        let cell = |row: u16, col: u16, cs: u16, rs: u16, txt: &str| Cell {
            list_attr: 0,
            col,
            row,
            col_span: cs,
            row_span: rs,
            width: HwpUnit(0),
            height: HwpUnit(0),
            margins: [0; 4],
            border_fill: BorderFillId(0),
            header_tail: vec![],
            paragraphs: vec![Paragraph {
                chars: txt.chars().map(HwpChar::Text).collect(),
                ..Paragraph::default()
            }],
        };
        let table = Table {
            common_data: vec![],
            placement: None,
            attr: 0,
            rows: 2,
            cols: 2,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1, 2],
            border_fill: BorderFillId(0),
            table_tail: vec![],
            caption: None,
            cells: vec![
                cell(0, 0, 2, 1, "병합"),
                cell(1, 0, 1, 1, "가"),
                cell(1, 1, 1, 1, "나"),
            ],
            extras: vec![],
        };
        let mut doc = from_markdown("표 문단\n");
        let p = &mut doc.sections[0].paragraphs[0];
        // Use the index after the first paragraph's injected section-definition control.
        let i0 = p.controls.len() as u32;
        p.chars.push(HwpChar::ExtCtrl {
            code: ctrl_char::OBJECT,
            ctrl_id: *b"tbl ",
            payload: vec![],
            ctrl_index: Some(i0),
        });
        p.controls.push(Control::Table(table));

        let md = to_markdown(&doc);
        assert!(md.contains("<table>"), "HTML 표: {md}");
        assert!(md.contains("<td colspan=\"2\">병합</td>"), "colspan: {md}");
        assert!(md.contains("<td>가</td><td>나</td>"), "나머지 행: {md}");
    }

    /// GH-46: exported HTML tables must satisfy the importer contract so tables survive
    /// the cat --format markdown -> new --from round trip.
    #[test]
    fn markdown_왕복_html_표_보존() {
        // Cover merged cells (HTML table), multiple cell paragraphs (<br/>), and bold (<strong>).
        let bold_para = Paragraph {
            chars: "굵은 글씨".chars().map(HwpChar::Text).collect(),
            char_shape_runs: vec![(0, CharShapeId(0)), (2, CharShapeId(1))],
            ..Paragraph::default()
        };
        let mut table = merged_table();
        table.cells[0].paragraphs = vec![bold_para, text_para("둘째 줄")];
        let mut doc = from_markdown("표\n");
        doc.header.char_shapes = vec![bold_shape(), CharShape::default()];
        insert_table(&mut doc.sections[0].paragraphs[0], table);

        let md = to_markdown(&doc);
        // Contract invariant: no bare <br>/<b>/<i>, matching html.rs's <br/> invariant.
        assert!(!md.contains("<br>"), "self-closing <br/>만 방출: {md}");
        assert!(!md.contains("<b>"), "<strong>만 방출: {md}");
        assert!(!md.contains("<i>"), "<em>만 방출: {md}");

        let (back, warnings) = crate::from_markdown::from_markdown_report(
            &md,
            &crate::from_markdown::MarkdownImportOptions::default(),
        )
        .unwrap();
        assert!(
            !warnings.iter().any(|w| w.contains("계약 위반")),
            "계약 위반 경고가 없어야 함: {warnings:?}"
        );
        let tables = back.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| p.controls.iter())
            .filter(|c| matches!(c, Control::Table(_)))
            .count();
        assert_eq!(tables, 1, "왕복 후 표가 보존되어야 함: {md}");
    }

    /// GH-47: footnotes/endnotes referenced inside an HTML table still emit their
    /// definitions as GFM (`[^N]:`), and the importer restores fnref markers to real
    /// footnote anchors, closing the round-trip.
    #[test]
    fn markdown_왕복_html_표_각주_보존() {
        let note = |id: &[u8; 4], txt: &str| {
            Control::Generic(GenericControl {
                ctrl_id: *id,
                data: vec![],
                paragraph_lists: vec![ParagraphList {
                    header_data: vec![],
                    paragraphs: vec![text_para(txt)],
                }],
                extras: vec![],
                raw_children: vec![],
                gso_shapes: vec![],
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
            })
        };
        let anchor = |idx: u32, id: &[u8; 4]| HwpChar::ExtCtrl {
            code: ctrl_char::FOOTNOTE_ENDNOTE,
            ctrl_id: *id,
            payload: vec![],
            ctrl_index: Some(idx),
        };
        let cell_para = Paragraph {
            chars: "셀 본문"
                .chars()
                .map(HwpChar::Text)
                .chain([anchor(0, b"fn  "), anchor(1, b"en  ")])
                .collect(),
            controls: vec![note(b"fn  ", "표 안 각주"), note(b"en  ", "표 안 미주")],
            ..Paragraph::default()
        };
        let mut table = merged_table();
        table.cells[0].paragraphs = vec![cell_para];
        let mut doc = from_markdown("표\n");
        insert_table(&mut doc.sections[0].paragraphs[0], table);

        let md = to_markdown(&doc);
        assert!(md.contains("[^1]: 표 안 각주"), "GFM 각주 정의: {md}");
        assert!(md.contains("[^e1]: 표 안 미주"), "GFM 미주 정의: {md}");
        assert!(!md.contains("hwp-footnote"), "HTML 정의 블록 금지: {md}");

        let (back, warnings) = crate::from_markdown::from_markdown_report(
            &md,
            &crate::from_markdown::MarkdownImportOptions::default(),
        )
        .unwrap();
        assert!(
            !warnings.iter().any(|w| w.contains("계약 위반")),
            "계약 위반 경고가 없어야 함: {warnings:?}"
        );
        // Find the fn/en anchors and their bodies inside the restored table cell.
        let mut bodies: Vec<(String, String)> = Vec::new();
        for p in &back.sections[0].paragraphs {
            for c in &p.controls {
                let Control::Table(t) = c else { continue };
                for cell in &t.cells {
                    for cp in &cell.paragraphs {
                        for cc in &cp.controls {
                            if let Control::Generic(g) = cc
                                && matches!(&g.ctrl_id, b"fn  " | b"en  ")
                            {
                                let text: String = g.paragraph_lists[0].paragraphs[0]
                                    .chars
                                    .iter()
                                    .filter_map(|ch| match ch {
                                        HwpChar::Text(c) => Some(*c),
                                        _ => None,
                                    })
                                    .collect();
                                bodies.push((
                                    String::from_utf8_lossy(&g.ctrl_id).trim().to_string(),
                                    text,
                                ));
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(
            bodies,
            vec![
                ("fn".to_string(), "표 안 각주".to_string()),
                ("en".to_string(), "표 안 미주".to_string())
            ],
            "각주/미주 본문 복원: {md}"
        );
        // The marker's back-link (<a href="#fn-N">) must not leak as a hyperlink.
        let cell_text: String = back.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| p.controls.iter())
            .filter_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .flat_map(|t| t.cells.iter())
            .flat_map(|c| c.paragraphs.iter())
            .flat_map(|p| p.chars.iter())
            .filter_map(|ch| match ch {
                HwpChar::Text(c) => Some(*c),
                _ => None,
            })
            .collect();
        assert!(
            !cell_text.contains('1') && !cell_text.contains('2'),
            "마커 숫자는 평문으로 남지 않음: {cell_text}"
        );
    }

    #[test]
    fn html_표_이미지_경로_속성_이스케이프() {
        let mut doc = from_markdown("사진: 여기");
        let png = png_bytes();
        insert_temp_image(&mut doc, "사진:", "md_html_attr.png", &png);
        let cell_para = doc.sections[0].paragraphs.remove(0);
        let mut parent = Paragraph::default();
        insert_table(&mut parent, one_cell_table(cell_para, 2, 2));
        doc.sections[0].paragraphs.push(parent);
        let dir = unique_dir("md_html_attr");

        let md = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                media_prefix: Some("my figs/\"draft\"&more"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            md.contains(r#"src="my figs/&quot;draft&quot;&amp;more/image1.png""#),
            "HTML 속성 이스케이프: {md}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Underline/strikeout/superscript/subscript emit spans.
    #[test]
    fn 글자효과_스팬() {
        let mut doc = from_markdown("효과\n");
        let shapes = [
            CharShape {
                attr: 1 << 2, // Underline below text.
                ..CharShape::default()
            },
            CharShape {
                strike: true,
                ..CharShape::default()
            },
            CharShape {
                attr: 1 << 15, // Superscript.
                ..CharShape::default()
            },
            CharShape {
                attr: 1 << 16, // Subscript.
                ..CharShape::default()
            },
        ];
        let base = doc.header.char_shapes.len() as u16;
        doc.header.char_shapes.extend(shapes);
        let para = &mut doc.sections[0].paragraphs[0];
        para.chars = "ABCD".chars().map(HwpChar::Text).collect();
        para.char_shape_runs = (0..4)
            .map(|i| (i as u32, CharShapeId(base + i as u16)))
            .collect();

        let md = to_markdown(&doc);
        assert!(
            md.contains("<u>A</u>~~B~~<sup>C</sup><sub>D</sub>"),
            "효과 스팬: {md}"
        );
    }

    /// Builds a one-paragraph document with text and (WCHAR start, shape index) runs.
    fn doc_with_runs(text: &str, shapes: &[CharShape], runs: &[(u32, usize)]) -> Document {
        let mut doc = from_markdown("x\n");
        let base = doc.header.char_shapes.len() as u16;
        doc.header.char_shapes.extend_from_slice(shapes);
        let para = &mut doc.sections[0].paragraphs[0];
        para.chars = text.chars().map(HwpChar::Text).collect();
        para.char_shape_runs = runs
            .iter()
            .map(|(pos, idx)| (*pos, CharShapeId(base + *idx as u16)))
            .collect();
        doc
    }

    fn bold_shape() -> CharShape {
        CharShape {
            attr: 1 << 1,
            ..CharShape::default()
        }
    }

    /// Move a bold run's trailing whitespace after the closing marker so GFM parses emphasis.
    #[test]
    fn 강조_후행공백_마커밖으로() {
        // The first part is bold, followed by ordinary text.
        let doc = doc_with_runs(
            "(목적) 예시",
            &[bold_shape(), CharShape::default()],
            &[(0, 0), (5, 1)],
        );
        let md = to_markdown(&doc);
        assert!(md.contains("**(목적)** 예시"), "후행 공백 재배치: {md}");
        assert!(!md.contains("(목적) **"), "닫는 마커 앞 공백 없어야: {md}");
    }

    /// Move a bold run's leading whitespace before its opening marker so GFM parses emphasis.
    #[test]
    fn 강조_선두공백_마커밖으로() {
        // Ordinary text precedes a bold span with leading whitespace.
        let doc = doc_with_runs(
            "앞 강조",
            &[CharShape::default(), bold_shape()],
            &[(0, 0), (1, 1)],
        );
        let md = to_markdown(&doc);
        assert!(md.contains("앞 **강조**"), "선두 공백 재배치: {md}");
        assert!(!md.contains("** 강조"), "여는 마커 뒤 공백 없어야: {md}");
    }

    /// A whitespace-only bold run emits no markers, avoiding `** **`.
    #[test]
    fn 강조_공백뿐이면_마커미방출() {
        // Ordinary text surrounds one bold space.
        let doc = doc_with_runs(
            "앞 뒤",
            &[CharShape::default(), bold_shape()],
            &[(0, 0), (1, 1), (2, 0)],
        );
        let md = to_markdown(&doc);
        assert!(md.contains("앞 뒤"), "공백만 남아야: {md}");
        assert!(!md.contains("**"), "마커 미방출: {md}");
    }

    /// Strikeout also moves trailing whitespace after its closing marker.
    #[test]
    fn 취소선_후행공백_마커밖으로() {
        let strike = CharShape {
            strike: true,
            ..CharShape::default()
        };
        // The struck-out span ends in a space and is followed by ordinary text.
        let doc = doc_with_runs(
            "지운 글",
            &[strike, CharShape::default()],
            &[(0, 0), (3, 1)],
        );
        let md = to_markdown(&doc);
        assert!(md.contains("~~지운~~ 글"), "취소선 후행 공백 재배치: {md}");
        assert!(!md.contains("지운 ~~"), "닫는 마커 앞 공백 없어야: {md}");
    }

    /// HTML-mode tags such as <strong> are whitespace-independent and keep trailing spaces.
    #[test]
    fn html_모드_볼드는_공백유지() {
        // A merged-cell table forces HTML mode; the cell contains a bold span ending in a space.
        let cell_para = Paragraph {
            chars: "가 나".chars().map(HwpChar::Text).collect(),
            char_shape_runs: vec![(0, CharShapeId(0)), (2, CharShapeId(1))],
            ..Paragraph::default()
        };
        let mut doc = from_markdown("표\n");
        doc.header.char_shapes = vec![bold_shape(), CharShape::default()];
        insert_table(
            &mut doc.sections[0].paragraphs[0],
            one_cell_table(cell_para, 2, 2),
        );
        let md = to_markdown(&doc);
        assert!(
            md.contains("<strong>가 </strong>나"),
            "HTML 볼드 공백 유지: {md}"
        );
    }

    // Segment map (feature A).

    /// Character slice for [start, end), matching Python str indexing.
    fn char_slice(md: &str, seg: &MarkdownSegment) -> String {
        md.chars()
            .skip(seg.start)
            .take(seg.end - seg.start)
            .collect()
    }

    /// Text-only top-level paragraph.
    fn text_para(text: &str) -> Paragraph {
        Paragraph {
            chars: text.chars().map(HwpChar::Text).collect(),
            ..Paragraph::default()
        }
    }

    /// Two-row table with a colspan cell, forcing the HTML path.
    fn merged_table() -> Table {
        use hwp_model::{BorderFillId, HwpUnit};
        let cell = |row: u16, col: u16, cs: u16, rs: u16, txt: &str| Cell {
            list_attr: 0,
            col,
            row,
            col_span: cs,
            row_span: rs,
            width: HwpUnit(0),
            height: HwpUnit(0),
            margins: [0; 4],
            border_fill: BorderFillId(0),
            header_tail: vec![],
            paragraphs: vec![text_para(txt)],
        };
        Table {
            common_data: vec![],
            placement: None,
            attr: 0,
            rows: 2,
            cols: 2,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1, 2],
            border_fill: BorderFillId(0),
            table_tail: vec![],
            caption: None,
            cells: vec![
                cell(0, 0, 2, 1, "제목행"),
                cell(1, 0, 1, 1, "가"),
                cell(1, 1, 1, 1, "나"),
            ],
            extras: vec![],
        }
    }

    /// Footnote/endnote control without an inline source anchor.
    fn note_control(id: &[u8; 4], txt: &str) -> Control {
        Control::Generic(GenericControl {
            ctrl_id: *id,
            data: vec![],
            paragraph_lists: vec![ParagraphList {
                header_data: vec![],
                paragraphs: vec![text_para(txt)],
            }],
            extras: vec![],
            raw_children: vec![],
            gso_shapes: vec![],
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        })
    }

    /// Makes the first paragraph an outline heading and returns its index.
    fn make_heading(doc: &mut Document, para_index: usize) {
        use hwp_model::StyleId;
        let style_idx = doc.header.styles.len() as u16;
        doc.header.styles.push(hwp_model::header::Style {
            name: "개요 1".to_string(),
            ..Default::default()
        });
        doc.sections[0].paragraphs[para_index].style = StyleId(style_idx);
    }

    /// (1) Korean paragraphs, including a heading, map character offsets to exact source text;
    /// the multibyte layout would expose accidental byte offsets.
    #[test]
    fn 세그먼트_한국어_문자오프셋_정합() {
        let mut doc = from_markdown("가나다\n\n라마바\n\n사아자\n");
        make_heading(&mut doc, 0); // The text becomes a level-one heading.
        let (md, segs) = to_markdown_with_segments(&doc, &MarkdownOptions::default()).unwrap();

        // (a) Non-ASCII byte counts differ from character counts.
        assert_ne!(
            md.len(),
            md.chars().count(),
            "한국어 포함이면 byte != char: {md:?}"
        );

        // (b)(c) Each slice and (section, paragraph) coordinate matches the expected value.
        let expected = [
            (0usize, 0usize, "# 가나다"),
            (0, 1, "라마바"),
            (0, 2, "사아자"),
        ];
        assert_eq!(
            segs.len(),
            expected.len(),
            "세그먼트 3개: {segs:?} / {md:?}"
        );
        for (seg, (sec, par, text)) in segs.iter().zip(expected) {
            assert_eq!((seg.section, seg.para), (sec, par), "좌표: {seg:?}");
            assert_eq!(char_slice(&md, seg), text, "슬라이스: {seg:?} in {md:?}");
        }
    }

    /// (2) Ordered/disjoint/in-bounds/index invariants and exact equality with to_markdown_with.
    #[test]
    fn 세그먼트_불변식과_문자열_동일() {
        let mut doc = from_markdown("서론 문단\n\n본론 문단\n\n결론 문단\n");
        make_heading(&mut doc, 0);
        // Include a paragraph containing a table to cover the block path.
        insert_table(&mut doc.sections[0].paragraphs[1], merged_table());

        let opts = MarkdownOptions::default();
        let (md, segs) = to_markdown_with_segments(&doc, &opts).unwrap();
        let plain = to_markdown_with(&doc, &opts).unwrap();
        assert_eq!(plain, md, "to_markdown_with와 세그먼트 경로 문자열 동일");

        let n = md.chars().count();
        for w in segs.windows(2) {
            assert!(w[0].start <= w[1].start, "정렬: {:?}", segs);
            assert!(w[0].end <= w[1].start, "비중첩: {:?}", segs);
        }
        for seg in &segs {
            assert!(seg.start < seg.end && seg.end <= n, "범위: {seg:?} (n={n})");
            assert!(seg.section < doc.sections.len(), "섹션 인덱스: {seg:?}");
            assert!(
                seg.para < doc.sections[seg.section].paragraphs.len(),
                "문단 인덱스: {seg:?}"
            );
        }
    }

    /// (3) Table-generated <tr> lines lie inside the containing paragraph's segment range.
    #[test]
    fn 세그먼트_표_문단_상속() {
        let mut doc = from_markdown("표 앞 문단\n\n표 문단\n\n표 뒤 문단\n");
        insert_table(&mut doc.sections[0].paragraphs[1], merged_table());
        let (md, segs) = to_markdown_with_segments(&doc, &MarkdownOptions::default()).unwrap();
        assert!(md.contains("<table>"), "HTML 표 경로: {md}");

        let seg = segs
            .iter()
            .find(|s| s.section == 0 && s.para == 1)
            .expect("표 문단 세그먼트");
        // Every <tr start offset in Markdown must lie inside the containing paragraph's range.
        let tr_offsets: Vec<usize> = md
            .char_indices()
            .enumerate()
            .filter(|(_, (b, _))| md[*b..].starts_with("<tr"))
            .map(|(ci, _)| ci)
            .collect();
        assert!(!tr_offsets.is_empty(), "<tr> 존재: {md}");
        for off in tr_offsets {
            assert!(
                seg.start <= off && off < seg.end,
                "<tr>@{off}가 표 문단 세그먼트 {seg:?} 밖: {md}"
            );
        }
    }

    /// (4) Segments still identify their paragraphs after cleanup deletes consecutive blank lines.
    /// Three hard line breaks create two blank lines; cleanup deletes one.
    #[test]
    fn 세그먼트_cleanup_재매핑() {
        let mut doc = from_markdown("머리 문단\n\n채움 문단\n\n꼬리 문단\n");
        // Create consecutive blank lines with three hard breaks in the middle paragraph.
        let p1 = &mut doc.sections[0].paragraphs[1];
        p1.char_shape_runs = vec![];
        p1.chars = vec![
            HwpChar::Text('앞'),
            HwpChar::CharCtrl(ctrl_char::LINE_BREAK),
            HwpChar::CharCtrl(ctrl_char::LINE_BREAK),
            HwpChar::CharCtrl(ctrl_char::LINE_BREAK),
            HwpChar::Text('뒤'),
        ];
        let (md, segs) = to_markdown_with_segments(&doc, &MarkdownOptions::default()).unwrap();
        assert_eq!(
            to_markdown_with(&doc, &MarkdownOptions::default()).unwrap(),
            md
        );

        let slice_of = |sec: usize, par: usize| -> String {
            let seg = segs
                .iter()
                .find(|s| s.section == sec && s.para == par)
                .unwrap_or_else(|| panic!("세그먼트 ({sec},{par}) 없음: {segs:?}"));
            char_slice(&md, seg)
        };
        // The segments before and after the deletion still point at the correct text.
        assert_eq!(slice_of(0, 0), "머리 문단", "삭제 앞 문단: {md:?}");
        assert_eq!(slice_of(0, 2), "꼬리 문단", "삭제 뒤 문단(재매핑): {md:?}");
        let mid = slice_of(0, 1);
        assert!(
            mid.starts_with('앞') && mid.ends_with('뒤'),
            "가운데 문단이 앞..뒤 범위를 덮어야: {mid:?}"
        );
    }

    /// Unit coverage for cleanup_with_map/DeletionMap offset remapping.
    #[test]
    fn cleanup_삭제맵_오프셋_재매핑() {
        // Three blank lines collapse to one, deleting two.
        let (cleaned, dels) = cleanup_with_map("a\n\n\n\nb\n");
        assert_eq!(cleaned, "a\n\nb\n");
        let map = DeletionMap::new(&dels);
        assert_eq!(map.remap(0), 0);
        assert_eq!(map.remap(1), 1);
        assert_eq!(map.remap(5), 3, "'b'는 원본 byte 5 → 정리본 byte 3");
        // A string without deletions maps identically.
        let (c2, d2) = cleanup_with_map("한\n글\n");
        assert_eq!(c2, "한\n글\n");
        assert!(d2.is_empty());
    }

    /// (5) Footnote-definition segments retain the referring paragraph coordinates.
    #[test]
    fn 세그먼트_각주_정의_귀속() {
        let mut doc = from_markdown("첫째 문단\n\n둘째 문단\n");
        // Attach a footnote to the second paragraph (index 1).
        let para = &mut doc.sections[0].paragraphs[1];
        let idx = para.controls.len() as u32;
        para.controls.push(note_control(b"fn  ", "각주 정의 내용"));
        para.chars.push(HwpChar::ExtCtrl {
            code: ctrl_char::FOOTNOTE_ENDNOTE,
            ctrl_id: *b"fn  ",
            payload: vec![],
            ctrl_index: Some(idx),
        });
        let (md, segs) = to_markdown_with_segments(&doc, &MarkdownOptions::default()).unwrap();
        assert!(md.contains("[^1]: 각주 정의 내용"), "각주 정의 방출: {md}");

        // The slice beginning [^1]: must belong to its referring paragraph (0, 1).
        let def_seg = segs
            .iter()
            .find(|s| char_slice(&md, s).starts_with("[^1]:"))
            .expect("각주 정의 세그먼트");
        assert_eq!(
            (def_seg.section, def_seg.para),
            (0, 1),
            "각주 정의는 참조 문단에 귀속: {def_seg:?}"
        );
    }

    /// (6) Section indices are correct in a two-section document.
    #[test]
    fn 세그먼트_다중_구역() {
        let mut doc = from_markdown("첫 구역 내용\n");
        let mut second = doc.sections[0].clone();
        second.paragraphs[0].char_shape_runs = vec![];
        second.paragraphs[0].chars = "둘째 구역 내용".chars().map(HwpChar::Text).collect();
        doc.sections.push(second);

        let (md, segs) = to_markdown_with_segments(&doc, &MarkdownOptions::default()).unwrap();
        let first = segs
            .iter()
            .find(|s| char_slice(&md, s) == "첫 구역 내용")
            .expect("첫 구역 세그먼트");
        let sec2 = segs
            .iter()
            .find(|s| char_slice(&md, s) == "둘째 구역 내용")
            .expect("둘째 구역 세그먼트");
        assert_eq!(first.section, 0, "첫 구역 section: {first:?}");
        assert_eq!(sec2.section, 1, "둘째 구역 section: {sec2:?}");
    }

    /// (7) One-line table-row invariant: HTML lines have equal opening/closing tr counts, including
    /// nested tables on one line. Trimmed GFM rows start and end with a pipe.
    #[test]
    fn 표_행_한줄_불변식_단위() {
        // A nested 1x1 table shares the outer row's line when serialized inline.
        let inner = one_cell_table(text_para("속"), 1, 1);
        let mut cell_para = Paragraph::default();
        insert_table(&mut cell_para, inner);
        let outer = one_cell_table(cell_para, 1, 1);
        let mut doc = from_markdown("표\n");
        insert_table(&mut doc.sections[0].paragraphs[0], outer);
        let md = to_markdown(&doc);
        assert!(
            md.contains("<table><tr><td>속</td></tr></table>"),
            "중첩 표가 한 줄로 인라인: {md}"
        );
        for line in md.lines() {
            assert_eq!(
                line.matches("<tr").count(),
                line.matches("</tr>").count(),
                "행 한 줄 불변식 위반: {line:?}"
            );
        }

        // GFM table: no merged cells, so rows use pipe syntax.
        let gfm = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let md2 = to_markdown(&gfm);
        let mut pipe_rows = 0;
        for line in md2.lines() {
            let t = line.trim();
            if t.starts_with('|') {
                pipe_rows += 1;
                assert!(
                    t.ends_with('|'),
                    "파이프 행이 '|'로 끝나야: {line:?} in {md2}"
                );
            }
        }
        assert!(pipe_rows >= 2, "파이프 표 행이 있어야: {md2}");
    }

    fn equation_control(script: &str, inline: bool) -> Control {
        Control::Generic(GenericControl {
            ctrl_id: *b"eqed",
            data: vec![],
            paragraph_lists: vec![],
            extras: vec![],
            raw_children: vec![],
            gso_shapes: vec![],
            equation: Some(Equation {
                script: script.to_string(),
                width: 0,
                height: 0,
                inline,
                x: 0,
                y: 0,
                ..Equation::default()
            }),
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        })
    }

    fn control_anchor(index: u32, id: &[u8; 4]) -> HwpChar {
        HwpChar::ExtCtrl {
            code: ctrl_char::OBJECT,
            ctrl_id: *id,
            payload: vec![],
            ctrl_index: Some(index),
        }
    }

    fn insert_table(paragraph: &mut Paragraph, table: Table) {
        let index = paragraph.controls.len() as u32;
        paragraph.controls.push(Control::Table(table));
        paragraph.chars.push(control_anchor(index, b"tbl "));
    }

    fn one_cell_table(paragraph: Paragraph, cols: u16, col_span: u16) -> Table {
        use hwp_model::{BorderFillId, HwpUnit};
        Table {
            common_data: vec![],
            placement: None,
            attr: 0,
            rows: 1,
            cols,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1],
            border_fill: BorderFillId(0),
            table_tail: vec![],
            caption: None,
            cells: vec![Cell {
                list_attr: 0,
                col: 0,
                row: 0,
                col_span,
                row_span: 1,
                width: HwpUnit(0),
                height: HwpUnit(0),
                margins: [0; 4],
                border_fill: BorderFillId(0),
                header_tail: vec![],
                paragraphs: vec![paragraph],
            }],
            extras: vec![],
        }
    }

    fn png_bytes() -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(96u32.to_be_bytes());
        png.extend(96u32.to_be_bytes());
        png.extend([0u8; 8]);
        png
    }

    /// Inserts `data` as an image at `anchor` through a per-process temp file, removed once the
    /// image is read (#349).
    fn insert_temp_image(doc: &mut Document, anchor: &str, name: &str, data: &[u8]) {
        let p = std::env::temp_dir().join(format!("hwp-md-test-{}-{name}", std::process::id()));
        std::fs::write(&p, data).unwrap();
        crate::image::insert_image(doc, anchor, &p, crate::image::ImageSize::Natural).unwrap();
        let _ = std::fs::remove_file(&p);
    }

    fn unique_dir(stem: &str) -> std::path::PathBuf {
        let uniq = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{stem}_{uniq}"))
    }
}
