class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.6.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/474788675",
          headers: ["Accept: application/octet-stream"]
      sha256 "da98b603c79d91a4f0459238b92a911e1bf90ea18989f11b846bfe4a1bfb2b8d"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/474788676",
          headers: ["Accept: application/octet-stream"]
      sha256 "0fbfe4a2ff00e95e8beae362c6fa3cf92dc4ed007795ae280be22f2bb7b1b396"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/474788678",
          headers: ["Accept: application/octet-stream"]
      sha256 "220ff442f9f203a824b6671bf8e83cecb31af3e7ca3641d77e5cb8fcedb3514a"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
