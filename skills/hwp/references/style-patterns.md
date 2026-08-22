[한국어](style-patterns.ko.md) · [English](style-patterns.md)

# Official-document and university style patterns (workspace corpus analysis)

Evidence document behind the `style_pass.py` defaults. Extracted 2026-07 from the
`~/workspace/work` real-document corpus (698 .hwpx files found, 137 XML-parsed and
tallied + 11 deep-dives + 6 bundled templates).

## Unit conversion

| Item | Unit | Conversion |
|------|------|------------|
| `charPr@height` | pt×100 | 1000 = 10pt |
| length HWPUNIT | 1/7200 inch | 283.46 = 1mm, 7200 = 25.4mm |
| A4 page | 59528×84186 | 210×297mm |
| body width (Regime A) | 42520 | ≈150mm (A4 − 30mm left/right margins) |

## Body styles — two regimes

### Regime A — official documents, drafts, minutes, regulations (default; matches the `hwp new` default)

- Margins L/R/T/B/H/F = `8504/8504/5668/4252/4252/4252` = **30/30/20/15/15/15mm**
- Body **함초롬바탕 10pt**, alignment **JUSTIFY**, line spacing **PERCENT 160**
- Title/headings 함초롬돋움, document title **15pt centered** (corpus convention)
- All 6 bundled templates (기안문·공문서·보고서·사업계획서·회의록) use this style

### Regime B — reports, plans, external forms (dense-content documents)

- Margins ≈ L/R `5669` (20mm), T/B `2834`–`4251` (10–15mm) — the most frequent single margin set in the corpus
- Body **휴먼명조 11–12pt** (most common in real documents, 68 files) or **맑은 고딕 11–12pt** (21 files, university/external forms)
- Title 15pt (1500). Line spacing mixed 120–160%
- Outside style_pass coverage (font replacement risks render-font availability) — follow-up if needed

### Corpus distribution (137 files)

- Body font: 휴먼명조 68 > 함초롬바탕 30 > 맑은 고딕 21 (plus 굴림체, KoPubWorld, etc.)
- Body size: 10pt 37 / 12pt 34 / 11pt 31 / 15pt 14
- Line spacing: **160% (76)** > 130% (21) > 120% (11)

## Table conventions (core style_pass targets)

### Vertical alignment

Cell `vertAlign="CENTER"` is effectively universal (header and body alike). `TOP`
appears only in multi-paragraph narrative cells (KOICA narrative tables, regulation
comparison tables).

### Header row

- Shading + horizontal **CENTER** + vertical CENTER. Bold is common (not universal).
- Observed shading palette:
  - Neutral grays: **`#F2F2F2`** (most frequent, the default), `#CCCCCC`, `#D6D6D6`
  - Light blues: **`#D9E2F3`**, `#D8E5F5`, `#E0E5FA` (preferred by RISE/글로컬 forms)
  - Creams: `#FCF5E7`, `#F7F6F1`
  - Navy banners (white text, full-width merged title row): `#092E99`, `#1A3072`

### Body cell alignment

By content type — labels/sequence numbers/figures/short columns → **CENTER**;
long-text/narrative columns → LEFT/JUSTIFY.

### Column width ratios (cellSz@width HWPUNIT observations)

| Pattern | Observed ratio | Example |
|---------|----------------|---------|
| Even (4-col minutes etc.) | 1:1:1:1 | meetings/…/산학협력회의록_통합 |
| 2-col label:value | **1:3 – 1:5** (label 7200–8500 = 25–30mm) | KOICA 출장보고 개요 7756:41849 |
| 2-col comparison (current/revised) | **1:1** | 규정 개정 대비표 23808:23910 |
| 4-col number·category·content·note | 1 : 1.4 : 3.7 : 3.9 | KOICA T2 4082:5847:15104:15738 |
| 5-col roster (no.·affiliation·name·position·contact) | 1 : 4.7 : 3.7 : 2.7 : 2 | rise 참석자 명단 서식 |
| full-width banner title row | merged single cell = full width + dark shading | 다수 서식 T1 |

- Narrow sequence-number columns: minimum ≈3400 hwpu (12mm)
- Row height follows content (no fixed-height or special header-height convention)

## style_pass.py defaults (user-confirmed 2026-07-02)

- Header-row shading **#F2F2F2** + bold + center/center (changeable via `--header-fill`)
- Document title (H1) **centered 15pt bold** (`--no-title-center` opt-out)
- Column widths: proportional to content display width; 2-col label:value special-cased to
  1:3~1:4; even content stays even; minimum 3400 hwpu; total table width preserved
- Short columns (display width ≤ 8): body cells CENTER
- Applied: automatically on all template-less generation paths (`--plain` opt-out);
  existing files via `beautify`
