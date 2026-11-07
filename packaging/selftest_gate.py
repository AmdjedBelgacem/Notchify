#!/usr/bin/env python3
"""Fail the build on UI self-test failures, except known-flaky checks.

Usage: packaging/selftest_gate.py [selftest/selftest.json] [packaging/selftest_flaky.txt]

Reads the report written by `notchforge --self-test <dir>`
({"checks": [{"name": ..., "passed": bool, "detail": ...}], ...}) and exits
non-zero if any failed check's name does not start with one of the prefixes
in the allowlist file (one prefix per line, '#' comments allowed). A missing
or unreadable report is a failure: the self-test must have crashed.
"""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def load_prefixes(path: Path) -> list[str]:
    if not path.is_file():
        return []
    prefixes = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            prefixes.append(line)
    return prefixes


def main(argv: list[str]) -> int:
    report = Path(argv[1]) if len(argv) > 1 else ROOT / "selftest" / "selftest.json"
    allowlist = Path(argv[2]) if len(argv) > 2 else ROOT / "packaging" / "selftest_flaky.txt"
    try:
        doc = json.loads(report.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        print(f"selftest gate: cannot read {report}: {e}", file=sys.stderr)
        return 2
    checks = doc.get("checks") or []
    if not checks:
        print(f"selftest gate: {report} has no checks", file=sys.stderr)
        return 2
    prefixes = load_prefixes(allowlist)
    failed = [c for c in checks if not c.get("passed")]
    flaky = [c for c in failed if any(str(c.get("name", "")).startswith(p) for p in prefixes)]
    blocking = [c for c in failed if c not in flaky]

    print(f"selftest gate: {len(checks) - len(failed)}/{len(checks)} checks passed")
    for c in flaky:
        print(f"  ALLOWED (flaky): {c.get('name')} -- {c.get('detail', '')}")
    for c in blocking:
        print(f"  FAILED:          {c.get('name')} -- {c.get('detail', '')}")
    if blocking:
        print(f"selftest gate: {len(blocking)} blocking failure(s)", file=sys.stderr)
        return 1
    print("selftest gate: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
