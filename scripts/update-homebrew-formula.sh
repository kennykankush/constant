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
need curl
need ruby

sha_for() {
  target="$1"
  asset="constant-$tag-$target.tar.gz"
  curl -fsSL "$base/$asset.sha256" | awk '{print $1}'
}

sha_aarch64_apple="$(sha_for aarch64-apple-darwin)"
sha_x86_64_apple="$(sha_for x86_64-apple-darwin)"
sha_x86_64_linux="$(sha_for x86_64-unknown-linux-gnu)"

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
      url "$base/constant-$tag-aarch64-apple-darwin.tar.gz"
      sha256 "$sha_aarch64_apple"
    end
    on_intel do
      url "$base/constant-$tag-x86_64-apple-darwin.tar.gz"
      sha256 "$sha_x86_64_apple"
    end
  end

  on_linux do
    on_intel do
      url "$base/constant-$tag-x86_64-unknown-linux-gnu.tar.gz"
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
