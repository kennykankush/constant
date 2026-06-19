class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.5.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/451807506",
          headers: ["Accept: application/octet-stream"]
      sha256 "37b9c4134c40dbd2852bd6a56a8ac8820f9d2b52af8666d044406060fb6210e8"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/451807504",
          headers: ["Accept: application/octet-stream"]
      sha256 "f75d2579c99c38933e22d67722602926240328122dab96df893c7ac62e40c07e"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/451807503",
          headers: ["Accept: application/octet-stream"]
      sha256 "00def2777f3e7ed0aafe2f8f2a63c44b98acbccd735053d42566b905e00b0ca2"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
