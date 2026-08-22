[한국어](23-hwpx-skill-absorption.ko.md) · [English](23-hwpx-skill-absorption.md)

# hwpx skill absorption: subcommand parity matrix and margin record

> **Status:** living checklist. Tracked by
> [issue #121](https://github.com/STAIxBWLB/hwp-cli/issues/121) (amended 2026-08-20: one unified
> skill, not two — D-01) and [skills#35](https://github.com/STAIxBWLB/skills/issues/35) (old-skill
> retirement). Phases 2.2 through 2.5 update this matrix **in place** as rows are resolved; it is
> the seed list for Phase 2.5's "nothing lost" proof. No status cell may be upgraded to "verified"
> without the evidence named in the row.

## 1. Scope

The user-scope `hwpx` skill (a Python package driving this binary from outside) is absorbed into
the single bundled skill `skills/hwp`: its prose, regulation reference and templates are embedded
in the binary and exported as a directory tree by `hwp skill export`. This document records the
two things that must not be silently lost or silently claimed:

1. **Parity** — every old `./hwpx` subcommand and every script-level engine guarantee mapped to a
   native command, a named gap, or an explicit drop (§2).
2. **The margin record** — the verified official-profile defaults and their regulatory boundary
   (§3).

Phase map: **2.1** scaffold + this matrix; **2.2** statutory 8-level numbering, preset family and
verified margin correction (GONG-01/02); **2.3** `hwp lint` (GONG-03's notation half);
**2.4** document frames, `--template`, table styling (GN-4, GN-5, GN-6); **2.5** editing parity
proof + old-skill retirement (EDIT-01, RET-01).

## 2. Parity matrix

One row per old `./hwpx` subcommand — **28** `add_parser` calls recounted verbatim from
`scripts/hwpx_cli.py` (lines 1385–1591) on 2026-08-20; CONTEXT's earlier "27" undercounted the
two aliases — plus one row per script-level guarantee the old scripts enforced outside the CLI
surface. `render-pdf` and `write-java` are aliases and marked as such.

Status legend: **verified** = re-measured against native source this phase; **inferred** = mapped
by reading the old-skill source, pending proof in the named phase; **resolved by absorption** =
the concern disappears because the skill now ships inside the binary.

### 2.1 Old subcommands (28)

| Old subcommand | Native equivalent | Phase | Status |
|---|---|---|---|
| read | `hwp cat` (+ `--format json` for structure) | 2.1 | verified |
| summary | none direct (`hwp info` + `hwp cat --format json`) | 2.5 | gap → EDIT-01 recipe (inferred) |
| to-md | `hwp convert --to md --media-dir` | 2.1 | verified |
| unpack | none | 2.5 | gap → raw-zip recipe / EDIT-01 (inferred) |
| repack | none (native writers guarantee the package layout) | 2.5 | gap → raw-zip recipe / EDIT-01 (inferred) |
| fill | `hwp fill` | 2.1 | verified — default path only; run-spanning fill is the §2.2 gap row, NOT covered here |
| slots | `hwp slots` | 2.1 | verified |
| edit | `hwp edit --replace` (not run-spanning) | 2.5 | partial → EDIT-01 proof item (inferred) |
| add-rows | `hwp edit --add-row` | 2.1 | verified |
| add-col | `hwp edit --add-col` | 2.1 | verified |
| fill-table | `hwp fill --data tables.json` | 2.1 | verified (data-driven row fill exists) |
| create | `hwp new --from` | 2.1 | verified |
| styled | `hwp new --preset official|report|plan|notice|minutes|press` | 2.2 | verified for profiles, numbering and layout; style pass remains absent |
| beautify | none | 2.4 | gap → `--style-tables` (GONG-03, inferred) |
| validate | `hwp validate` | 2.1 | verified |
| analyze | none | 2.5 | gap → EDIT-01 documented recipe (inferred) |
| guard | none (`hwp render --report` gives page counts) | 2.5 | gap → EDIT-01 documented recipe (inferred) |
| edit-section | none direct | 2.5 | gap → EDIT-01 documented recipe (inferred) |
| fill-form | none direct | 2.5 | gap → `--set-cell-by-label` (EDIT-01, inferred) |
| to-pdf | `hwp convert --to pdf` / `hwp render` | 2.1 | verified — the old soffice fallback is **intentionally dropped** (native engine only) |
| render-pdf | same as to-pdf | 2.1 | verified (**alias** of `to-pdf --engine hwp`) |
| to-html | `hwp cat --format html` | 2.1 | verified |
| info | `hwp info` | 2.1 | verified |
| fields | `hwp fields` | 2.1 | verified |
| bookmarks | `hwp bookmarks` | 2.1 | verified |
| render | `hwp render` | 2.1 | verified |
| convert | `hwp convert` | 2.1 | verified |
| write-java | `hwp new --from` | 2.1 | verified (**alias**, legacy name) |

### 2.2 Script-level guarantees (7)

| Old script guarantee | Native equivalent | Phase | Status |
|---|---|---|---|
| run-spanning `{{slot}}` fill | none today — native `hwp fill` is a raw-XML string replace (`crates/hwpx/src/patch.rs:52-56`), so a slot split across `<hp:t>` runs does not match | 2.5 | gap → EDIT-01 (inferred) — **not verified parity**; templates authored via `hwp new --from` must keep each slot inside one run |
| `linesegarray` clearing on edit | engine-inherent: the native IR round-trip rewrites line segments, and the byte-preserving patch path never edits text | 2.5 | engine-inherent, confirm in 2.5 (inferred) |
| sec-index section edits | none | 2.5 | gap → EDIT-01 documented recipes (inferred) |
| mimetype-first STORED repack | native writers obey the package layout; no native replacement for the raw-zip path | 2.5 | resolved for the writer path; raw-zip recipe gap → EDIT-01 (inferred) |
| style_pass table rules | none | 2.4 | gap → GONG-03 (inferred) — **not verified parity** |
| page_guard structural drift checks | none | 2.5 | gap → EDIT-01 documented recipes (inferred) — **not verified parity** |
| binary discovery (`$HWP_CLI`, highest-version selection) | obsolete — the skill ships inside the binary it drives | 2.1 | resolved by absorption |

## 3. Margin record (D-14)

**Question:** does the official-profile margin set have a regulatory source?

**Current engine behavior (verified):** each canonical profile starts with **top 20 / bottom 10 /
left 20 / right 20 mm**. `official` has no header/footer margin or page number; `report` and `plan`
use 15 mm header/footer margins and `- N -`; `notice` and `press` use 10 mm and
`- N -`; `minutes` has neither. A caller may override one side through the four `hwp new`
`--margin-*` flags or the matching MCP `margin_*_mm` inputs.

**Evidence (secondary source):** kordoc's statute compilation refutes the retired top-30 value —
`gongmunseo-reference.md` §3.2: *"'위 30mm' 같은 수치는 어느 권위 출처에도 없음"* (no
authoritative source carries "top 30 mm"; marked refuted), and records the 2020 행정업무운영
편람 official set as **top 20 / bottom 10 / left 20 / right 20 mm** (header/footer/gutter 0).
The old `hwpx` skill's reference asserted 30/15/20/15 without a citation, which is where the
preset's values trace to.

**Verdict (closed 2026-08-22):** top 30 mm was unsourced; Phase 2.2 changed every canonical
profile to 20/10/20/20 and verified the resulting profile layout in 14 of 14 genuine Hancom
HWP/HWPX observations. GN-9 is resolved at that verified boundary. This closes only profile
margins and numbering/layout behavior; `hwp lint`, document frames, `--template`, table styling
and editing parity remain deferred to their named phases.

## 4. Decisions recorded

- **Q1 — templates have no `.ko.md` mirrors** (D-11): the eight `skills/hwp/templates/*.md` files
  have Korean bodies by design; EN/KO mirrors would be empty duplicates. The drift and parity
  gates exclude `templates/` from the mirror requirement and the parity walk (02.1-01).
- **Q2 — `claude-web/` is excluded from the embedded table and the export** (02.1-01):
  `skills/hwp/claude-web/bootstrap.sh` is a repo/release artifact, not skill content; the drift
  walk excludes it too.
- **Q3 — the release web bundle keeps shipping SKILL.md only this phase**:
  `hwp-skill-claude-web.zip` continues to contain `SKILL.md`, `bootstrap.sh` and the Linux x86_64
  `bin/hwp`. Deferred idea (recorded, not scheduled): a tree-shaped web bundle once the claude.ai
  sandbox skills UI is re-checked.

## 5. Sources

- **kordoc** (`~/workspace/references/ai-tools/kordoc`, MIT) — credited as the **secondary source**
  for the regulation compilation used here, in particular `docs/gongmunseo-reference.md` §3.2
  (margin refutation). Rules are restated from the statutes, not copied.
- **jkf87/hwpx-skill** — rule ancestry of parts of the old `hwpx` skill's lint/gonmun rule set is
  acknowledged. The repository carries **no license** (verified 2026-08-20 via the GitHub API:
  `license: null`, no LICENSE file), so **no rule text is copied** from it into this repository;
  this one-line acknowledgment is the full extent of the reuse.
- **Old `hwpx` skill** (`STAIxBWLB/skills`, read-only) — the subcommand inventory of §2.1 is
  recounted from `scripts/hwpx_cli.py` and the engine mapping from reading each `cmd_*` body
  (research of 2026-08-20).
- **Primary regulation sources** (cited by article, not reproduced): 행정기관의 업무효율화를 위한
  규칙 and the 2020 행정업무운영 편람. Per D-12, individual rules carry statute article + 편람 page
  + a confidence tag in `skills/hwp/references/korean-official-format.md`.
