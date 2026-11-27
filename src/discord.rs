//! Discord Rich Presence через локальный IPC Discord (named pipe на Windows, unix-сокет на Linux) — без сторонних библиотек.
//! Показывает «Слушает …»: трек, исполнитель, обложка, полоска прогресса, кнопка на трек.

use crate::api::Track;
use crate::config::log;
use crate::player::{Player, Status};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Приложение Discord, под которым все пользователи показывают статус «Слушает».
/// Его имя видно в профиле («Слушает …»), картинка `logo` — значок в углу обложки.
pub const APP_ID: &str = "1570815647133474856";
/// Ссылка второй кнопки статуса.
const PROJECT_URL: &str = env!("CARGO_PKG_REPOSITORY");
/// Discord принимает не больше 5 обновлений статуса за 20 секунд.
const RATE_MAX: usize = 5;
const RATE_WINDOW: Duration = Duration::from_secs(20);
/// Discord ограничивает тексты 128 символами, а подпись кнопки — 32, но сервер
/// считает именно байты UTF-8. Если лимит превышен, он молча выбрасывает всю
/// активность, хотя клиент отвечает «принято»: статус виден только себе.
const TEXT_MAX: usize = 128;
const LABEL_MAX: usize = 32;
/// «Слушать в Яндекс Музыке» — 43 байта, и статус перестаёт уходить другим.
const LISTEN_LABEL: &str = "Слушать в Музыке"; // 30 байт

#[derive(Clone, Default)]
pub struct DiscordCfg {
    pub enabled: bool,
    /// Своё приложение вместо общего (для сборок-форков); пусто — `APP_ID`.
    pub app_id: String,
}

pub struct Discord {
    pub cfg: Mutex<DiscordCfg>,
    pub status: Mutex<String>,
}

#[cfg(windows)]
type Pipe = std::fs::File;
#[cfg(unix)]
type Pipe = std::os::unix::net::UnixStream;

struct Conn {
    pipe: Pipe,
    nonce: u64,
}

#[cfg(windows)]
fn open_pipe(i: u32) -> Option<Pipe> {
    std::fs::OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}")).ok()
}

/// Сокет лежит в XDG_RUNTIME_DIR, у Flatpak и Snap — в своих подпапках.
#[cfg(unix)]
fn open_pipe(i: u32) -> Option<Pipe> {
    let dirs = ["XDG_RUNTIME_DIR", "TMPDIR"].into_iter().filter_map(std::env::var_os).map(std::path::PathBuf::from);
    dirs.chain([std::path::PathBuf::from("/tmp")])
        .flat_map(|d| ["", "app/com.discordapp.Discord", "snap.discord"].map(|sub| d.join(sub).join(format!("discord-ipc-{i}"))))
        .find_map(|p| Pipe::connect(p).ok())
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Отмечает отправку в лимите Discord: 5 обновлений за 20 секунд. false — лимит исчерпан,
/// отправку лучше повторить на следующем круге, а не терять.
fn take_slot(sends: &mut VecDeque<Instant>) -> bool {
    let now = Instant::now();
    while sends.front().is_some_and(|t| now.duration_since(*t) >= RATE_WINDOW) {
        sends.pop_front();
    }
    if sends.len() >= RATE_MAX {
        return false;
    }
    sends.push_back(now);
    true
}

impl Conn {
    fn connect(app_id: &str) -> Result<Conn, String> {
        let mut last = "Discord не запущен".to_string();
        for i in 0..10 {
            let Some(pipe) = open_pipe(i) else {
                continue;
            };
            let mut c = Conn { pipe, nonce: 0 };
            c.send(0, &json!({ "v": 1, "client_id": app_id }))?;
            let (op, v) = c.recv()?;
            if op == 1 && v.get("evt").and_then(|e| e.as_str()) == Some("READY") {
                return Ok(c);
            }
            let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
            let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
            if code == 4000 || msg.to_lowercase().contains("client") {
                return Err("Discord не принял Application ID".into());
            }
            last = format!("Discord ответил: {msg}");
        }
        Err(last)
    }

    fn send(&mut self, op: u32, v: &Value) -> Result<(), String> {
        let body = serde_json::to_vec(v).map_err(|e| e.to_string())?;
        let mut buf = Vec::with_capacity(8 + body.len());
        buf.extend_from_slice(&op.to_le_bytes());
        buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
        buf.extend_from_slice(&body);
        self.pipe.write_all(&buf).map_err(|e| format!("запись в Discord: {e}"))
    }

    fn recv(&mut self) -> Result<(u32, Value), String> {
        let mut h = [0u8; 8];
        self.pipe.read_exact(&mut h).map_err(|e| format!("чтение из Discord: {e}"))?;
        let op = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
        let len = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
        if len > 1 << 20 {
            return Err("слишком большой ответ Discord".into());
        }
        let mut b = vec![0u8; len];
        self.pipe.read_exact(&mut b).map_err(|e| format!("чтение из Discord: {e}"))?;
        Ok((op, serde_json::from_slice(&b).unwrap_or(Value::Null)))
    }

    /// Ok(None) — принято, Ok(Some(msg)) — Discord вернул ошибку на эту активность.
    fn set_activity(&mut self, activity: Value) -> Result<Option<String>, String> {
        // YAMUSIC_DISCORD_DEBUG=1 — в журнал попадают отправленный JSON и ответ Discord.
        let debug = std::env::var_os("YAMUSIC_DISCORD_DEBUG").is_some();
        if debug {
            log(format!("discord TX {}", serde_json::to_string(&activity).unwrap_or_default()));
        }
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        self.send(
            1,
            &json!({
                "cmd": "SET_ACTIVITY",
                "args": { "pid": std::process::id(), "activity": activity },
                "nonce": nonce,
            }),
        )?;
        loop {
            let (op, v) = self.recv()?;
            match op {
                2 => return Err("Discord закрыл соединение".into()),
                3 => self.send(4, &v)?, // ping → pong
                _ => {
                    if v.get("nonce").and_then(|n| n.as_str()) != Some(nonce.as_str()) {
                        continue;
                    }
                    if v.get("evt").and_then(|e| e.as_str()) == Some("ERROR") {
                        let msg = v.pointer("/data/message").and_then(|m| m.as_str()).unwrap_or("?");
                        if debug {
                            log(format!("discord RX {}", serde_json::to_string(&v).unwrap_or_default()));
                        }
                        return Ok(Some(msg.to_string()));
                    }
                    if debug {
                        log(format!("discord OK {}", serde_json::to_string(&v).unwrap_or_default()));
                    }
                    return Ok(None);
                }
            }
        }
    }
}

fn fit(s: &str, max: usize, fallback: &str) -> String {
    let s = s.trim();
    let s = if s.chars().count() < 2 { fallback } else { s };
    let mut out = String::new();
    for c in s.chars() {
        // Режем по байтам, не разрывая многобайтовый символ.
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out.trim_end().to_string()
}

fn activity(t: &Track, start: i64, end: Option<i64>, playing: bool, listening: bool, with_display_type: bool) -> Value {
    let kind = if listening { 2 } else { 0 };
    let url = match &t.album_id {
        Some(a) => format!("https://music.yandex.ru/album/{a}/track/{}", t.id),
        None => format!("https://music.yandex.ru/track/{}", t.id),
    };
    let mut a = json!({
        // 2 — «Слушает», 0 — «Играет»: так показываем, если клиент не принял «Слушает».
        "type": kind,
        "details": fit(&t.title, TEXT_MAX, "Без названия"),
        "state": fit(&t.artists, TEXT_MAX, "Моя волна"),
        "buttons": [
            { "label": LISTEN_LABEL, "url": url },
            { "label": fit(env!("APP_NAME"), LABEL_MAX, "yamusic-rs"), "url": PROJECT_URL },
        ],
    });
    if playing {
        // На паузе меток нет: иначе прогресс в профиле продолжал бы идти.
        a["timestamps"] = json!({ "start": start });
        if let Some(end) = end {
            a["timestamps"]["end"] = json!(end);
        }
    }
    if with_display_type {
        // В списке участников: «Слушает <название трека>», а не имя приложения.
        a["status_display_type"] = json!(2);
    }
    let album = fit(&t.album_title, TEXT_MAX, env!("APP_NAME"));
    a["assets"] = json!({
        "large_image": t.cover_url(400).unwrap_or_else(|| "logo".into()),
        "large_text": album,
        "small_image": "logo",
        "small_text": fit(env!("APP_NAME"), TEXT_MAX, "yamusic-rs"),
    });
    a
}

impl Discord {
    pub fn spawn(player: Player, cfg: DiscordCfg) -> Arc<Discord> {
        let d = Arc::new(Discord { cfg: Mutex::new(cfg), status: Mutex::new(String::new()) });
        let me = d.clone();
        thread::Builder::new()
            .name("discord".into())
            .spawn(move || me.run(player))
            .expect("discord thread");
        d
    }

    fn set_status(&self, s: impl Into<String>) {
        *self.status.lock().unwrap() = s.into();
    }

    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }

    fn run(&self, player: Player) {
        let mut conn: Option<Conn> = None;
        let mut conn_app = String::new();
        let mut next_try = Instant::now();
        // Что сейчас показано: (id трека, время старта, играет). None — ничего.
        let mut shown: Option<(String, i64, bool)> = None;
        let mut display_type_ok = true;
        // Клиент может не знать «Слушает» (type 2) — тогда показываем статус как «Играет».
        let mut listening_ok = true;
        // Discord отклонил саму активность: не долбим его одной и той же ошибкой каждую секунду.
        let mut retry_at = Instant::now();
        // Отправки за последние 20 секунд: Discord принимает не больше пяти.
        let mut sends: VecDeque<Instant> = VecDeque::new();
        loop {
            thread::sleep(Duration::from_secs(1));
            let cfg = self.cfg.lock().unwrap().clone();
            let app = match cfg.app_id.trim() {
                "" => APP_ID.to_string(),
                own => own.to_string(),
            };
            if !cfg.enabled || app != conn_app {
                conn = None; // закрытие канала само убирает статус в Discord
                shown = None;
                conn_app = app.clone();
                next_try = Instant::now();
                retry_at = Instant::now();
            }
            if !cfg.enabled {
                self.set_status("Выключено");
                continue;
            }
            if app.is_empty() {
                self.set_status("В этой сборке не задан Application ID");
                continue;
            }
            if conn.is_none() {
                if Instant::now() < next_try {
                    continue;
                }
                match Conn::connect(&app) {
                    Ok(c) => {
                        conn = Some(c);
                        shown = None;
                        sends.clear(); // у нового соединения свежий лимит
                        self.set_status("Подключено, статус отправлен");
                    }
                    Err(e) => {
                        self.set_status(e);
                        next_try = Instant::now() + Duration::from_secs(15);
                        continue;
                    }
                }
            }
            let Some(c) = conn.as_mut() else { continue };

            let s = player.state();
            // Загрузка следующего трека и пауза не должны гасить статус: иначе он мигает
            // и пропадает из профиля на каждой смене трека.
            let track = if s.status == Status::Ready { s.track.clone() } else { None };
            let res = match track {
                Some(_) if s.loading => Ok(()), // дождёмся трека, показанное оставляем как есть
                Some(t) => {
                    let playing = s.playing;
                    let start = now_ms() - (s.position * 1000.0) as i64;
                    let end = (s.duration > 1.0).then(|| start + (s.duration * 1000.0) as i64);
                    let changed = match &shown {
                        // Новый трек, пауза/возобновление или перемотка.
                        Some((id, st, was)) => *id != t.id || *was != playing || (playing && (st - start).abs() > 3000),
                        None => true,
                    };
                    // Лимит Discord исчерпан или прошлый отказ: подождём следующий круг,
                    // трек не потеряется.
                    if !changed || Instant::now() < retry_at || !take_slot(&mut sends) {
                        Ok(())
                    } else {
                        let mut r = c.set_activity(activity(&t, start, end, playing, listening_ok, display_type_ok));
                        if matches!(&r, Ok(Some(_))) {
                            // Старый клиент Discord не знает status_display_type — повторим без него.
                            if display_type_ok {
                                display_type_ok = false;
                                r = c.set_activity(activity(&t, start, end, playing, listening_ok, false));
                            }
                            // Совсем старый клиент не знает «Слушает» — покажем как «Играет».
                            if listening_ok && matches!(&r, Ok(Some(_))) {
                                listening_ok = false;
                                r = c.set_activity(activity(&t, start, end, playing, false, false));
                            }
                        }
                        match r {
                            Ok(None) => {
                                shown = Some((t.id.clone(), start, playing));
                                self.set_status("Подключено, статус отправлен");
                                Ok(())
                            }
                            Ok(Some(msg)) => {
                                // Трек не считаем показанным: иначе он больше не переотправится,
                                // а в настройках будет написано, что статус уже стоит.
                                log(format!("discord SET_ACTIVITY: {msg}"));
                                retry_at = Instant::now() + Duration::from_secs(30);
                                self.set_status("Discord отклонил статус, повторю позже");
                                Ok(())
                            }
                            Err(e) => Err(e),
                        }
                    }
                }
                None if shown.is_some() => {
                    if !take_slot(&mut sends) {
                        Ok(())
                    } else {
                        shown = None;
                        c.set_activity(Value::Null).map(|_| ())
                    }
                }
                None => Ok(()),
            };
            if let Err(e) = res {
                log(format!("discord: {e}"));
                conn = None;
                shown = None;
                self.set_status("Связь с Discord потеряна, переподключение…");
                next_try = Instant::now() + Duration::from_secs(5);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Нужен запущенный Discord: с неверным ID он должен вежливо отказать.
    #[test]
    #[ignore]
    fn handshake_rejects_bad_id() {
        let r = Conn::connect("1");
        println!("{:?}", r.as_ref().err());
        assert!(matches!(r, Err(ref e) if e.contains("Application ID")));
    }

    /// Discord считает лимиты в байтах: кириллическая подпись кнопки должна
    /// уложиться в 32 байта, иначе сервер выбрасывает всю активность.
    #[test]
    fn fit_counts_utf8_bytes() {
        let label = fit("Слушать в Яндекс Музыке", LABEL_MAX, "Слушать");
        assert!(label.len() <= LABEL_MAX, "{label:?} — {} байт", label.len());
        assert!(label.starts_with("Слушать"));
        assert_eq!(fit("  title  ", TEXT_MAX, ""), "title");
        assert_eq!(fit("", TEXT_MAX, "fallback"), "fallback");
        let long = "я".repeat(200);
        assert!(fit(&long, TEXT_MAX, "").len() <= TEXT_MAX);
        assert_eq!(fit(&long, TEXT_MAX, "").chars().count(), TEXT_MAX / 2);
    }
}
