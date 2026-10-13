class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.7.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/636341257",
          headers: ["Accept: application/octet-stream"]
      sha256 "e3b72f4dd4db50f4b76437099b4beab008a7853a49b0f250663b6ecc5ec2eaf0"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/636341262",
          headers: ["Accept: application/octet-stream"]
      sha256 "4cb964193a2cb85a4aeefac51224289a428650e267077ba10fc8b2a6d8107389"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/636341253",
          headers: ["Accept: application/octet-stream"]
      sha256 "38108315119c128a8a51cd5b7838356d99a33a306e2b7d40b56416b98dfafdb8"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
