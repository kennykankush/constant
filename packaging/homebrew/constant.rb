class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.3.3"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/444711056",
          headers: ["Accept: application/octet-stream"]
      sha256 "a6f5c9535f7efecc9ce2c7aa4fac8eb4659bb82396f6e137f742325d6198794c"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/444711058",
          headers: ["Accept: application/octet-stream"]
      sha256 "ee3771c5a386e77db91975e49cda1942bfdeafbff1e4d133d00e65ae650eaccc"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/444711053",
          headers: ["Accept: application/octet-stream"]
      sha256 "68390c610f143d22d91069efee5048d96f531c9877ee993ef566af7ade62bc62"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
