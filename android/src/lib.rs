//! Яндекс Музыка RS для Android и iOS. Общий с десктопом код (API, вход, плеер волны) берётся
//! из ../src, здесь только окно, системные вызовы и фон «Моей волны».
//! Крейт ios/ собирает этот же файл: своё у iOS только `sys` и `media` из ../ios/src.

#[path = "../../src/ahead.rs"]
mod ahead;
#[path = "../../src/api.rs"]
#[allow(dead_code)]
mod api;
#[path = "../../src/assets.rs"]
#[allow(dead_code)]
mod assets;
#[path = "../../src/auth.rs"]
#[allow(dead_code)]
mod auth;
#[path = "../../src/client.rs"]
#[allow(dead_code)]
mod client;
#[path = "../../src/config.rs"]
#[allow(dead_code)]
mod config;
#[path = "../../src/eq.rs"]
#[allow(dead_code)]
mod eq;
#[path = "../../src/fade.rs"]
mod fade;
#[path = "../../src/flac.rs"]
#[allow(dead_code)]
mod flac;
#[path = "../../src/history.rs"]
#[allow(dead_code)]
mod history;
#[path = "../../src/images.rs"]
#[allow(dead_code)]
mod images;
#[path = "../../src/landing.rs"]
#[allow(dead_code)]
mod landing;
#[path = "../../src/net.rs"]
#[allow(dead_code)]
mod net;
#[path = "../../src/output.rs"]
mod output;
#[path = "../../src/palette.rs"]
mod palette;
#[path = "../../src/player.rs"]
#[allow(dead_code)]
mod player;
#[path = "../../src/save.rs"]
#[allow(dead_code)]
mod save;
#[path = "../../src/secret.rs"]
mod secret;
#[path = "../../src/stream.rs"]
mod stream;
#[path = "../../src/tags.rs"]
#[allow(dead_code)]
mod tags;
#[path = "../../src/update.rs"]
#[allow(dead_code)]
mod update;
#[path = "../../src/ynison.rs"]
mod ynison;

mod blob;
mod download;
mod home;
#[cfg(not(target_os = "ios"))]
mod media;
#[cfg(target_os = "ios")]
#[path = "../../ios/src/media.rs"]
mod media;
mod mood;
mod now;
mod original;
mod pages;
mod profile;
#[cfg(not(target_os = "ios"))]
mod sys;
#[cfg(target_os = "ios")]
#[path = "../../ios/src/sys.rs"]
mod sys;
mod vector;
// На iOS у Slint нет OpenGL: оттуда берутся только переходы и тайминги, рисует wave_cpu.
#[cfg_attr(target_os = "ios", allow(dead_code))]
mod wave_gl;
#[cfg(target_os = "ios")]
#[path = "../../ios/src/wave_cpu.rs"]
mod wave_cpu;

/// Сцена «Моей волны»: шейдер на OpenGL ES на Android, расчёт на процессоре на iOS.
#[cfg(not(target_os = "ios"))]
type WaveScene = wave_gl::Wave;
#[cfg(target_os = "ios")]
type WaveScene = wave_cpu::Wave;

use api::{Api, Track};
use config::{log, Config};
use images::Images;
use player::{After, Cmd, ListKind, Player, Repeat, Source, Status};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

slint::include_modules!();

static UI: std::sync::OnceLock<slint::Weak<App>> = std::sync::OnceLock::new();

/// Короткое сообщение над мини-плеером, с любого потока.
pub fn toast(text: impl Into<String>) {
    let text = text.into();
    let Some(weak) = UI.get().cloned() else { return };
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        ui.global::<Now>().set_toast(text.as_str().into());
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_secs(3), move || {
            if let Some(ui) = weak.upgrade() {
                if ui.global::<Now>().get_toast() == text.as_str() {
                    ui.global::<Now>().set_toast("".into());
                }
            }
        });
    });
}

thread_local! {
    // Бюджет обложек как у «Лёгкого» режима десктопа: экран телефона меньше.
    static IMAGES: Images = Images::new(true);
}

/// Таймер сна: пауза в назначенное время или когда доиграет трек `after_track`.
#[derive(Default)]
struct SleepTimer {
    at: Option<Instant>,
    after_track: Option<String>,
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: slint::android::AndroidApp) {
    // Slint ставит платформу раз на процесс: второе окно в живом процессе останется чёрным.
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        std::process::exit(0);
    }
    sys::init(&app);
    if let Err(e) = slint::android::init_with_event_listener(app, on_lifecycle) {
        log(format!("slint: {e}"));
        return;
    }
    if let Err(e) = run() {
        log(format!("окно: {e}"));
    }
}

/// Точка входа iOS: её зовёт `main` из ios/src/main.rs на главном потоке. Платформу Slint
/// (winit, Skia на Metal) выбирает сам.
#[cfg(target_os = "ios")]
pub fn ios_main() {
    sys::init();
    if let Err(e) = run() {
        log(format!("окно: {e}"));
    }
}

/// Система просит освободить память: обложки скачаются заново, когда понадобятся.
#[cfg(target_os = "ios")]
pub(crate) fn low_memory() {
    IMAGES.with(|i| i.clear());
}

/// Приложение скрыто или системе не хватает памяти: обложки скачаются заново, когда понадобятся,
/// а освобождённое аллокатор сразу отдаёт системе.
#[cfg(target_os = "android")]
fn on_lifecycle(event: &slint::android::android_activity::PollEvent<'_>) {
    use slint::android::android_activity::{MainEvent, PollEvent};
    if let PollEvent::Main(MainEvent::Stop | MainEvent::LowMemory) = event {
        IMAGES.with(|i| i.clear());
        // M_PURGE_ALL у scudo, Android 12+. На старых версиях вызов просто ничего не делает.
        const M_PURGE_ALL: i32 = -104;
        unsafe extern "C" {
            fn mallopt(param: i32, value: i32) -> i32;
        }
        unsafe { mallopt(M_PURGE_ALL, 0) };
    }
}

fn run() -> Result<(), slint::PlatformError> {
    let ui = App::new()?;
    let _ = UI.set(ui.as_weak());
    if let Some(top) = sys::status_bar_height() {
        ui.global::<T>().set_top(top);
    }
    // На iOS окно появляется только внутри цикла событий: отступ под «чёлку» ставим потом.
    #[cfg(target_os = "ios")]
    sys::watch_safe_area(&ui);
    let shader = match original::Apk::open_installed() {
        Some(mut apk) => {
            original::register_fonts(&mut apk);
            // В другой версии оригинала иконку могут переименовать: тогда берём её из веб-версии.
            let mut sprite = None;
            set_icons(&ui, load_icons(|name| {
                apk.icon(name).and_then(|svg| slint::Image::load_from_svg_data(svg.as_bytes()).ok()).or_else(|| {
                    log(format!("иконки {name} нет в оригинале, берём с music.yandex.ru"));
                    let sprite = sprite.get_or_insert_with(|| crate::assets::Sprite::load(&mut crate::assets::Source::open()));
                    web_icon(sprite.as_ref()?, name)
                })
            }));
            original::wave_shader(&mut apk)
        }
        None => {
            log("Яндекс Музыка не установлена: шрифты и иконки с music.yandex.ru, волна своя");
            load_web_assets(&ui);
            // Свой фон: три мягких пятна из палитры настроения кружат над заголовком.
            // Комментарии в .frag только латиницей: часть драйверов GLSL ES не берёт остальное.
            Some(include_str!("wave.frag").to_string())
        }
    };
    profile::load_pictures(&ui);

    let wave: Rc<RefCell<Option<WaveScene>>> = Default::default();
    if let Some(src) = shader {
        #[cfg(target_os = "android")]
        install_wave(&ui, src, wave.clone());
        // Своего шейдера у iOS нет: рисунок из wave.frag повторён в wave_cpu.
        #[cfg(target_os = "ios")]
        {
            let _ = src;
            install_wave_cpu(&ui, wave.clone());
        }
    }

    let weak = ui.as_weak();
    let player = Player::spawn(std::sync::Arc::new(move || {
        media::changed();
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_refresh();
            }
        });
    }));

    // Громкость на телефоне задают кнопки системы, своего ползунка у оригинала нет.
    player.send(Cmd::Volume(1.0));
    media::start(player.clone());
    profile::bind(&ui, &player);
    {
        let (weak, p) = (ui.as_weak(), player.clone());
        ui.global::<Prof>().on_logout(move || {
            let Some(ui) = weak.upgrade() else { return };
            logout(&ui, &p);
        });
    }
    let cfg = Config::load();
    api::QUALITY.store(cfg.quality, std::sync::atomic::Ordering::Relaxed);
    ui.global::<Now>().set_quality(quality_index(cfg.quality));
    if let Some(token) = cfg.token {
        logged_in(&ui, &token, &player);
        player.send(Cmd::Login(token));
    } else {
        start_login(&ui, player.clone());
    }

    let sleep: Rc<RefCell<SleepTimer>> = Default::default();
    bind_player(&ui, &player, &sleep);
    {
        let p = player.clone();
        ui.on_play_wheel(move |i| {
            let Some(w) = home::wheel_item(i as usize) else { return };
            let source = match w.station() {
                Some(station) => Source::Seed { station, header: "Моя волна".into(), title: w.name },
                None => Source::Wave,
            };
            p.send(Cmd::Wave(source));
            p.send(Cmd::Resume);
        });
    }
    {
        let weak = ui.as_weak();
        let p = player.clone();
        ui.on_retry_login(move || {
            if let Some(ui) = weak.upgrade() {
                start_login(&ui, p.clone());
            }
        });
    }
    ui.on_open_login(|| sys::open_url("https://ya.ru/device"));

    // Плеер шлёт notify на каждое событие, а позиция трека меняется и без событий.
    let refresh = {
        let weak = ui.as_weak();
        let player = player.clone();
        let wave = wave.clone();
        let queue: Rc<RefCell<Vec<String>>> = Default::default();
        move || {
            if let Some(ui) = weak.upgrade() {
                sync(&ui, &player, &wave, &queue);
                tick_sleep(&ui, &player, &sleep);
            }
        }
    };
    ui.on_refresh(refresh.clone());
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), refresh);

    ui.run()
}

fn logged_in(ui: &App, token: &str, player: &Player) {
    let api = Arc::new(Api::new(token));
    let images = IMAGES.with(|i| i.clone());
    home::start(ui, api.clone(), images.clone());
    pages::start(ui, api.clone(), player.clone());
    now::start(ui, api.clone(), images);
    let p = player.clone();
    let a = api.clone();
    ui.global::<Now>().on_download(move || {
        if let Some(t) = p.state().track {
            download::start(a.clone(), "", vec![t]);
        }
    });
    profile::start(ui, api);
    ui.set_logged_in(true);
}

fn logout(ui: &App, player: &Player) {
    player.send(Cmd::Logout);
    let mut c = Config::load();
    if let Some(token) = c.token.clone() {
        std::thread::spawn(move || {
            if let Err(e) = auth::revoke(&token) {
                log(format!("выход: токен не отозван: {e}"));
            }
        });
    }
    c.forget_token();
    c.forget_refresh();
    c.token_expires = None;
    c.save();
    profile::clear(ui);
    ui.set_tab(0);
    ui.global::<Pages>().set_shown(false);
    start_login(ui, player.clone());
}

/// Кнопки плеера, шторок и плашки волны.
fn bind_player(ui: &App, player: &Player, sleep: &Rc<RefCell<SleepTimer>>) {
    let now = ui.global::<Now>();
    let p = player.clone();
    now.on_play_pause(move || p.send(Cmd::PlayPause));
    let p = player.clone();
    now.on_next(move || p.send(Cmd::Next));
    let p = player.clone();
    now.on_prev(move || p.send(Cmd::Prev));
    let p = player.clone();
    now.on_like(move || {
        if let Some(t) = p.state().track {
            p.send(Cmd::LikeTrack(t, None));
        }
    });
    let p = player.clone();
    now.on_dislike(move || p.send(Cmd::Dislike(None)));
    let p = player.clone();
    now.on_seek(move |f| {
        let s = p.state();
        if s.duration > 0.0 {
            p.send(Cmd::Seek(f * s.duration));
        }
    });
    let p = player.clone();
    now.on_toggle_repeat(move || {
        let next = match p.state().repeat {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        };
        p.send(Cmd::Repeat(next));
    });
    let p = player.clone();
    now.on_toggle_shuffle(move || p.send(Cmd::Shuffle(!p.state().shuffle)));
    let p = player.clone();
    now.on_track_wave(move || {
        let Some(t) = p.state().track else { return };
        p.send(Cmd::Wave(Source::Seed { station: format!("track:{}", t.id), header: "Моя волна по треку".into(), title: t.title }));
        p.send(Cmd::Resume);
    });
    let p = player.clone();
    now.on_share(move || {
        if let Some(t) = p.state().track {
            sys::share(&track_link(&t));
        }
    });
    let p = player.clone();
    now.on_trailer(move || {
        let (Some(t), Some(api)) = (p.state().track, now::api()) else { return };
        let Some(album) = t.album_id else { return };
        let p = p.clone();
        std::thread::spawn(move || match api.trailer(&format!("albums/{album}")) {
            Ok((title, tracks)) => {
                p.send(Cmd::PlayList { source: Source::List { kind: ListKind::Album, id: album, title }, tracks, start: 0, after: After::Wave(Source::Wave) });
                p.send(Cmd::Resume);
            }
            Err(e) => log(format!("трейлер: {e}")),
        });
    });
    let p = player.clone();
    now.on_play_similar(move |i| {
        let tracks = now::similar();
        if (i as usize) < tracks.len() {
            p.send(Cmd::PlayList {
                source: Source::List { kind: ListKind::Tracks, id: String::new(), title: "Похожие треки".into() },
                tracks,
                start: i as usize,
                after: After::Wave(Source::Wave),
            });
            p.send(Cmd::Resume);
        }
    });
    let p = player.clone();
    now.on_add_to_playlist(move |i| {
        let (Some(t), Some((uid, list)), Some(api)) = (p.state().track, now::playlist(i as usize), now::api()) else { return };
        std::thread::spawn(move || match api.add_to_playlist(&uid, &list.kind, &[t]) {
            Ok(()) => {
                let _ = slint::invoke_from_event_loop(now::reload_playlists);
            }
            Err(e) => log(format!("добавить в плейлист: {e}")),
        });
    });
    now.on_like_artist(|i| now::toggle_artist_like(i as usize));
    let p = player.clone();
    now.on_open_artist(move |i| {
        if i < 0 {
            pages::open_artists(&p.state().track.map(|t| t.artist_list).unwrap_or_default());
        } else if let Some(id) = now::artist_id(i) {
            pages::open(&format!("artist:{id}"));
        }
    });
    let p = player.clone();
    now.on_open_album(move || {
        if let Some(a) = p.state().track.and_then(|t| t.album_id) {
            pages::open(&format!("album:{a}"));
        }
    });
    let p = player.clone();
    now.on_open_source(move || {
        let Source::List { kind, id, .. } = p.state().source else { return };
        match kind {
            ListKind::Album => pages::open(&format!("album:{id}")),
            ListKind::Playlist => pages::open(&format!("playlist:{id}")),
            ListKind::Artist => pages::open(&format!("artist:{id}")),
            ListKind::Likes => pages::open("likes:"),
            ListKind::Tracks => {}
        }
    });
    now.on_open_playlists(|| pages::open("playlists"));
    let p = player.clone();
    now.on_back_to_wave(move || {
        p.send(Cmd::Wave(Source::Wave));
        p.send(Cmd::Resume);
    });
    let p = player.clone();
    now.on_jump(move |i| p.send(Cmd::Jump(i as usize)));
    let weak = ui.as_weak();
    now.on_set_quality(move |i| {
        let q = [3, 0, 1, 2][i.clamp(0, 3) as usize];
        api::QUALITY.store(q, std::sync::atomic::Ordering::Relaxed);
        let mut c = Config::load();
        c.quality = q;
        c.save();
        if let Some(ui) = weak.upgrade() {
            ui.global::<Now>().set_quality(i);
        }
    });
    let (s, p) = (sleep.clone(), player.clone());
    now.on_start_timer(move |minutes, end_of_track| {
        *s.borrow_mut() = if end_of_track {
            SleepTimer { at: None, after_track: p.state().track.map(|t| t.id) }
        } else {
            SleepTimer { at: Some(Instant::now() + Duration::from_secs(minutes.max(1) as u64 * 60)), after_track: None }
        };
    });
    let s = sleep.clone();
    now.on_stop_timer(move || *s.borrow_mut() = SleepTimer::default());
}

/// Качество из настроек → пункт шторки. «Автоматическое» хранится как 3, на десктопе его нет.
fn quality_index(q: u8) -> i32 {
    match q {
        3 => 0,
        0 => 1,
        1 => 2,
        _ => 3,
    }
}

pub(crate) fn track_link(t: &Track) -> String {
    match &t.album_id {
        Some(a) => format!("https://music.yandex.ru/album/{a}/track/{}", t.id),
        None => format!("https://music.yandex.ru/track/{}", t.id),
    }
}

fn tick_sleep(ui: &App, player: &Player, sleep: &Rc<RefCell<SleepTimer>>) {
    let mut t = sleep.borrow_mut();
    let s = player.state();
    let now = ui.global::<Now>();
    let left = match (&t.at, &t.after_track) {
        (Some(at), _) => at.saturating_duration_since(Instant::now()).as_secs_f32(),
        (None, Some(id)) if s.track.as_ref().is_some_and(|t| &t.id == id) => (s.duration - s.position).max(0.1),
        // Заиграл следующий трек: тот, на котором ставили таймер, доиграл.
        (None, Some(_)) => 0.0,
        (None, None) => {
            now.set_timer_on(false);
            return;
        }
    };
    if left <= 0.0 {
        if s.playing {
            player.send(Cmd::PlayPause);
        }
        *t = SleepTimer::default();
    }
    now.set_timer_on(left > 0.0);
    now.set_timer_left(now::time(left.ceil()).into());
}

/// Состояние плеера → свойства окна и сцена волны.
fn sync(ui: &App, player: &Player, wave: &Rc<RefCell<Option<WaveScene>>>, queue: &Rc<RefCell<Vec<String>>>) {
    let s = player.state();
    now::set_uid(&s.uid);
    now::track_changed(s.track.as_ref());
    let now = ui.global::<Now>();
    now.set_track_id(s.track.as_ref().map(|t| t.id.as_str()).unwrap_or("").into());
    now.set_playing(s.playing);
    now.set_loading(s.loading);
    if s.playing {
        ui.set_started(true);
    }
    now.set_liked(s.track.as_ref().is_some_and(|t| player.is_liked(&t.id)));
    now.set_progress(if s.duration > 0.0 { s.position / s.duration } else { 0.0 });
    now.set_position_text(now::time(s.position).into());
    if s.duration > 0.0 {
        now.set_duration_text(now::time(s.duration).into());
    }
    now.set_lyric(now::lyric_at(s.position));
    now.set_shuffle(s.shuffle);
    now.set_repeat(match s.repeat {
        Repeat::Off => 0,
        Repeat::All => 1,
        Repeat::One => 2,
    });
    let source = match &s.source {
        Source::Wave => "Моя волна",
        Source::Seed { header, .. } => header.as_str(),
        Source::List { title, .. } => title.as_str(),
    };
    now.set_source(source.into());
    let pill = match &s.source {
        Source::Wave => "",
        Source::Seed { title, .. } if !title.is_empty() => title.as_str(),
        Source::Seed { header, .. } => header.as_str(),
        Source::List { title, .. } => title.as_str(),
    };
    now.set_pill(pill.into());
    let status = match &s.status {
        Status::LoggedOut => "",
        Status::Connecting => "Подключаемся…",
        Status::Ready if s.track.is_none() => "Загружаем волну…",
        Status::Ready => "",
        Status::Failed(e) => e.as_str(),
    };
    let status = s.error.as_ref().map(|(e, _)| e.as_str()).unwrap_or(status);
    now.set_status(status.into());

    let ids: Vec<String> = s.upcoming.iter().map(|t| t.id.clone()).collect();
    if *queue.borrow() != ids {
        set_queue(ui, &s.upcoming);
        *queue.borrow_mut() = ids;
    }
    if wave.borrow_mut().as_mut().is_some_and(|w| w.set_mood(s.track.as_ref().and_then(|t| t.mood))) {
        ui.window().request_redraw();
    }
}

fn set_queue(ui: &App, tracks: &[Track]) {
    let model = Rc::new(VecModel::from(tracks.iter().map(now::track_row).collect::<Vec<_>>()));
    let px = (52.0 * ui.window().scale_factor()).round() as u32;
    IMAGES.with(|images| {
        for (i, t) in tracks.iter().enumerate() {
            let Some(uri) = &t.cover else { continue };
            let m = model.clone();
            images.get(&api::pic(uri, px), px, move |img| {
                if let Some(mut row) = m.row_data(i) {
                    row.cover = img.clone();
                    m.set_row_data(i, row);
                }
            });
        }
    });
    ui.global::<Now>().set_queue(ModelRc::from(model));
}

/// Вход по коду: показать код, открыть ya.ru/device, ждать подтверждения в фоне.
fn start_login(ui: &App, player: Player) {
    ui.set_logged_in(false);
    ui.set_login_code("".into());
    ui.set_login_hint("Получаем код…".into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let set = |code: String, hint: String| {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_login_code(code.into());
                    ui.set_login_hint(hint.into());
                }
            });
        };
        let code = match auth::device_code() {
            Ok(c) => c,
            Err(e) => return set(String::new(), format!("Не удалось получить код: {e}")),
        };
        set(code.user_code.clone(), "Откройте ya.ru/device и введите этот код. Можно на этом же телефоне.".into());
        let deadline = std::time::Instant::now() + Duration::from_secs(code.expires_in);
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs(code.interval));
            match auth::poll(&code.device_code) {
                auth::Poll::Pending => continue,
                auth::Poll::Failed(e) => return set(String::new(), e),
                auth::Poll::Done(token) => {
                    let mut c = Config::load();
                    c.token = Some(token.access.clone());
                    c.refresh_token = token.refresh;
                    c.token_expires = token.expires_in.map(|s| config::now_secs() + s);
                    c.save();
                    player.send(Cmd::Login(token.access.clone()));
                    let weak = weak.clone();
                    let player = player.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = weak.upgrade() {
                            logged_in(&ui, &token.access, &player);
                        }
                    });
                    return;
                }
            }
        }
        set(String::new(), "Код истёк. Откройте приложение заново.".into());
    });
}

fn set_icons(ui: &App, icons: Icons) {
    ui.set_icons(icons.clone());
    ui.global::<Now>().set_icons(icons);
}

/// Те же файлы, что берёт десктоп без оригинала. Шрифта YS Display в веб-версии нет.
fn load_web_assets(ui: &App) {
    let mut src = crate::assets::Source::open();
    crate::assets::register_fonts(&mut src);
    ui.global::<T>().set_display_font("YS Text".into());
    let Some(sprite) = crate::assets::Sprite::load(&mut src) else {
        log("спрайт иконок music.yandex.ru не скачался");
        return;
    };
    set_icons(ui, load_icons(|name| web_icon(&sprite, name)));
}

/// Иконка по имени из оригинала: своя или из спрайта веб-версии.
fn web_icon(sprite: &crate::assets::Sprite, name: &str) -> Option<slint::Image> {
    if let Some((_, svg)) = OWN_ICONS.iter().find(|(android, _)| *android == name) {
        return slint::Image::load_from_svg_data(svg.as_bytes()).ok();
    }
    let id = WEB_ICONS.iter().find(|(android, _)| *android == name)?.1;
    let vars = std::collections::HashMap::from([
        ("--ym-logo-color-primary-back", "transparent"),
        ("--ym-logo-text-color", "#fff"),
        ("currentColor", "#fff"),
    ]);
    Some(sprite.image(id, &vars))
}

/// Иконки, для которых в спрайте веб-версии нет похожих.
const OWN_ICONS: &[(&str, &str)] = &[
    (
        "ui_kit_ic_profile_24",
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="8" r="4"/><path d="M4.5 20c1.4-3.3 4.2-5 7.5-5s6.1 1.7 7.5 5"/></svg>"##,
    ),
    (
        "ic_logout_24",
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 4H6.5A2.5 2.5 0 0 0 4 6.5v11A2.5 2.5 0 0 0 6.5 20H10"/><path d="M15 8l4 4-4 4M19 12H9"/></svg>"##,
    ),
];

/// Иконки Android и ближайшие к ним символы спрайта веб-версии.
const WEB_ICONS: &[(&str, &str)] = &[
    ("ic_yandex_music_logo_ru", "musicLogoLeftRu"),
    ("ic_search_24", "search_xs"),
    ("ic_play_32", "play_m"),
    ("ic_pause_32", "pause_m"),
    ("ic_like_unactive_24", "like_xs"),
    ("ic_like_active_24", "liked_xs"),
    ("ic_android_my_wibe", "navigationMyVibe_xs"),
    ("ic_landing_default", "home_m"),
    ("ic_android_concerts_light_theme_default", "navigationConcerts_xs"),
    ("ic_android_my_music_test_default", "navigationCollection_xs"),
    ("ic_arrow_mid_down_24", "arrowDown_xs"),
    ("ui_kit_ic_list_24", "playQueue_xs"),
    ("ic_share_new_24", "share_m"),
    ("ic_more_24", "more_xs"),
    ("ic_dislike_24", "dislike_xs"),
    ("ic_repeat_default_40", "repeat_xs"),
    ("ic_repeat_active_1_40", "repeat_one_xs"),
    ("ic_shuffle_default_40", "shuffle_xs"),
    ("ic_shuffle_active_40", "shuffle_xs"),
    ("ic_lyrics_default_40", "syncLyrics_m"),
    ("ui_kit_ic_download_24", "download_xxs"),
    ("ic_timer_24", "history_m"),
    ("ui_kit_ic_filter_24", "filter_xs"),
    ("ic_listeners_16", "users_xxxs"),
    ("ui_kit_ic_circle_attention_outline_16", "info_xxs"),
    ("ui_kit_ic_add_24", "add_xxs"),
    ("ui_kit_ic_full_screen_24", "fullscreen_xs"),
    ("ic_arrow_right_small", "arrowRight_xxs"),
    ("ui_kit_ic_done_24", "check_xs"),
    ("ic_tracklyrics_24", "lyrics_xxs"),
    ("ic_trailer_24", "trailer_xs"),
    ("ui_kit_ic_play_24", "play_xs"),
    ("ic_chart_crown_20", "crown_xxs"),
    ("ic_book_24", "note_xs"),
    ("ui_kit_ic_kids_24", "kids_m"),
    ("ic_podcast_24", "podcasts_xxs"),
    ("ic_clip_24", "clip_xxs"),
    ("ic_playlist_24", "playlist_xs"),
    ("ic_album_24", "album_xs"),
    ("ui_kit_ic_open_new_24", "link_xxs"),
];

fn load_icons(mut icon: impl FnMut(&str) -> Option<slint::Image>) -> Icons {
    let mut icon = |name: &str| icon(name).unwrap_or_default();
    Icons {
        logo: icon("ic_yandex_music_logo_ru"),
        search: icon("ic_search_24"),
        play: icon("ic_play_32"),
        pause: icon("ic_pause_32"),
        like: icon("ic_like_unactive_24"),
        liked: icon("ic_like_active_24"),
        tab_wave: icon("ic_android_my_wibe"),
        tab_home: icon("ic_landing_default"),
        tab_concerts: icon("ic_android_concerts_light_theme_default"),
        tab_collection: icon("ic_android_my_music_test_default"),
        chevron_down: icon("ic_arrow_mid_down_24"),
        queue: icon("ui_kit_ic_list_24"),
        share: icon("ic_share_new_24"),
        more: icon("ic_more_24"),
        dislike: icon("ic_dislike_24"),
        repeat: icon("ic_repeat_default_40"),
        repeat_one: icon("ic_repeat_active_1_40"),
        shuffle: icon("ic_shuffle_default_40"),
        shuffle_on: icon("ic_shuffle_active_40"),
        lyrics: icon("ic_lyrics_default_40"),
        download: icon("ui_kit_ic_download_24"),
        timer: icon("ic_timer_24"),
        sound: icon("ui_kit_ic_filter_24"),
        listeners: icon("ic_listeners_16"),
        explicit: icon("ui_kit_ic_circle_attention_outline_16"),
        info: icon("ui_kit_ic_circle_attention_outline_16"),
        add: icon("ui_kit_ic_add_24"),
        fullscreen: icon("ui_kit_ic_full_screen_24"),
        arrow_right: icon("ic_arrow_right_small"),
        check: icon("ui_kit_ic_done_24"),
        text: icon("ic_tracklyrics_24"),
        trailer: icon("ic_trailer_24"),
        play_small: icon("ui_kit_ic_play_24"),
        chart: icon("ic_chart_crown_20"),
        book: icon("ic_book_24"),
        kids: icon("ui_kit_ic_kids_24"),
        podcast: icon("ic_podcast_24"),
        clip: icon("ic_clip_24"),
        playlist: icon("ic_playlist_24"),
        album: icon("ic_album_24"),
        profile: icon("ui_kit_ic_profile_24"),
        logout: icon("ic_logout_24"),
        open: icon("ui_kit_ic_open_new_24"),
    }
}

/// Волна на iOS: кадр считается на процессоре и ставится картинкой под интерфейс, до 30 раз
/// в секунду, пока пятно движется или идёт переход цветов.
#[cfg(target_os = "ios")]
fn install_wave_cpu(ui: &App, wave: Rc<RefCell<Option<WaveScene>>>) {
    *wave.borrow_mut() = Some(wave_cpu::Wave::new());
    let weak = ui.as_weak();
    let mut drawn = false;
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut wave = wave.borrow_mut();
        let Some(w) = wave.as_mut() else { return };
        let moving = ui.get_wave_moving();
        if drawn && !moving && !w.animating() {
            return;
        }
        let size = ui.window().size().to_logical(ui.window().scale_factor());
        if size.width <= 0.0 || size.height <= 0.0 {
            return;
        }
        // Со сдвигом кадр шире окна, чтобы центр пятна встал куда надо (как в wave_gl).
        let extra = ui.get_wave_shift().abs() * 2.0;
        ui.set_wave_image(w.frame(size.width + extra, size.height, moving));
        drawn = true;
    });
    // Таймер нужен, пока живёт приложение.
    std::mem::forget(timer);
}

/// Шейдер волны рисуется перед интерфейсом в каждом кадре, пока окно на экране.
#[cfg(target_os = "android")]
fn install_wave(ui: &App, fragment: String, wave: Rc<RefCell<Option<wave_gl::Wave>>>) {
    let weak = ui.as_weak();
    let res = ui.window().set_rendering_notifier(move |state, api| match state {
        slint::RenderingState::RenderingSetup => {
            let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api else {
                log("волна: рендерер не на OpenGL");
                return;
            };
            // SAFETY: Slint вызывает колбэк с текущим контекстом GL своего окна.
            let gl = unsafe { glow::Context::from_loader_function_cstr(|name| get_proc_address(name)) };
            match unsafe { wave_gl::Wave::new(gl, &fragment) } {
                Ok(w) => *wave.borrow_mut() = Some(w),
                Err(e) => log(format!("волна: {e}")),
            }
        }
        slint::RenderingState::BeforeRendering => {
            let Some(ui) = weak.upgrade() else { return };
            if let Some(w) = wave.borrow_mut().as_mut() {
                let size = ui.window().size();
                let shift = (ui.get_wave_shift() * ui.window().scale_factor()).round() as i32;
                let motion = if ui.get_wave_moving() { wave_gl::Motion::Moving } else { wave_gl::Motion::Still };
                unsafe { w.draw(size.width, size.height, shift, motion) };
                if w.animating() || motion == wave_gl::Motion::Moving {
                    ui.window().request_redraw();
                }
            }
        }
        slint::RenderingState::RenderingTeardown => *wave.borrow_mut() = None,
        _ => {}
    });
    if let Err(e) = res {
        log(format!("волна: {e:?}"));
    }
}
