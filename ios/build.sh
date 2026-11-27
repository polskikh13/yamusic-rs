#!/usr/bin/env bash
# Сборка iOS-версии без Xcode-проекта: cargo собирает исполняемый файл, скрипт кладёт его
# в .app вместе с Info.plist и значками. Нужен Mac с Xcode (или macOS-раннер GitHub Actions).
#   ./build.sh            IPA для iPhone без подписи: подпишет Sideloadly или AltStore
#   ./build.sh --sim      .app для симулятора, ставится и запускается в открытом симуляторе
#   ./build.sh --fast     профиль fast: без LTO, для проверок
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$here/.."
target=aarch64-apple-ios
profile=release
sim=0
for arg in "$@"; do
    case "$arg" in
        --sim) sim=1; target=aarch64-apple-ios-sim ;;
        --fast) profile=fast ;;
        *) echo "неизвестный ключ: $arg" >&2; exit 2 ;;
    esac
done

rustup target add "$target" >/dev/null
export IPHONEOS_DEPLOYMENT_TARGET=15.0
cargo build --manifest-path "$here/Cargo.toml" --target "$target" --profile "$profile" --bin yamusic-rs

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/Cargo.toml" | head -n1)"
out="${CARGO_TARGET_DIR:-$here/target}"
app="$out/ios/YamusicRS.app"
rm -rf "$out/ios"
mkdir -p "$app"
cp "$out/$target/$profile/yamusic-rs" "$app/"
sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Info.plist"
plutil -lint "$app/Info.plist" >/dev/null
printf 'APPL????' > "$app/PkgInfo"

# Значки из того же рисунка, что у десктопа: iPhone @2x/@3x и iPad @2x.
icon="$root/packaging/icons/256.png"
sips -z 120 120 "$icon" --out "$app/AppIcon60x60@2x.png" >/dev/null
sips -z 180 180 "$icon" --out "$app/AppIcon60x60@3x.png" >/dev/null
sips -z 152 152 "$icon" --out "$app/AppIcon76x76@2x~ipad.png" >/dev/null

if [ "$sim" = 1 ]; then
    # Симулятору хватает подписи «для себя».
    codesign --force --sign - "$app"
    xcrun simctl install booted "$app"
    xcrun simctl launch --console-pty booted io.github.spolzermod.yamusicrs
else
    rm -rf "$out/ios/Payload"
    mkdir -p "$out/ios/Payload"
    cp -R "$app" "$out/ios/Payload/"
    (cd "$out/ios" && zip -qry yamusic-rs.ipa Payload)
    rm -rf "$out/ios/Payload"
    echo "Готово: $out/ios/yamusic-rs.ipa (без подписи)"
fi
