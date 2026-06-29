class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.5.1"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/461009359",
          headers: ["Accept: application/octet-stream"]
      sha256 "35e32840fa32d186869004fb96b9c32963978abbc87d3b315fc4a974aadf3046"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/461009363",
          headers: ["Accept: application/octet-stream"]
      sha256 "5c496b8950e730005aff35e8f77607ab413ffe79955128993822e06e63cef036"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/461009361",
          headers: ["Accept: application/octet-stream"]
      sha256 "8d24d07bf7df1391d0817a30f84797b034d1bcc6c9becef98d2b4fb5b6eeff76"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
