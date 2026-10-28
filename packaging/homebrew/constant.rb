class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.8.1"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/684590697",
          headers: ["Accept: application/octet-stream"]
      sha256 "1e23d92321548d444d6423863829a0de3a256b09c8ef970e237d6d47352b9da8"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/684590698",
          headers: ["Accept: application/octet-stream"]
      sha256 "67692e1cb91f2a452fbe4bed34e7dc9e91c38cbb63a4c02215dd18424634454a"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/684590696",
          headers: ["Accept: application/octet-stream"]
      sha256 "5021419ec5aff84c99d378407c488a34930f691b0ec555e4b56eb8baf2437c85"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
