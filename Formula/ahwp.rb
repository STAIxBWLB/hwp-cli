# Homebrew formula - this repository is the tap itself (there is no separate homebrew-* repo).
#
#   brew tap staixbwlb/hwp https://github.com/STAIxBWLB/agent-hwp
#   brew install ahwp
#
# `ahwp` is the command name since 1.6.0, and `hwp` is installed beside it as the 1.x
# compatibility name (issue #510). formula_renames.json maps the old formula name to this one, so
# a machine that has `hwp` installed migrates on `brew update && brew upgrade` without a retap.
#
# It installs a prebuilt binary from the release archive, so no Rust toolchain is needed.
# `version` and every `sha256` are rewritten by release.yml's update-formula job on a tag push
# (do not edit them by hand - the next release overwrites them).
#
# The URLs keep the legacy `hwp-<tag>-<target>` asset name on purpose: 1.x publishes that name for
# every release as well as `ahwp-`, so this formula installs from the newest release at any moment,
# including before the bridge release. 2.0.0 retires the legacy assets and these URLs move to
# `ahwp-` with them (issue #510, Phase 4).
#
# The user-facing strings here (desc, caveats) are English: they are what Homebrew prints during
# install and upgrade, and a tap's output is read by people who never opened this repository.
class Ahwp < Formula
  # brew style: desc must not begin with the formula name (ahwp).
  desc "Read, convert, render and edit Hangul HWP 5.0 and HWPX documents"
  homepage "https://github.com/STAIxBWLB/agent-hwp"
  version "1.6.3"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/STAIxBWLB/agent-hwp/releases/download/v#{version}/hwp-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "38aa6daeafcfe7468805f02620267d505512032129ce55ba3204815d2bbbf6bd"
    end
    on_intel do
      url "https://github.com/STAIxBWLB/agent-hwp/releases/download/v#{version}/hwp-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "e44ab561cddcb4c96c20f7d050959e5c68933fe597b0cadaed8cdfa80cc6628e"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/STAIxBWLB/agent-hwp/releases/download/v#{version}/hwp-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0fc25833cdb2e2adb495b1ac85a90d8461d41774b1a51e07bc4bb603b3955773"
    end
  end

  def install
    if File.exist?("ahwp")
      # Every archive from 1.6.0 on carries both names.
      bin.install "ahwp", "hwp"
    else
      # Archives released before the rename carry `hwp` only, so the new name is a link to it.
      bin.install "hwp"
      bin.install_symlink "hwp" => "ahwp"
    end
  end

  def caveats
    <<~EOS
      `ahwp` is the command name from 1.6.0. `hwp` is installed beside it and keeps working
      until 2.0.0.

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
    # The command runs under its new name, and the compatibility name is proved to reach the same
    # binary rather than merely existing.
    assert_match version.to_s, shell_output("#{bin}/ahwp --version")
    assert_match version.to_s, shell_output("#{bin}/hwp --version")
    (testpath/"t.md").write("# 제목\n\n본문입니다.\n")
    system bin/"ahwp", "new", "--from", testpath/"t.md", "-o", testpath/"t.hwpx"
    assert_match "본문입니다", shell_output("#{bin}/ahwp cat #{testpath}/t.hwpx")
  end
end
