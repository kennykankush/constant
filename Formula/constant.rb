class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.8.2"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/684940804",
          headers: ["Accept: application/octet-stream"]
      sha256 "3d9583d813b65b351775531f8b9115e624de03266048c4dd0d2e882b9dd4f27a"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/684940806",
          headers: ["Accept: application/octet-stream"]
      sha256 "f258dc8e7b9054d4143cbea326c7fe033647edaf9d7f865141e3ce27abee81fd"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/684940803",
          headers: ["Accept: application/octet-stream"]
      sha256 "4a826ed082dd6d7de76a24c5ec1d49d8e815539c276767bcf1129381ab6e1c29"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
