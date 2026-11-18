#!/usr/bin/env python3
"""Build tree-sitter grammar libraries for rcode's runtime `grammars/` folder.

Grammar sources come from the published crates on crates.io (pinned versions below).
Each language becomes `<out>/<folder>/{parser.dll|parser.so, highlights.scm, config.txt}`.

    python3 tools/build_grammars.py --target windows --out target/x86_64-pc-windows-gnu/release/grammars
    python3 tools/build_grammars.py --target linux   --out target/debug/grammars

Needs `zig` (used as the C/C++ compiler and cross linker) on PATH or in $ZIG.
To add a language: add a line to GRAMMARS and run the script again; the editor needs no rebuild.
"""

import argparse
import io
import os
import shutil
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

# folder, crate, version, grammar dir inside the crate, display name, extensions, comment,
# extra query sources prepended to highlights.scm (crate:dir), file names
GRAMMARS = [
    ("python", "tree-sitter-python", "0.25.0", ".", "Python", "py pyw pyi", "#", [], ""),
    ("rust", "tree-sitter-rust", "0.24.2", ".", "Rust", "rs", "//", [], ""),
    ("javascript", "tree-sitter-javascript", "0.25.0", ".", "JavaScript", "js mjs cjs jsx", "//", [], ""),
    ("typescript", "tree-sitter-typescript", "0.23.2", "typescript", "TypeScript", "ts mts cts", "//", ["tree-sitter-javascript"], ""),
    ("tsx", "tree-sitter-typescript", "0.23.2", "tsx", "TypeScript JSX", "tsx", "//", ["tree-sitter-javascript"], ""),
    ("json", "tree-sitter-json", "0.24.8", ".", "JSON", "json jsonc", "//", [], ""),
    ("toml", "tree-sitter-toml-ng", "0.7.0", ".", "TOML", "toml", "#", [], "cargo.lock"),
    ("yaml", "tree-sitter-yaml", "0.7.2", ".", "YAML", "yaml yml", "#", [], ""),
    ("bash", "tree-sitter-bash", "0.25.1", ".", "Shell Script", "sh bash zsh", "#", [], ".bashrc .zshrc .profile"),
    ("c", "tree-sitter-c", "0.24.2", ".", "C", "c h", "//", [], ""),
    ("cpp", "tree-sitter-cpp", "0.23.4", ".", "C++", "cpp cc cxx hpp hh hxx", "//", ["tree-sitter-c"], ""),
    ("go", "tree-sitter-go", "0.25.0", ".", "Go", "go", "//", [], ""),
    ("java", "tree-sitter-java", "0.23.5", ".", "Java", "java", "//", [], ""),
    ("c_sharp", "tree-sitter-c-sharp", "0.23.5", ".", "C#", "cs", "//", [], ""),
    ("html", "tree-sitter-html", "0.23.2", ".", "HTML", "html htm", "", [], ""),
    ("css", "tree-sitter-css", "0.25.0", ".", "CSS", "css", "", [], ""),
    ("lua", "tree-sitter-lua", "0.5.0", ".", "Lua", "lua", "--", [], ""),
    ("ruby", "tree-sitter-ruby", "0.23.1", ".", "Ruby", "rb", "#", [], "gemfile rakefile"),
    ("php", "tree-sitter-php", "0.25.1", "php", "PHP", "php", "//", [], ""),
    ("kotlin", "tree-sitter-kotlin-ng", "1.1.0", ".", "Kotlin", "kt kts", "//", [], ""),
    ("dockerfile", "tree-sitter-dockerfile", "0.2.0", ".", "Dockerfile", "dockerfile", "#", [], "dockerfile"),
    ("xml", "tree-sitter-xml", "0.7.0", "xml", "XML", "xml svg xaml csproj", "", [], ""),
]

CACHE = Path(os.environ.get("RCODE_GRAMMAR_CACHE", Path(__file__).resolve().parent.parent / "target" / "grammar-src"))
VERSIONS = {crate: ver for _, crate, ver, *_ in GRAMMARS}
VERSIONS.setdefault("tree-sitter-javascript", "0.25.0")
VERSIONS.setdefault("tree-sitter-c", "0.24.2")


def fetch(crate: str) -> Path:
    ver = VERSIONS[crate]
    dest = CACHE / f"{crate}-{ver}"
    if dest.is_dir():
        return dest
    url = f"https://static.crates.io/crates/{crate}/{crate}-{ver}.crate"
    print(f"  downloading {crate} {ver}")
    data = urllib.request.urlopen(url, timeout=60).read()
    CACHE.mkdir(parents=True, exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tf:
        try:
            tf.extractall(CACHE, filter="data")
        except TypeError:  # Python < 3.12 without the backport
            tf.extractall(CACHE)
    return dest


def find_queries(root, sub):
    for cand in [root / sub / "queries", root / "queries" / sub, root / "queries"]:
        if (cand / "highlights.scm").is_file():
            return cand
    return None


def build(zig: str, target: str, out: Path, g) -> str:
    folder, crate, _ver, sub, name, exts, comment, extra, files = g
    root = fetch(crate)
    src = root / sub / "src"
    if not (src / "parser.c").is_file():
        return f"no parser.c in {src}"
    sources = [src / "parser.c"]
    cxx = False
    for s in ["scanner.c", "scanner.cc"]:
        if (src / s).is_file():
            sources.append(src / s)
            cxx = s.endswith(".cc")
    dest = out / folder
    dest.mkdir(parents=True, exist_ok=True)
    if target == "windows":
        lib, triple = dest / "parser.dll", ["-target", "x86_64-windows-gnu"]
    else:
        lib, triple = dest / "parser.so", ["-fPIC"]
    cmd = [zig, "c++" if cxx else "cc", *triple, "-shared", "-O2", "-std=c11" if not cxx else "-std=c++14",
           "-I", str(src), *map(str, sources), "-o", str(lib)]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        return "compile failed:\n" + r.stderr[-2000:]
    for junk in dest.glob("parser.lib"):
        junk.unlink()
    for junk in dest.glob("parser.pdb"):
        junk.unlink()
    # highlights: base grammars first (TypeScript extends JavaScript, C++ extends C)
    parts = []
    for base in extra:
        q = find_queries(fetch(base), ".")
        if q:
            parts.append((q / "highlights.scm").read_text(encoding="utf-8"))
    q = find_queries(root, sub)
    if not q:
        return "no highlights.scm"
    parts.append((q / "highlights.scm").read_text(encoding="utf-8"))
    (dest / "highlights.scm").write_text("\n".join(parts), encoding="utf-8")
    symbol = "tree_sitter_" + folder
    lines = [f"name = {name}", f"extensions = {exts}", f"comment = {comment}", f"symbol = {symbol}"]
    if files:
        lines.append(f"files = {files}")
    (dest / "config.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
    return ""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", choices=["windows", "linux"], required=True)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("only", nargs="*", help="folders to build (default: all)")
    a = ap.parse_args()
    zig = os.environ.get("ZIG") or shutil.which("zig")
    if not zig:
        sys.exit("zig not found (set $ZIG)")
    failed = []
    for g in GRAMMARS:
        if a.only and g[0] not in a.only:
            continue
        print(f"{g[0]}")
        err = build(zig, a.target, a.out, g)
        if err:
            print(f"  FAILED: {err}")
            failed.append(g[0])
            shutil.rmtree(a.out / g[0], ignore_errors=True)
    print(f"done, {len(failed)} failed: {' '.join(failed)}" if failed else "done")


if __name__ == "__main__":
    main()
