#!/usr/bin/env python3
"""Coleta métricas estáticas do Pulse e faz append em docs/evolution/metrics.jsonl.

Uso:
    python3 docs/evolution/metrics.py [--typecheck 12.3] [--lint 4.1] [--vitest 30.2] [--build 22.0]

Os tempos são opcionais: passe o que o gate já mediu. Nunca reescreve linhas antigas.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from datetime import datetime, timedelta, timezone

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "docs", "evolution", "metrics.jsonl")
BRT = timezone(timedelta(hours=-3))
SKIP_DIRS = {".git", "node_modules", "target", "dist", "build", "bundle", ".vite"}
CODE_EXT = {".rs": "rust", ".ts": "ts", ".tsx": "tsx", ".css": "css", ".js": "js", ".mjs": "js"}


def walk(base: str):
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for name in filenames:
            ext = os.path.splitext(name)[1]
            if ext in CODE_EXT:
                yield os.path.join(dirpath, name), ext


def loc_table() -> tuple[dict, list]:
    totals = {"rust": 0, "ts": 0, "tsx": 0, "css": 0, "js": 0}
    files = []
    for base in ("src", "src-tauri/src"):
        path = os.path.join(ROOT, base)
        if not os.path.isdir(path):
            continue
        for full, ext in walk(path):
            try:
                with open(full, encoding="utf-8", errors="replace") as fh:
                    n = sum(1 for _ in fh)
            except OSError:
                continue
            lang = CODE_EXT[ext]
            totals[lang] += n
            files.append((n, os.path.relpath(full, ROOT)))
    files.sort(reverse=True)
    return totals, files


def count_tests() -> tuple[int, int]:
    rust = tsx = 0
    for base in ("src-tauri/src", "src"):
        path = os.path.join(ROOT, base)
        if not os.path.isdir(path):
            continue
        for full, ext in walk(path):
            if ext not in (".rs", ".ts", ".tsx"):
                continue
            try:
                with open(full, encoding="utf-8", errors="replace") as fh:
                    text = fh.read()
            except OSError:
                continue
            if ext == ".rs":
                rust += len(re.findall(r"#\[(?:tokio::)?test\]", text))
            elif re.search(r"\.test\.(?:ts|tsx)$", full):
                tsx += 1
    return rust, tsx


def count_hex() -> int:
    total = 0
    for ext in (".css", ".tsx", ".ts"):
        for base in ("src",):
            path = os.path.join(ROOT, base)
            if not os.path.isdir(path):
                continue
            for full, e in walk(path):
                if e != ext:
                    continue
                try:
                    with open(full, encoding="utf-8", errors="replace") as fh:
                        text = fh.read()
                except OSError:
                    continue
                total += len(re.findall(r"#[0-9a-fA-F]{3,8}\b", text))
    return total


def bundle_kb() -> int | None:
    path = os.path.join(ROOT, "dist")
    if not os.path.isdir(path):
        return None
    size = 0
    for dirpath, _dirs, files in os.walk(path):
        for name in files:
            try:
                size += os.path.getsize(os.path.join(dirpath, name))
            except OSError:
                pass
    return round(size / 1024)


def head_commit() -> str:
    try:
        return subprocess.run(
            ["git", "-C", ROOT, "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, check=False,
        ).stdout.strip()
    except OSError:
        return ""


def main() -> int:
    ap = argparse.ArgumentParser()
    for key in ("typecheck", "lint", "vitest", "build", "clippy", "cargo_test"):
        ap.add_argument(f"--{key}", type=float, default=None)
    args = ap.parse_args()

    totals, files = loc_table()
    rust_tests, ts_tests = count_tests()
    record = {
        "at": datetime.now(BRT).isoformat(timespec="seconds"),
        "commit": head_commit(),
        "loc": totals,
        "loc_total": sum(totals.values()),
        "maior_arquivo": files[0][1] if files else None,
        "maior_arquivo_linhas": files[0][0] if files else 0,
        "arquivos_2000_mais": sum(1 for n, _ in files if n >= 2000),
        "testes_rust": rust_tests,
        "arquivos_teste_ts": ts_tests,
        "hex_hardcoded": count_hex(),
        "bundle_kb": bundle_kb(),
        "tempos_s": {k: v for k, v in vars(args).items() if v is not None},
    }
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "a", encoding="utf-8") as fh:
        fh.write(json.dumps(record, ensure_ascii=False) + "\n")
    print(json.dumps(record, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
