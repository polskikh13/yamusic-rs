<div align="center">

<img src="docs/logo.png" width="96" height="96" alt="">

# Яндекс Музыка RS

**Интерфейс официального приложения Яндекс Музыки на Rust: один файл и в 13 раз меньше памяти.**

[![Релиз](https://img.shields.io/github/v/release/SpolzerMod/yamusic-rs?label=%D1%80%D0%B5%D0%BB%D0%B8%D0%B7&color=ffd60a&labelColor=1c1c1f)](https://github.com/SpolzerMod/yamusic-rs/releases/latest)
![Windows 10 | 11](https://img.shields.io/badge/Windows-10%20%7C%2011-2b2b30?labelColor=1c1c1f)
![Linux бета](https://img.shields.io/badge/Linux-%D0%B1%D0%B5%D1%82%D0%B0-2b2b30?labelColor=1c1c1f)
![Android бета](https://img.shields.io/badge/Android-%D0%B1%D0%B5%D1%82%D0%B0-2b2b30?labelColor=1c1c1f)
[![Поддержать](https://img.shields.io/badge/%D0%BF%D0%BE%D0%B4%D0%B4%D0%B5%D1%80%D0%B6%D0%B0%D1%82%D1%8C-CloudTips-ffd60a?labelColor=1c1c1f)](https://pay.cloudtips.ru/p/befb620d)
[![GPL-3.0](https://img.shields.io/badge/%D0%BB%D0%B8%D1%86%D0%B5%D0%BD%D0%B7%D0%B8%D1%8F-GPL--3.0-2b2b30?labelColor=1c1c1f)](LICENSE)

[Скачать](https://github.com/SpolzerMod/yamusic-rs/releases/latest) ·
[Возможности](#возможности) ·
[Замеры](#замеры) ·
[Какие системы](#какие-системы-поддерживаются) ·
[Вопросы](#частые-вопросы) ·
[Как помочь](#как-помочь) ·
[Для разработчиков](#для-разработчиков) ·
[English](#english)

<br>

<img src="docs/compare.gif" width="100%" alt="Шторка: слева официальное приложение, справа Яндекс Музыка RS. Страницы альбома и исполнителя в светлой и тёмной теме">

<sub>Слева официальное приложение, справа RS. Аккаунт, страница и момент одни и те же.</sub>

</div>

<br>

Официальное приложение Яндекс Музыки для Windows сделано на Electron: внутри браузер Chromium,
семь процессов и около 850 МБ памяти. **Яндекс Музыка RS** повторяет его интерфейс (те же
шрифты, цвета, отступы и страницы), но написана на Rust и рисует окно сама, на процессоре.
В итоге один файл на 26 МБ, один процесс и около 65 МБ памяти. Работает на Windows, а в бете
на Linux и Android.

<div align="center">
<img src="docs/stats.gif" width="100%" alt="Оперативная память: 852 МБ против 64 МБ. Процессы: 7 против 1. Размер на диске: 356 МБ против 26 МБ">
</div>

<sub>Память — в конце одинакового сценария: играет трек, открыт текст песни. Как мерили, показано
[ниже](#замеры).</sub>

> [!IMPORTANT]
> Это неофициальный проект. Он не связан с Яндексом, и Яндекс его не поддерживает. Названия
> и товарные знаки принадлежат их владельцам. Нужен ваш аккаунт Яндекса, а для полных треков —
> подписка Плюс, как и в официальном приложении.

## Возможности

<img src="docs/lyrics.gif" width="100%" alt="Полноэкранный плеер RS: обложка слева, справа текст песни прокручивается вместе с треком">

- «Моя волна» с колесом вайбов и настройками «по характеру», «под настроение» и «по языку»,
  «Для вас и Тренды», «Концерты», «Книги и подкасты», чарт, поиск и коллекция.
- Страницы исполнителей, альбомов и плейлистов. Свои плейлисты можно переименовывать, менять им
  обложку и описание, удалять из них треки.
- Полноэкранный плеер с текстом песни, который идёт вместе с треком.
- Скачивание треков с обложкой и тегами ID3. По умолчанию в `Музыка\Яндекс Музыка`
  (на Linux в `~/Музыка/Яндекс Музыка`), папку можно сменить в настройках.
- Эквалайзер на 10 полос, светлая и тёмная тема, русский, английский, узбекский и казахский языки.
- Автозапуск вместе с системой. На Windows есть трей: если закрыть окно во время игры, музыка
  не остановится.
- Мини-плеер поверх всех окон: обложка, название и три кнопки. Включается кнопкой рядом
  с «Свернуть».
- Медиаклавиши клавиатуры работают, даже когда окно свёрнуто.
- Статус «Слушает …» в Discord и переключатель «Меньше ИИ» для рекомендаций.
- Обновление из приложения: когда выходит новая версия, сверху появляется кнопка «Обновить».
- Режим «Лёгкий» без анимаций для слабых компьютеров.

## Замеры

<img src="docs/live.gif" width="100%" alt="Графики памяти и процессора за 50 секунд. Официальное приложение держится около 800–850 МБ, RS — около 60 МБ. Процессор в среднем 2,9% против 0,4%">

Оба приложения по очереди проходят один и тот же сценарий, клики идут по таймеру:

1. запуск;
2. на 10-й секунде — «Для вас и Тренды» и прокрутка вниз;
3. на 24-й — включается трек;
4. на 32-й — полноэкранный плеер с текстом песни.

Каждые полсекунды снимаются Private Bytes и процессорное время всех процессов приложения. Окна видны
на экране, звук выключен.

<sub>Windows 11, Intel Core i3-13100, монитор 2560×1440 с масштабом 125%, окно 1280×800. Процессор
считается в процентах от всего процессора, как в диспетчере задач. Размер на диске — папка установленного
официального приложения против одного `yamusic-rs.exe`. На другом компьютере и с другими страницами
цифры будут другими, но разница того же порядка.</sub>

## Установка

### Windows

1. Скачайте `yamusic-rs.exe` со страницы [последнего релиза](https://github.com/SpolzerMod/yamusic-rs/releases/latest).
2. Положите файл в любую папку, например в `Документы` или `C:\Programs`, и запустите.
   Установщика нет, приложение целиком помещается в один exe.
3. Приложение покажет код. Откройте [ya.ru/device](https://ya.ru/device) на компьютере или телефоне
   и введите его. Пароль в приложение вводить не нужно.

> [!NOTE]
> У exe нет цифровой подписи, поэтому при первом запуске Windows SmartScreen может показать «Windows
> защитила ваш компьютер». Нажмите «Подробнее», затем «Выполнить в любом случае». Контрольная сумма
> SHA-256 файла есть на странице релиза. Exe собирается на GitHub Actions прямо из исходников,
> лог сборки открыт. Можно собрать и самим ([как это сделать](#сборка)).

**Обновление.** Приложение раз в полчаса проверяет релизы на GitHub. Кнопка «Обновить» скачивает
новый `yamusic-rs.exe`, сверяет его SHA-256 с релизом, заменяет старый файл и перезапускается.
Если папка закрыта для записи (например, `Program Files`), кнопка откроет страницу релиза,
и файл можно заменить вручную.

**Удаление.**
1. Выключите в настройках «Запускать вместе с Windows», если включали.
2. Удалите `yamusic-rs.exe`.
3. Удалите папку `%APPDATA%\yamusic-rs` с настройками, входом и кэшем.

Скачанные треки лежат в `Музыка\Яндекс Музыка`, их удаляйте по желанию.

### Linux (бета)

В релизе три файла, внутри одна и та же программа. Выберите тот, что подходит вашей системе:

| Система | Файл | Установка |
| --- | --- | --- |
| Debian, Ubuntu, Mint и производные | `yamusic-rs_<версия>-1_amd64.deb` | `sudo apt install ./yamusic-rs_*.deb` |
| Fedora, openSUSE | `yamusic-rs-<версия>-1.x86_64.rpm` | `sudo dnf install ./yamusic-rs-*.rpm` или `sudo zypper install ./yamusic-rs-*.rpm` |
| Любая другая, в том числе Arch | `yamusic-rs-x86_64.AppImage` | `chmod +x yamusic-rs-x86_64.AppImage` и запустить |

Для ARM (Raspberry Pi 4/5, Asahi Linux на Mac с M1/M2 и т. п.) те же файлы с `arm64`/`aarch64` в имени.

AppImage монтирует себя через FUSE. Если пишет `Cannot mount AppImage`, поставьте пакет `fuse3`
или запускайте с распаковкой: `APPIMAGE_EXTRACT_AND_RUN=1 ./yamusic-rs-x86_64.AppImage`.

Для Arch есть и [`PKGBUILD`](packaging/aur/PKGBUILD): он собирает пакет из `.deb` релиза (`makepkg -si`).

Приложение появится в меню как «Яндекс Музыка RS». Вход такой же, по коду на ya.ru/device.

**Обновление.** Приложение сообщает о новой версии, кнопка «Обновить» открывает страницу релиза.
Пакет ставится поверх старого той же командой, AppImage заменяется новым файлом.

**Удаление.** `sudo apt remove yamusic-rs`, `sudo dnf remove yamusic-rs` или удалить AppImage. Настройки
и вход лежат в `~/.config/yamusic-rs`, кэш в `~/.cache/yamusic-rs`, токен в связке ключей под
именем «Яндекс Музыка RS» (его можно удалить в «Паролях и ключах» или в KWallet).

### Android (бета)

1. Скачайте `yamusic-rs-arm64-v8a.apk` со страницы [последнего релиза](https://github.com/SpolzerMod/yamusic-rs/releases/latest)
   прямо на телефон.
2. Откройте файл. Android попросит разрешить установку из этого источника (браузера или
   файлового менеджера): разрешите и установите.
3. Вход такой же, по коду: откройте [ya.ru/device](https://ya.ru/device) на любом устройстве и введите
   код с экрана.

Нужен Android 8 или новее на 64-битном процессоре, то есть почти любой телефон последних лет.
APK весит около 11 МБ. Играет в фоне, управляется из шторки и с экрана блокировки, скачанные
треки кладёт в `Музыка/Яндекс Музыка`.

**Обновление.** В профиле появится «Доступна версия …». Кнопка скачает новый APK, его нужно
открыть и установить поверх, вход и настройки сохранятся.

### iOS (альфа)

Готового файла в релизах пока нет: IPA без подписи собирает GitHub Actions (задание `ios`,
артефакт `ipa`) или `ios/build.sh` на Mac. Без подписи iPhone его не поставит, поэтому IPA
ставится через [Sideloadly](https://sideloadly.io) или [AltStore](https://altstore.io) с Windows
или Mac: они подписывают файл вашим Apple ID. С бесплатным Apple ID подпись живёт 7 дней, потом
приложение нужно переподписать тем же способом.

Нужен iOS 15 или новее. Интерфейс тот же, что у Android-версии. Играет в фоне, управляется
с экрана блокировки, из Пункта управления и кнопками наушников. Скачанные треки лежат в «Файлах»,
в папке приложения. Шрифты и иконки скачиваются с music.yandex.ru: читать файлы другого приложения
iOS не даёт. Фон «Моей волны» считается на процессоре: OpenGL у Slint на iOS нет, только Metal.

## Какие системы поддерживаются

| Система | Состояние |
| --- | --- |
| **Windows 11** x64 | Основная платформа, проверяется каждая версия. |
| **Windows 10** x64 | Должна работать: приложение использует те же API. Углы окна будут прямыми, потому что скругление есть только в Windows 11. Регулярно не проверяется, о проблемах пишите в [issues](https://github.com/SpolzerMod/yamusic-rs/issues). |
| Windows на ARM | Должна запускаться через эмуляцию x64. Не проверялась. |
| Windows 7 и 8.1 | Не поддерживаются: текущий Rust под них не собирает. |
| **Linux** x86_64 | Бета. Нужна glibc 2.35 или новее: Ubuntu 22.04+, Debian 12+, Mint 21+, Fedora, openSUSE, Arch и другие. Проверено на Ubuntu 22.04, Debian 13, Fedora 45 и Arch, в Wayland и X11. Звук через PipeWire, PulseAudio или ALSA. Медиаклавиши и плеер в панели GNOME и KDE работают через MPRIS. Трея пока нет, поэтому закрытие окна закрывает приложение. О проблемах пишите в [issues](https://github.com/SpolzerMod/yamusic-rs/issues). |
| **Linux** ARM64 | Бета, те же условия, что для x86_64. Проверено на ALT Linux (Asahi) на MacBook Pro M1. |
| **Android** 8+ arm64 | Бета. Проверено на эмуляторе с Android 15. Шрифты и иконки берутся из установленной Яндекс Музыки, без неё скачиваются с music.yandex.ru. |
| **iOS** 15+ | Альфа: ещё не проверялась на устройстве. Ставится через Sideloadly или AltStore, см. [выше](#ios-альфа). |
| macOS | Не поддерживается. Есть сторонний порт: [rnikitin/yamusic-rs](https://github.com/rnikitin/yamusic-rs), сборка для Apple Silicon и Intel лежит в его [релизах](https://github.com/rnikitin/yamusic-rs/releases). |

> Порт под macOS — отдельный проект. Он основан на коде этого репозитория, но ведётся независимо:
> этот проект в его разработке не участвует и его сборки не проверяет. За безопасность и работу
> порта отвечает его автор, вопросы и ошибки по macOS направляйте ему.

**Нужно ещё:**
- аккаунт Яндекса. Без Плюса сервер отдаёт только 30-секундные отрывки;
- интернет. Офлайн-режима, как в мобильном приложении, нет;
- шрифты YS Text и иконки. Если официальное приложение установлено, RS берёт их из него.
  Если нет, один раз скачивает с music.yandex.ru в `%APPDATA%\yamusic-rs\assets`
  (на Linux в `~/.cache/yamusic-rs/assets`). Официального приложения под Linux нет, поэтому
  там интерфейс пока только на русском: словари других языков есть только в нём.

## Частые вопросы

<details>
<summary><b>Это безопасно? Куда уходит мой токен?</b></summary>

<br>

Вход идёт по коду устройства через ya.ru/device, пароль приложение не видит. Полученный токен
шифруется средствами Windows (DPAPI) и хранится в `%APPDATA%\yamusic-rs\config.json`. Расшифровать
его может только ваша учётная запись Windows на этом компьютере.

На Linux токен лежит в связке ключей (GNOME Keyring, KWallet или другая служба Secret Service),
а в `~/.config/yamusic-rs/config.json` остаётся только имя записи. Если связки ключей нет, например
в голом оконном менеджере, токен хранится в этом файле, который может читать только ваш пользователь.

Токен отправляется только по https и только на домены `yandex.ru` и `yandex.net`. Проверка
встроена в код (`src/client.rs`). Кроме Яндекса приложение ходит только к `api.github.com`,
чтобы проверить обновления, и к Discord на этом же компьютере (локальный канал), если включён
статус. Аналитики и телеметрии нет. Проверку обновлений можно выключить в настройках.

Если вы больше не доверяете приложению или потеряли компьютер, токен можно отозвать: на
[id.yandex.ru](https://id.yandex.ru/security) в разделе «Безопасность» откройте «Входы и устройства»
и выйдите на нужном устройстве или сразу на всех. После этого токен перестанет работать, а RS
попросит войти заново. Выход из аккаунта в самом RS только стирает токен с компьютера.
</details>

<details>
<summary><b>Как проверить, что exe собран из этих исходников?</b></summary>

<br>

Релизы собирает GitHub Actions из кода тега, руками файлы не загружаются. В логе сборки
(вкладка Actions, шаг «SHA-256») напечатан хеш получившегося `yamusic-rs.exe`, `.deb` и
бинарника для Linux. Он совпадает с хешем файла в релизе, а GitHub показывает его рядом с каждым
файлом.

Сборка воспроизводимая: два прогона одного и того же кода дают побайтно одинаковые exe, бинарник
и `.deb` (`.rpm` и AppImage — нет, в них записано время сборки). Самый честный способ проверить:
сделать форк, запустить в нём сборку на том же теге и сравнить хеши. Учтите, что в exe записан
путь к файлу отладки с именем репозитория, поэтому для exe сравнение сработает, только если форк
называется так же — `yamusic-rs`.

Собрать то же самое у себя на компьютере не выйдет: в CI другая версия линкера MSVC, и в exe
попадают пути к папке `.cargo`. Хеш совпадёт только у сборки в GitHub Actions.

Хеш скачанного файла на Windows:

```powershell
Get-FileHash .\yamusic-rs.exe -Algorithm SHA256
```
</details>

<details>
<summary><b>Почему так мало памяти?</b></summary>

<br>

Официальное приложение — это сайт внутри Chromium: отдельные процессы для окна, страницы, GPU,
звука и расширений плюс движок JavaScript. RS — обычная программа на Rust. Интерфейс описан
на [Slint](https://slint.dev) и рисуется библиотекой [Skia](https://skia.org) на процессоре,
без видеокарты. Видеокарта сделала бы прокрутку чуть дешевле, но драйвер сразу забирает
100–150 МБ, поэтому от неё отказались. Обложки декодируются ровно под размер на экране и живут
в кэше с ограничением по объёму.
</details>

<details>
<summary><b>Чем RS отличается от официального приложения?</b></summary>

<br>

Внешне почти ничем, это видно на сравнении выше. Чего пока нет или что работает иначе:

- на «Для вас и Тренды» показываются не все блоки: например, «Подборки музыки» и «Артист рекомендует»
  пока пропускаются;
- нет выбора города для концертов и загрузки своих треков;
- на Windows нет панели управления в системном меню громкости (на Linux она есть через MPRIS);
- собственные настройки RS («Режим работы», «Статус в Discord», автозапуск) переведены только на русский.

Есть и то, чего нет в оригинале: режим «Лёгкий», статус в Discord, автозапуск в трее.
</details>

<details>
<summary><b>Где посмотреть, что пошло не так?</b></summary>

<br>

Ошибки пишутся в `%APPDATA%\yamusic-rs\log.txt`, на Linux в `~/.config/yamusic-rs/log.txt`.
API Яндекса неофициальный и может меняться без предупреждения.
Как сообщить о проблеме, написано [ниже](#как-помочь).
</details>

<details>
<summary><b>Можно запускать сразу свёрнутым? А если запустить второй раз?</b></summary>

<br>

`yamusic-rs.exe --tray` запускает приложение сразу в трее, так же работает автозапуск. Второй
запуск не открывает вторую копию, а показывает уже открытое окно. На Linux трея пока нет, поэтому
автозапуск открывает окно, а `--tray` ничего не делает. Второй запуск так же показывает открытое окно.
</details>

## Как помочь

### Поддержать рублём

RS бесплатный и без рекламы. Если он вам пригодился, можно закинуть на развитие через
[CloudTips](https://pay.cloudtips.ru/p/befb620d), от 49 ₽. Деньги пойдут на RS и следующие открытые проекты.

### Сообщить об ошибке

Откройте [issue](https://github.com/SpolzerMod/yamusic-rs/issues/new/choose) и напишите:

- версию RS из «Настройки → О приложении» и версию системы (на Linux ещё дистрибутив и окружение: GNOME, KDE…);
- что вы делали, что ожидали увидеть и что получилось;
- последние строки `%APPDATA%\yamusic-rs\log.txt` (на Linux `~/.config/yamusic-rs/log.txt`),
  если там есть что-то про эту ошибку.

Токена в логе нет, но в адресах запросов может встретиться числовой id вашего аккаунта. Его можно
заменить на `***`. Со скриншотами то же самое: закрепы и рекомендации показывают ваши вкусы,
их можно закрасить.

Если вы нашли расхождение с официальным приложением, приложите два скриншота при одинаковом
размере окна: оригинал и RS.

### Критические ошибки

Если ошибка касается безопасности, **не открывайте публичный issue**. Это, например:

- токен уходит куда-то кроме `yandex.ru` и `yandex.net` или попадает в лог;
- обновлялка ставит файл, который не совпадает с релизом;
- другая программа или другой пользователь компьютера может прочитать токен.

Напишите через [Security → Report a vulnerability](https://github.com/SpolzerMod/yamusic-rs/security/advisories/new).
Сообщение увидят только владельцы репозитория. Подробнее в [SECURITY.md](SECURITY.md).

Если после обновления приложение перестало запускаться, это тоже срочно, но писать можно
в обычный issue, в заголовке укажите версию. Пока ждёте исправления, предыдущую версию можно
скачать со страницы [релизов](https://github.com/SpolzerMod/yamusic-rs/releases).

### Прислать код

- Одна правка — один pull request. Крупное изменение лучше сначала обсудить в issue.
- Перед отправкой должны проходить `cargo clippy --release --all-targets -- -D warnings`
  и `cargo test --release`. То же самое запускает CI.
- Код и комментарии пишутся на русском, интерфейс обращается к пользователю на «вы».
- `ui/theme.slint` и `src/palette.rs` руками не правятся, они генерируются скриптами из `tools/`.
- Если сломался запрос после обновления Яндекса, обычно достаточно поправить `src/client.rs`
  или разбор ответа в `src/api.rs`.

## Для разработчиков

### Сборка

Нужен Rust 1.92 или новее (`rustup update stable`). Дальше на любой системе одинаково:

```bash
git clone https://github.com/SpolzerMod/yamusic-rs
cd yamusic-rs
cargo build --release
```

Первая сборка занимает 5–10 минут, в основном из-за Skia и LTO. Для отладки подойдёт
`cargo run`: в debug-сборке лог дублируется в консоль.

#### Windows

Нужны Visual Studio Build Tools с компонентом «Разработка классических приложений на C++»
(MSVC и Windows SDK: из SDK берётся `rc.exe` для значка и описания exe).
Готовый файл: `target\release\yamusic-rs.exe`.

#### Linux

Нужны компилятор C и заголовки библиотек.

Debian, Ubuntu, Mint:

```bash
sudo apt install build-essential pkg-config libasound2-dev libfontconfig1-dev libfreetype-dev libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libx11-dev libx11-xcb-dev libxcursor-dev libxi-dev libxrandr-dev
```

Fedora:

```bash
sudo dnf install gcc gcc-c++ make pkgconf-pkg-config alsa-lib-devel fontconfig-devel freetype-devel wayland-devel libxkbcommon-devel libxkbcommon-x11-devel libX11-devel libXcursor-devel libXi-devel libXrandr-devel
```

Arch:

```bash
sudo pacman -S --needed base-devel alsa-lib fontconfig freetype2 wayland libxkbcommon libxkbcommon-x11 libx11 libxcursor libxi libxrandr
```

Готовый файл: `target/release/yamusic-rs`. Пакеты собираются из него: `cargo deb --no-build`
([cargo-deb](https://github.com/kornelski/cargo-deb)), `cargo generate-rpm`
([cargo-generate-rpm](https://github.com/cat-in-136/cargo-generate-rpm)) и `tools/appimage.sh`.
Релизы собираются на Ubuntu 22.04: собранное на более новой системе потребует её версию glibc.

#### iOS

Нужен Mac с Xcode. `ios/build.sh` собирает `ios/target/ios/yamusic-rs.ipa` без подписи,
`ios/build.sh --sim` ставит и запускает приложение в открытом симуляторе (Apple Silicon).
Крейт `ios/` собирает тот же `android/src/lib.rs` и интерфейс `android/ui`: своё у iOS только
`ios/src/sys.rs` (UIKit), `ios/src/media.rs` (экран блокировки, аудиосессия) и `ios/src/wave_cpu.rs`
(фон волны без OpenGL). Код, общий для
телефонов, в `src/` отмечен `cfg(mobile)`.

#### Если Skia не скачивается

`skia-bindings` при сборке скачивает готовые бинарники Skia. Если не получается (корпоративный
прокси, ошибка curl), скачайте архив для своей системы с
[rust-skia/skia-binaries](https://github.com/rust-skia/skia-binaries/releases) и укажите путь
в переменной окружения:

```bash
SKIA_BINARIES_URL=file:///путь/к/skia.tar.gz cargo build --release
```

На Windows путь пишется как `file://C:/путь/к/skia.tar.gz`.

### Как устроено

| Файл | Что внутри |
| --- | --- |
| `src/main.rs` | Точка входа: один экземпляр на пользователя, выбор рендерера Skia без GPU. |
| `src/gui.rs`, `src/gui/landing_ui.rs` | Связь интерфейса с данными: страницы, меню, настройки, обновления. |
| `src/api.rs` | Запросы к API: каталог, лайки, плейлисты, волна, стримы. |
| `src/client.rs` | Всё, что Яндекс может поменять: OAuth-клиент, ключи подписи, заголовки. Ломается — правим здесь. |
| `src/player.rs` | Аудиодвижок в своём потоке: очередь, предзагрузка, сессии волны, фидбек. |
| `src/landing.rs` | Страницы из серверных блоков («Для вас», «Концерты», «Книги и подкасты»). |
| `src/assets.rs`, `src/i18n.rs` | Шрифты, спрайт иконок и словари переводов из `app.asar` оригинала или с music.yandex.ru. |
| `src/images.rs`, `src/net.rs` | Фоновая загрузка обложек и данных страниц, кэши с бюджетом памяти. |
| `src/sys/windows.rs`, `src/tray.rs` | Окна, трей, автозапуск и прочий Win32. |
| `src/sys/linux.rs` | То же для Linux: пути XDG, один экземпляр через сокет, диалоги, автозапуск. |
| `src/mpris.rs` | Медиаклавиши и плеер в панели Linux (MPRIS по D-Bus). |
| `src/secret.rs` | Где хранится токен: DPAPI на Windows, связка ключей на Linux. |
| `src/discord.rs` | Rich Presence через локальный канал Discord, без сторонних библиотек. |
| `src/update.rs` | Проверка релизов на GitHub и замена exe на лету (на Linux — ссылка на релиз). |
| `ui/theme.slint`, `src/palette.rs` | Сгенерированы `tools/gen_theme.py` и `tools/gen_palette.py` из CSS и JS оригинала. Руками не править. |

Приложение не перерисовывали на глаз. Цвета, размеры и отступы
сняты из CSS официального приложения скриптами в `tools/` (их запускают на распакованном
`app.asar`: `npx @electron/asar extract app.asar app`). Новые экраны сверяются с оригиналом:
его запускают с `--remote-debugging-port`, через DevTools Protocol снимают размеры DOM и скриншот
1280×800, а затем сравнивают с окном RS попиксельно.

Страницы собирает сервер, как у оригинала: `GET /landing/skeleton/<id>` возвращает список блоков
с типами и адресами данных, и каждый тип блока — свой компонент в `ui/landing.slint`. Неизвестные
типы пропускаются, поэтому новые блоки Яндекса не ломают страницу.

«Моя волна» работает через сессии ротора (`/rotor/session/new`, `/tracks`, `/feedback`), с настройками
из `/rotor/wave/last`. Пропуски и дослушанные треки уходят вместе со следующим запросом треков,
чтобы сервер подбирал продолжение уже с их учётом.

Шрифты, иконки и переводы не лежат в репозитории. `assets.rs` читает их из `app.asar`
установленного оригинала (WOFF переводится в sfnt на лету), а если оригинала нет, скачивает
с веб-версии. Словари ru/en/uz/kk — это чанки webpack оригинала, ключом служит русская строка.

### Релиз

Файлы для релизов собирает GitHub Actions ([`.github/workflows/build.yml`](.github/workflows/build.yml))
из исходников тега. Лог сборки открыт, поэтому видно, из какого кода получился файл.

```bash
git tag v1.1.0          # версия должна совпадать с Cargo.toml
git push origin v1.1.0
```

Сборка прогоняет clippy и тесты на Windows и на Linux, собирает `yamusic-rs.exe`, `.deb`, `.rpm`
и AppImage, а когда обе сборки готовы, создаёт релиз со всеми файлами. На каждый push и pull
request всё это тоже собирается, файлы можно скачать из артефактов сборки. Встроенная обновлялка
ищет в релизе файл с именем ровно `yamusic-rs.exe` и сверяет его с `digest`, который GitHub
считает при загрузке.

## Лицензия

Copyright © 2026 Spolzer. Код распространяется по лицензии [GNU GPL 3.0](LICENSE): его можно
использовать, менять и распространять, но изменённая версия, которую вы раздаёте, должна выходить
под той же лицензией и с исходным кодом.

Шестерёнка в логотипе (`icons/app.svg` и собранные из него значки) — логотип Rust от Rust Foundation,
он распространяется по лицензии [CC-BY 4.0](https://creativecommons.org/licenses/by/4.0/).
Шрифты, иконки интерфейса и словари официального приложения в репозиторий не входят: RS читает их
с вашего компьютера или с music.yandex.ru.

## English

**Yandex Music RS** is an unofficial Yandex Music client for Windows, Linux and Android written in Rust.
It mirrors the official desktop app closely (same fonts, colours, layout and pages), but instead
of Electron it is a single 26 MB executable that renders its window on the CPU with Slint and Skia.
Running the same scenario, the official app uses about 850 MB of RAM across 7 processes, and RS
uses about 65 MB in one process.

- **Windows:** download `yamusic-rs.exe` from the [latest release](https://github.com/SpolzerMod/yamusic-rs/releases/latest)
  and run it; there is no installer. Sign in with the code shown at [ya.ru/device](https://ya.ru/device).
  The app updates itself from GitHub releases.
- **Linux (beta):** the release has a `.deb` (Debian, Ubuntu, Mint), an `.rpm` (Fedora, openSUSE)
  and an AppImage for everything else; a [`PKGBUILD`](packaging/aur/PKGBUILD) is available for Arch.
  Needs glibc 2.35 or newer. Sound goes through PipeWire, PulseAudio or ALSA, media keys and
  the GNOME/KDE player widget work via MPRIS. There is no tray icon yet.
  ARM64 builds (`aarch64`/`arm64` in the file name) are there too.
- **Android (beta):** install `yamusic-rs-arm64-v8a.apk` from the latest release on Android 8+
  (64-bit). Plays in the background and from the notification shade and lock screen.
- **Requirements:** Windows 11 (Windows 10 should work but is not tested regularly) or Linux, a Yandex
  account, and Yandex Plus for full-length tracks. macOS is not supported.
- **macOS port:** a third-party port is available at [rnikitin/yamusic-rs](https://github.com/rnikitin/yamusic-rs).
  It is a separate project based on this code. This project does not take part in its development
  and does not test its builds; its author is responsible for its security and support.
- **Privacy:** the token is encrypted with Windows DPAPI or kept in the Linux keyring (Secret Service)
  and only sent over HTTPS to `yandex.ru` / `yandex.net`. Besides Yandex, the app only talks to
  `api.github.com` (update checks) and the local Discord client (optional status). There is no telemetry.
- **Build:** Rust 1.92+, then `cargo build --release`. On Windows you need MSVC Build Tools with the
  Windows SDK, on Linux a C toolchain and the `-dev` packages listed in the Russian section above.
- **Bugs:** open an [issue](https://github.com/SpolzerMod/yamusic-rs/issues/new/choose) with the app
  version, your OS version (on Linux also the distro and desktop) and the last lines of
  `%APPDATA%\yamusic-rs\log.txt` (`~/.config/yamusic-rs/log.txt` on Linux). Security
  problems go to [Report a vulnerability](https://github.com/SpolzerMod/yamusic-rs/security/advisories/new),
  not to public issues.
- **Support:** if the app is useful to you, you can [send a tip via CloudTips](https://pay.cloudtips.ru/p/befb620d).

The code is released under the [GNU GPL v3](LICENSE): forks you distribute must stay open source
under the same license. This project is not affiliated with or endorsed by Yandex. All trademarks
belong to their owners.
