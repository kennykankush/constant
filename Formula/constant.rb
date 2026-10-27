class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.8.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/680456750",
          headers: ["Accept: application/octet-stream"]
      sha256 "feb079e993fcb3f30809cc09e027db860c1d28b61e12f10e7268774ce1a68e18"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/680456748",
          headers: ["Accept: application/octet-stream"]
      sha256 "17d103613086c358cd507d24be27aee49372d8963cc90a46176303924b126429"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/680456753",
          headers: ["Accept: application/octet-stream"]
      sha256 "ba72595f2c436f3617d025b667deb87fd86b5e3f4cc84be66f2a8bf972f40857"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
