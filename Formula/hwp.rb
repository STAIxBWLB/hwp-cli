# Homebrew formula — 저장소 자체가 tap 이다(별도 homebrew-* 저장소 없음).
#
#   brew tap staixbwlb/hwp https://github.com/STAIxBWLB/hwp-cli
#   brew install hwp
#
# 릴리스 아카이브(사전 빌드 바이너리)를 받아 설치하므로 Rust 툴체인이 필요 없다.
# version/sha256 은 태그 푸시 때 release.yml 의 update-formula 잡이 자동 갱신한다
# (손으로 고치지 말 것 — 다음 릴리스에서 덮어써진다).
class Hwp < Formula
  # brew style: desc 는 formula 이름(hwp)으로 시작하면 안 된다.
  desc "한글 문서(HWP 5.0·HWPX) 읽기·변환·렌더·편집 단일 바이너리"
  homepage "https://github.com/STAIxBWLB/hwp-cli"
  version "0.8.6"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/STAIxBWLB/hwp-cli/releases/download/v#{version}/hwp-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "82b6c1085822c591666eaa49cf1754773202d0900b0b07cf1af1eae1a692776b"
    end
    on_intel do
      url "https://github.com/STAIxBWLB/hwp-cli/releases/download/v#{version}/hwp-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "c38f4ce7bb8d2977d36863d31777d79ce4eb0da5054491b0b3e5698fd4b29336"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/STAIxBWLB/hwp-cli/releases/download/v#{version}/hwp-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "dc96ac6958197d1e9236f13888f4a8d2c38dbaf188e7f3c543811afeca935d20"
    end
  end

  def install
    bin.install "hwp"
  end

  def caveats
    <<~EOS
      렌더링(render/convert -o *.pdf|png)에는 CJK 폰트가 필요하다:
        brew install --cask font-noto-sans-cjk-kr
      또는 함초롬 폰트 디렉터리를 지정한다:
        hwp render doc.hwp -o out.png --font-dir <폰트디렉터리>
        HWP_FONT_DIR=<폰트디렉터리> hwp convert doc.hwp -o out.pdf
      텍스트 추출·포맷 변환(cat/convert -o *.md|hwpx|json)은 폰트 없이 동작한다.
    EOS
  end

  test do
    # 버전 출력 + 실제 문서 생성/재읽기까지 확인한다(바이너리만 놓고 통과하지 않게).
    assert_match version.to_s, shell_output("#{bin}/hwp --version")
    (testpath/"t.md").write("# 제목\n\n본문입니다.\n")
    system bin/"hwp", "new", "--from", testpath/"t.md", "-o", testpath/"t.hwpx"
    assert_match "본문입니다", shell_output("#{bin}/hwp cat #{testpath}/t.hwpx")
  end
end
