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
  version "1.6.1"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/STAIxBWLB/agent-hwp/releases/download/v#{version}/hwp-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "3e0c6517f4406f87c7319d8b6e56722d78919ef504a4d1564dc3a6c2c21c6995"
    end
    on_intel do
      url "https://github.com/STAIxBWLB/agent-hwp/releases/download/v#{version}/hwp-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "d31f23c92cdbc0884fb2f769d10c918ad532e00dc3f74bfaeea93061e143e388"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/STAIxBWLB/agent-hwp/releases/download/v#{version}/hwp-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "3c21e3c4bd10633cedf45825b4dad1c35c8b0b6024177ece1ef17d1ec19e3380"
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
