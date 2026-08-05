class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.6.1"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/502079641",
          headers: ["Accept: application/octet-stream"]
      sha256 "5adb40152c6a3a7cff626f97a298aba5fc481e901dbfe23e67684e0d731c6ce9"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/502079645",
          headers: ["Accept: application/octet-stream"]
      sha256 "d0ea3644752709942f568177c6d4503c7f940ad8513607552b8bbe26f0e63642"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/502079646",
          headers: ["Accept: application/octet-stream"]
      sha256 "56098288ad3b4db38605122272c5cb4fe4f458bb8991f1d4b9f2d1f70836ad78"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
