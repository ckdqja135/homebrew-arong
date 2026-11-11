# 배포 (개인 Homebrew tap)

아롱이는 서버·계정 없이 동작하고, 배포는 개인 tap의 cask로 한다.
Developer ID 서명·공증은 하지 않는다(ad-hoc 서명). 그래서 받은 사람은 처음 한 번
시스템 설정 → 개인정보 보호 및 보안에서 "그래도 열기"를 눌러야 한다.

## 준비 (한 번)

1. GitHub에 공개 저장소 `homebrew-arong`을 만든다(이름이 `homebrew-`로 시작해야 tap이 된다).
2. 그 저장소에 `Casks/` 폴더를 둔다.

## 릴리스할 때

```sh
./scripts/release.sh          # dist/Arong-<버전>-arm64.zip, dist/arong.rb 생성
```

1. tap 저장소에 릴리스 `v<버전>`을 만들고 zip을 올린다.
2. `dist/arong.rb`를 tap 저장소의 `Casks/arong.rb`로 넣고 커밋한다.

다른 계정·저장소 이름을 쓰려면: `TAP_REPO=<계정>/homebrew-<이름> ./scripts/release.sh`

## 설치·업데이트·삭제

```sh
brew install --cask ckdqja135/arong/arong
brew upgrade --cask arong
brew uninstall --cask arong          # 앱만 삭제, 육성 데이터는 남는다
brew uninstall --zap --cask arong    # 육성 데이터까지 삭제
```

## 알아둘 점

- Apple Silicon 전용이다(Intel 빌드는 만들지 않는다).
- ad-hoc 서명은 빌드할 때마다 달라진다. macOS는 업데이트된 앱을 다른 앱으로 볼 수 있어서,
  입력 모니터링 권한과 "Mac 켤 때 자동 실행"을 업데이트 뒤 다시 켜야 할 수 있다.
- 육성 데이터는 `~/Library/Application Support/dev.cbsong.arong/`에 있고, 앱 번들과 따로 관리된다.
- Homebrew가 내려받는 것은 설치·업데이트 때뿐이다. 앱 자체는 실행 중 외부로 통신하지 않는다.
