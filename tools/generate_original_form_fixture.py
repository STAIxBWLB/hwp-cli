#!/usr/bin/env python3
"""Generate owner-authored DocumentSpec fixtures for the original-form contract.

Usage: python3 tools/generate_original_form_fixture.py OUTPUT_DIR
Then compose original-form.spec.json into .hwpx or .hwp with the existing CLI.
Only JSON and tiny PNG assets are generated here; no native bytes are authored.
"""

from __future__ import annotations

import argparse
import binascii
import hashlib
import json
from pathlib import Path
import struct
from typing import Any
import zlib

GENERATOR_VERSION = 1
SPEC_NAME = "original-form.spec.json"
MANIFEST_NAME = "expected.json"
COLUMNS_MM = [30, 50, 30, 50]
FONT_FAMILY = "Noto Sans CJK KR"


def paragraph(text: str, style: str = "body") -> dict[str, Any]:
    return {"type": "paragraph", "style": style, "runs": [{"type": "text", "text": text}]}


def cell(*texts: str, col_span: int = 1, row_span: int = 1) -> dict[str, Any]:
    return {
        "col_span": col_span,
        "row_span": row_span,
        "blocks": [paragraph(text) for text in texts],
    }


def form_table(title: str, rows: list[list[dict[str, Any]]]) -> dict[str, Any]:
    return {
        "type": "table",
        "width_mm": sum(COLUMNS_MM),
        "columns": [{"width_mm": width} for width in COLUMNS_MM],
        "rows": [{"cells": [cell(title, col_span=4)]}]
        + [{"cells": cells} for cells in rows],
    }


def signature_rows() -> list[list[dict[str, Any]]]:
    return [
        [cell("Signature"), cell("Signature area (leave unchanged)", col_span=3)],
        [
            cell("Approval", row_span=2),
            cell("Prepared by"),
            cell("Reviewed by"),
            cell("Approved by"),
        ],
        # The first column is occupied by the row-spanning Approval cell.
        [cell(" "), cell(" "), cell(" ")],
    ]


def png(pixels: list[tuple[int, int, int]]) -> bytes:
    """Encode fixed 2x2 RGB pixels with a deterministic uncompressed zlib stream."""
    assert len(pixels) == 4
    raw = b"".join(
        bytes([0]) + bytes(channel for pixel in pixels[row * 2 : row * 2 + 2] for channel in pixel)
        for row in range(2)
    )
    # One final stored DEFLATE block avoids compression-version-dependent output.
    compressed = (
        bytes([120, 1, 1])
        + struct.pack("<HH", len(raw), len(raw) ^ 65535)
        + raw
        + struct.pack(">I", zlib.adler32(raw) & 0xFFFFFFFF)
    )

    def chunk(kind: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", binascii.crc32(kind + data) & 0xFFFFFFFF)
        )

    return (
        bytes([137, 80, 78, 71, 13, 10, 26, 10])
        + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 2, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", compressed)
        + chunk(b"IEND", b"")
    )


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def json_bytes(value: Any) -> bytes:
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode("utf-8")


def table_expectation(
    table: dict[str, Any], table_id: str, index: int, paragraph_index: int
) -> dict[str, Any]:
    """Record authored expectations, including actual merged-cell coordinates."""
    rows = table["rows"]
    occupied = [[False] * len(COLUMNS_MM) for _ in rows]
    expected_cells = []
    for row_index, row in enumerate(rows):
        col = 0
        for authored in row["cells"]:
            while col < len(COLUMNS_MM) and occupied[row_index][col]:
                col += 1
            col_span, row_span = authored["col_span"], authored["row_span"]
            assert col + col_span <= len(COLUMNS_MM)
            assert row_index + row_span <= len(rows)
            for rr in range(row_index, row_index + row_span):
                for cc in range(col, col + col_span):
                    assert not occupied[rr][cc], "overlapping authored cells"
                    occupied[rr][cc] = True
            texts = []
            images = []
            for block in authored["blocks"]:
                if block["type"] == "paragraph":
                    texts.append("".join(run["text"] for run in block["runs"]))
                elif block["type"] == "image":
                    images.append(block["path"])
                else:
                    raise AssertionError("unexpected fixture cell block")
            expected_cells.append(
                {
                    "row": row_index,
                    "col": col,
                    "row_span": row_span,
                    "col_span": col_span,
                    "width_mm": sum(COLUMNS_MM[col : col + col_span]),
                    "paragraph_texts": texts,
                    "image_paths": images,
                }
            )
            col += col_span
    assert all(all(row) for row in occupied), "uncovered authored grid cell"
    return {
        "id": table_id,
        "table_index": index,
        "section": 0,
        "paragraph": paragraph_index,
        "rows": len(rows),
        "cols": len(COLUMNS_MM),
        "columns_mm": COLUMNS_MM,
        "width_mm": sum(COLUMNS_MM),
        "cells": expected_cells,
    }


def generate() -> dict[str, bytes]:
    inside = png([(40, 120, 200), (240, 240, 240), (240, 240, 240), (40, 120, 200)])
    outside = png([(220, 100, 40), (20, 20, 20), (20, 20, 20), (220, 100, 40)])
    assert inside != outside
    blocks: list[dict[str, Any]] = [
        paragraph("UNRELATED_PREFIX: excluded cover material"),
        {
            "type": "image",
            "path": "outside.png",
            "width_mm": 8,
            "height_mm": 8,
            "alt": "Owner-authored outside-form orange checker",
            "placement": "inline",
        },
    ]
    forms = []
    tables = []

    def append_form(form_id: str, marker: str, table: dict[str, Any]) -> None:
        # DocumentSpec compilation emits one section-setup paragraph before all blocks.
        break_paragraph = len(blocks) + 1
        blocks.append({"type": "break", "kind": "page"})
        heading_paragraph = len(blocks) + 1
        blocks.append(paragraph(marker, "heading"))
        table_paragraph = len(blocks) + 1
        blocks.append(table)
        tables.append(table_expectation(table, form_id, len(tables), table_paragraph))
        forms.append(
            {
                "id": form_id,
                "marker": marker,
                "section": 0,
                "paragraph": heading_paragraph,
                "break_paragraph": break_paragraph,
                "table_paragraph": table_paragraph,
                "spec_block_index": heading_paragraph - 1,
            }
        )

    retained = cell("Retained image cell marker", col_span=3)
    retained["blocks"].append(
        {
            "type": "image",
            "path": "inside.png",
            "width_mm": 8,
            "height_mm": 8,
            "alt": "Owner-authored retained blue checker",
            "placement": "inline",
        }
    )
    application = form_table(
        "APPLICATION_FORM",
        [
            [cell("Course"), cell("Synthetic Methods"), cell("Year"), cell("2030")],
            [cell("Applicant"), cell("Sample Author"), cell("Hours"), cell("90")],
            [cell("Emblem"), retained],
        ]
        + signature_rows(),
    )
    append_form("application", "FORM_START_APPLICATION", application)
    allocations = [("Module A", "18", "12"), ("Module B", "24", "6"), ("Module C", "20", "10")]
    development = form_table(
        "DEVELOPMENT_REPORT",
        [
            [cell("Course"), cell("Synthetic Methods"), cell("Year"), cell("2030")],
            [cell("Module"), cell("Lecture"), cell("Practice"), cell("Subtotal")],
        ]
        + [[cell(name), cell(lecture), cell(practice), cell("30")] for name, lecture, practice in allocations]
        + [[cell("Total"), cell("62"), cell("28"), cell("90")]]
        + signature_rows(),
    )
    append_form("development-report", "FORM_START_DEVELOPMENT", development)
    for number in range(1, 5):
        profile = form_table(
            f"PROFILE_{number}",
            [
                [cell("Course"), cell("Synthetic Methods"), cell("Year"), cell("2030")],
                [cell("Profile"), cell(f"Synthetic profile {number}"), cell("Hours"), cell("90")],
                [
                    cell("Objective"),
                    cell(
                        f"PROFILE_{number}_VALUE: preserve every requested value.",
                        "Nested item: original wording remains unchanged.",
                        col_span=3,
                    ),
                ],
            ]
            + signature_rows(),
        )
        append_form(f"profile-{number}", f"FORM_START_PROFILE_{number}", profile)
    styles = {
        "body": {
            "font_family": FONT_FAMILY,
            "font_size_pt": 10,
            "align": "left",
            "margin_left_mm": 0,
            "margin_right_mm": 0,
            "spacing_before_pt": 0,
            "spacing_after_pt": 0,
            "line_height_percent": 120,
        },
        "heading": {
            "based_on": "body",
            "bold": True,
            "font_size_pt": 12,
            "spacing_before_pt": 0,
            "spacing_after_pt": 0,
        },
    }
    spec = {
        "version": "2.0",
        "document": {
            "version": "1.0",
            "metadata": {
                "title": "Owner-authored synthetic original-form contract fixture",
                "author": "hwp-cli regression fixture",
                "description": "Generated project test data, without private or third-party source forms.",
            },
            "page": {
                "width_mm": 210,
                "height_mm": 297,
                "orientation": "portrait",
                "margin_left_mm": 20,
                "margin_right_mm": 20,
                "margin_top_mm": 20,
                "margin_bottom_mm": 20,
                "margin_header_mm": 0,
                "margin_footer_mm": 0,
                "gutter_mm": 0,
            },
            "styles": styles,
            "sections": [{"id": "original-forms", "blocks": blocks}],
        },
        "tables": {"placement": "floating"},
        "visuals": [],
    }
    spec_data = json_bytes(spec)
    manifest = {
        "schema": "hwp-original-form-fixture-v1",
        "generator": {"path": "tools/generate_original_form_fixture.py", "version": GENERATOR_VERSION},
        "provenance": {
            "kind": "owner-authored-synthetic",
            "source_materials": [],
            "image_origin": "Fixed original 2x2 RGB checker pixels encoded by this generator.",
            "private_or_third_party_forms": False,
            "hancom_spec_derivatives": False,
            "fonts_bundled": False,
            "native_open_evidence": "unverified",
        },
        "spec": {"path": SPEC_NAME, "sha256": digest(spec_data), "version": "2.0", "base_version": "1.0"},
        "fonts": {"requested_family": FONT_FAMILY, "identity": "unpinned; supply explicit font bytes for page/pixel evidence"},
        "limitations": [
            "CI compares structure and relative anchors, not golden page counts or pixels.",
            "Zero paragraph before/after spacing avoids the unresolved #415 unit calibration.",
            "Table borders and cell margins use existing DocumentSpec compiler defaults.",
            "Expected paragraph indices describe DocumentSpec compilation, not post-extraction addresses.",
            "Page-break blocks compile as separate body paragraphs before the heading markers.",
            "Native validation and CLI rendering do not establish Hancom compatibility.",
        ],
        "form_starts": forms,
        "tables": tables,
        "images": [
            {"role": "retained", "path": "inside.png", "sha256": digest(inside), "width_px": 2, "height_px": 2,
             "location": {"table_id": "application", "row": 3, "col": 1}},
            {"role": "excluded", "path": "outside.png", "sha256": digest(outside), "width_px": 2, "height_px": 2,
             "location": {"section": 0, "paragraph": 2}},
        ],
        "expected": {
            "table_count": 6,
            "profile_count": 4,
            "image_count": 2,
            "form_start_count": 6,
            "allocation_rows": [{"module": name, "lecture": int(lecture), "practice": int(practice), "subtotal": 30}
                                for name, lecture, practice in allocations],
            "total_hours": 90,
            "stable_labels": ["Course", "Year", "Signature", "Approval"],
        },
    }
    assert len(forms) == len(tables) == 6
    assert len({form["marker"] for form in forms}) == 6
    assert all(blocks[form["paragraph"] - 1]["runs"][0]["text"] == form["marker"] for form in forms)
    assert all(blocks[form["break_paragraph"] - 1] == {"type": "break", "kind": "page"} for form in forms)
    return {SPEC_NAME: spec_data, "inside.png": inside, "outside.png": outside, MANIFEST_NAME: json_bytes(manifest)}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output_dir", type=Path)
    args = parser.parse_args()
    artifacts = generate()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    for name, data in artifacts.items():
        (args.output_dir / name).write_bytes(data)
    print(f"Generated {len(artifacts)} synthetic fixture artifacts in {args.output_dir}")


if __name__ == "__main__":
    main()
