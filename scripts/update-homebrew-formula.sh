#!/bin/sh
# Generate the direct-repository Homebrew formula from published release assets.
# Run this AFTER the GitHub Release workflow has uploaded the tarballs and .sha256
# files for the tag.
set -eu

usage() {
  echo "usage: scripts/update-homebrew-formula.sh <vX.Y.Z>" >&2
  exit 2
}

[ "$#" -eq 1 ] || usage
tag="$1"
version="${tag#v}"

case "$tag" in
  v[0-9]*.[0-9]*.[0-9]*) ;;
  *) usage ;;
esac
case "$tag" in
  *[!A-Za-z0-9._-]*) usage ;;
esac

repo="kennykankush/constant"
base="https://github.com/$repo/releases/download/$tag"

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "error: required tool not found: $1" >&2
    exit 1
  }
}
need gh
need python3
need ruby

assets_json="$(gh release view "$tag" --repo "$repo" --json assets)"

sha_for() {
  target="$1"
  asset="constant-$tag-$target.tar.gz"
  ASSETS_JSON="$assets_json" python3 - "$asset" <<'PY'
import json
import os
import re
import sys

want = sys.argv[1]
data = json.loads(os.environ["ASSETS_JSON"])
for item in data.get("assets", []):
    if item.get("name") == want:
        digest = item.get("digest") or ""
        if not digest.startswith("sha256:"):
            raise SystemExit(f"error: missing sha256 digest for {want}")
        sha = digest.split(":", 1)[1]
        if not re.fullmatch(r"[0-9a-f]{64}", sha):
            raise SystemExit(f"error: invalid sha256 for {want}: {sha}")
        print(sha)
        raise SystemExit(0)
raise SystemExit(f"error: release asset not found: {want}")
PY
}

api_url_for() {
  target="$1"
  asset="constant-$tag-$target.tar.gz"
  ASSETS_JSON="$assets_json" python3 - "$asset" <<'PY'
import json
import os
import sys

want = sys.argv[1]
data = json.loads(os.environ["ASSETS_JSON"])
for item in data.get("assets", []):
    if item.get("name") == want:
        api_url = item.get("apiUrl") or ""
        if not api_url.startswith("https://api.github.com/"):
            raise SystemExit(f"error: missing GitHub API URL for {want}")
        print(api_url)
        raise SystemExit(0)
raise SystemExit(f"error: release asset not found: {want}")
PY
}

sha_aarch64_apple="$(sha_for aarch64-apple-darwin)"
sha_x86_64_apple="$(sha_for x86_64-apple-darwin)"
sha_x86_64_linux="$(sha_for x86_64-unknown-linux-gnu)"
url_aarch64_apple="$(api_url_for aarch64-apple-darwin)"
url_x86_64_apple="$(api_url_for x86_64-apple-darwin)"
url_x86_64_linux="$(api_url_for x86_64-unknown-linux-gnu)"

mkdir -p Formula packaging/homebrew

write_formula() {
  path="$1"
  cat >"$path" <<EOF
class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/$repo"
  version "$version"
  license "MIT"

  on_macos do
    on_arm do
      url "$url_aarch64_apple",
          headers: ["Accept: application/octet-stream"]
      sha256 "$sha_aarch64_apple"
    end
    on_intel do
      url "$url_x86_64_apple",
          headers: ["Accept: application/octet-stream"]
      sha256 "$sha_x86_64_apple"
    end
  end

  on_linux do
    on_intel do
      url "$url_x86_64_linux",
          headers: ["Accept: application/octet-stream"]
      sha256 "$sha_x86_64_linux"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
EOF
}

write_formula Formula/constant.rb
write_formula packaging/homebrew/constant.rb

ruby -c Formula/constant.rb >/dev/null
ruby -c packaging/homebrew/constant.rb >/dev/null

echo "updated Formula/constant.rb and packaging/homebrew/constant.rb for $tag"
