class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.1.2"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/439810253",
          headers: ["Accept: application/octet-stream"]
      sha256 "de152e6c4439966e50a565b93402f2187aebd95d03de7aa6e09882436b20f912"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/439810254",
          headers: ["Accept: application/octet-stream"]
      sha256 "f3a31920e41a754aad0e0862dac2f85572dad0889b241ffb00d645a2fc4dbf59"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/439810251",
          headers: ["Accept: application/octet-stream"]
      sha256 "7f05317b7d5fd438b60542f8af69238c7f6b27b2316551af4efbd415a34a37c4"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
