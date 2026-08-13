#!/usr/bin/env python3
"""Regression tests for the Hancom PDF parity runner."""

import hashlib
import tempfile
import unittest
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


if __name__ == "__main__":
    unittest.main()
