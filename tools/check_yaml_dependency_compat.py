#!/usr/bin/env python3
"""Opt-in authored YAML migration proof, isolated from the root dependency graph.

Run manually on an admitted build host. The disposable Cargo workspace is created
under ignored target/yaml-compat-proof; it alone depends on legacy serde_yaml.
No fixtures, host fonts, certification execution, or Hancom evidence are used.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepare-only", action="store_true", help="write the isolated project without invoking Cargo")
    parser.add_argument("--timeout", type=int, default=1200, help="Cargo wall-clock budget in seconds (default: 1200)")
    args = parser.parse_args()
    if args.timeout < 1:
        parser.error("--timeout must be positive")
    try:
        lock_bytes = (ROOT / "Cargo.lock").read_bytes()
        packages = tomllib.loads(lock_bytes.decode())["package"]
        versions = {}
        for name in ["serde_norway", "serde", "serde_json"]:
            matches = [package["version"] for package in packages if package["name"] == name]
            if len(matches) != 1:
                raise ValueError(f"expected one locked {name} version, found {len(matches)}; inspect Cargo.lock before migration proof")
            versions[name] = matches[0]
        if any(package["name"] in {"serde_yaml", "unsafe-libyaml"} for package in packages):
            raise ValueError("legacy YAML remains in root Cargo.lock; remove the root legacy edge before running the isolated comparison")
        source = (ROOT / "tools/yaml_dependency_compat.rs").read_bytes()
        parent = ROOT / "target/yaml-compat-proof"
        parent.mkdir(parents=True, exist_ok=True)
        project = Path(tempfile.mkdtemp(prefix="run-", dir=parent))
        (project / "src").mkdir()
        (project / "src/main.rs").write_bytes(source)
        quote = lambda value: json.dumps(str(value), ensure_ascii=False)
        manifest = '\n'.join([
            '[package]', 'name = "hwp-yaml-dependency-compat"', 'version = "0.0.0"',
            'edition = "2024"', 'publish = false', '', '[workspace]', '', '[dependencies]',
            *[f'{name} = {{ path = {quote(ROOT / "crates" / name)} }}' for name in ["hwp-cli", "hwp-model", "hwp5", "hwpx"]],
            'yaml_legacy = { package = "serde_yaml", version = "=0.9.34" }',
            f'yaml_candidate = {{ package = "serde_norway", version = "={versions["serde_norway"]}" }}',
            f'serde = {{ version = "={versions["serde"]}", features = ["derive"] }}',
            f'serde_json = "={versions["serde_json"]}"', '',
        ])
        (project / "Cargo.toml").write_text(manifest)
        # Preserve the production resolution; only the isolated baseline parser is added.
        (project / "Cargo.lock").write_bytes(lock_bytes)
        revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True, timeout=30)
        receipt = {"schema": "hwp-yaml-dependency-compat-v1",
                   "source_commit": revision.stdout.strip() if revision.returncode == 0 else None, "candidate": versions["serde_norway"],
                   "legacy": "0.9.34+deprecated", "root_lock_sha256": digest(lock_bytes),
                   "harness_sha256": digest(source), "project": str(project),
                   "scope": "authored public models and exact writer bytes; no font/render/native-open proof"}
        if args.prepare_only:
            receipt["status"] = "prepared-not-run"
            print(json.dumps(receipt, sort_keys=True))
            return 0
        env = {**os.environ, "CARGO_BUILD_JOBS": "2"}
        result = subprocess.run(["cargo", "run", "--offline", "--manifest-path", str(project / "Cargo.toml"),
                                 "--", str(project / "outputs")], cwd=project, env=env,
                                capture_output=True, text=True, timeout=args.timeout)
        if result.returncode:
            cause = result.stderr.strip()[-4000:]
            raise RuntimeError(f"isolated Cargo comparison exited {result.returncode}: {cause}\nNext step: inspect the isolated project; provision exact dependencies on the admitted host if offline cache is missing, then rerun. Do not add legacy YAML to the root graph.")
        proof_lock = (project / "Cargo.lock").read_bytes()
        root_packages = {(item["name"], item["version"], item.get("source"), item.get("checksum")) for item in packages}
        comparison_packages = tomllib.loads(proof_lock.decode())["package"]
        additions = [item for item in comparison_packages
                     if (item["name"], item["version"], item.get("source"), item.get("checksum")) not in root_packages]
        if {item["name"] for item in additions} != {"hwp-yaml-dependency-compat", "serde_yaml", "unsafe-libyaml"}:
            raise RuntimeError("comparison changed a production dependency resolution; inspect the isolated lockfile and pin it to the root resolution before rerunning")
        receipt["comparison_lock_sha256"] = digest(proof_lock)
        receipt["production_resolution_preserved"] = True
        receipt["proof"] = json.loads(result.stdout)
        if (ROOT / "Cargo.lock").read_bytes() != lock_bytes:
            raise RuntimeError("root Cargo.lock changed during proof; inspect concurrent changes and rerun against an immutable head")
        receipt["status"] = "passed"
        print(json.dumps(receipt, ensure_ascii=False, sort_keys=True))
        return 0
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"YAML compatibility proof failed: {error}\nNext step: correct the stated cause and rerun on an admitted build host; no compatibility result was established.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
