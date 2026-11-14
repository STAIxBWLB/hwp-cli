# Homebrew formula - this repository is the tap itself (there is no separate homebrew-* repo).
#
#   brew tap staixbwlb/hwp https://github.com/STAIxBWLB/agent-hwp
#   brew install ahwp
#
# `ahwp` is the command name since 1.6.0; 2.0 removed the `hwp` compatibility name (issue #510).
# formula_renames.json still maps the old formula name to this one and stays: Homebrew applies a
# rename only during `brew update` and `brew migrate`, so a machine still on formula `hwp` that has
# not updated since the rename needs the record to find this formula.
#
# It installs a prebuilt binary from the release archive, so no Rust toolchain is needed.
# `version` and every `sha256` are rewritten by release.yml's update-formula job on a tag push
# (do not edit them by hand - the next release overwrites them).
#
# The URLs use the `ahwp-<tag>-<target>` asset name, the only one 2.0 publishes. Every release
# from 1.6.0 on carries it, so this formula installs from the current release too.
#
# The user-facing strings here (desc, caveats) are English: they are what Homebrew prints during
# install and upgrade, and a tap's output is read by people who never opened this repository.
class Ahwp < Formula
  # brew style: desc must not begin with the formula name (ahwp).
  desc "Read, convert, render and edit Hangul HWP 5.0 and HWPX documents"
  homepage "https://github.com/STAIxBWLB/agent-hwp"
  version "2.0.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/STAIxBWLB/agent-hwp/releases/download/v#{version}/ahwp-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "6ce7d60f2816d49078ea6269023bf53ed9150d02ebd5518801c65006fd7b92fc"
    end
    on_intel do
      url "https://github.com/STAIxBWLB/agent-hwp/releases/download/v#{version}/ahwp-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "199f44e6d5cc1f22e81c9188bd31031d247817ca7096ee98ebb58f4123b73497"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/STAIxBWLB/agent-hwp/releases/download/v#{version}/ahwp-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "2b11ff98e43f22561fabe5f4a5ba98066ee3b5afb41a6e064814fc1844002822"
    end
  end

  def install
    bin.install "ahwp"
  end

  def caveats
    <<~EOS
      The command is `ahwp`. 2.0.0 removed the `hwp` compatibility name.

      Rendering (render/convert -o *.pdf|png) needs a CJK font:
        brew install --cask font-noto-sans-cjk-kr
      Or point at a Hamchorom font directory:
        ahwp render doc.hwp -o out.png --font-dir <font-dir>
        HWP_FONT_DIR=<font-dir> ahwp convert doc.hwp -o out.pdf
      Text extraction and format conversion (cat/convert -o *.md|hwpx|json) need no fonts.
    EOS
  end

  test do
    # Version output plus a real create/re-read round trip, so the test cannot pass on a binary
    # that merely exists. The fixture text stays Korean: handling it is what this tool is for.
    assert_match version.to_s, shell_output("#{bin}/ahwp --version")
    (testpath/"t.md").write("# 제목\n\n본문입니다.\n")
    system bin/"ahwp", "new", "--from", testpath/"t.md", "-o", testpath/"t.hwpx"
    assert_match "본문입니다", shell_output("#{bin}/ahwp cat #{testpath}/t.hwpx")
  end
end
