# Сборка APK без Gradle: cargo собирает libyamusic.so, aapt2 упаковывает манифест,
# zipalign и apksigner из build-tools подписывают отладочным ключом.
#   .\build.ps1                      x86_64 для эмулятора
#   .\build.ps1 -Abi arm64-v8a       для телефона
#   .\build.ps1 -Install             и сразу поставить через adb
param(
    [ValidateSet('x86_64', 'arm64-v8a')] [string] $Abi = 'x86_64',
    [switch] $Install,
    [switch] $DebugBuild,
    [switch] $Fast
)
# Не Stop: PowerShell 5.1 считает ошибкой любой вывод cargo в stderr. Внешние программы
# проверяются по $LASTEXITCODE, у командлетов стоит -ErrorAction Stop.
$ErrorActionPreference = 'Continue'
$PSDefaultParameterValues['*:ErrorAction'] = 'Stop'

# Переменная могла появиться после запуска терминала: тогда берём её из настроек пользователя.
$sdk = @($env:ANDROID_HOME, [Environment]::GetEnvironmentVariable('ANDROID_HOME', 'User')) | Where-Object { $_ } | Select-Object -First 1
if (-not $sdk) { throw 'ANDROID_HOME не задан' }
# Slint собирает свой Java-помощник и ищет android.jar по этой переменной.
$env:ANDROID_HOME = $sdk
$ndk = Get-ChildItem "$sdk\ndk" -Directory | Sort-Object { [version]$_.Name } | Select-Object -Last 1 -ExpandProperty FullName
# Skia из Slint 1.19 собрана под libc++ из NDK r30: со старым libc++_shared.so APK падает при запуске.
if ([version](Split-Path $ndk -Leaf) -lt [version]'30.0') { throw "Нужен NDK r30 или новее, а есть $ndk" }
$bt = Get-ChildItem "$sdk\build-tools" -Directory | Sort-Object { [version]$_.Name } | Select-Object -Last 1 -ExpandProperty FullName
$platform = Get-ChildItem "$sdk\platforms" -Directory | Sort-Object { [int]($_.Name -replace 'android-', '') } | Select-Object -Last 1 -ExpandProperty FullName
if (-not $env:JAVA_HOME) { $env:JAVA_HOME = 'E:\Android\android-studio\jbr' }

$triple = @{ 'x86_64' = 'x86_64-linux-android'; 'arm64-v8a' = 'aarch64-linux-android' }[$Abi]
$api = 26
$llvm = "$ndk\toolchains\llvm\prebuilt\windows-x86_64"
$clang = "$llvm\bin\$triple$api-clang.cmd"
$key = $triple.Replace('-', '_')
Set-Item "env:CC_$key" $clang
Set-Item "env:CXX_$key" "$llvm\bin\$triple$api-clang++.cmd"
Set-Item "env:AR_$key" "$llvm\bin\llvm-ar.exe"
Set-Item "env:CARGO_TARGET_$($key.ToUpper())_LINKER" $clang
$env:ANDROID_NDK_ROOT = $ndk
$env:ANDROID_NDK = $ndk
# skia-bindings не может скачать свои бинарники, если в пути есть кириллица (curl 23),
# поэтому сборка идёт в папку с латинским путём.
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = 'E:\Android\cargo-target\yamusic-rs' }
$target = $env:CARGO_TARGET_DIR
$env:ANDROID_NDK_HOME = $ndk

$here = $PSScriptRoot
$buildProfile = if ($DebugBuild) { 'debug' } elseif ($Fast) { 'fast' } else { 'release' }
$cargoArgs = @('build', '--target', $triple, '--manifest-path', "$here\Cargo.toml")
if (-not $DebugBuild) { $cargoArgs += @('--profile', $buildProfile) }
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { throw 'cargo build упал' }

$out = "$target\apk"
$stage = "$out\stage"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$stage\lib\$Abi" | Out-Null
Copy-Item "$target\$triple\$buildProfile\libyamusic.so" "$stage\lib\$Abi\"
# oboe собран с c++_shared: библиотеку надо положить рядом.
Copy-Item "$llvm\sysroot\usr\lib\$triple\libc++_shared.so" "$stage\lib\$Abi\"

$unsigned = "$out\unsigned.apk"
$gen = "$out\gen"
$classes = "$out\classes"
Remove-Item $gen, $classes, "$out\res", "$out\java" -Recurse -Force -ErrorAction SilentlyContinue
# aapt2 и javac не открывают файлы с кириллицей в пути.
Copy-Item "$here\res" "$out\res" -Recurse
Copy-Item "$here\java" "$out\java" -Recurse
& "$bt\aapt2.exe" compile --dir "$out\res" -o "$out\res.zip"
if ($LASTEXITCODE -ne 0) { throw 'aapt2 compile упал' }
& "$bt\aapt2.exe" link -o $unsigned --debug-mode --manifest "$here\AndroidManifest.xml" -I "$platform\android.jar" --java $gen "$out\res.zip"
if ($LASTEXITCODE -ne 0) { throw 'aapt2 link упал' }

# Java нужен только сервису шторки: у NativeActivity своего кода нет.
$sources = Get-ChildItem "$out\java", $gen -Recurse -Filter *.java | ForEach-Object FullName
& "$env:JAVA_HOME\bin\javac.exe" --release 11 -nowarn -encoding UTF-8 -classpath "$platform\android.jar" -d $classes @sources
if ($LASTEXITCODE -ne 0) { throw 'javac упал' }
$classFiles = Get-ChildItem $classes -Recurse -Filter *.class | ForEach-Object FullName
& "$bt\d8.bat" --release --min-api 26 --lib "$platform\android.jar" --output $stage @classFiles
if ($LASTEXITCODE -ne 0) { throw 'd8 упал' }

Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::Open($unsigned, 'Update')
try {
    Get-ChildItem $stage -Recurse -File | ForEach-Object {
        $name = $_.FullName.Substring($stage.Length + 1).Replace('\', '/')
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $_.FullName, $name) | Out-Null
    }
} finally { $zip.Dispose() }

$aligned = "$out\aligned.apk"
& "$bt\zipalign.exe" -p -f 4 $unsigned $aligned
if ($LASTEXITCODE -ne 0) { throw 'zipalign упал' }

$userHome = @($env:ANDROID_USER_HOME, [Environment]::GetEnvironmentVariable('ANDROID_USER_HOME', 'User'), "$HOME\.android") | Where-Object { $_ } | Select-Object -First 1
$keystore = Join-Path $userHome 'debug.keystore'
if (-not (Test-Path $keystore)) {
    & "$env:JAVA_HOME\bin\keytool.exe" -genkeypair -keystore $keystore -storepass android -alias androiddebugkey -keypass android -dname 'CN=Android Debug,O=Android,C=US' -keyalg RSA -keysize 2048 -validity 10000 | Out-Null
}
$apk = "$out\yamusic-rs-$Abi.apk"
& "$bt\apksigner.bat" sign --v1-signing-enabled true --ks $keystore --ks-pass pass:android --out $apk $aligned
if ($LASTEXITCODE -ne 0) { throw 'apksigner упал' }
"APK: $apk ({0:N1} МБ)" -f ((Get-Item $apk).Length / 1MB)

if ($Install) {
    & "$sdk\platform-tools\adb.exe" install -r $apk
}
