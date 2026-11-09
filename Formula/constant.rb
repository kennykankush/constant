class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.9.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/728950450",
          headers: ["Accept: application/octet-stream"]
      sha256 "2ea2041fb054f1a99dc78f87cb857ef61ddb47bdf1485c6d016bb0006d812bc9"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/728950447",
          headers: ["Accept: application/octet-stream"]
      sha256 "ec278f9e6351d14f355a6fdecbbbc1398009503b6f796b4ea74e200b721bd75c"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/728950449",
          headers: ["Accept: application/octet-stream"]
      sha256 "34dadf5ebd3c511a9997666947861934de54c63db7ac343111c2084e2c27b6d0"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
