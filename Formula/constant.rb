class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.8.3"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/685059956",
          headers: ["Accept: application/octet-stream"]
      sha256 "e21afcf58c31e96b9781fbe0efb3d38558e7b2297c097a35aa08af7f5a483cc2"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/685059951",
          headers: ["Accept: application/octet-stream"]
      sha256 "5ef4bd22bfa4da81854647605029d193369b7b11bca61e41f7fbdd90a694ba97"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/685059957",
          headers: ["Accept: application/octet-stream"]
      sha256 "3b7fced6df10cd9fd864468af5317639653c74cab3b07e4fc7eb5875b21bc432"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
