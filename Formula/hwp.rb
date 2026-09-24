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
  version "0.20.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/STAIxBWLB/hwp-cli/releases/download/v#{version}/hwp-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "75d97556a2b8e7ff5db4c77626d141be0d19faf2f364e9f8043c5d7a9e236257"
    end
    on_intel do
      url "https://github.com/STAIxBWLB/hwp-cli/releases/download/v#{version}/hwp-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "ac7d8259b7f57a823c681b920355a38829b5b2a90abcef3e57696f04e4402dc4"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/STAIxBWLB/hwp-cli/releases/download/v#{version}/hwp-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "ab41363d153a0c97f50b90582d459df6e1fada88105b57922e67f71946e75d9c"
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
