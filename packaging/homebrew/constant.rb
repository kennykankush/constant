class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.1.4"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/441337198",
          headers: ["Accept: application/octet-stream"]
      sha256 "447e9a8ac662d6188cea9abdbc8511dcb93609e52e3bbb9e66f8ff25985633d2"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/441337196",
          headers: ["Accept: application/octet-stream"]
      sha256 "55fc70048958cc715dc8e8fae3d8ad53e184b67f9484d152743b4d4aa5875159"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/441337197",
          headers: ["Accept: application/octet-stream"]
      sha256 "ea3666e7a1f509efd334a279f658fd5891b9db08b77249094573e594044b3030"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
