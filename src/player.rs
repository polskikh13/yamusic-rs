//! Аудио-движок: очередь, загрузка и предзагрузка треков, фидбек волне, воспроизведение.
//! Живёт в своём потоке, UI шлёт ему команды и читает `Shared`.

use crate::ahead::Ahead;
use crate::api::{Account, Api, Batch, Res, Track, WaveSession, WAVE};
use crate::config::log;
use crate::eq::{Eq, EqParams};
use crate::fade::{Fade, FadeOut};
use crate::output::Output;
use crate::stream::{Buf, Reader};
use rodio::{Decoder, Sink, Source as _};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, PartialEq, Default, Debug)]
pub enum Status {
    #[default]
    LoggedOut,
    Connecting,
    Ready,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ListKind {
    Playlist,
    Album,
    Artist,
    Likes,
    Tracks,
}

/// Что сейчас играет.
#[derive(Clone, PartialEq, Debug, Default)]
pub enum Source {
    #[default]
    Wave,
    /// Волна по станции: `artist:ID`, `track:ID`, `album:ID`, `playlist:UID_KIND`.
    Seed { station: String, header: String, title: String },
    List { kind: ListKind, id: String, title: String },
}

impl Source {
    pub fn station(&self) -> Option<&str> {
        match self {
            Source::Wave => Some(WAVE),
            Source::Seed { station, .. } => Some(station),
            Source::List { .. } => None,
        }
    }
}

/// Что играть, когда список закончится.
#[derive(Clone, Debug)]
pub enum After {
    Loop,
    Wave(Source),
}

#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

#[derive(Clone, Default)]
pub struct Shared {
    pub status: Status,
    pub uid: String,
    pub login: String,
    pub has_plus: bool,
    pub track: Option<Track>,
    pub source: Source,
    pub upcoming: Vec<Track>,
    pub playing: bool,
    pub likes_revision: i32,
    /// Текущий трек в «Не нравится».
    pub disliked: bool,
    pub loading: bool,
    pub position: f32,
    track_generation: u64,
    seek_request: u64,
    pub duration: f32,
    pub volume: f32,
    pub repeat: Repeat,
    pub shuffle: bool,
    pub error: Option<(String, Instant)>,
    /// Трек продолжен с того места, где его оставили в прошлый раз: плеер виден сразу, на паузе.
    pub resumed: bool,
    /// Формат играющего файла: «MP3 320», «AAC».
    pub format: String,
}

/// Как положение ползунка громкости переводится в усиление.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Loudness {
    Square,
    /// 50 дБ на всю шкалу: каждый шаг ползунка на слух одинаковый (#44).
    Decibels,
}

pub enum Cmd {
    Login(String),
    Logout,
    PlayPause,
    /// Снять с паузы (если уже играет — ничего не делать).
    Resume,
    Next,
    Prev,
    Seek(f32),
    Volume(f32),
    Loudness(Loudness),
    LikeTrack(Track, Option<LikeCompleted>),
    Dislike(Option<LikeCompleted>),
    /// Дизлайк трека из меню или снять его, если уже стоит; трек не пропускается.
    DislikeTrack(Track, Option<LikeCompleted>),
    Wave(Source),
    /// Заново начать «Мою волну» с настройками (seeds `setting…`).
    TuneWave(Vec<String>),
    PlayList { source: Source, tracks: Vec<Track>, start: usize, after: After },
    /// Перейти к треку из очереди (индекс в `upcoming`).
    Jump(usize),
    Repeat(Repeat),
    Shuffle(bool),
    /// «Играть следующим» (`next`) или «Добавить в конец очереди»: трек или весь альбом и
    /// плейлист, в своём порядке.
    Enqueue { tracks: Vec<Track>, next: bool },
    /// Переключатель «Треки 18+».
    AllowExplicit(bool),
    /// Сколько треки перетекают друг в друга; ноль — без кроссфейда.
    Crossfade(Duration),
    /// Приложение закрывается: записать в Ynison, где остановились, и ответить.
    Leave(Sender<()>),
}

enum Msg {
    Cmd(Cmd),
    Seek { seconds: f32, track_generation: u64, request: u64 },
    Account(Res<Account>),
    Likes(String, Res<Vec<String>>),
    Dislikes(String, Res<Vec<String>>),
    ReactionFinished { uid: String, id: String, seq: u64, kind: Reaction, remove: bool, result: Res<()> },
    Batch { res: Res<Batch>, replace: bool, gen: u64 },
    Audio { gen: u64, res: Res<Audio> },
    Prefetch { id: String, res: Res<Audio> },
    Resume(Res<Option<Resume>>),
}

type Notify = Arc<dyn Fn() + Send + Sync>;
type LikeCompleted = Box<dyn FnOnce(&Track, bool, &Res<()>) + Send>;
type Job = Box<dyn FnOnce() + Send>;
pub type Likes = Arc<Mutex<HashSet<String>>>;

#[derive(Clone)]
pub struct Player {
    tx: Sender<Msg>,
    shared: Arc<Mutex<Shared>>,
    likes: Likes,
    dislikes: Likes,
    pub eq: Arc<EqParams>,
}

impl Player {
    pub fn spawn(notify: Notify) -> Player {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Shared { volume: 0.6, ..Default::default() }));
        let likes: Likes = Default::default();
        let eq: Arc<EqParams> = Default::default();
        let dislikes: Likes = Default::default();
        let p = Player { tx: tx.clone(), shared: shared.clone(), likes: likes.clone(), dislikes: dislikes.clone(), eq: eq.clone() };
        thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                let mut engine = Engine::new(tx, shared, likes, notify, eq);
                engine.dislikes = dislikes;
                engine.run(rx)
            })
            .expect("audio thread");
        p
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(Msg::Cmd(c));
    }

    /// Перед выходом: дождаться, пока Ynison запишет, где остановились, но не дольше 3 секунд.
    pub fn leave(&self) {
        let (done, wait) = mpsc::channel();
        self.send(Cmd::Leave(done));
        let _ = wait.recv_timeout(Duration::from_secs(3));
    }

    /// Сразу запоминаем выбранное время, иначе бегунок на миг вернётся к старой позиции.
    pub fn seek(&self, seconds: f32) -> Option<f32> {
        let mut s = self.shared.lock().unwrap();
        if !seconds.is_finite() || s.loading || s.track.is_none() || s.duration <= 0.0 {
            return None;
        }
        let seconds = seconds.clamp(0.0, s.duration);
        let request = s.seek_request.wrapping_add(1);
        self.tx.send(Msg::Seek { seconds, track_generation: s.track_generation, request }).ok()?;
        s.seek_request = request;
        s.position = seconds;
        Some(seconds)
    }

    pub fn state(&self) -> Shared {
        self.shared.lock().unwrap().clone()
    }

    pub fn is_liked(&self, id: &str) -> bool {
        self.likes.lock().unwrap().contains(id)
    }

    pub fn is_disliked(&self, id: &str) -> bool {
        self.dislikes.lock().unwrap().contains(id)
    }

}

/// Сколько живёт сессия волны без прослушивания: оригинал начинает новую через 9 часов.
const WAVE_SESSION_TTL: u64 = 9 * 3600;

/// `from` «Моей волны» у оригинала, когда её запускают с главной.
const WAVE_FROM: &str = "desktop-wave_landing_screen-my_wave-radio-default";

struct WaveState {
    uid: String,
    session: String,
    /// Последний трек волны: с ним сессия клонируется после перезапуска.
    last: String,
    at: u64,
}

fn load_wave_state() -> Option<WaveState> {
    let text = std::fs::read_to_string(crate::config::dir().join("wave.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let str_at = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
    Some(WaveState {
        uid: str_at("uid"),
        session: str_at("session"),
        last: str_at("last"),
        at: v.get("at").and_then(|x| x.as_u64()).unwrap_or(0),
    })
}

/// Где звучит трек — для `/plays`, в тех же полях, что у оригинала.
#[derive(Clone)]
struct PlayContext {
    from: String,
    context: &'static str,
    item: Option<String>,
    playlist: Option<String>,
    /// Сессия волны; пустая у списков.
    session: String,
}

/// Одно прослушивание трека: сколько его реально слушали, была ли пауза и перемотка.
/// Уходит в `/plays`, когда трек сменился.
struct Listen {
    track: Track,
    ctx: PlayContext,
    play_id: String,
    add_time: String,
    start: String,
    played: f64,
    since: Option<Instant>,
    pause: bool,
    seek: bool,
    repeated: bool,
}

impl Listen {
    fn played(&self) -> f64 {
        self.played + self.since.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0)
    }

    fn stop(&mut self) {
        if let Some(s) = self.since.take() {
            self.played += s.elapsed().as_secs_f64();
        }
    }

    fn go(&mut self) {
        self.since.get_or_insert_with(Instant::now);
    }
}

/// Случайные 128 бит без отдельной зависимости: sha256 от времени, счётчика и pid.
fn random_u128() -> u128 {
    use sha2::{Digest, Sha256};
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let h = Sha256::digest(format!("{t}:{n}:{}", std::process::id()));
    u128::from_le_bytes(h[..16].try_into().unwrap())
}

/// uuid v4, как `playId` у оригинала.
fn uuid4() -> String {
    let mut b = random_u128().to_be_bytes();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Трек, который качается, и декодер, уже прочитавший его начало.
/// Скачиваемый трек, его декодер и формат («MP3 320»).
type Audio = (Arc<Buf>, Decoder<Reader>, String);

/// Имя выхода Windows по умолчанию. На Linux звук идёт в «default», и за сменой устройства
/// там следит PipeWire или PulseAudio.
#[cfg(windows)]
fn default_output_name() -> Option<String> {
    use rodio::cpal::traits::HostTrait;
    use rodio::DeviceTrait;
    rodio::cpal::default_host().default_output_device()?.name().ok()
}

/// Многоканальные выходы Windows (виртуальные 7.1 у гарнитур) искажают f32, а i32 играют чисто: cpal#892.
#[cfg(windows)]
fn open_surround() -> Option<Output> {
    use rodio::cpal::traits::HostTrait;
    use rodio::cpal::SampleFormat;
    use rodio::DeviceTrait;
    let dev = rodio::cpal::default_host().default_output_device()?;
    let def = dev.default_output_config().ok()?;
    if def.channels() <= 2 || def.sample_format() == SampleFormat::I32 {
        return None;
    }
    let rate = def.sample_rate();
    let cfg = dev
        .supported_output_configs()
        .ok()?
        .find(|c| c.sample_format() == SampleFormat::I32 && c.channels() == def.channels() && c.min_sample_rate() <= rate && rate <= c.max_sample_rate())?
        .with_sample_rate(rate);
    let out = Output::open_with(&dev, cfg).ok()?;
    log(format!("аудиовыход: {} каналов, i32", def.channels()));
    Some(out)
}

/// Только выход по умолчанию: `try_default` при неудаче молча берёт любое другое устройство,
/// и звук уходит, например, в неподключённый HDMI (#53).
fn open_output() -> Result<Output, rodio::StreamError> {
    use rodio::cpal::traits::HostTrait;
    #[cfg(windows)]
    if let Some(out) = open_surround() {
        return Ok(out);
    }
    let dev = rodio::cpal::default_host().default_output_device().ok_or(rodio::StreamError::NoDevice)?;
    Output::open(&dev)
}

#[cfg(windows)]
const NO_OUTPUT: &str = "Не найдено аудиоустройство";
#[cfg(not(windows))]
const NO_OUTPUT: &str = "Не открылся звуковой выход по умолчанию. Для PipeWire нужен pipewire-alsa";

/// Как часто записывать позицию играющего трека в Ynison.
const YNISON_EVERY: Duration = Duration::from_secs(60);

/// Сколько позиция может стоять на месте у играющего скачанного трека.
const STALL_LIMIT: Duration = Duration::from_secs(3);

/// Если выход снова сломался быстрее этого, не переоткрываем его, а ставим паузу до Play.
const BROKEN_AGAIN: Duration = Duration::from_secs(30);

/// Пауза перед повтором входа после сетевой ошибки: 2, 5, 10, 20, 30 с, дальше раз в минуту.
fn account_retry_delay(attempt: u32) -> Duration {
    const STEPS: [u64; 5] = [2, 5, 10, 20, 30];
    Duration::from_secs(STEPS.get(attempt.saturating_sub(1) as usize).copied().unwrap_or(60))
}

fn load_track(api: &Api, t: &Track) -> Res<Audio> {
    let (url, format) = api.stream_url(&t.id)?;
    let (again, id) = (api.clone(), t.id.clone());
    // Ссылка подписана и живёт недолго, поэтому для докачки берётся новая.
    let buf = Buf::download(api.open_from(&url, 0)?, move |from| again.open_from(&again.stream_url(&id)?.0, from))?;
    let dec = Decoder::new(buf.reader()).map_err(|e| format!("не удалось декодировать: {e}"))?;
    Ok((buf, dec, format))
}

/// Что последним играло в любом клиенте, уже с треками из API.
struct Resume {
    source: Source,
    tracks: Vec<Track>,
    index: usize,
    position: f32,
}

fn fetch_resume(api: &Api) -> Res<Option<Resume>> {
    let p = crate::ynison::playing(api.token(), &crate::ynison::device_id())?;
    let Some(current) = p.track_id() else { return Ok(None) };
    // Волна дальше подберёт треки сама, из её очереди нужен только текущий.
    let ids = if p.entity_type == "RADIO" { vec![current.to_string()] } else { p.track_ids.clone() };
    let tracks = api.tracks(&ids)?;
    let Some(index) = tracks.iter().position(|t| t.id == current) else { return Ok(None) };
    let track = &tracks[index];
    let artist = |id: &str| track.artist_list.iter().find(|a| a.id == id).map(|a| a.name.clone()).unwrap_or_default();
    let album = |id: &str| if track.album_id.as_deref() == Some(id) { track.album_title.clone() } else { String::new() };
    let entity = p.entity_id.clone();
    let source = match p.entity_type.as_str() {
        "RADIO" if entity == WAVE => Source::Wave,
        "RADIO" => {
            let (kind, id) = entity.split_once(':').unwrap_or((entity.as_str(), ""));
            let (header, title) = match kind {
                "artist" => ("Моя волна по исполнителю", artist(id)),
                "album" => ("Моя волна по альбому", album(id)),
                "track" if id == track.id => ("Моя волна по треку", track.title.clone()),
                "track" => ("Моя волна по треку", String::new()),
                "playlist" => ("Моя волна по плейлисту", String::new()),
                _ => ("Моя волна", String::new()),
            };
            // Занятие, настроение, плейлист по треку не узнать, и после перезапуска вместо
            // «Моя волна под занятие «В дороге»» была просто «Моя волна». Играла последняя волна
            // аккаунта, её название знает сервер.
            let (header, title) = match title.is_empty().then(|| api.wave_title(&entity)).flatten() {
                Some((desc, name)) => (if desc.is_empty() { header.to_string() } else { desc }, name),
                None => (header.to_string(), title),
            };
            Source::Seed { station: entity.clone(), header, title }
        }
        "PLAYLIST" => Source::List { kind: ListKind::Playlist, title: String::new(), id: entity },
        "ALBUM" => Source::List { kind: ListKind::Album, title: album(&entity), id: entity },
        "ARTIST" => Source::List { kind: ListKind::Artist, title: artist(&entity), id: entity },
        _ => Source::List { kind: ListKind::Tracks, id: String::new(), title: String::new() },
    };
    Ok(Some(Resume { source, tracks, index, position: p.progress_ms as f32 / 1000.0 }))
}

/// Перемешивание без лишних зависимостей (xorshift от времени).
pub fn shuffle<T>(v: &mut [T]) {
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
        | 1;
    for i in (1..v.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.swap(i, (x % (i as u64 + 1)) as usize);
    }
}

enum End {
    Finished,
    Skip,
    Dislike,
    /// Волну запустили заново: пропуск, но очередь уже свежая.
    Restart,
}

struct ListPlay {
    deferred: VecDeque<Track>,
    all: Vec<Track>,
    after: After,
}

struct Engine {
    tx: Sender<Msg>,
    bg: Sender<Job>,
    shared: Arc<Mutex<Shared>>,
    likes: Likes,
    // Запоминаем подтверждённые реакции, чтобы запоздалый список с сервера их не заменил.
    reaction_writes: HashMap<String, ReactionWrite>,
    reaction_seq: u64,
    dislikes: Likes,
    notify: Notify,
    eq: Arc<EqParams>,
    audio: Option<(Output, Sink)>,
    /// Затухание играющего трека.
    fade_out: Arc<FadeOut>,
    /// Прошлый трек, который затухает под начало нового.
    fading: Option<Sink>,
    crossfade: Duration,
    /// С каким нарастанием начать следующий загруженный трек.
    fade_in: Duration,
    /// Выход Windows по умолчанию, на котором открыт звук, и когда его последний раз сверяли.
    #[cfg(windows)]
    out_name: Option<String>,
    #[cfg(windows)]
    out_checked: Instant,
    /// Когда выход последний раз сломался.
    broke_at: Option<Instant>,
    /// Выход пропал посреди трека: с этого места трек продолжится по Play.
    held_pos: Option<f32>,
    /// Позиция играющего трека и с какого момента она не меняется.
    stall: (f32, Instant),
    api: Option<Api>,
    uid: String,
    source: Source,
    list: Option<ListPlay>,
    queue: VecDeque<Track>,
    /// Сколько треков в начале очереди вернулось туда по «назад»: пропуск в волне их не сбрасывает.
    returned: usize,
    history: Vec<Track>,
    current: Option<Track>,
    current_buf: Option<Arc<Buf>>,
    batch_id: String,
    /// Сессия ротора текущей волны; radioStarted уходит в неё, когда она появится.
    session: String,
    radio_started_pending: bool,
    /// Пропуски и дослушанные треки, которые уйдут со следующим запросом треков волны.
    deferred: Vec<serde_json::Value>,
    /// Станция, для которой уже проверена сохранённая сессия «Моей волны».
    played_station: String,
    /// Сессия «Моей волны» с прошлого запуска уже подхвачена (или не нашлась).
    wave_restored: bool,
    /// Сессия с прошлого запуска: первый пакет берётся её клоном, как у оригинала.
    clone_pending: bool,
    /// Последний трек волны с прошлого запуска — queue для клона.
    restore_last: Option<String>,
    /// Настройки, с которыми начать следующую сессию «Моей волны».
    tune: Option<Vec<String>>,
    /// С какой секунды продолжить трек, когда он загрузится.
    resume_at: Option<(String, f32)>,
    /// Когда записать состояние в Ynison: правки идут пачкой (перемотка, пауза), пишем одну.
    ynison_due: Option<Instant>,
    ynison_at: Instant,
    /// Текущее прослушивание для `/plays`.
    listen: Option<Listen>,
    fetching: bool,
    batch_gen: u64,
    retry_at: Option<Instant>,
    /// Вход не удался из-за сети: когда повторить и какая это попытка.
    account_retry: Option<(Instant, u32)>,
    gen: u64,
    loaded: bool,
    paused: bool,
    volume: f32,
    loudness: Loudness,
    repeat: Repeat,
    shuffle: bool,
    prefetch: Option<(String, Audio)>,
    prefetching: Option<String>,
    errors_in_row: u32,
    was_playing: bool,
    handled_seek: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reaction { Like, Dislike }

impl Reaction {
    fn applied(self, saved: Option<Self>, remove: bool) -> Option<Self> {
        if remove { saved.filter(|old| *old != self) } else { Some(self) }
    }
}

struct ReactionWrite {
    first: u64,
    latest: u64,
    saved: Option<Reaction>,
    confirmed: bool,
}

impl Engine {
    fn new(tx: Sender<Msg>, shared: Arc<Mutex<Shared>>, likes: Likes, notify: Notify, eq: Arc<EqParams>) -> Self {
        // Фидбек и лайки — по порядку в отдельном потоке, чтобы не блокировать плеер.
        let (bg, bg_rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("bg".into())
            .spawn(move || {
                for job in bg_rx {
                    job();
                }
            })
            .expect("bg thread");
        Engine {
            tx,
            bg,
            shared,
            likes,
            reaction_writes: HashMap::new(),
            reaction_seq: 0,
            dislikes: Default::default(),
            notify,
            eq,
            audio: None,
            #[cfg(windows)]
            out_name: None,
            #[cfg(windows)]
            out_checked: Instant::now(),
            broke_at: None,
            held_pos: None,
            stall: (0.0, Instant::now()),
            api: None,
            uid: String::new(),
            source: Source::Wave,
            list: None,
            queue: VecDeque::new(),
            returned: 0,
            history: Vec::new(),
            current: None,
            current_buf: None,
            batch_id: String::new(),
            session: String::new(),
            radio_started_pending: false,
            deferred: Vec::new(),
            played_station: String::new(),
            wave_restored: false,
            clone_pending: false,
            tune: None,
            resume_at: None,
            ynison_due: None,
            ynison_at: Instant::now(),
            restore_last: None,
            listen: None,
            fetching: false,
            batch_gen: 0,
            retry_at: None,
            account_retry: None,
            gen: 0,
            loaded: false,
            paused: true,
            volume: 0.6,
            loudness: Loudness::Square,
            repeat: Repeat::Off,
            shuffle: false,
            prefetch: None,
            prefetching: None,
            errors_in_row: 0,
            was_playing: false,
            handled_seek: 0,
            fade_out: Arc::default(),
            fading: None,
            crossfade: Duration::ZERO,
            fade_in: Duration::ZERO,
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(msg) => {
                    self.handle(msg);
                    self.sync();
                    (self.notify)();
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.tick();
        }
    }

    // ---------- вспомогательное ----------

    fn sh(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap()
    }

    fn sink(&self) -> Option<&Sink> {
        self.audio.as_ref().map(|(_, s)| s)
    }

    fn ensure_audio(&mut self) -> bool {
        if self.audio.is_none() {
            match open_output() {
                Ok(out) => {
                    let sink = out.sink();
                    self.audio = Some((out, sink));
                    #[cfg(windows)]
                    {
                        self.out_name = default_output_name();
                    }
                }
                Err(e) => log(format!("audio device: {e}")),
            }
        }
        self.audio.is_some()
    }

    fn gain(&self) -> f32 {
        let v = self.volume;
        match self.loudness {
            Loudness::Square => v * v,
            Loudness::Decibels if v <= 0.0 => 0.0,
            Loudness::Decibels => 10f32.powf((v - 1.0) * 50.0 / 20.0),
        }
    }

    fn apply_volume(&self) {
        let gain = self.gain();
        for s in self.sink().into_iter().chain(&self.fading) {
            s.set_volume(gain);
        }
    }

    /// Источник для звука: эквалайзер, декодирование наперёд и затухание для кроссфейда.
    fn source(&mut self, src: Decoder<Reader>, fade_in: Duration) -> impl rodio::Source<Item = f32> + Send + 'static {
        self.fade_out = Arc::default();
        let rate = self.audio.as_ref().map_or_else(|| src.sample_rate(), |(out, _)| out.sample_rate());
        let eq = Eq::new(src.convert_samples::<f32>(), self.eq.clone());
        Fade::new(Ahead::new(eq, rate), fade_in, self.fade_out.clone())
    }

    /// Пора ли начать кроссфейд: до конца трека осталось его время и следующий трек уже скачан.
    fn crossfade_due(&self) -> bool {
        if self.crossfade.is_zero() || !self.loaded || self.paused || self.fading.is_some() || self.repeat == Repeat::One {
            return false;
        }
        let next_ready = match (self.queue.front(), &self.prefetch) {
            (Some(next), Some((id, _))) => *id == next.id,
            _ => false,
        };
        let fade = self.crossfade.as_secs_f32();
        let duration = self.sh().duration;
        next_ready && duration > 3.0 * fade && duration - self.pos() <= fade
    }

    /// Прошлый трек уходит в отдельный звуковой поток и затухает, а следующий начинается в новом.
    fn start_crossfade(&mut self) {
        let Some((out, sink)) = &mut self.audio else { return };
        let old = std::mem::replace(sink, out.sink());
        self.fade_out.start(self.crossfade);
        self.fade_in = self.crossfade;
        // advance() глушит затухающий поток через stop_audio(), поэтому он отдаётся после.
        self.advance(End::Finished);
        self.fading = Some(old);
    }

    fn pos(&self) -> f32 {
        match self.sink() {
            Some(s) => s.get_pos().as_secs_f32(),
            None => self.held_pos.unwrap_or(0.0),
        }
    }

    /// Звук открыт на одном устройстве и сам за выходом Windows не переходит. Раз в секунду
    /// сверяем выход по умолчанию: сменился или пропал — переоткрываем.
    #[cfg(windows)]
    fn check_output(&mut self) {
        if self.audio.is_none() || self.out_checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.out_checked = Instant::now();
        let now = default_output_name();
        if now == self.out_name {
            return;
        }
        log(format!("аудиовыход: {} → {}", self.out_name.as_deref().unwrap_or("нет"), now.as_deref().unwrap_or("нет")));
        let pos = self.pos();
        self.reopen_output(pos, now.is_some());
    }

    /// Трек играет и скачан целиком, а позиция стоит: выход умер молча. На Linux так бывает
    /// после блокировки экрана (#49).
    fn check_stall(&mut self) {
        let pos = self.pos();
        let playing = self.loaded
            && !self.paused
            && self.current_buf.as_ref().is_some_and(|b| b.done())
            && self.sink().is_some_and(|s| !s.empty());
        if !playing || pos != self.stall.0 {
            self.stall = (pos, Instant::now());
            return;
        }
        if self.stall.1.elapsed() < STALL_LIMIT {
            return;
        }
        log(format!("звук встал на {pos:.1} с, выход переоткрывается"));
        self.stall = (pos, Instant::now());
        self.reopen_output(pos, true);
    }

    /// Поток выхода сообщил об ошибке: закрываем его, пока ALSA не крутит ошибку без конца (#82).
    fn check_broken(&mut self) {
        if !self.audio.as_ref().is_some_and(|(out, _)| out.broken()) {
            return;
        }
        let again = self.broke_at.is_some_and(|at| at.elapsed() < BROKEN_AGAIN);
        self.broke_at = Some(Instant::now());
        let pos = self.pos();
        log(if again { "выход сломался снова, пауза до Play" } else { "выход сломался, переоткрывается" });
        self.reopen_output(pos, !again);
    }

    /// Закрыть выход и продолжить трек с `pos` на новом. Не вышло — пауза до Play с того же места.
    fn reopen_output(&mut self, pos: f32, available: bool) {
        self.audio = None;
        self.fading = None;
        #[cfg(windows)]
        {
            self.out_name = None;
        }
        if !self.loaded {
            return;
        }
        if !available || !self.restore_at(pos) {
            self.held_pos = Some(pos);
            if !self.paused {
                self.paused = true;
                if let Some(l) = &mut self.listen {
                    l.stop();
                    l.pause = true;
                }
            }
            (self.notify)();
        }
    }

    /// Открыть выход заново и продолжить текущий трек из скачанного буфера с места `pos`.
    fn restore_at(&mut self, pos: f32) -> bool {
        let Some(buf) = self.current_buf.clone() else { return false };
        if !self.ensure_audio() {
            return false;
        }
        let Ok(src) = Decoder::new(buf.reader()) else { return false };
        let src = self.source(src, Duration::ZERO);
        let (vol, paused) = (self.gain(), self.paused);
        let Some(sink) = self.sink() else { return false };
        sink.pause();
        sink.set_volume(vol);
        sink.append(src);
        if let Err(e) = sink.try_seek(Duration::from_secs_f32(pos)) {
            log(format!("seek: {e:?}"));
        }
        if !paused {
            sink.play();
        }
        self.held_pos = None;
        true
    }

    fn error(&self, e: impl Into<String>) {
        let e = e.into();
        log(format!("error: {e}"));
        self.sh().error = Some((e, Instant::now()));
        (self.notify)();
    }

    fn feedback(&mut self, kind: &'static str, track: Option<&Track>, played: Option<f64>) {
        self.send_feedback(kind, track, played, true);
    }

    /// `from` волны, как у оригинала: «Моя волна» с главной или волна по исполнителю, альбому…
    fn rotor_from(&self) -> String {
        match &self.source {
            Source::Seed { station, .. } => format!("desktop-radio-{}-default", station.replace(':', "-")),
            _ => WAVE_FROM.to_string(),
        }
    }

    /// Фидбек волне. Пропуск, дизлайк и «дослушал» при `defer` ждут следующего запроса треков
    /// и уходят вместе с ним, как у оригинала; «назад» он отправляет сразу.
    fn send_feedback(&mut self, kind: &'static str, track: Option<&Track>, played: Option<f64>, defer: bool) {
        let Some(station) = self.source.station().map(String::from) else { return };
        let Some(api) = self.api.clone() else { return };
        if self.session.is_empty() && kind == "radioStarted" {
            self.radio_started_pending = true;
            return;
        }
        // Фидбек идёт с пакетом самого трека: следующий пакет часто приходит, пока доигрывает прошлый.
        let batch = track
            .map(|t| t.batch_id.clone())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| self.batch_id.clone());
        let length = track
            .filter(|t| kind == "trackFinished" && t.duration_ms > 0)
            .map(|t| t.duration_ms as f64 / 1000.0);
        let track = track.map(|t| t.full_id());
        let session = self.session.clone();
        let from = self.rotor_from();
        if session.is_empty() {
            let _ = self.bg.send(Box::new(move || {
                if let Err(e) = api.feedback(&station, kind, track.as_deref(), played, &batch, &from) {
                    log(format!("feedback {kind}: {e}"));
                }
            }));
            return;
        }
        let body = crate::api::session_event(kind, track.as_deref(), played, length, &batch, &from);
        if defer && matches!(kind, "skip" | "dislike" | "trackFinished") {
            self.deferred.push(body);
            return;
        }
        let _ = self.bg.send(Box::new(move || {
            if let Err(e) = api.session_feedback(&session, &body) {
                log(format!("feedback {kind}: {e}"));
            }
        }));
    }

    /// Отложенные фидбеки, которые не ушли с запросом треков, отправляются отдельно.
    fn flush_feedback(&mut self) {
        if self.deferred.is_empty() {
            return;
        }
        let list = std::mem::take(&mut self.deferred);
        let (Some(api), false) = (self.api.clone(), self.session.is_empty()) else { return };
        let session = self.session.clone();
        let _ = self.bg.send(Box::new(move || {
            for body in list {
                if let Err(e) = api.session_feedback(&session, &body) {
                    log(format!("feedback: {e}"));
                }
            }
        }));
    }

    /// Контекст `/plays` для трека, который сейчас заиграл.
    fn play_context(&self) -> PlayContext {
        let radio = |from: String, item: &str| PlayContext {
            from,
            context: "radio",
            item: Some(item.to_string()),
            playlist: None,
            session: self.session.clone(),
        };
        let list = |from: &str, context: &'static str, item: Option<String>, playlist: Option<String>| PlayContext {
            from: from.to_string(),
            context,
            item,
            playlist,
            session: String::new(),
        };
        match &self.source {
            Source::Wave => radio(WAVE_FROM.to_string(), WAVE),
            Source::Seed { station, .. } => radio(self.rotor_from(), station),
            Source::List { kind, id, .. } => match kind {
                ListKind::Album => list("desktop-album-album-default", "album", Some(id.clone()), None),
                ListKind::Artist => list("desktop-artist-artist-default", "artist", Some(id.clone()), None),
                ListKind::Playlist => list("desktop-playlist-playlist-default", "playlist", Some(id.clone()), Some(id.clone())),
                ListKind::Likes => {
                    let likes = format!("{}:3", self.uid);
                    list("desktop-playlist-playlist-default", "playlist", Some(likes.clone()), Some(likes))
                }
                ListKind::Tracks if id == "search" => list("desktop-search-search-default", "search", None, None),
                ListKind::Tracks => list("desktop-other-other-default", "other", None, None),
            },
        }
    }

    /// Трек стал текущим: новое прослушивание. Время слушания идёт, только когда трек звучит.
    fn begin_listen(&mut self, track: &Track, repeated: bool) {
        let ctx = self.play_context();
        let radio = ctx.context == "radio";
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        self.listen = Some(Listen {
            track: track.clone(),
            ctx,
            play_id: uuid4(),
            add_time: format!("{:016}-{}", random_u128() % 10_000_000_000_000_000, ms.div_ceil(1000)),
            start: crate::api::iso_now(),
            played: 0.0,
            since: (self.loaded && !self.paused).then(Instant::now),
            pause: false,
            // В волне оригинал всегда шлёт seek: при старте трека плеер ставит его на начало.
            seek: radio,
            repeated,
        });
    }

    /// Трек сменился: фидбек волне (если нужен) и запись в `/plays`, как у оригинала.
    /// Второй раз для того же прослушивания ничего не шлёт.
    fn end_current(&mut self, fb: Option<&'static str>, defer: bool, reason: &'static str) {
        let Some(mut l) = self.listen.take() else { return };
        l.stop();
        let len = l.track.duration_ms as f64 / 1000.0;
        let near = |x: f64| if (len - x).abs() < 0.5 { len } else { round3(x) };
        let played = near(l.played());
        let end = if reason == "finish" { len } else { near(self.pos() as f64) };
        if let Some(kind) = fb {
            let t = l.track.clone();
            self.send_feedback(kind, Some(&t), Some(played), defer);
        }
        let (Some(api), false) = (self.api.clone(), self.uid.is_empty()) else { return };
        let t = &l.track;
        let mut play = serde_json::json!({
            "playId": l.play_id,
            "from": l.ctx.from,
            "totalPlayedSeconds": played,
            "endPositionSeconds": end,
            "trackLengthSeconds": len,
            "timestamp": crate::api::iso_now(),
            "context": l.ctx.context,
            "contextItem": l.ctx.item,
            "addTracksToPlayerTime": l.add_time,
            "fromCache": false,
            "isRestored": false,
            "audioAuto": "none",
            "audioOutputName": "Динамик",
            "audioOutputType": "Speaker",
            "trackId": t.id,
            "seek": l.seek,
            "pause": l.pause,
            "startTimestamp": l.start,
            "maxPlayerStage": "play",
            "isRepeated": l.repeated,
            "changeReason": reason,
            "isLivePlayableIndex": l.ctx.context == "radio",
        });
        if let Some(a) = &t.album_id {
            play["albumId"] = a.clone().into();
        }
        if let Some(p) = &l.ctx.playlist {
            play["playlistId"] = p.clone().into();
        }
        if l.ctx.context == "radio" {
            let batch = if t.batch_id.is_empty() { self.batch_id.clone() } else { t.batch_id.clone() };
            play["radioSessionId"] = l.ctx.session.clone().into();
            play["batchId"] = batch.into();
            play["isFromAutoflow"] = false.into();
            play["isFromPumpkin"] = false.into();
        }
        let uid = self.uid.clone();
        let _ = self.bg.send(Box::new(move || {
            if let Err(e) = api.plays(&uid, &play) {
                log(format!("plays: {e}"));
            }
        }));
    }

    /// queue для следующего пакета, как у оригинала: трек, на котором сменился трек, и тот,
    /// что заиграл после него. Всю историю оригинал туда не кладёт.
    fn rotor_queue(&self) -> Vec<String> {
        let mut q: Vec<String> = self.history.last().filter(|t| !t.batch_id.is_empty()).map(|t| t.full_id()).into_iter().collect();
        if let Some(c) = &self.current {
            q.push(c.full_id());
        }
        q
    }

    fn stop_audio(&mut self) {
        if let Some(s) = self.sink() {
            s.clear();
        }
        self.fading = None;
        self.loaded = false;
        self.held_pos = None;
    }

    fn clear_queue(&mut self) {
        self.flush_feedback();
        self.queue.clear();
        self.returned = 0;
        self.prefetch = None;
        self.prefetching = None;
        self.fetching = false;
        self.batch_gen += 1;
        self.retry_at = None;
        self.batch_id.clear();
        self.session.clear();
    }

    fn reset(&mut self) {
        self.stop_audio();
        self.gen += 1;
        self.clear_queue();
        self.history.clear();
        self.listen = None;
        self.played_station.clear();
        self.wave_restored = false;
        self.clone_pending = false;
        self.restore_last = None;
        self.current = None;
        self.current_buf = None;
        self.list = None;
        self.source = Source::Wave;
        let mut s = self.sh();
        s.track = None;
        s.source = Source::Wave;
        s.loading = false;
    }

    fn set_source(&mut self, src: Source) {
        // Повтора списка в волне нет: оригинал снимает его, когда включается волна, а повтор
        // трека оставляет.
        if src.station().is_some() && self.repeat == Repeat::All {
            self.repeat = Repeat::Off;
        }
        self.source = src.clone();
        self.sh().source = src;
    }

    /// Состояние движка → Shared для UI.
    fn sync(&mut self) {
        let playing = self.loaded && !self.paused && self.sink().map(|s| !s.is_paused()).unwrap_or(false);
        let pos = if self.loaded { self.pos() } else { 0.0 };
        let upcoming: Vec<Track> = self.queue.iter().take(30).cloned().collect();
        let mut s = self.shared.lock().unwrap();
        s.playing = playing;
        if s.seek_request == self.handled_seek || s.track.is_none() {
            s.position = pos;
        }
        s.volume = self.volume;
        s.repeat = self.repeat;
        s.shuffle = self.shuffle;
        s.upcoming = upcoming;
    }

    // ---------- главный цикл ----------

    fn tick(&mut self) {
        #[cfg(windows)]
        self.check_output();
        self.check_broken();
        self.check_stall();
        // Пока играет, позиция уходит в Ynison раз в минуту: закроют приложение — потеряется не больше.
        let playing = self.loaded && !self.paused;
        if self.ynison_due.is_some_and(|at| Instant::now() >= at) || (playing && self.ynison_at.elapsed() >= YNISON_EVERY) {
            self.publish();
        }
        if let Some(at) = self.retry_at {
            if Instant::now() >= at {
                self.retry_at = None;
                self.fetch_batch();
            }
        }
        if let (Some((at, n)), Some(api)) = (self.account_retry, self.api.clone()) {
            if Instant::now() >= at {
                // Повтор уже в пути: следующий срок назначит его ответ.
                self.account_retry = Some((Instant::now() + Duration::from_secs(3600), n));
                self.sh().status = Status::Connecting;
                let tx = self.tx.clone();
                thread::spawn(move || {
                    let _ = tx.send(Msg::Account(api.account()));
                });
            }
        }
        if self.fading.as_ref().is_some_and(|s| s.empty()) {
            self.fading = None;
        }
        if self.crossfade_due() {
            self.start_crossfade();
            self.sync();
            (self.notify)();
            return;
        }
        if self.loaded && !self.paused && self.sink().map(|s| s.empty()).unwrap_or(false) {
            if self.repeat == Repeat::One && self.current_buf.is_some() {
                if let Some(l) = &mut self.listen {
                    l.repeated = true;
                }
                self.end_current(None, true, "finish");
                if self.replay() {
                    if let Some(t) = self.current.clone() {
                        self.begin_listen(&t, false);
                    }
                    return;
                }
            }
            self.advance(End::Finished);
            self.sync();
            (self.notify)();
            return;
        }
        self.sync();
        if self.loaded && self.prefetch.is_none() && self.prefetching.is_none() {
            self.prefetch_next();
        }
        let playing = self.sh().playing;
        if playing != self.was_playing {
            self.was_playing = playing;
            (self.notify)();
        }
    }

    fn replay(&mut self) -> bool {
        let Some(buf) = self.current_buf.clone() else { return false };
        let Ok(src) = Decoder::new(buf.reader()) else { return false };
        let src = self.source(src, Duration::ZERO);
        if let Some(sink) = self.sink() {
            sink.append(src);
            sink.play();
        }
        true
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Cmd(c) => self.cmd(c),
            Msg::Seek { seconds, track_generation, request } => {
                if self.gen == track_generation && self.sh().seek_request == request {
                    self.cmd(Cmd::Seek(seconds));
                }
                self.handled_seek = request;
            }
            Msg::Account(res) => match res {
                Ok(acc) => {
                    self.account_retry = None;
                    self.uid = acc.uid.clone();
                    {
                        let mut s = self.sh();
                        s.status = Status::Ready;
                        s.uid = acc.uid.clone();
                        s.login = acc.login;
                        s.has_plus = acc.has_plus;
                        s.error = None;
                    }
                    self.feedback("radioStarted", None, None);
                    if let Some(api) = self.api.clone() {
                        let tx = self.tx.clone();
                        // Волну качаем, когда станет ясно, не продолжить ли то, что играло.
                        let (resume_api, resume_tx) = (api.clone(), tx.clone());
                        thread::spawn(move || {
                            let _ = resume_tx.send(Msg::Resume(fetch_resume(&resume_api)));
                        });
                        let uid = acc.uid;
                        thread::spawn(move || {
                            let ids = api.liked_ids(&uid);
                            let _ = tx.send(Msg::Likes(uid.clone(), ids));
                            let ids = api.disliked_ids(&uid);
                            let _ = tx.send(Msg::Dislikes(uid, ids));
                        });
                    }
                }
                // Сеть пропала на входе (таймаут, нет соединения): вход повторяется сам, иначе
                // клиент остался бы без волны и запросов до перезапуска. Ошибка показывается
                // один раз, дальше — «подключение». Отказ сервера (токен и т. п.) — как раньше.
                Err(e) if e.starts_with("сеть") && self.api.is_some() => {
                    log(format!("account: {e}"));
                    let n = self.account_retry.map_or(0, |(_, n)| n) + 1;
                    self.account_retry = Some((Instant::now() + account_retry_delay(n), n));
                    self.sh().status = if n == 1 { Status::Failed(e) } else { Status::Connecting };
                }
                Err(e) => {
                    log(format!("account: {e}"));
                    self.account_retry = None;
                    self.api = None;
                    self.sh().status = Status::Failed(e);
                }
            },
            Msg::Resume(res) => match res {
                Ok(Some(r)) if self.paused && self.current.is_none() => self.apply_resume(r),
                Ok(_) => self.fetch_batch(),
                Err(e) => {
                    log(format!("продолжение: {e}"));
                    self.fetch_batch();
                }
            },
            Msg::Likes(uid, _) if uid != self.uid => {}
            Msg::Likes(_, res) => match res {
                Ok(ids) => self.merge_reactions(ids, Reaction::Like),
                Err(e) => log(format!("likes: {e}")),
            },
            Msg::Dislikes(uid, _) if uid != self.uid => {}
            Msg::Dislikes(_, res) => match res {
                Ok(ids) => self.merge_reactions(ids, Reaction::Dislike),
                Err(e) => log(format!("dislikes: {e}")),
            },
            Msg::ReactionFinished { uid, id, seq, kind, remove, result } => {
                if uid != self.uid { return; }
                let Some(write) = self.reaction_writes.get_mut(&id) else { return };
                if seq < write.first { return; }
                if result.is_ok() {
                    write.saved = kind.applied(write.saved, remove);
                    write.confirmed = true;
                }
                if seq == write.latest {
                    let saved = write.saved;
                    if !write.confirmed { self.reaction_writes.remove(&id); }
                    self.show_reaction(&id, saved);
                }
                if let Err(e) = result {
                    let name = if kind == Reaction::Like { "Лайк" } else { "Дизлайк" };
                    self.error(format!("{name} не сохранился: {e}"));
                }
            }
            Msg::Batch { res, replace, gen } => {
                if gen != self.batch_gen {
                    return;
                }
                self.fetching = false;
                match res {
                    Ok(b) => self.on_batch(b, replace),
                    Err(e) => {
                        self.error(format!("Не удалось получить треки волны: {e}"));
                        self.retry_at = Some(Instant::now() + Duration::from_secs(5));
                    }
                }
            }
            Msg::Audio { gen, res } => {
                if gen == self.gen {
                    match res {
                        Ok(b) => self.play_loaded(b),
                        Err(e) => self.on_load_error(e),
                    }
                }
            }
            Msg::Prefetch { id, res } => {
                if self.prefetching.as_deref() == Some(id.as_str()) {
                    self.prefetching = None;
                }
                let waiting = !self.loaded && self.current.as_ref().map(|t| t.id == id).unwrap_or(false);
                match res {
                    Ok(b) if waiting => self.play_loaded(b),
                    Ok(b) => self.prefetch = Some((id, b)),
                    Err(e) => {
                        log(format!("prefetch {id}: {e}"));
                        if waiting {
                            self.start_download();
                        }
                    }
                }
            }
        }
    }

    fn cmd(&mut self, c: Cmd) {
        match c {
            Cmd::Login(token) => {
                self.account_retry = None;
                self.reset();
                self.reaction_writes.clear();
                self.dislikes.lock().unwrap().clear();
                self.sh().disliked = false;
                self.likes.lock().unwrap().clear();
                self.likes_changed();
                let api = Api::new(&token);
                self.api = Some(api.clone());
                {
                    let mut s = self.sh();
                    s.status = Status::Connecting;
                    s.error = None;
                }
                let tx = self.tx.clone();
                thread::spawn(move || {
                    let _ = tx.send(Msg::Account(api.account()));
                });
            }
            Cmd::Logout => {
                self.account_retry = None;
                self.reset();
                self.api = None;
                self.uid.clear();
                self.likes.lock().unwrap().clear();
                self.reaction_writes.clear();
                self.dislikes.lock().unwrap().clear();
                self.sh().disliked = false;
                self.likes_changed();
                let mut s = self.sh();
                s.status = Status::LoggedOut;
                s.login.clear();
                s.uid.clear();
            }
            Cmd::Resume => self.resume(),
            Cmd::Leave(done) => {
                if let (Some(api), Some(now)) = (self.api.clone(), self.ynison_now()) {
                    if let Err(e) = crate::ynison::publish(api.token(), &crate::ynison::device_id(), &now) {
                        log(format!("ynison: {e}"));
                    }
                }
                let _ = done.send(());
            }
            Cmd::PlayPause if self.paused => self.resume(),
            Cmd::PlayPause => {
                self.paused = true;
                if let Some(l) = &mut self.listen {
                    l.stop();
                    l.pause = true;
                }
                self.fading = None;
                // На паузе AAudio-поток остаётся запущенным, и Galaxy Buds не шлют PLAY, когда
                // наушник возвращают в ухо (#96). Выход закрываем, продолжим с того же места.
                #[cfg(target_os = "android")]
                {
                    if self.loaded {
                        self.held_pos = Some(self.pos());
                    }
                    self.audio = None;
                }
                #[cfg(not(target_os = "android"))]
                if let Some(sink) = self.sink() { sink.pause(); }
                self.publish_soon();
            }
            Cmd::Next => {
                self.errors_in_row = 0;
                if self.current.is_some() {
                    self.advance(End::Skip);
                }
            }
            Cmd::Prev => self.prev(),
            Cmd::Seek(sec) => {
                if let Some(l) = &mut self.listen {
                    l.seek = true;
                }
                self.fading = None;
                if self.loaded {
                    if let Some(s) = self.sink() {
                        if let Err(e) = s.try_seek(Duration::from_secs_f32(sec.max(0.0))) {
                            log(format!("seek: {e:?}"));
                        }
                    } else if self.held_pos.is_some() {
                        self.held_pos = Some(sec.max(0.0));
                    }
                }
                self.publish_soon();
            }
            Cmd::Volume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                self.apply_volume();
            }
            Cmd::Loudness(l) => {
                self.loudness = l;
                self.apply_volume();
            }
            Cmd::LikeTrack(t, saved) => self.toggle_like(t, saved),
            Cmd::Dislike(done) => {
                let Some(track) = self.current.clone() else { return };
                let remove = self.dislikes.lock().unwrap().contains(&track.id);
                self.write_reaction(&track, Reaction::Dislike, remove, done);
                if !remove { self.advance(End::Dislike); }
            }
            Cmd::DislikeTrack(track, done) => {
                let remove = self.dislikes.lock().unwrap().contains(&track.id);
                self.write_reaction(&track, Reaction::Dislike, remove, done);
            }
            Cmd::Wave(src) => self.start_wave(src, true),
            Cmd::TuneWave(settings) => {
                self.start_wave(Source::Wave, false);
                self.session.clear();
                self.clone_pending = false;
                self.restore_last = None;
                // Сессию с прошлого запуска уже не подхватывать: волна начинается заново.
                self.wave_restored = true;
                self.tune = Some(settings);
                self.fetch_batch_with(true);
            }
            Cmd::PlayList { source, tracks, start, after } => {
                if tracks.is_empty() {
                    return;
                }
                // Играть нечего: текущий трек не трогаем, а говорим, почему ничего не включилось.
                if !crate::api::allow_explicit() && tracks.iter().all(|t| t.explicit) {
                    self.error("Здесь только треки 18+. Их можно включить в меню профиля");
                    return;
                }
                self.end_current(Some("skip"), true, "other_track");
                self.clear_queue();
                let start = start.min(tracks.len() - 1);
                let mut order: Vec<Track> = tracks.clone();
                order.rotate_left(start);
                let first = order.remove(0);
                if self.shuffle {
                    shuffle(&mut order);
                }
                self.queue.push_back(first);
                self.queue.extend(order);
                self.list = Some(ListPlay { deferred: VecDeque::new(), all: tracks, after });
                self.set_source(source);
                self.advance(End::Skip);
            }
            Cmd::Jump(i) => {
                if i >= self.queue.len() {
                    return;
                }
                let skipped: Vec<Track> = self.queue.drain(..i).collect();
                self.end_current(Some("skip"), true, "other_track");
                if let Some(cur) = self.current.take() {
                    self.history.push(cur);
                }
                self.history.extend(skipped);
                self.trim_history();
                self.stop_audio();
                self.play_from_queue();
                self.refresh_wave();
                self.flush_feedback();
            }
            Cmd::Repeat(r) => self.repeat = r,
            Cmd::Enqueue { tracks, next } => {
                if next {
                    for t in tracks.into_iter().rev() {
                        self.queue.push_front(t);
                    }
                    self.prefetch = None;
                    if self.loaded {
                        self.prefetch_next();
                    }
                } else {
                    self.queue.extend(tracks);
                }
            }
            Cmd::AllowExplicit(allow) => self.set_allow_explicit(allow),
            Cmd::Crossfade(over) => self.crossfade = over,
            Cmd::Shuffle(on) => {
                self.shuffle = on;
                if self.list.is_some() {
                    let mut v: Vec<Track> = self.queue.drain(..).collect();
                    if on {
                        shuffle(&mut v);
                    } else if let (Some(l), Some(cur)) = (&self.list, &self.current) {
                        let i = l.all.iter().position(|t| t.id == cur.id).map(|i| i + 1).unwrap_or(0);
                        v = l.all[i.min(l.all.len())..].to_vec();
                    }
                    self.queue.extend(v);
                    self.prefetch = None;
                    if self.loaded {
                        self.prefetch_next();
                    }
                }
            }
        }
    }

    fn likes_changed(&self) {
        let mut s = self.sh();
        s.likes_revision = s.likes_revision.wrapping_add(1);
    }

    fn merge_reactions(&mut self, ids: Vec<String>, kind: Reaction) {
        let mut likes = self.likes.lock().unwrap();
        let mut changed = false;
        for id in ids {
            if let Some(write) = self.reaction_writes.get_mut(&id) {
                // Пока первый запрос ещё выполняется, снимок уточняет состояние для отката.
                if !write.confirmed { write.saved = Some(kind); }
                continue;
            }
            match kind {
                Reaction::Like => { self.dislikes.lock().unwrap().remove(&id); changed |= likes.insert(id); }
                Reaction::Dislike => { changed |= likes.remove(&id); self.dislikes.lock().unwrap().insert(id); }
            }
        }
        drop(likes);
        if changed { self.likes_changed(); }
        self.sh().disliked = self.current.as_ref().is_some_and(|t| self.dislikes.lock().unwrap().contains(&t.id));
    }

    fn show_reaction(&mut self, id: &str, state: Option<Reaction>) {
        let mut likes = self.likes.lock().unwrap();
        let changed = if state == Some(Reaction::Like) { likes.insert(id.to_string()) } else { likes.remove(id) };
        drop(likes);
        if changed { self.likes_changed(); }
        let disliked = state == Some(Reaction::Dislike);
        if disliked { self.dislikes.lock().unwrap().insert(id.to_string()); } else { self.dislikes.lock().unwrap().remove(id); }
        if self.current.as_ref().is_some_and(|t| t.id == id) { self.sh().disliked = disliked; }
    }

    fn write_reaction(&mut self, track: &Track, kind: Reaction, remove: bool, done: Option<LikeCompleted>) {
        let Some(api) = self.api.clone() else { return };
        let old = if self.likes.lock().unwrap().contains(&track.id) { Some(Reaction::Like) }
            else if self.dislikes.lock().unwrap().contains(&track.id) { Some(Reaction::Dislike) } else { None };
        self.reaction_seq += 1;
        let seq = self.reaction_seq;
        self.reaction_writes.entry(track.id.clone()).or_insert(ReactionWrite {
            first: seq, latest: seq, saved: old, confirmed: false,
        }).latest = seq;
        self.show_reaction(&track.id, kind.applied(old, remove));
        let (uid, track, tx) = (self.uid.clone(), track.clone(), self.tx.clone());
        let _ = self.bg.send(Box::new(move || {
            let id = track.full_id();
            let result = match kind {
                Reaction::Like => api.like(&uid, &id, remove),
                Reaction::Dislike => api.dislike(&uid, &id, remove),
            };
            if let Some(done) = done { done(&track, !remove, &result); }
            let _ = tx.send(Msg::ReactionFinished { uid, id: track.id, seq, kind, remove, result });
        }));
    }

    fn toggle_like(&mut self, t: Track, done: Option<LikeCompleted>) {
        let remove = self.likes.lock().unwrap().contains(&t.id);
        self.write_reaction(&t, Reaction::Like, remove, done);
        // Лайк трека из волны оригинал сообщает и самой волне, чтобы она его учла.
        if self.session.is_empty() {
            return;
        }
        let in_wave = self
            .current
            .iter()
            .chain(self.queue.iter())
            .chain(self.history.iter())
            .find(|w| w.id == t.id && !w.batch_id.is_empty())
            .cloned();
        if let Some(w) = in_wave {
            self.feedback(if remove { "unlike" } else { "like" }, Some(&w), None);
        }
    }

    // ---------- очередь ----------

    fn start_wave(&mut self, src: Source, fetch: bool) {
        // Пропуск уходит ещё в старую волну: clear_queue отправит его туда.
        self.end_current(Some("skip"), true, "other_track");
        self.clear_queue();
        self.list = None;
        self.set_source(src);
        self.feedback("radioStarted", None, None);
        if fetch {
            self.fetch_batch_with(true);
        }
    }

    fn fetch_batch(&mut self) {
        self.fetch_batch_with(false);
    }

    fn fetch_batch_with(&mut self, replace: bool) {
        if self.fetching || self.list.is_some() {
            return;
        }
        let Some(station) = self.source.station().map(String::from) else { return };
        let Some(api) = self.api.clone() else { return };
        self.fetching = true;
        self.retry_at = None;
        if self.played_station != station {
            self.played_station = station.clone();
            if station == WAVE {
                self.restore_wave();
            }
        }
        let (session, queue) = if let Some(settings) = self.tune.take() {
            (WaveSession::Tuned(settings), Vec::new())
        } else if self.session.is_empty() {
            (WaveSession::New, if replace { Vec::new() } else { self.rotor_queue() })
        } else if self.clone_pending {
            (WaveSession::Clone(self.session.clone()), self.restore_last.clone().into_iter().collect())
        } else {
            (WaveSession::Next(self.session.clone()), self.rotor_queue())
        };
        let feedbacks = std::mem::take(&mut self.deferred);
        let tx = self.tx.clone();
        let gen = self.batch_gen;
        // В очереди фонового потока: запрос уйдёт после уже отправленных фидбеков.
        let _ = self.bg.send(Box::new(move || {
            let res = api.wave_session(&station, session, &queue, &feedbacks);
            let _ = tx.send(Msg::Batch { res, replace, gen });
        }));
    }

    fn on_batch(&mut self, b: Batch, replace: bool) {
        self.batch_id = b.batch_id;
        self.clone_pending = false;
        self.restore_last = None;
        if !b.session.is_empty() {
            // Сессия могла подхватиться с прошлого запуска — radioStarted нужен и ей.
            let changed = b.session != self.session;
            self.session = b.session;
            if std::mem::take(&mut self.radio_started_pending) {
                self.feedback("radioStarted", None, None);
            }
            if changed {
                self.save_wave();
            }
        }
        self.merge_reactions(b.liked, Reaction::Like);
        if replace {
            self.queue.clear();
            self.returned = 0;
            self.prefetch = None;
        }
        // Пакет берётся как есть, как у оригинала: сервер сам решает, что повторить.
        // Убираем только то, что уже стоит в очереди или играет, и 18+, если их выключили.
        let queued = |t: &Track| {
            self.queue.iter().any(|q| q.id == t.id) || (!replace && self.current.as_ref().is_some_and(|c| c.id == t.id))
        };
        let no_explicit = !crate::api::allow_explicit();
        let tracks: Vec<Track> = b.tracks.iter().filter(|t| !queued(t) && !(no_explicit && t.explicit)).cloned().collect();
        self.queue.extend(tracks);
        if replace {
            self.advance(End::Restart);
        } else if self.current.is_none() {
            self.play_from_queue();
        } else if self.loaded {
            self.prefetch_next();
        }
    }

    fn trim_history(&mut self) {
        if self.history.len() > 50 {
            let n = self.history.len() - 50;
            self.history.drain(..n);
        }
    }

    /// Смена трека. Фидбек «дослушал» и «пропуск» ждёт следующего запроса треков и уходит
    /// вместе с ним, как у оригинала.
    fn advance(&mut self, end: End) {
        let (kind, reason) = match end {
            End::Finished => ("trackFinished", "finish"),
            End::Skip => ("skip", "skip"),
            End::Dislike => ("dislike", "dislike"),
            End::Restart => ("skip", "other_track"),
        };
        self.end_current(Some(kind), true, reason);
        if let Some(cur) = self.current.take() {
            self.history.push(cur);
            self.trim_history();
        }
        self.stop_audio();
        self.play_from_queue();
        if matches!(end, End::Skip | End::Dislike) {
            self.refresh_wave();
        }
    }

    /// Пропуск в волне, как у оригинала: заиграл следующий трек, остальные несыгранные треки
    /// волны убираются и сразу просятся новые, а пропуск уходит в том же запросе. Так волна
    /// подстраивается сразу, а не через пакет. Треки, добавленные вручную, остаются.
    fn refresh_wave(&mut self) {
        if self.list.is_some() || self.source.station().is_none() || self.session.is_empty() {
            return;
        }
        let back: Vec<Track> = self.queue.drain(..self.returned.min(self.queue.len())).collect();
        self.queue.retain(|t| t.batch_id.is_empty());
        for t in back.into_iter().rev() {
            self.queue.push_front(t);
        }
        if let Some((id, _)) = &self.prefetch {
            if !self.queue.iter().any(|t| t.id == *id) {
                self.prefetch = None;
            }
        }
        self.fetch_batch();
    }

    /// Список закончился: по кругу (исполнитель) или волна по нему (плейлист, альбом).
    fn list_finished(&mut self) {
        let Some(l) = self.list.take() else { return };
        match l.after {
            After::Loop => {
                let mut all = l.all.clone();
                if self.shuffle {
                    shuffle(&mut all);
                }
                self.queue.extend(all);
                self.list = Some(l);
            }
            After::Wave(src) => {
                self.flush_feedback();
                self.set_source(src);
                self.session.clear();
                self.feedback("radioStarted", None, None);
            }
        }
    }

    /// Как у оригинала при смене настроек волны: текущий трек доигрывает (если он не 18+, а их
    /// выключили), а несыгранные треки волны заменяет новая сессия уже с этим флагом.
    fn set_allow_explicit(&mut self, allow: bool) {
        crate::api::ALLOW_EXPLICIT.store(allow, std::sync::atomic::Ordering::Relaxed);
        let wave = self.list.is_none() && self.source.station().is_some();
        let skip_current = !allow && self.current.as_ref().is_some_and(|t| t.explicit);
        if wave {
            self.queue.retain(|t| t.batch_id.is_empty());
        }
        if !allow {
            if self.blocked_list() {
                if skip_current {
                    self.queue.push_front(self.current.as_ref().unwrap().clone());
                }
                self.defer_explicit();
            } else {
                self.queue.retain(|t| !t.explicit);
            }
        }
        self.prefetch = None;
        if skip_current {
            self.end_current(Some("skip"), true, "skip");
            if let Some(cur) = self.current.take() {
                self.history.push(cur);
                self.trim_history();
            }
            self.stop_audio();
        }
        if wave {
            self.flush_feedback();
            self.session.clear();
            self.fetching = false;
            self.batch_gen += 1;
        }
        if skip_current {
            self.play_from_queue();
        }
        if wave {
            self.fetch_batch();
        }
    }

    fn blocked_list(&self) -> bool {
        !crate::api::allow_explicit()
            && self.list.as_ref().is_some_and(|list| !list.all.is_empty() && list.all.iter().all(|t| t.explicit))
    }

    fn defer_explicit(&mut self) {
        if let Some(list) = &mut self.list {
            self.queue.retain(|track| {
                if track.explicit { list.deferred.push_back(track.clone()); }
                !track.explicit
            });
        }
    }

    fn resume(&mut self) {
        self.paused = false;
        self.publish_soon();
        if let Some(pos) = self.held_pos.filter(|_| self.loaded && self.audio.is_none()) {
            if !self.restore_at(pos) {
                self.paused = true;
                self.error(NO_OUTPUT);
                return;
            }
        }
        let loaded = self.loaded;
        if let Some(l) = self.listen.as_mut().filter(|_| loaded) {
            l.go();
        }
        if self.current.is_none() {
            if self.list.is_some() {
                self.play_from_queue();
                return;
            }
            if self.sh().status == Status::Ready { self.fetch_batch(); }
        }
        if let Some(sink) = self.sink() { sink.play(); }
    }

    fn play_from_queue(&mut self) {
        let blocked_list = self.blocked_list();
        if blocked_list {
            self.defer_explicit();
        } else if crate::api::allow_explicit() {
            if let Some(list) = &mut self.list { self.queue.append(&mut list.deferred); }
        } else {
            self.queue.retain(|t| !t.explicit);
        }
        if self.queue.is_empty() && self.list.is_some() && !blocked_list {
            match (&self.list, self.repeat) {
                (Some(l), Repeat::All) => {
                    let all = l.all.clone();
                    self.queue.extend(all);
                }
                _ => self.list_finished(),
            }
        }
        if !crate::api::allow_explicit() {
            self.queue.retain(|t| !t.explicit);
        }
        self.returned = self.returned.saturating_sub(1);
        match self.queue.pop_front() {
            Some(t) => {
                self.current = Some(t);
                self.start_current();
            }
            None => {
                self.current = None;
                self.current_buf = None;
                if self.list.is_some() { self.paused = true; }
                self.gen += 1;
                let mut s = self.sh();
                s.track = None;
                s.loading = self.list.is_none() && self.source.station().is_some();
            }
        }
        // Как у оригинала: новый пакет волны нужен, когда заиграл последний трек очереди.
        if self.queue.is_empty() {
            self.fetch_batch();
        }
    }

    fn prev(&mut self) {
        if self.loaded && self.pos() > 4.0 {
            if let Some(s) = self.sink() {
                let _ = s.try_seek(Duration::ZERO);
            }
            return;
        }
        if !crate::api::allow_explicit() {
            self.history.retain(|t| !t.explicit);
        }
        if let Some(p) = self.history.pop() {
            self.end_current(Some("skip"), false, "back_skip");
            if let Some(c) = self.current.take() {
                self.queue.push_front(c);
                self.returned += 1;
            }
            self.stop_audio();
            self.current = Some(p);
            self.start_current();
        } else if self.loaded {
            if let Some(s) = self.sink() {
                let _ = s.try_seek(Duration::ZERO);
            }
        }
    }

    // ---------- загрузка ----------

    fn start_current(&mut self) {
        self.gen += 1;
        self.loaded = false;
        self.listen = None;
        self.current_buf = None;
        let Some(t) = self.current.clone() else { return };
        // Как у оригинала, трек начат сразу, а не когда докачался: иначе пропуск до загрузки
        // не доходит до волны, и она отдаёт этот трек снова.
        self.begin_listen(&t, false);
        self.feedback("trackStarted", Some(&t), None);
        {
            let mut s = self.sh();
            s.track_generation = self.gen;
            s.disliked = self.dislikes.lock().unwrap().contains(&t.id);
            s.track = Some(t.clone());
            s.position = 0.0;
            s.duration = t.duration_ms as f32 / 1000.0;
            s.loading = true;
        }
        if self.prefetch.as_ref().map(|(id, _)| *id == t.id).unwrap_or(false) {
            let (_, b) = self.prefetch.take().unwrap();
            self.play_loaded(b);
            return;
        }
        if self.prefetching.as_deref() == Some(t.id.as_str()) {
            return;
        }
        self.start_download();
    }

    fn start_download(&mut self) {
        let (Some(api), Some(t)) = (self.api.clone(), self.current.clone()) else { return };
        let gen = self.gen;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let res = load_track(&api, &t);
            let _ = tx.send(Msg::Audio { gen, res });
        });
    }

    fn prefetch_next(&mut self) {
        // Следующий трек качаем под конец текущего: не держать в памяти два файла всё время.
        let near_end = {
            let s = self.shared.lock().unwrap();
            s.duration > 0.0 && s.duration - s.position <= 45.0
        };
        if !near_end {
            return;
        }
        let Some(next) = self.queue.front().cloned() else { return };
        let have = self.prefetch.as_ref().map(|(id, _)| *id == next.id).unwrap_or(false);
        if have || self.prefetching.is_some() {
            return;
        }
        let Some(api) = self.api.clone() else { return };
        self.prefetching = Some(next.id.clone());
        let tx = self.tx.clone();
        thread::spawn(move || {
            let res = load_track(&api, &next);
            let _ = tx.send(Msg::Prefetch { id: next.id, res });
        });
    }

    fn play_loaded(&mut self, (buf, src, format): Audio) {
        let Some(t) = self.current.clone() else { return };
        if !self.ensure_audio() {
            self.error("Не найдено аудиоустройство");
            self.sh().loading = false;
            return;
        }
        let fade_in = std::mem::take(&mut self.fade_in);
        let src = self.source(src, fade_in);
        let vol = self.gain();
        let paused = self.paused;
        if let Some(sink) = self.sink() {
            sink.clear();
            sink.set_volume(vol);
            sink.append(src);
            if paused {
                sink.pause()
            } else {
                sink.play()
            }
        }
        self.current_buf = Some(buf);
        self.sh().format = format;
        self.loaded = true;
        self.held_pos = None;
        self.errors_in_row = 0;
        self.sh().loading = false;
        if let Some(l) = self.listen.as_mut().filter(|_| !paused) {
            l.go();
        }
        self.remember_wave(&t);
        if let Some((id, pos)) = self.resume_at.take() {
            if id == t.id && pos > 1.0 {
                self.cmd(Cmd::Seek(pos));
            }
        }
        self.publish_soon();
        self.prefetch_next();
    }

    fn publish_soon(&mut self) {
        self.ynison_due = Some(Instant::now() + Duration::from_secs(1));
    }

    /// Записать в Ynison, что играет: оригинал и следующий запуск продолжат отсюда.
    fn publish(&mut self) {
        self.ynison_due = None;
        self.ynison_at = Instant::now();
        let (Some(api), Some(now)) = (self.api.clone(), self.ynison_now()) else { return };
        let _ = self.bg.send(Box::new(move || {
            if let Err(e) = crate::ynison::publish(api.token(), &crate::ynison::device_id(), &now) {
                log(format!("ynison: {e}"));
            }
        }));
    }

    fn ynison_now(&self) -> Option<crate::ynison::Now> {
        let current = self.current.as_ref()?;
        let (entity_type, entity_id) = match &self.source {
            Source::Wave => ("RADIO", WAVE.to_string()),
            Source::Seed { station, .. } => ("RADIO", station.clone()),
            Source::List { kind: ListKind::Playlist, id, .. } => ("PLAYLIST", id.clone()),
            Source::List { kind: ListKind::Likes, .. } => ("PLAYLIST", format!("{}:3", self.uid)),
            Source::List { kind: ListKind::Album, id, .. } => ("ALBUM", id.clone()),
            Source::List { kind: ListKind::Artist, id, .. } => ("ARTIST", id.clone()),
            Source::List { kind: ListKind::Tracks, .. } => ("VARIOUS", String::new()),
        };
        // У списка — весь список, у волны — текущий трек и то, что уже подобрано дальше.
        let (tracks, index): (Vec<&Track>, usize) = match &self.list {
            Some(l) => match l.all.iter().position(|t| t.id == current.id) {
                Some(i) => (l.all.iter().collect(), i),
                None => (vec![current], 0),
            },
            None => (std::iter::once(current).chain(self.queue.iter().take(20)).collect(), 0),
        };
        let playable = |t: &&Track| crate::ynison::Playable {
            id: t.id.clone(),
            album_id: t.album_id.clone().unwrap_or_default(),
            title: t.title.clone(),
        };
        // Пока трек загружается, декодер ещё не получил сохранённую позицию.
        let position = if self.loaded {
            self.pos()
        } else {
            self.resume_at.as_ref().filter(|(id, _)| id == &current.id).map_or(0.0, |(_, seconds)| *seconds)
        };
        Some(crate::ynison::Now {
            entity_type,
            entity_id,
            from: self.play_context().from,
            tracks: tracks.iter().map(playable).collect(),
            index,
            progress_ms: (position * 1000.0) as u64,
            duration_ms: current.duration_ms,
            paused: self.paused,
            session: if self.list.is_none() { self.session.clone() } else { String::new() },
        })
    }

    /// Продолжить то, что играло последним в любом клиенте, с той же секунды и на паузе.
    fn apply_resume(&mut self, r: Resume) {
        let Some(track) = r.tracks.get(r.index).cloned() else { return };
        self.resume_at = Some((track.id.clone(), r.position));
        if r.source.station().is_some() {
            if r.source != self.source {
                self.set_source(r.source);
                self.feedback("radioStarted", None, None);
            }
            self.queue.push_back(track);
            self.play_from_queue();
            self.fetch_batch();
        } else {
            self.cmd(Cmd::PlayList { source: r.source, tracks: r.tracks, start: r.index, after: After::Loop });
        }
        self.sh().resumed = true;
    }

    // ---------- память волны ----------

    fn remember_wave(&mut self, t: &Track) {
        if self.source.station() == Some(self.played_station.as_str()) && !t.batch_id.is_empty() {
            self.save_wave_with(Some(t.full_id()));
        }
    }

    /// «Моя волна» после перезапуска продолжается, как у оригинала: сервер клонирует прошлую
    /// сессию вместе с её историей. Иначе волна начинает с чистого листа.
    fn restore_wave(&mut self) {
        let first = !std::mem::replace(&mut self.wave_restored, true);
        let Some(w) = load_wave_state() else { return };
        if w.uid != self.uid {
            return;
        }
        // Сессию берём только при запуске; если сервер её уже закрыл, wave_session начнёт новую.
        let fresh = crate::config::now_secs().saturating_sub(w.at) < WAVE_SESSION_TTL;
        if first && fresh && self.session.is_empty() && !w.session.is_empty() {
            self.session = w.session;
            self.clone_pending = true;
            self.restore_last = Some(w.last).filter(|l| !l.is_empty());
        }
    }

    fn save_wave(&self) {
        let last = self.current.as_ref().filter(|t| !t.batch_id.is_empty()).map(|t| t.full_id());
        self.save_wave_with(last);
    }

    fn save_wave_with(&self, last: Option<String>) {
        if self.played_station != WAVE || self.source.station() != Some(WAVE) || self.uid.is_empty() {
            return;
        }
        let w = serde_json::json!({
            "uid": self.uid,
            "session": self.session,
            "last": last.unwrap_or_default(),
            "at": crate::config::now_secs(),
        });
        let _ = self.bg.send(Box::new(move || {
            let path = crate::config::dir().join("wave.json");
            if let Err(e) = std::fs::write(&path, w.to_string()) {
                log(format!("wave.json: {e}"));
            }
        }));
    }

    fn on_load_error(&mut self, e: String) {
        self.errors_in_row += 1;
        let name = self.current.as_ref().map(|t| t.title.clone()).unwrap_or_default();
        self.error(format!("«{name}» не загрузился: {e}"));
        self.listen = None;
        if self.errors_in_row < 5 {
            self.current = None;
            self.stop_audio();
            self.play_from_queue();
        } else {
            self.sh().loading = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn login_retries_after_network_error() {
        let mut e = Engine::new(mpsc::channel().0, Default::default(), Default::default(), Arc::new(|| {}), Default::default());
        e.api = Some(Api::new(""));
        // Сеть: ошибка показывается один раз, вход повторится через 2 с.
        e.handle(Msg::Account(Err("сеть: operation timed out".into())));
        assert!(matches!(e.sh().status, Status::Failed(_)));
        assert!(e.api.is_some());
        let (at, n) = e.account_retry.expect("повтор назначен");
        assert_eq!(n, 1);
        assert!(at > Instant::now() + Duration::from_millis(1500));
        // Снова сеть: тоста больше нет, пауза длиннее.
        e.handle(Msg::Account(Err("сеть: connection refused".into())));
        assert_eq!(e.sh().status, Status::Connecting);
        assert_eq!(e.account_retry.map(|(_, n)| n), Some(2));
        // Отказ сервера — без повторов, как раньше.
        e.handle(Msg::Account(Err("нет доступа (HTTP 401) — токен не подошёл или истёк.".into())));
        assert!(e.account_retry.is_none() && e.api.is_none());
        assert_eq!(account_retry_delay(1), Duration::from_secs(2));
        assert_eq!(account_retry_delay(5), Duration::from_secs(30));
        assert_eq!(account_retry_delay(9), Duration::from_secs(60));
    }

    #[test]
    fn wave_skip_keeps_tracks_returned_by_back() {
        let mut e = Engine::new(mpsc::channel().0, Default::default(), Default::default(), Arc::new(|| {}), Default::default());
        let t = |id: u32| {
            let mut t = crate::api::parse_track(&serde_json::json!({"id": id, "title": "t"})).unwrap();
            t.batch_id = "b".into();
            t
        };
        e.session = "s".into();
        e.queue = VecDeque::from([t(2), t(3), t(9)]);
        e.returned = 2;
        e.play_from_queue();
        e.refresh_wave();
        assert_eq!(e.queue.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["3"]);
    }

    #[test]
    fn enqueued_list_keeps_its_order() {
        let mut e = Engine::new(mpsc::channel().0, Default::default(), Default::default(), Arc::new(|| {}), Default::default());
        let t = |id: u32| crate::api::parse_track(&serde_json::json!({"id": id, "title": "t"})).unwrap();
        let ids = |e: &Engine| e.queue.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        e.queue.push_back(t(1));
        e.cmd(Cmd::Enqueue { tracks: vec![t(2), t(3)], next: true });
        assert_eq!(ids(&e), ["2", "3", "1"]);
        e.cmd(Cmd::Enqueue { tracks: vec![t(4), t(5)], next: false });
        assert_eq!(ids(&e), ["2", "3", "1", "4", "5"]);
    }

    #[test]
    fn closing_while_restored_track_loads_keeps_ynison_position() {
        let mut engine = Engine::new(mpsc::channel().0, Default::default(), Default::default(), Arc::new(|| {}), Default::default());
        let track = crate::api::parse_track(&serde_json::json!({"id": 7, "title": "Track", "durationMs": 120000})).unwrap();
        engine.apply_resume(Resume { source: Source::Wave, tracks: vec![track], index: 0, position: 37.5 });
        assert!(!engine.loaded);
        let now = engine.ynison_now().unwrap();
        assert!(now.paused);
        assert_eq!(now.progress_ms, 37_500);
    }

    #[test]
    fn seek_keeps_latest_position_until_audio_acknowledges_it() {
        let (tx, rx) = mpsc::channel();
        let mut e = Engine::new(tx.clone(), Default::default(), Default::default(), Arc::new(|| {}), Default::default());
        let player = Player { tx, shared: e.shared.clone(), likes: e.likes.clone(), dislikes: e.dislikes.clone(), eq: e.eq.clone() };
        e.loaded = true;
        e.held_pos = Some(10.0);
        e.sh().track = crate::api::parse_track(&serde_json::json!({"id": 1, "title": "seek"}));
        e.sh().duration = 100.0;

        for seconds in [80.0, 20.0] {
            assert_eq!(player.seek(seconds), Some(seconds));
            e.sync();
            assert_eq!(player.state().position, seconds);
        }
        e.handle(rx.recv().unwrap());
        e.sync();
        assert_eq!(e.held_pos, Some(10.0));
        assert_eq!(player.state().position, 20.0);
        e.handle(rx.recv().unwrap());
        e.sync();
        assert_eq!(e.held_pos, Some(20.0));
        e.held_pos = Some(21.0);
        e.sync();
        assert_eq!(player.state().position, 21.0);

        player.seek(90.0);
        e.gen += 1; // Перед обработкой команды начался другой трек.
        e.held_pos = Some(0.0);
        e.handle(rx.recv().unwrap());
        e.sync();
        assert_eq!(player.state().position, 0.0);
    }

    #[test]
    fn dislike_survives_an_earlier_like_reply() {
        let mut e = Engine::new(mpsc::channel().0, Default::default(), Default::default(), Arc::new(|| {}), Default::default());
        let (bg, _jobs) = mpsc::channel();
        e.bg = bg;
        e.api = Some(Api::new(""));
        e.uid = "account".into();
        let track = crate::api::parse_track(&serde_json::json!({"id": 7, "title": "Track"})).unwrap();
        e.current = Some(track.clone());
        e.toggle_like(track, None);
        e.cmd(Cmd::Dislike(None));
        e.handle(Msg::ReactionFinished { uid: "account".into(), id: "7".into(), seq: 1, kind: Reaction::Like, remove: false, result: Ok(()) });
        assert!(!e.likes.lock().unwrap().contains("7"));
        assert!(e.dislikes.lock().unwrap().contains("7"));
        e.handle(Msg::Likes("account".into(), Ok(vec!["7".into()])));
        assert!(!e.likes.lock().unwrap().contains("7"));
        e.handle(Msg::ReactionFinished { uid: "account".into(), id: "7".into(), seq: 2, kind: Reaction::Dislike,
            remove: false, result: Err("offline".into()) });
        assert!(e.likes.lock().unwrap().contains("7"));
        assert!(!e.dislikes.lock().unwrap().contains("7"));
    }

    #[test]
    fn queued_likes_keep_the_last_saved_state_on_error() {
        let mut e = Engine::new(mpsc::channel().0, Default::default(), Default::default(), Arc::new(|| {}), Default::default());
        let (bg, _jobs) = mpsc::channel();
        e.bg = bg;
        e.api = Some(Api::new(""));
        e.uid = "account".into();
        let track = crate::api::parse_track(&serde_json::json!({"id": 7, "title": "Track"})).unwrap();
        let reply = |seq, remove, result| Msg::ReactionFinished { uid: "account".into(), id: "7".into(), seq, kind: Reaction::Like, remove, result };
        e.toggle_like(track.clone(), None);
        e.toggle_like(track.clone(), None);
        e.handle(reply(1, false, Ok(())));
        assert!(!e.likes.lock().unwrap().contains("7"));
        e.handle(reply(2, true, Err("offline".into())));
        assert!(e.likes.lock().unwrap().contains("7"));
        e.toggle_like(track, None);
        e.handle(reply(3, true, Ok(())));
        e.handle(Msg::Likes("account".into(), Ok(vec!["7".into(), "9".into()])));
        let likes = e.likes.lock().unwrap();
        assert!(!likes.contains("7"));
        assert!(likes.contains("9"));
    }

    /// Сетевой тест: ссылка на поток → скачивание → декодирование (без токена — 30-сек превью).
    #[test]
    #[ignore]
    fn stream_and_decode() {
        let api = Api::new("");
        let (url, _) = api.stream_url("57703").expect("stream url");
        let bytes = api.fetch_bytes(&url).expect("download");
        assert!(bytes.len() > 50_000);
        let dec = Decoder::new(Cursor::new(bytes)).expect("decode");
        let samples = dec.take(44100).count();
        assert!(samples > 1000);
    }
}

#[cfg(test)]
#[test]
fn explicit_filter_survives_repeat_and_previous() {
    let mut engine = Engine::new(mpsc::channel().0, Default::default(), Default::default(), Arc::new(|| {}), Default::default());
    let blocked = crate::api::parse_track(&serde_json::json!({"id":1,"title":"E","explicit":true})).unwrap();
    let allowed = crate::api::parse_track(&serde_json::json!({"id":2,"title":"A"})).unwrap();
    let previous = crate::api::ALLOW_EXPLICIT.swap(false, std::sync::atomic::Ordering::Relaxed);
    engine.list = Some(ListPlay { deferred: VecDeque::new(), all: vec![blocked.clone(), allowed.clone()], after: After::Loop });
    engine.repeat = Repeat::All;
    engine.play_from_queue();
    let repeated = engine.current.take().map(|t| t.id);
    engine.history = vec![allowed, blocked];
    engine.prev();
    let previous_track = engine.current.take().unwrap().id;
    let explicit = engine.list.as_ref().unwrap().all[0].clone();
    engine.list.as_mut().unwrap().all = vec![explicit.clone()];
    engine.list.as_mut().unwrap().after = After::Wave(Source::Wave);
    engine.repeat = Repeat::Off;
    engine.queue = VecDeque::from([explicit]);
    engine.play_from_queue();
    engine.set_allow_explicit(true);
    engine.cmd(Cmd::PlayPause);
    let resumed = engine.current.take().map(|t| t.id);
    // Список только из 18+ при выключенном фильтре не включается и не трогает текущий трек.
    crate::api::ALLOW_EXPLICIT.store(false, std::sync::atomic::Ordering::Relaxed);
    let kept = engine.list.as_ref().unwrap().all[0].clone();
    let mut other = kept.clone();
    other.id = "3".into();
    engine.current = Some(kept.clone());
    engine.cmd(Cmd::PlayList { source: Source::Wave, tracks: vec![other, kept], start: 1, after: After::Loop });
    let still = engine.current.as_ref().map(|t| t.id.clone());
    let told = engine.sh().error.is_some();
    crate::api::ALLOW_EXPLICIT.store(previous, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(still.as_deref(), Some("1"));
    assert!(told);
    assert_eq!(resumed.as_deref(), Some("1"));
    assert_eq!(repeated.as_deref(), Some("2"));
    assert_eq!(previous_track, "2");
}

#[cfg(test)]
#[test]
fn wave_drops_list_repeat_and_keeps_track_repeat() {
    let mut engine = Engine::new(mpsc::channel().0, Default::default(), Default::default(), Arc::new(|| {}), Default::default());
    engine.repeat = Repeat::All;
    engine.cmd(Cmd::Wave(Source::Wave));
    let after_list = engine.repeat;
    engine.cmd(Cmd::Repeat(Repeat::One));
    engine.cmd(Cmd::Wave(Source::Seed { station: "activity:road-trip".into(), header: String::new(), title: String::new() }));
    assert_eq!(after_list, Repeat::Off);
    assert_eq!(engine.repeat, Repeat::One);
}
