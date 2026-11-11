#!/bin/sh
# 배포 산출물을 만든다: dist/Arong-<버전>-arm64.zip 과, 그 체크섬이 들어간 Homebrew cask(dist/arong.rb).
#
# 올리는 일은 하지 않는다. 만든 뒤 직접:
#   1) tap 저장소(TAP_REPO)의 릴리스 v<버전>에 zip을 올리고
#   2) dist/arong.rb 를 tap 저장소의 Casks/arong.rb 로 넣는다.
#
# 서명은 로컬 실행용 ad-hoc이다(Developer ID 서명·공증 없음). 그래서 받은 사람은 처음 한 번
# 시스템 설정 → 개인정보 보호 및 보안에서 '그래도 열기'를 눌러야 한다.
set -eu

cd "$(dirname "$0")/.."
TAP_REPO="${TAP_REPO:-ckdqja135/homebrew-arong}"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/arong-app/Cargo.toml | head -1)
ZIP="dist/Arong-$VERSION-arm64.zip"

./scripts/bundle.sh
rm -f "$ZIP"
# --keepParent: zip 안에 Arong.app 폴더째 넣는다.
# --norsrc --noextattr: 리소스 포크·확장 속성용 "._" 파일을 넣지 않는다(번들 안에 섞이면 서명 검증이 깨진다).
ditto -c -k --norsrc --noextattr --keepParent dist/Arong.app "$ZIP"
SHA=$(shasum -a 256 "$ZIP" | cut -d' ' -f1)

sed -e "s/@VERSION@/$VERSION/" -e "s/@SHA256@/$SHA/" -e "s|@TAP_REPO@|$TAP_REPO|g" packaging/arong.rb.in > dist/arong.rb
ruby -c dist/arong.rb >/dev/null

echo "zip:    $ZIP"
echo "sha256: $SHA"
echo "cask:   dist/arong.rb  (tap: $TAP_REPO)"
