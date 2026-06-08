class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.1.3"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/441317140",
          headers: ["Accept: application/octet-stream"]
      sha256 "4abb464f8f562399807634e8abed332f0d3d56f58468f94389cefc18ba8302f7"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/441317142",
          headers: ["Accept: application/octet-stream"]
      sha256 "49cbb49b314c048e88f86d2e6cb9b34433040c866bbfa05a24d19e4caf4c802b"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/441317138",
          headers: ["Accept: application/octet-stream"]
      sha256 "13cb479ca674b19ab5f13dc5bdce706da8a375daf4dc1412c6f8947b86368b18"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
