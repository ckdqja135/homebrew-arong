cask "arong" do
  version "0.1.0"
  sha256 "7bf9ffa1b7756a9a6cb4ad60a8906fc473b6cd99912bbed773dcbcb7dc30817b"

  url "https://github.com/ckdqja135/homebrew-arong/releases/download/v#{version}/Arong-#{version}-arm64.zip"
  name "Arong"
  name "아롱이"
  desc "일하는 동안 화면 구석에서 같이 지내는 데스크톱 반려견"
  homepage "https://github.com/ckdqja135/homebrew-arong"

  depends_on arch: :arm64
  depends_on macos: :ventura

  app "Arong.app"

  # 업그레이드·삭제 전에 앱을 정상 종료시킨다(종료하면서 상태를 저장한다).
  uninstall quit: "dev.cbsong.arong"

  # `brew uninstall --zap arong` 일 때만 육성 데이터까지 지운다. 그냥 uninstall은 데이터를 남긴다.
  zap trash: "~/Library/Application Support/dev.cbsong.arong"

  caveats <<~EOS
    처음 열 때 막히면 시스템 설정 → 개인정보 보호 및 보안에서 '그래도 열기'를 누르세요.
    아롱이는 메뉴 막대의 발바닥 아이콘으로 실행됩니다(Dock 아이콘 없음).

    타이핑 집계를 켜 두었다면, 업데이트한 뒤 입력 모니터링 권한을 다시 허용해야 할 수 있습니다.
    육성 데이터는 앱을 지워도 남습니다. 함께 지우려면: brew uninstall --zap arong
  EOS
end
