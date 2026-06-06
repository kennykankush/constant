# Homebrew formula for Constant (prebuilt release artifacts).
#
# PREPARED, NOT YET LIVE. On the first v0.1.0 release:
#   1. fill in each sha256 from the published *.tar.gz.sha256 files,
#   2. copy this file to the tap repo (kennykankush/homebrew-tap) as
#      Formula/constant.rb — only after the release exists and with approval.
#
# Then: brew install kennykankush/tap/constant
class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/kennykankush/constant/releases/download/v0.1.0/constant-v0.1.0-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_AARCH64_APPLE_DARWIN_SHA256"
    end
    on_intel do
      url "https://github.com/kennykankush/constant/releases/download/v0.1.0/constant-v0.1.0-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_X86_64_APPLE_DARWIN_SHA256"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/kennykankush/constant/releases/download/v0.1.0/constant-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_X86_64_UNKNOWN_LINUX_GNU_SHA256"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    system "#{bin}/constant", "--version"
  end
end
