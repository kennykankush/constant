class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.3.1"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/444027400",
          headers: ["Accept: application/octet-stream"]
      sha256 "da887ed039ea3b7b3be988668cdc23b73a2205572bd2e7737e42a8c6ad505aaf"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/444027402",
          headers: ["Accept: application/octet-stream"]
      sha256 "d58e38174585aa0d2a13640dc9a8203c01492f5e37e14f8e223ef77cc5d64a65"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/444027399",
          headers: ["Accept: application/octet-stream"]
      sha256 "73d9b8e3cf469815029eedad435b177c7cc65b4674f37081f4485386aa0af29b"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
