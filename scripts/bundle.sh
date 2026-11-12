#!/bin/sh
# 릴리스 빌드를 dist/Arong.app 번들로 묶는다.
# 서명은 로컬 실행용 ad-hoc이다. 배포용 Developer ID 서명·공증은 아직 없다.
set -eu

cd "$(dirname "$0")/.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/arong-app/Cargo.toml | head -1)
APP=dist/Arong.app

cargo build --release -p arong-app

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/arong-app "$APP/Contents/MacOS/Arong"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>dev.cbsong.arong</string>
    <key>CFBundleName</key><string>Arong</string>
    <key>CFBundleDisplayName</key><string>아롱이</string>
    <key>CFBundleExecutable</key><string>Arong</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>LSUIElement</key><true/>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSMicrophoneUsageDescription</key><string>훈련 화면에서 "말로 훈련"을 켠 동안만, 앉아·왼손 같은 명령을 듣기 위해 마이크를 씁니다. 소리는 저장하지 않습니다.</string>
    <key>NSSpeechRecognitionUsageDescription</key><string>말한 명령(앉아, 왼손, 오른손, 엎드려, 일어서)을 이 Mac 안에서만 인식합니다. 인식한 문장은 저장하지 않습니다.</string>
</dict>
</plist>
EOF

codesign --force --sign - "$APP"
echo "built $APP ($VERSION)"
