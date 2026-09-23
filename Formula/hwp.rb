# Homebrew formula - this repository is the tap itself (there is no separate homebrew-* repo).
#
#   brew tap staixbwlb/hwp https://github.com/STAIxBWLB/hwp-cli
#   brew install hwp
#
# It installs a prebuilt binary from the release archive, so no Rust toolchain is needed.
# `version` and every `sha256` are rewritten by release.yml's update-formula job on a tag push
# (do not edit them by hand - the next release overwrites them).
#
# The user-facing strings here (desc, caveats) are English: they are what Homebrew prints during
# install and upgrade, and a tap's output is read by people who never opened this repository.
class Hwp < Formula
  # brew style: desc must not begin with the formula name (hwp).
  desc "Read, convert, render and edit Hangul HWP 5.0 and HWPX documents"
  homepage "https://github.com/STAIxBWLB/hwp-cli"
  version "0.19.2"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/STAIxBWLB/hwp-cli/releases/download/v#{version}/hwp-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "1c2b973b8fbbc3738f725aa4cc6c1ce2a0f6c5862610a9af4604ee0e3aafd96f"
    end
    on_intel do
      url "https://github.com/STAIxBWLB/hwp-cli/releases/download/v#{version}/hwp-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "3df834c04532e316325549fb0fbf65adbe81491bf641337a179adc0238953ad5"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/STAIxBWLB/hwp-cli/releases/download/v#{version}/hwp-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "39082f8e91e0a2e90cdf95b7625cbe612038a8bf3820b95599c9076cf1059d73"
    end
  end

  def install
    bin.install "hwp"
  end

  def caveats
    <<~EOS
      Rendering (render/convert -o *.pdf|png) needs a CJK font:
        brew install --cask font-noto-sans-cjk-kr
      Or point at a Hamchorom font directory:
        hwp render doc.hwp -o out.png --font-dir <font-dir>
        HWP_FONT_DIR=<font-dir> hwp convert doc.hwp -o out.pdf
      Text extraction and format conversion (cat/convert -o *.md|hwpx|json) need no fonts.
    EOS
  end

  test do
    # Version output plus a real create/re-read round trip, so the test cannot pass on a binary
    # that merely exists. The fixture text stays Korean: handling it is what this tool is for.
    assert_match version.to_s, shell_output("#{bin}/hwp --version")
    (testpath/"t.md").write("# 제목\n\n본문입니다.\n")
    system bin/"hwp", "new", "--from", testpath/"t.md", "-o", testpath/"t.hwpx"
    assert_match "본문입니다", shell_output("#{bin}/hwp cat #{testpath}/t.hwpx")
  end
end
