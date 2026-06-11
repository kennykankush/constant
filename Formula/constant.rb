class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.3.2"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/444694546",
          headers: ["Accept: application/octet-stream"]
      sha256 "0246ee2f52a6090d89e72f27b0b33a93d34e42dc7c7ca199d0a37c9c1761d894"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/444694545",
          headers: ["Accept: application/octet-stream"]
      sha256 "44b3d2b16ecd8d329215325fbcebbcbb8d478bba0d3a759d2ed0dca1e3163a88"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/444694541",
          headers: ["Accept: application/octet-stream"]
      sha256 "78ca9804a3ae7d17656941517d5fb06b00c7b1f6abf539c96cefcad063ef62f8"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
