#!/bin/sh
# Bump Constant's crate version and local version references that should move
# with the next release. Homebrew SHA256s are intentionally NOT updated here:
# run scripts/update-homebrew-formula.sh after GitHub Release assets exist.
set -eu

usage() {
  echo "usage: scripts/bump-version.sh <X.Y.Z|vX.Y.Z>" >&2
  exit 2
}

[ "$#" -eq 1 ] || usage
version="${1#v}"
tag="v$version"

case "$version" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) usage ;;
esac
case "$version" in
  *[!0-9.]* | *.*.*.*) usage ;;
esac

python3 - "$version" "$tag" <<'PY'
import re
import sys
from pathlib import Path

version, tag = sys.argv[1], sys.argv[2]

def edit(path: str, fn) -> None:
    p = Path(path)
    s = p.read_text()
    n = fn(s)
    if n != s:
        p.write_text(n)

edit("Cargo.toml", lambda s: re.sub(
    r'(?m)^version = "[^"]+"',
    f'version = "{version}"',
    s,
    count=1,
))

for path in ("README.md", "scripts/install.sh"):
    edit(path, lambda s: re.sub(r"CONSTANT_VERSION=v[0-9]+\.[0-9]+\.[0-9]+", f"CONSTANT_VERSION={tag}", s))
    edit(path, lambda s: re.sub(r"e\.g\. v[0-9]+\.[0-9]+\.[0-9]+", f"e.g. {tag}", s))
PY

cargo check

echo "bumped crate/docs references to $tag"
echo "next:"
echo "  git diff"
echo "  git commit -am 'Bump version to $tag'"
echo "  git tag -a $tag -m 'Constant $tag'"
echo "  git push origin main $tag"
echo "  scripts/update-homebrew-formula.sh $tag"
