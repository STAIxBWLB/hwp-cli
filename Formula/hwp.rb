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
  version "0.4.1"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/STAIxBWLB/hwp-cli/releases/download/v#{version}/hwp-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "c51384b1fb0bf5b5b9f227d769228f540592b7dbdbbfc878978ecced62d5373c"
    end
    on_intel do
      url "https://github.com/STAIxBWLB/hwp-cli/releases/download/v#{version}/hwp-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "6bef1444275554b2aedde07b680fbd959675ec7ec6ebd0b944711fd8e87762b1"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/STAIxBWLB/hwp-cli/releases/download/v#{version}/hwp-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "61db250203d40d8aa528b14c380be0cf08e6bfdbecc0cd99748c0d7ec95326d5"
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
