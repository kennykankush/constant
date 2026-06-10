class Constant < Formula
  desc "Switch active agent CLIs mid-conversation without re-explaining the thread"
  homepage "https://github.com/kennykankush/constant"
  version "0.3.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/443967312",
          headers: ["Accept: application/octet-stream"]
      sha256 "542b5385892296e1d4121a3fed7cff61209b849ac12cf10335ee0b1ef29188d3"
    end
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/443967314",
          headers: ["Accept: application/octet-stream"]
      sha256 "48b3cb06ae146b8d074367bcb8e7b072f78d405a190f0cec92214b270e67544b"
    end
  end

  on_linux do
    on_intel do
      url "https://api.github.com/repos/kennykankush/constant/releases/assets/443967304",
          headers: ["Accept: application/octet-stream"]
      sha256 "02289425834b455a2715163dc2f379c9e79909d207862f5f205592a5f8e47e75"
    end
  end

  def install
    bin.install "constant"
  end

  test do
    assert_match "constant #{version}", shell_output("#{bin}/constant --version")
  end
end
