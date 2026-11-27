//! Настройки (config.json), папка данных приложения и лог.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

/// Длина кроссфейда, когда его включают в настройках.
pub const CROSSFADE_SECS: u32 = 6;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    /// Токен OAuth в памяти. На диске лежит только зашифрованным (`token_sealed`).
    #[serde(skip)]
    pub token: Option<String>,
    #[serde(skip)]
    pub refresh_token: Option<String>,
    /// Токены под DPAPI или в связке ключей Linux (src/secret.rs).
    #[serde(skip_serializing_if = "Option::is_none")]
    token_sealed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_sealed: Option<String>,
    /// Токен открытым текстом: старый формат или Linux без связки ключей. Как только спрятать есть
    /// куда, при запуске переносится туда.
    #[serde(rename = "token", skip_serializing_if = "Option::is_none")]
    token_plain: Option<String>,
    /// Unix-время, когда токен истечёт.
    pub token_expires: Option<u64>,
    pub volume: f32,
    pub discord_enabled: bool,
    pub discord_app_id: String,
    /// Версия RS, в которой спрятали карточку доната: в следующей она снова видна.
    pub donate_hidden: String,
    /// system / light / dark
    pub theme: String,
    /// 0 превосходное, 1 оптимальное, 2 экономичное
    pub quality: u8,
    pub eq_on: bool,
    pub eq_preset: usize,
    pub eq_preamp: f32,
    pub eq_gains: [f32; 10],
    /// Язык интерфейса Яндекса: ru, en, uz, kk — уходит в Accept-Language.
    pub lang: String,
    pub navbar_collapsed: bool,
    /// smooth — все анимации, lite — без анимаций и с урезанными кэшами.
    pub mode: String,
    /// Куда скачивать треки. Пусто — Музыка\Яндекс Музыка.
    pub download_dir: String,
    /// Скачивать FLAC, если он есть у трека.
    pub download_flac: bool,
    /// Громкость в децибелах, а не квадратом положения ползунка.
    pub log_volume: bool,
    /// Блоки главной и «Книг и подкастов», которые скрыл пользователь.
    pub hidden_blocks: Vec<HiddenBlock>,
    /// Раз в полчаса спрашивать GitHub о новой версии.
    pub check_updates: bool,
    /// «Треки 18+» в меню профиля.
    pub allow_explicit: bool,
    /// Добавлять треки в конец плейлиста, а не в начало.
    pub add_to_end: bool,
    /// Пятно «Моей волны» движется. Выключено: анимация всё время тратит процессор.
    pub wave_motion: bool,
    /// Сколько секунд треки перетекают друг в друга; ноль — без кроссфейда.
    pub crossfade_secs: u32,
    /// Обычное окно при выходе, в пикселях экрана. На Wayland места окна не узнать.
    pub window_pos: Option<[i32; 2]>,
    pub window_size: Option<[u32; 2]>,
    pub window_maximized: bool,
    /// Место мини-плеера при выходе.
    pub mini_pos: Option<[i32; 2]>,
    /// Горячие клавиши, которые поменял пользователь: действие → клавиши по ячейкам, "" — пусто.
    pub hotkeys: std::collections::BTreeMap<String, Vec<String>>,
}

/// Блок запоминается по id из скелета: он постоянный («recommended_playlists»), а заголовок
/// меняется («Летняя» → «Осенняя»). Заголовок — только чтобы показать список в настройках.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct HiddenBlock {
    pub id: String,
    pub title: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            token: None,
            refresh_token: None,
            token_sealed: None,
            refresh_sealed: None,
            token_plain: None,
            token_expires: None,
            volume: 0.6,
            discord_enabled: true,
            discord_app_id: String::new(),
            donate_hidden: String::new(),
            theme: "system".into(),
            quality: 0,
            eq_on: false,
            eq_preset: 1,
            eq_preamp: 1.0,
            eq_gains: [0.0; 10],
            lang: "ru".into(),
            navbar_collapsed: false,
            mode: "smooth".into(),
            download_dir: String::new(),
            download_flac: false,
            log_volume: false,
            hidden_blocks: Vec::new(),
            check_updates: true,
            allow_explicit: true,
            add_to_end: false,
            wave_motion: false,
            crossfade_secs: 0,
            window_pos: None,
            window_size: None,
            window_maximized: false,
            mini_pos: None,
            hotkeys: Default::default(),
        }
    }
}

/// %APPDATA%\yamusic-rs или ~/.config/yamusic-rs. Папка прежнего названия (yamusic-lite, ещё раньше yawave) переносится
/// молча, вместе с токеном и настройками. Если её держит запущенная старая версия, этот запуск
/// работает со старой папкой, а перенос повторится в следующий раз.
pub fn dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let base = crate::sys::config_base();
        let dir = base.join("yamusic-rs");
        if !dir.exists() {
            if let Some(old) = ["yamusic-lite", "yawave"].iter().map(|n| base.join(n)).find(|p| p.exists()) {
                if std::fs::rename(&old, &dir).is_err() {
                    return old;
                }
            }
        }
        dir
    })
    .clone()
}

/// Скачанные шрифты и значки. На Windows лежат рядом с настройками, на Linux — в ~/.cache.
pub fn cache_dir() -> PathBuf {
    #[cfg(windows)]
    return dir();
    #[cfg(not(windows))]
    return crate::sys::cache_base().join("yamusic-rs");
}

impl Config {
    /// Длина кроссфейда.
    pub fn crossfade(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.crossfade_secs.into())
    }

    pub fn load() -> Self {
        let path = dir().join("config.json");
        let mut c = match std::fs::read_to_string(&path) {
            Ok(s) => Self::parse(&s).unwrap_or_else(|| {
                // Испорченный файл не затираем молча: копия остаётся рядом.
                let _ = std::fs::copy(&path, dir().join("config.json.bad"));
                let (c, bad) = Self::parse_by_field(&s);
                log(format!("config.json не читается целиком, сброшены: {}; копия в config.json.bad", bad.join(", ")));
                c
            }),
            Err(_) => Config::default(),
        };
        c.token = c.token_sealed.as_deref().and_then(crate::secret::open);
        c.refresh_token = c.refresh_sealed.as_deref().and_then(crate::secret::open);
        if c.token.is_none() && c.token_sealed.is_some() {
            log("сохранённый токен не прочитался (другая учётная запись, связка ключей заперта или в ней нет записи?) — нужно войти заново");
        }
        if let Some(plain) = c.token_plain.clone().filter(|t| !t.trim().is_empty()) {
            if c.token.is_none() {
                c.token = Some(plain.trim().to_string());
            }
            // Открытый токен с диска убираем сразу.
            c.save();
        }
        c
    }

    fn parse(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }

    /// Настройки по одной: одно нечитаемое значение (например, `null` вместо числа после NaN)
    /// сбрасывает только себя, а не весь файл вместе со входом. Второе — имена сброшенных.
    fn parse_by_field(s: &str) -> (Self, Vec<String>) {
        let Ok(serde_json::Value::Object(file)) = serde_json::from_str(s) else { return (Config::default(), vec!["всё".into()]) };
        let Ok(mut merged) = serde_json::to_value(Config::default()) else { return (Config::default(), vec!["всё".into()]) };
        let mut c = Config::default();
        let mut bad = Vec::new();
        for (key, value) in file {
            let mut probe = merged.clone();
            probe[key.as_str()] = value;
            if let Ok(parsed) = serde_json::from_value::<Config>(probe.clone()) {
                merged = probe;
                c = parsed;
            } else {
                bad.push(key);
            }
        }
        (c, bad)
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(dir());
        let mut disk = self.clone();
        // Токен не прочитался (связка ключей заперта) — запись о нём не трогаем, иначе после
        // разблокировки входить придётся заново. Стирают её только forget_token и forget_refresh.
        if let Some(t) = &self.token {
            disk.token_sealed = crate::secret::seal("token", t);
        }
        if let Some(t) = &self.refresh_token {
            disk.refresh_sealed = crate::secret::seal("refresh", t);
        }
        // Спрятать некуда (Linux без связки ключей) — открытым текстом, в файле только для владельца.
        disk.token_plain = if disk.token_sealed.is_none() { self.token.clone() } else { None };
        if let Ok(s) = serde_json::to_string_pretty(&disk) {
            let path = dir().join("config.json");
            let tmp = dir().join("config.json.tmp");
            if write_private(&tmp, s.as_bytes()).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    /// Выход или токен отозван: убрать и из памяти, и с диска.
    pub fn forget_token(&mut self) {
        self.token = None;
        self.token_sealed = None;
        crate::secret::forget("token");
    }

    pub fn forget_refresh(&mut self) {
        self.refresh_token = None;
        self.refresh_sealed = None;
        crate::secret::forget("refresh");
    }
}

/// На Linux без связки ключей токен лежит открытым текстом, поэтому файл сразу создаётся
/// только для владельца, а не получает права после записи.
fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
        // Права задаются только новому файлу, у оставшегося старого .tmp они бы не поменялись.
        let _ = std::fs::remove_file(path);
    }
    o.open(path)?.write_all(data)
}

/// Ошибка вместе с причинами. reqwest сам пишет только «error sending request», а что
/// случилось на самом деле (DNS, TLS, прокси, таймаут), лежит в source().
pub fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(x) = src {
        s.push_str(": ");
        s.push_str(&x.to_string());
        src = x.source();
    }
    s
}

/// Лог в log.txt папки настроек.
pub fn log(msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    #[cfg(debug_assertions)]
    eprintln!("{msg}");
    // Тесты нарочно вызывают ошибки («Лайк не сохранился: offline»): настоящему журналу они не нужны.
    if cfg!(test) {
        return;
    }
    let _ = std::fs::create_dir_all(dir());
    let path = dir().join("log.txt");
    if std::fs::metadata(&path).map(|m| m.len() > 512 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{t}] {msg}");
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}


#[cfg(test)]
#[test]
fn bad_value_resets_only_itself() {
    let s = r#"{"volume": null, "theme": "dark", "crossfade_secs": 6, "eq_gains": [1.0]}"#;
    assert!(Config::parse(s).is_none());
    let (c, mut bad) = Config::parse_by_field(s);
    bad.sort();
    assert_eq!(bad, ["eq_gains", "volume"]);
    assert_eq!(c.theme, "dark");
    assert_eq!(c.crossfade_secs, 6);
    assert_eq!(c.volume, Config::default().volume);
    assert_eq!(c.eq_gains, Config::default().eq_gains);
}
