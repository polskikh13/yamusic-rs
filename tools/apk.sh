#!/bin/bash
# Собирает APK для arm64-v8a без Gradle, как android/build.ps1, но на Linux (для CI).
# Нужны ANDROID_HOME (SDK с NDK, build-tools и platforms) и JAVA_HOME.
# Подпись: ключ из ANDROID_KEYSTORE (base64), ANDROID_KEYSTORE_PASSWORD и ANDROID_KEY_ALIAS.
# Без них APK подписывается временным отладочным ключом: такой годится только для проверки.
# Результат: target/apk/yamusic-rs-arm64-v8a.apk.
set -euo pipefail
cd "$(dirname "$0")/.."

: "${ANDROID_HOME:?ANDROID_HOME не задан}"
: "${JAVA_HOME:?JAVA_HOME не задан}"
latest() { ls -d "$1"/*/ | sort -V | tail -1 | sed 's:/$::'; }
# Skia из Slint 1.19 собрана под libc++ из NDK r30: с libc++_shared.so из NDK постарше APK
# падает при запуске (нет std::__hash_memory).
NDK_VERSION=30.0.16248370
NDK="$ANDROID_HOME/ndk/$NDK_VERSION"
[ -d "$NDK" ] || { echo "Нет NDK $NDK_VERSION: sdkmanager \"ndk;$NDK_VERSION\"" >&2; exit 1; }
BT=$(latest "$ANDROID_HOME/build-tools")
# Как targetSdkVersion в манифесте: у раннера самая новая платформа бывает предварительной.
PLATFORM="$ANDROID_HOME/platforms/android-35"
[ -d "$PLATFORM" ] || PLATFORM=$(ls -d "$ANDROID_HOME"/platforms/android-*/ | sed 's:/$::' | sort -t- -k2 -n | tail -1)

ABI=arm64-v8a
TRIPLE=aarch64-linux-android
API=26
LLVM="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
export CC_aarch64_linux_android="$LLVM/bin/$TRIPLE$API-clang"
export CXX_aarch64_linux_android="$LLVM/bin/$TRIPLE$API-clang++"
export AR_aarch64_linux_android="$LLVM/bin/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
export ANDROID_NDK_ROOT="$NDK" ANDROID_NDK="$NDK" ANDROID_NDK_HOME="$NDK"

TARGET="${CARGO_TARGET_DIR:-android/target}"
cargo build --release --target "$TRIPLE" --manifest-path android/Cargo.toml --target-dir "$TARGET"

OUT=target/apk
STAGE="$OUT/stage"
rm -rf "$OUT"
mkdir -p "$STAGE/lib/$ABI"
cp "$TARGET/$TRIPLE/release/libyamusic.so" "$STAGE/lib/$ABI/"
# oboe собран с c++_shared: библиотеку надо положить рядом.
cp "$LLVM/sysroot/usr/lib/$TRIPLE/libc++_shared.so" "$STAGE/lib/$ABI/"

UNSIGNED="$OUT/unsigned.apk"
"$BT/aapt2" compile --dir android/res -o "$OUT/res.zip"
"$BT/aapt2" link -o "$UNSIGNED" --manifest android/AndroidManifest.xml -I "$PLATFORM/android.jar" --java "$OUT/gen" "$OUT/res.zip"

# Java нужен только сервису шторки: у NativeActivity своего кода нет.
find android/java "$OUT/gen" -name '*.java' > "$OUT/sources.txt"
"$JAVA_HOME/bin/javac" --release 11 -nowarn -encoding UTF-8 -classpath "$PLATFORM/android.jar" -d "$OUT/classes" @"$OUT/sources.txt"
"$BT/d8" --release --min-api "$API" --lib "$PLATFORM/android.jar" --output "$STAGE" $(find "$OUT/classes" -name '*.class')

(cd "$STAGE" && zip -q -r ../unsigned.apk .)
"$BT/zipalign" -p -f 4 "$UNSIGNED" "$OUT/aligned.apk"

APK="$OUT/yamusic-rs-$ABI.apk"
# Подпись v1 apksigner при minSdk 26 пропускает, а установщики некоторых оболочек без неё пишут «нет сертификата».
SIGN="--v1-signing-enabled true"
if [ -n "${ANDROID_KEYSTORE:-}" ]; then
  KS="$OUT/release.keystore"
  echo "$ANDROID_KEYSTORE" | base64 -d > "$KS"
  "$BT/apksigner" sign $SIGN --ks "$KS" --ks-pass env:ANDROID_KEYSTORE_PASSWORD --ks-key-alias "$ANDROID_KEY_ALIAS" --out "$APK" "$OUT/aligned.apk"
  rm -f "$KS"
else
  echo "::warning::Нет ключа выпуска: APK подписан временным ключом, в релиз он не годится"
  KS="$OUT/debug.keystore"
  "$JAVA_HOME/bin/keytool" -genkeypair -keystore "$KS" -storepass android -alias debug -keypass android \
    -dname 'CN=Debug' -keyalg RSA -keysize 2048 -validity 30 >/dev/null
  "$BT/apksigner" sign $SIGN --ks "$KS" --ks-pass pass:android --out "$APK" "$OUT/aligned.apk"
fi
"$BT/apksigner" verify "$APK"
ls -l "$APK"
