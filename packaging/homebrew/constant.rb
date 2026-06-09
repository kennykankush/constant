class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.2.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/443108138",
          headers: ["Accept: application/octet-stream"]
      sha256 "34cc1b14cd0e16449856edd2d9757008927798a8449a5d97d0f968291ebdacca"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/443108140",
          headers: ["Accept: application/octet-stream"]
      sha256 "509b837aed6f23d1ae50652c662c6f20f54cf263bf8b1e1600af4a39ff5a5a34"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/443108136",
          headers: ["Accept: application/octet-stream"]
      sha256 "5c63604f8ffae86bf06d4010e2b979543edbb090904b1790ad436da032499da7"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
