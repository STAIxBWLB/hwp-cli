#!/usr/bin/env python3
"""Regression tests for the Hancom PDF parity runner."""

import copy
import hashlib
import json
import struct
import subprocess
import tempfile
import unittest
import zlib
from pathlib import Path
from unittest import mock

from tools import pdf_parity as parity


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def concrete_manifest() -> dict:
    return {
        "pins": {
            "hancom_build": "Hancom Office 2024 13.0.0.1",
            "windows_version": "Windows 11 24H2",
            "pdf_settings": "File > Save as PDF, defaults",
            "poppler_version": "pdfinfo version 26.0.0",
            "dpi": 150,
            "fonts": {},
        },
        "cases": [],
    }


class ManifestPinTests(unittest.TestCase):
    def test_poppler_and_font_hashes_are_enforced(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            font_dir = Path(temporary)
            font = font_dir / "Pinned.ttf"
            font.write_bytes(b"pinned font")
            manifest = concrete_manifest()
            manifest["pins"]["fonts"] = {"Pinned": digest(b"pinned font")}
            with mock.patch.dict("os.environ", {"HWP_FONT_DIR": str(font_dir)}), mock.patch.object(
                parity, "poppler_version", return_value="pdfinfo version 26.0.0"
            ):
                self.assertEqual(
                    parity.verify_manifest_pins(manifest),
                    "pdfinfo version 26.0.0",
                )

            manifest["pins"]["fonts"]["Pinned"] = "0" * 64
            with mock.patch.dict("os.environ", {"HWP_FONT_DIR": str(font_dir)}), mock.patch.object(
                parity, "poppler_version", return_value="pdfinfo version 26.0.0"
            ), self.assertRaises(SystemExit):
                parity.verify_manifest_pins(manifest)

    def test_placeholder_metadata_is_rejected(self) -> None:
        manifest = concrete_manifest()
        manifest["pins"]["hancom_build"] = "TODO-owner"
        with self.assertRaises(SystemExit):
            parity.verify_manifest_pins(manifest)


class CasePreflightTests(unittest.TestCase):
    def test_case_hashes_are_verified_and_duplicate_names_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest_path = root / "public" / "manifest.json"
            source_dir = manifest_path.parent / "source"
            oracle_dir = root / "oracle"
            source_dir.mkdir(parents=True)
            oracle_dir.mkdir()
            (source_dir / "case.hwp").write_bytes(b"source")
            (oracle_dir / "case.pdf").write_bytes(b"oracle")
            case = {
                "name": "case",
                "source": "case.hwp",
                "source_sha256": digest(b"source"),
                "oracle": "case.pdf",
                "oracle_sha256": digest(b"oracle"),
            }
            manifest = {"cases": [case]}
            prepared = parity.prepare_cases(manifest, manifest_path, oracle_dir)
            self.assertEqual(prepared[0]["source_sha256"], digest(b"source"))

            manifest["cases"] = [case, case.copy()]
            with self.assertRaises(SystemExit):
                parity.prepare_cases(manifest, manifest_path, oracle_dir)

    def test_case_file_cannot_escape_its_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            allowed = root / "allowed"
            allowed.mkdir()
            (root / "secret.hwp").write_bytes(b"secret")
            with self.assertRaises(SystemExit):
                parity.resolve_case_file(allowed, "../secret.hwp", "source")


class ScoreGateTests(unittest.TestCase):
    def score_with(self, coverage, font_rows) -> dict:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "case.hwp"
            oracle = root / "case.pdf"
            source.write_bytes(b"source")
            oracle.write_bytes(b"oracle")
            metrics = {
                "dx": 0,
                "dy": 0,
                "ink_ratio": 1.0,
                "bad_pixel_pct": 0.0,
                "mae": 0.0,
            }
            with mock.patch.object(parity, "render_ours", return_value=coverage), mock.patch.object(
                parity, "pdf_pages", return_value=1
            ), mock.patch.object(parity, "pdf_fonts", side_effect=font_rows), mock.patch.object(
                parity, "page_text", return_value="same"
            ), mock.patch.object(parity, "rasterize", return_value=root / "page.png"), mock.patch.object(
                parity, "raster_diff", return_value=metrics
            ):
                return parity.score_case(
                    "case", source, oracle, root / "hwp", 150, root
                )

    def test_missing_font_coverage_fails_closed(self) -> None:
        valid_font = {
            "name": "Pinned",
            "type": "TrueType",
            "embedded": True,
            "subset": True,
            "unicode": True,
        }
        card = self.score_with(None, [[valid_font], [valid_font]])
        self.assertFalse(card["scored"])
        self.assertFalse(card["substitution_free"])
        self.assertIn("font_coverage_unavailable", card["unscored_reasons"])

    def test_pdf_font_contract_applies_to_both_pdfs(self) -> None:
        valid_font = {
            "name": "Pinned",
            "type": "TrueType",
            "embedded": True,
            "subset": True,
            "unicode": True,
        }
        invalid_font = {**valid_font, "unicode": False}
        coverage = {
            "matched": 1,
            "substituted": 0,
            "missing": 0,
            "subset_fallback": 0,
            "substitution_free": True,
        }
        card = self.score_with(coverage, [[valid_font], [invalid_font]])
        self.assertFalse(card["scored"])
        self.assertFalse(card["fonts"]["all_embedded_subset_unicode"])
        self.assertIn("pdf_font_contract", card["unscored_reasons"])


def tiny_png(width: int, height: int, ink: set[tuple[int, int]]) -> bytes:
    """Create an 8-bit grayscale PNG for ROI tests without Pillow."""
    rows = []
    for y in range(height):
        rows.append(bytes(0 if (x, y) in ink else 255 for x in range(width)))
    raster = b"".join(b"\x00" + row for row in rows)

    def chunk(kind: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raster))
        + chunk(b"IEND", b"")
    )


class V2ContractTests(unittest.TestCase):
    def test_render_report_json_and_stderr_fallback_are_normalized(self) -> None:
        report = parity.normalize_render_report(
            {
                "complete": True,
                "font_coverage": {
                    "matched": 4,
                    "substituted": 0,
                    "missing": 0,
                    "subset_fallback": 0,
                },
                "issues": [
                    {"code": "picture_effects_unsupported", "severity": "warning", "count": 2},
                    {"code": "fatal_issue", "severity": "fatal", "count": 1},
                ],
            }
        )
        self.assertEqual(report["unsupported"], 2)
        self.assertEqual(report["fatal"], 1)
        self.assertTrue(report["coverage"]["substitution_free"])

        inconsistent = parity.normalize_render_report(
            {
                "font_coverage": {
                    "matched": 1,
                    "substituted": 0,
                    "missing": 0,
                    "subset_fallback": 0,
                },
                "issues": [{"code": "font_substituted", "severity": "warning", "count": 1}],
            }
        )
        self.assertFalse(inconsistent["coverage"]["substitution_free"])

        fallback = parity.parse_render_stderr(
            "렌더: layout/warning/picture_effects_unsupported count=1 samples_complete=true\n"
            "렌더: 글꼴 커버리지 matched=2 substituted=1 missing=0 subset_fallback=0\n"
        )
        self.assertEqual(fallback["unsupported"], 1)
        self.assertFalse(fallback["coverage"]["substitution_free"])

        truncated = parity.normalize_render_report(
            {
                "complete": True,
                "issue_count": 1,
                "issues": [],
                "font_coverage": {
                    "matched": 1,
                    "substituted": 0,
                    "missing": 0,
                    "subset_fallback": 0,
                },
            }
        )
        self.assertEqual(truncated["fatal"], 1)
        self.assertIn("unclassified_issue", truncated["issue_codes"])

        clean_info = parity.normalize_render_report(
            {
                "complete": True,
                "issue_count": 0,
                "info_count": 2,
                "issues": [],
                "info": [
                    {"code": "font_matched", "severity": "info", "count": 2}
                ],
                "font_coverage": {
                    "matched": 2,
                    "substituted": 0,
                    "missing": 0,
                    "subset_fallback": 0,
                },
            }
        )
        self.assertEqual(clean_info["fatal"], 0)
        self.assertNotIn("unclassified_issue", clean_info["issue_codes"])

    def test_old_cli_report_flag_falls_back_to_stderr(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "candidate.pdf"
            output.write_bytes(b"candidate")
            calls = []

            def run_process(command, **kwargs):
                calls.append(command)
                if "--report" in command:
                    return mock.Mock(
                        returncode=2,
                        stdout="",
                        stderr="error: unexpected argument '--report' found",
                    )
                return mock.Mock(
                    returncode=0,
                    stdout="",
                    stderr="렌더: 글꼴 커버리지 matched=1 substituted=0 missing=0 subset_fallback=0",
                )

            with mock.patch.object(parity.subprocess, "run", side_effect=run_process):
                result = parity.render_candidate_once(
                    root / "hwp",
                    root / "source.hwpx",
                    output,
                    150,
                    root / "report.json",
                )
            self.assertEqual(len(calls), 2)
            self.assertNotIn("--report", calls[-1])
            self.assertEqual(result["report"]["report_source"], "stderr")
            self.assertTrue(result["report"]["coverage"]["substitution_free"])

    def test_roi_uses_top_left_normalized_coordinates(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            ours = root / "ours.png"
            oracle = root / "oracle.png"
            ours.write_bytes(tiny_png(4, 4, {(0, 0), (1, 0)}))
            oracle.write_bytes(tiny_png(4, 4, {(0, 0), (1, 0), (2, 0)}))
            precision, recall = parity.roi_ink_precision_recall(
                ours, oracle, 0.0, 0.0, 0.75, 0.5
            )
            self.assertAlmostEqual(precision, 1.0)
            self.assertAlmostEqual(recall, 2 / 3)

    def test_roi_accepts_media_box_rounding_pixel(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            ours = root / "ours.png"
            oracle = root / "oracle.png"
            ours.write_bytes(tiny_png(5, 4, {(0, 0), (1, 0)}))
            oracle.write_bytes(tiny_png(4, 5, {(0, 0), (1, 0)}))
            precision, recall = parity.roi_ink_precision_recall(
                ours, oracle, 0.0, 0.0, 0.5, 0.5
            )
            self.assertAlmostEqual(precision, 1.0)
            self.assertAlmostEqual(recall, 1.0)

    def test_roi_match_radius_tolerates_nearby_rasterization_not_missing_ink(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            ours = root / "ours.png"
            oracle = root / "oracle.png"
            ours.write_bytes(tiny_png(8, 4, {(1, 1), (6, 1)}))
            oracle.write_bytes(tiny_png(8, 4, {(2, 1)}))
            precision, recall = parity.roi_ink_precision_recall(
                ours, oracle, 0.0, 0.0, 1.0, 1.0, match_radius_px=1
            )
            self.assertAlmostEqual(precision, 0.5)
            self.assertAlmostEqual(recall, 1.0)

    def test_page_text_ignores_layout_spacing_but_preserves_order(self) -> None:
        completed = mock.Mock(stdout="-  1  -\n가   나\f")
        with mock.patch.object(parity, "run", return_value=completed):
            self.assertEqual(parity.page_text(Path("case.pdf"), 1), "-1-가나")

    def test_media_box_delta_is_coordinate_based(self) -> None:
        self.assertAlmostEqual(
            parity.media_box_delta((0, 0, 100, 200), (0, 0, 100.4, 199.8)), 0.4
        )

    def test_v2_gate_lists_are_closed_and_ordered(self) -> None:
        passed, failed = parity._v2_gate_lists(
            {"page_count": True, "media_box": False, "text": True}
        )
        self.assertEqual(passed, ["page_count", "text"])
        self.assertEqual(
            failed,
            ["media_box", "fonts", "render_issues", "raster", "roi", "determinism"],
        )

    def test_v2_scorecard_fails_unsupported_and_nondeterministic_runs(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "case.hwpx"
            oracle = root / "case.pdf"
            ours_pdf = root / "ours.pdf"
            source.write_bytes(b"source")
            oracle.write_bytes(b"oracle")
            ours_pdf.write_bytes(b"ours")
            render = {
                "first": {"path": ours_pdf},
                "report": {
                    "coverage": {
                        "matched": 1,
                        "substituted": 0,
                        "missing": 0,
                        "subset_fallback": 0,
                        "substitution_free": True,
                    },
                    "complete": True,
                    "unsupported": 1,
                    "incomplete": 0,
                    "fatal": 0,
                    "issue_codes": ["picture_effects_unsupported"],
                    "report_available": True,
                    "report_source": "json",
                },
                "byte_equal": False,
                "sha256": [digest(b"ours"), digest(b"different")],
            }
            valid_font = {
                "name": "Pinned",
                "type": "TrueType",
                "embedded": True,
                "subset": True,
                "unicode": True,
            }
            with mock.patch.object(parity, "render_candidate_twice", return_value=render), mock.patch.object(
                parity, "pdf_pages", return_value=1
            ), mock.patch.object(
                parity, "pdf_media_boxes", return_value=[(0.0, 0.0, 100.0, 100.0)]
            ), mock.patch.object(
                parity, "pdf_fonts", return_value=[valid_font]
            ), mock.patch.object(
                parity, "page_text", return_value="same"
            ), mock.patch.object(
                parity, "rasterize", return_value=root / "missing.png"
            ), mock.patch.object(
                parity, "raster_diff", return_value={
                    "dx": 0,
                    "dy": 0,
                    "ink_ratio": 1.0,
                    "bad_pixel_pct": 0.0,
                    "mae": 0.0,
                }
            ):
                card = parity.score_case_v2(
                    "case", source, oracle, root / "hwp", 150, root, rois=[]
                )
            self.assertFalse(card["eligible"])
            self.assertIn("render_issues", card["failed_gates"])
            self.assertIn("determinism", card["failed_gates"])

    def test_v2_scoreboard_summary_has_no_paths_or_text(self) -> None:
        card = {
            "case": "case",
            "source": {"sha256": "a" * 64},
            "oracle": {"sha256": "b" * 64},
            "passed_gates": ["page_count"],
            "failed_gates": ["roi"],
            "excluded_gates": [],
            "blocking_failed_gates": ["roi"],
            "eligible": False,
            "pages": {"delta": 0},
            "media_box": {"max_delta_pt": 0.0},
            "text": {"pages_equal": 1, "pages_compared": 1},
            "raster": [
                {"dx": 0, "dy": 0, "ink_ratio": 1.0, "bad_pixel_pct": 0.0, "mae": 0.0}
            ],
            "rois": [{"precision": 0.9, "recall": 0.9}],
            "fonts": {"passed": True},
            "render": {"passed": True},
            "determinism": {"passed": True},
        }
        row = parity.summarize_v2(card)
        self.assertNotIn("/", json.dumps(row))
        self.assertEqual(row["failed_gates"], ["roi"])
        self.assertEqual(row["excluded_gates"], [])
        self.assertEqual(row["blocking_failed_gates"], ["roi"])

    def test_committed_history_is_monotonic_and_path_free(self) -> None:
        history_path = (
            Path(__file__).resolve().parent.parent
            / "fixtures/pdf-parity/public/scoreboard/history.json"
        )
        history = json.loads(history_path.read_text(encoding="utf-8"))
        self.assertEqual(
            [row["label"] for row in history["rows"]],
            ["PR4", "PR5", "PR6", "PR7", "PR8", "PR9"],
        )
        for metric in history["monotonic_metrics"]:
            values = [row[metric] for row in history["rows"]]
            self.assertTrue(
                all(current <= previous for previous, current in zip(values, values[1:])),
                f"{metric} regressed: {values}",
            )
        self.assertEqual(
            [row["max_bad_pixel_pct"] for row in history["rows"]],
            [0.6, 0.6, 0.6, 0.3, 0.3, 0.3],
        )
        serialized = json.dumps(history, ensure_ascii=False)
        self.assertNotIn(str(Path.home()), serialized)
        self.assertNotIn("/tmp/", serialized)

    def test_font_path_names_are_hashed_before_publication(self) -> None:
        rows = parity.privacy_font_rows(
            [{"name": "/Users/private/fonts/secret.ttf", "type": "TrueType"}]
        )
        self.assertNotIn("/Users/private", rows[0]["name"])
        self.assertTrue(rows[0]["name"].startswith("font-"))


class V2FontGateTests(unittest.TestCase):
    def test_fonts_array_normalization_keeps_only_outcome_and_byte_hash(self) -> None:
        report = parity.normalize_render_report(
            {
                "complete": True,
                "font_resolution_complete": True,
                "font_coverage": {
                    "matched": 2,
                    "substituted": 0,
                    "missing": 0,
                    "subset_fallback": 0,
                },
                "fonts": [
                    {
                        "requested_sha256": "a" * 64,
                        "requested_bold": False,
                        "resolved_family_sha256": "b" * 64,
                        "resolved_sha256": "c" * 64,
                        "resolved_face_index": 0,
                        "outcome": "matched",
                    },
                    {
                        # Certification-style byte-hash field name.
                        "requested_name_sha256": "d" * 64,
                        "font_file_sha256": "e" * 64,
                        "outcome": "substituted",
                    },
                    {
                        "requested_sha256": "f" * 64,
                        "resolved_sha256": "not-a-hash",
                        "outcome": "missing",
                    },
                    {"outcome": "bogus"},
                    "not-a-dict",
                ],
            }
        )
        self.assertEqual(
            report["fonts"],
            [
                {"outcome": "matched", "resolved_sha256": "c" * 64},
                {"outcome": "substituted", "resolved_sha256": "e" * 64},
                {"outcome": "missing", "resolved_sha256": None},
            ],
        )
        self.assertTrue(report["font_resolution_complete"])
        self.assertEqual(report["incomplete"], 0)

    def test_incomplete_font_resolution_is_incomplete_evidence(self) -> None:
        report = parity.normalize_render_report(
            {
                "complete": True,
                "font_resolution_complete": False,
                "font_coverage": {
                    "matched": 1,
                    "substituted": 0,
                    "missing": 0,
                    "subset_fallback": 0,
                },
            }
        )
        self.assertFalse(report["font_resolution_complete"])
        self.assertEqual(report["incomplete"], 1)
        self.assertIn("font_resolution_incomplete", report["issue_codes"])

    def score_with_report(self, report: dict, pinned, rois=None, excluded=None) -> dict:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "case.hwpx"
            oracle = root / "case.pdf"
            ours_pdf = root / "ours.pdf"
            source.write_bytes(b"source")
            oracle.write_bytes(b"oracle")
            ours_pdf.write_bytes(b"ours")
            render = {
                "first": {"path": ours_pdf},
                "report": report,
                "byte_equal": True,
                "sha256": [digest(b"ours"), digest(b"ours")],
            }
            valid_font = {
                "name": "Pinned",
                "type": "TrueType",
                "embedded": True,
                "subset": True,
                "unicode": True,
            }
            with mock.patch.object(parity, "render_candidate_twice", return_value=render), mock.patch.object(
                parity, "pdf_pages", return_value=1
            ), mock.patch.object(
                parity, "pdf_media_boxes", return_value=[(0.0, 0.0, 100.0, 100.0)]
            ), mock.patch.object(
                parity, "pdf_fonts", return_value=[valid_font]
            ), mock.patch.object(
                parity, "page_text", return_value="same"
            ), mock.patch.object(
                parity, "rasterize", return_value=root / "missing.png"
            ), mock.patch.object(
                parity, "roi_ink_precision_recall", return_value=(1.0, 1.0)
            ), mock.patch.object(
                parity, "raster_diff", return_value={
                    "dx": 0,
                    "dy": 0,
                    "ink_ratio": 1.0,
                    "bad_pixel_pct": 0.0,
                    "mae": 0.0,
                }
            ):
                return parity.score_case_v2(
                    "case", source, oracle, root / "hwp", 150, root,
                    rois=rois or [], pinned_font_hashes=pinned,
                    excluded_gates=excluded,
                )

    def clean_report(self, fonts, complete=True) -> dict:
        return {
            "coverage": {
                "matched": 1,
                "substituted": 0,
                "missing": 0,
                "subset_fallback": 0,
                "substitution_free": True,
            },
            "complete": True,
            "unsupported": 0,
            "incomplete": 0,
            "fatal": 0,
            "issue_codes": [],
            "report_available": True,
            "report_source": "json",
            "fonts": fonts,
            "font_resolution_complete": complete,
        }

    def test_outside_manifest_resolved_hash_fails_fonts_gate(self) -> None:
        pinned = frozenset({"c" * 64})
        clean = self.score_with_report(
            self.clean_report([{"outcome": "matched", "resolved_sha256": "c" * 64}]),
            pinned,
        )
        self.assertIn("fonts", clean["passed_gates"])
        self.assertTrue(clean["fonts"]["pinned_faces"])
        self.assertEqual(clean["fonts"]["outside_manifest_faces"], 0)

        outside = self.score_with_report(
            self.clean_report([{"outcome": "matched", "resolved_sha256": "9" * 64}]),
            pinned,
        )
        self.assertIn("fonts", outside["failed_gates"])
        self.assertFalse(outside["fonts"]["pinned_faces"])
        self.assertEqual(outside["fonts"]["outside_manifest_faces"], 1)
        self.assertFalse(outside["eligible"])

        missing_records = self.score_with_report(self.clean_report(None), pinned)
        self.assertIn("fonts", missing_records["failed_gates"])

        incomplete = self.score_with_report(self.clean_report([], complete=False), pinned)
        self.assertIn("fonts", incomplete["failed_gates"])
        self.assertFalse(incomplete["fonts"]["resolution_complete"])

    def test_fonts_gate_fails_on_substitution_not_render_issues(self) -> None:
        report = self.clean_report([{"outcome": "substituted", "resolved_sha256": "c" * 64}])
        report["coverage"]["substituted"] = 1
        report["coverage"]["substitution_free"] = False
        card = self.score_with_report(report, frozenset({"c" * 64}))
        self.assertIn("fonts", card["failed_gates"])
        self.assertFalse(card["fonts"]["substitution_free"])
        # Substitution evidence moved to the fonts gate; the general
        # incomplete/fatal gate is unaffected by it.
        self.assertIn("render_issues", card["passed_gates"])

    def test_clean_aggregate_does_not_override_coverage_substituted_record(self) -> None:
        # The aggregate claims substitution-free, but a glyph-level coverage
        # fallback record must still fail the fonts gate.
        report = self.clean_report(
            [{"outcome": "coverage_substituted", "resolved_sha256": "c" * 64}]
        )
        card = self.score_with_report(report, frozenset({"c" * 64}))
        self.assertIn("fonts", card["failed_gates"])
        self.assertFalse(card["fonts"]["substitution_free"])
        self.assertFalse(card["eligible"])

    def test_resolved_record_without_byte_hash_counts_as_outside_manifest(self) -> None:
        records = [
            {"outcome": "matched", "resolved_sha256": None},
            {"outcome": "substituted", "resolved_sha256": None},
            {"outcome": "coverage_substituted", "resolved_sha256": None},
            # A missing font legitimately carries no hash and is not counted.
            {"outcome": "missing", "resolved_sha256": None},
        ]
        card = self.score_with_report(self.clean_report(records), frozenset({"c" * 64}))
        self.assertIn("fonts", card["failed_gates"])
        self.assertFalse(card["fonts"]["pinned_faces"])
        self.assertEqual(card["fonts"]["outside_manifest_faces"], 3)

    def test_all_matched_pinned_records_pass_fonts_gate(self) -> None:
        records = [
            {"outcome": "matched", "resolved_sha256": "c" * 64},
            {"outcome": "matched", "resolved_sha256": "d" * 64},
        ]
        pinned = frozenset({"c" * 64, "d" * 64})
        card = self.score_with_report(self.clean_report(records), pinned)
        self.assertIn("fonts", card["passed_gates"])
        self.assertTrue(card["fonts"]["substitution_free"])
        self.assertTrue(card["fonts"]["pinned_faces"])
        self.assertTrue(card["fonts"]["resolution_complete"])
        self.assertEqual(card["fonts"]["outside_manifest_faces"], 0)


class V2GateExclusionTests(unittest.TestCase):
    """Manifest-declared gate exclusions relax eligibility only, never measurement."""

    ROI = {"name": "header", "page": 1, "x": 0.0, "y": 0.0, "width": 0.5, "height": 0.5}

    def test_manifest_gate_exclusions_are_validated(self) -> None:
        self.assertEqual(parity._v2_gate_exclusions({}), frozenset())
        self.assertEqual(
            parity._v2_gate_exclusions({"gate_exclusions": ["fonts"]}),
            frozenset({"fonts"}),
        )
        with self.assertRaises(SystemExit):
            parity._v2_gate_exclusions({"gate_exclusions": ["bogus"]})
        with self.assertRaises(SystemExit):
            parity._v2_gate_exclusions({"gate_exclusions": ["fonts", "fonts"]})
        with self.assertRaises(SystemExit):
            parity._v2_gate_exclusions({"gate_exclusions": "fonts"})

    def fonts_failing_card(self, excluded):
        gate_tests = V2FontGateTests()
        report = gate_tests.clean_report(
            [{"outcome": "matched", "resolved_sha256": "9" * 64}]
        )
        return gate_tests.score_with_report(
            report, frozenset({"c" * 64}), rois=[self.ROI], excluded=excluded
        )

    def test_excluded_fonts_failure_is_reported_but_not_blocking(self) -> None:
        card = self.fonts_failing_card(excluded={"fonts"})
        # Measurement is untouched: the failure is still reported in full.
        self.assertIn("fonts", card["failed_gates"])
        self.assertFalse(card["fonts"]["pinned_faces"])
        # Eligibility is decided by blocking_failed_gates alone.
        self.assertEqual(card["excluded_gates"], ["fonts"])
        self.assertEqual(card["blocking_failed_gates"], [])
        self.assertTrue(card["eligible"])

    def test_no_exclusion_keeps_fonts_failure_blocking(self) -> None:
        card = self.fonts_failing_card(excluded=None)
        self.assertIn("fonts", card["failed_gates"])
        self.assertEqual(card["excluded_gates"], [])
        self.assertEqual(card["blocking_failed_gates"], ["fonts"])
        self.assertFalse(card["eligible"])

    def test_excluding_a_passing_gate_is_a_no_op(self) -> None:
        gate_tests = V2FontGateTests()
        report = gate_tests.clean_report(
            [{"outcome": "matched", "resolved_sha256": "c" * 64}]
        )
        card = gate_tests.score_with_report(
            report, frozenset({"c" * 64}), rois=[self.ROI], excluded={"fonts"}
        )
        self.assertIn("fonts", card["passed_gates"])
        self.assertEqual(card["failed_gates"], [])
        self.assertEqual(card["excluded_gates"], ["fonts"])
        self.assertEqual(card["blocking_failed_gates"], [])
        self.assertTrue(card["eligible"])


FIXTURE_SCOREBOARD = (
    Path(__file__).resolve().parent.parent
    / "fixtures/pdf-parity/public/scoreboard"
)


@unittest.skipUnless(
    parity.validator_path().is_file(),
    "JSON schema validator example not built (scripts/pdf-parity.sh builds it)",
)
class V2GateExclusionSchemaTests(unittest.TestCase):
    """Schema-level checks: blocking_failed_gates must equal failed minus excluded.

    The schemas cannot express a set difference, so each gate carries two
    if/then implications; these tests exercise both directions against the
    Rust validator on real fixture-shaped documents.
    """

    def validates(self, schema: str, doc: dict) -> bool:
        with tempfile.TemporaryDirectory() as temporary:
            document = Path(temporary) / "doc.json"
            document.write_text(json.dumps(doc), encoding="utf-8")
            proc = subprocess.run(
                [
                    str(parity.validator_path()),
                    str(parity.SCHEMAS / f"{schema}.schema.json"),
                    str(document),
                ],
                capture_output=True,
                text=True,
            )
        return proc.returncode == 0

    def load(self, name: str) -> dict:
        return json.loads((FIXTURE_SCOREBOARD / name).read_text(encoding="utf-8"))

    @staticmethod
    def mutate(doc: dict, failed, excluded, blocking, eligible) -> dict:
        mutated = copy.deepcopy(doc)
        mutated["failed_gates"] = failed
        mutated["excluded_gates"] = excluded
        mutated["blocking_failed_gates"] = blocking
        passed = [gate for gate in mutated["passed_gates"] if gate not in failed]
        mutated["passed_gates"] = passed
        mutated["eligible"] = eligible
        return mutated

    def test_committed_public_fixtures_validate(self) -> None:
        board = self.load("scoreboard.json")
        self.assertTrue(self.validates("pdf-parity-scoreboard-v2", board))
        for name in ("public-rfp-hwp.json", "public-rfp-hwpx.json"):
            self.assertTrue(self.validates("pdf-parity-scorecard-v2", self.load(name)))

    def test_legitimate_exclusion_validates_in_all_positions(self) -> None:
        card = self.mutate(self.load("public-rfp-hwp.json"), ["fonts"], ["fonts"], [], True)
        self.assertTrue(self.validates("pdf-parity-scorecard-v2", card))
        board = self.mutate(self.load("scoreboard.json"), ["fonts"], ["fonts"], [], True)
        board["cases"] = [
            self.mutate(case, ["fonts"], ["fonts"], [], True) for case in board["cases"]
        ]
        self.assertTrue(self.validates("pdf-parity-scoreboard-v2", board))

    def test_non_excluded_failure_cannot_be_hidden_from_blocking(self) -> None:
        # The review counterexample: a non-excluded gate failed, but
        # blocking_failed_gates was supplied empty with eligible=true.
        card = self.mutate(self.load("public-rfp-hwp.json"), ["text"], ["fonts"], [], True)
        self.assertFalse(self.validates("pdf-parity-scorecard-v2", card))
        board_root = self.mutate(self.load("scoreboard.json"), ["text"], ["fonts"], [], True)
        self.assertFalse(self.validates("pdf-parity-scoreboard-v2", board_root))
        board_case = self.load("scoreboard.json")
        board_case["excluded_gates"] = ["fonts"]
        board_case["cases"] = [
            self.mutate(case, ["text"], ["fonts"], [], True)
            for case in board_case["cases"]
        ]
        self.assertFalse(self.validates("pdf-parity-scoreboard-v2", board_case))

    def test_blocking_gates_must_be_real_non_excluded_failures(self) -> None:
        # A blocking gate absent from failed_gates is rejected.
        phantom = self.mutate(self.load("public-rfp-hwp.json"), [], [], ["roi"], False)
        self.assertFalse(self.validates("pdf-parity-scorecard-v2", phantom))
        # A blocking gate that is also excluded is rejected.
        doubled = self.mutate(
            self.load("scoreboard.json"), ["roi"], ["roi"], ["roi"], False
        )
        self.assertFalse(self.validates("pdf-parity-scoreboard-v2", doubled))
