//! Светлые страницы: стек живёт в UI-потоке, данные качаются в фоне.

use crate::api::{self, Api, Track};
use crate::config::log;
use crate::landing::{self, LBlock, LCard};
use crate::player::{After, Cmd, ListKind, Player, Source};
use crate::{now, sys, App, Icons, PBlock, PCard, PPage, PRow, Pages, IMAGES};
use serde_json::Value;
use slint::{Color, ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

// Виды блоков, те же в pages.slint (global K).
const CARDS: i32 = 1;
const ARTISTS: i32 = 2;
const TRACKS: i32 = 3;
const PAIR: i32 = 4;
const PROMO: i32 = 5;
const VIBES: i32 = 6;
const WAVE: i32 = 7;
const CONCERTS: i32 = 8;
const SHORTCUTS: i32 = 9;
const LINK: i32 = 10;
const ABOUT: i32 = 11;
const TILES: i32 = 12;
const MIXES: i32 = 13;
const CHIPS: i32 = 14;
const SUGGEST: i32 = 15;
const ARTIST_HEAD: i32 = 16;
const LIST_HEAD: i32 = 17;
const PLAY: i32 = 18;
const BANNERS: i32 = 19;
const GRID: i32 = 20;
const TEXT: i32 = 21;
const LIKES_HEAD: i32 = 22;
const ROWS: i32 = 23;
const EMPTY: i32 = 24;
const HEADING: i32 = 25;
const ALBUM_HEAD: i32 = 26;
const NUMBERED: i32 = 27;
const RELEASES: i32 = 28;

const FEED: &str = "/concerts/feed";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Home,
    Concerts,
    Collection,
    Search,
    Results(String),
    Landing(String, usize, String),
    Tag(String),
    Artist(String),
    ArtistTracks(String),
    ArtistAlbums(String),
    ArtistPlaylists(String),
    Similar(String),
    About(String),
    Album(String),
    Playlist(String, String),
    Likes,
    LikedArtists,
    Dislikes,
    Playlists,
    Albums,
    History,
    Block(String, String, String),
}

/// Что делают кнопки шапки: лайк, трейлер, слушать, поделиться.
#[derive(Clone, Debug, Default)]
enum Entity {
    #[default]
    None,
    Artist(String),
    Album(String),
    Playlist(String, String),
}

#[derive(Clone, Default)]
struct Block {
    kind: i32,
    title: String,
    subtitle: String,
    more: Option<Key>,
    artists: Vec<api::Named>,
    cards: Vec<LCard>,
    tabs: Vec<(String, Vec<LCard>)>,
    chip: usize,
    tracks: Vec<Track>,
    pos: Vec<String>,
    image: Option<String>,
    text: String,
    bg: Option<[u8; 3]>,
    liked: bool,
    disliked: bool,
}

impl Block {
    fn new(kind: i32, title: &str) -> Self {
        Block { kind, title: title.to_string(), ..Default::default() }
    }

    fn cards(kind: i32, title: &str, cards: Vec<LCard>) -> Self {
        Block { cards, ..Block::new(kind, title) }
    }

    fn tracks(title: &str, tracks: Vec<Track>) -> Self {
        Block { tracks, ..Block::new(TRACKS, title) }
    }

    fn more(mut self, key: Key) -> Self {
        self.more = Some(key);
        self
    }

    fn is_empty(&self) -> bool {
        matches!(self.kind, CARDS | ARTISTS | MIXES | PROMO | BANNERS | GRID | CONCERTS | PAIR | TILES | ROWS | SUGGEST | VIBES | RELEASES)
            && self.cards.is_empty()
            && self.tabs.iter().all(|t| t.1.is_empty())
            || matches!(self.kind, TRACKS | NUMBERED) && self.tracks.is_empty()
    }

    /// Карточки выбранной вкладки блока, если вкладки есть.
    fn shown_cards(&self) -> &[LCard] {
        match self.tabs.get(self.chip) {
            Some((_, c)) => c,
            None => &self.cards,
        }
    }
}

#[derive(Clone, Default)]
struct PageData {
    title: String,
    overlay: bool,
    has_search: bool,
    /// «Мои плейлисты» как есть, чтобы пересортировать вкладки без запроса.
    playlists: Vec<(String, Vec<api::Playlist>)>,
    head: Vec<Block>,
    /// Вкладки страницы («Популярное / История»): полоса вкладок встаёт после head.
    tabs: Vec<(String, Vec<Block>)>,
    tab: usize,
    entity: Entity,
}

impl PageData {
    fn new(title: &str, head: Vec<Block>) -> Self {
        PageData { title: title.to_string(), head: head.into_iter().filter(|b| !b.is_empty()).collect(), ..Default::default() }
    }

    fn visible(&self) -> Vec<&Block> {
        let mut out: Vec<&Block> = self.head.iter().collect();
        if let Some((_, blocks)) = self.tabs.get(self.tab) {
            out.extend(blocks.iter());
        }
        out
    }

    fn visible_mut(&mut self, i: usize) -> Option<&mut Block> {
        let n = self.head.len();
        if i < n {
            return self.head.get_mut(i);
        }
        let tab = self.tab;
        self.tabs.get_mut(tab).and_then(|(_, b)| b.get_mut(i - n))
    }
}

struct Entry {
    id: u64,
    key: Key,
    data: Option<PageData>,
    error: String,
    scroll: f32,
    blocks: Rc<VecModel<PBlock>>,
    parts: Vec<Part>,
}

/// Сколько треков в одной части списка: ListView создаёт только видимые части.
const PART_ROWS: usize = 40;

/// Блок на экране: блок страницы `block` и его треки `rows`. Длинный список треков
/// режется на части, иначе «Мне нравится» на тысячи треков создаёт все строки сразу.
struct Part {
    block: usize,
    rows: Range<usize>,
    loaded: bool,
}

fn parts_of(block: usize, b: &Block) -> Vec<Part> {
    let n = b.tracks.len();
    if !matches!(b.kind, TRACKS | NUMBERED) || n <= PART_ROWS {
        return vec![Part { block, rows: 0..n, loaded: false }];
    }
    (0..n).step_by(PART_ROWS).map(|s| Part { block, rows: s..(s + PART_ROWS).min(n), loaded: false }).collect()
}

struct St {
    ui: slint::Weak<App>,
    api: Arc<Api>,
    player: Player,
    stack: Vec<Entry>,
    tab: i32,
    serial: u64,
    cache: HashMap<Key, PageData>,
    search_timer: slint::Timer,
    /// Номер запроса поиска: устаревшие ответы не показываются.
    search_seq: u64,
    menu_track: Option<Track>,
}

thread_local! {
    static ST: RefCell<Option<St>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut St) -> R) -> Option<R> {
    ST.with(|s| s.borrow_mut().as_mut().map(f))
}

pub fn start(ui: &App, api: Arc<Api>, player: Player) {
    ST.with(|s| {
        *s.borrow_mut() = Some(St {
            ui: ui.as_weak(),
            api,
            player,
            stack: Vec::new(),
            tab: 0,
            serial: 0,
            cache: HashMap::new(),
            search_timer: slint::Timer::default(),
            search_seq: 0,
            menu_track: None,
        })
    });
    ui.on_tab_clicked(|t| tab(t));
    let p = ui.global::<Pages>();
    p.on_back(back);
    p.on_open_search(|| push(Key::Search));
    p.on_card(|b, i| card(part(b).0, i as usize));
    p.on_track(|b, i| {
        let (b, first) = part(b);
        track(b, first + i as usize)
    });
    p.on_appeared(|b| {
        with(|s| s.load_part(b as usize));
    });
    p.on_more(|b| more(part(b).0));
    p.on_chip(|b, i| chip(part(b).0, i as usize));
    p.on_sort_picked(sort_picked);
    p.on_action(|b, n| action(part(b).0, n));
    p.on_search_edited(|t| search_edited(t.to_string(), Duration::from_millis(450)));
    p.on_search_submit(|t| search_edited(t.to_string(), Duration::ZERO));
    p.on_menu(|b, i| {
        let (b, first) = part(b);
        menu(b, first + i as usize)
    });
    p.on_menu_action(menu_action);
    p.on_about_current(|| {
        if let Some(t) = with(|s| s.player.state().track).flatten() {
            about(&t);
        }
    });
    p.on_pick(|i| pick(i as usize));
    p.on_pick_artist(|i| {
        if let Some(id) = ARTIST_IDS.with(|ids| ids.borrow().get(i as usize).cloned()) {
            open(&format!("artist:{id}"));
        }
    });
}

thread_local! {
    static ARTIST_IDS: RefCell<Vec<String>> = RefCell::default();
}

/// Одного исполнителя открывает сразу, из нескольких даёт выбрать в шторке.
pub fn open_artists(artists: &[api::Named]) {
    match artists {
        [] => {}
        [one] => open(&format!("artist:{}", one.id)),
        many => {
            let Some(ui) = crate::UI.get().and_then(|w| w.upgrade()) else { return };
            ARTIST_IDS.with(|ids| *ids.borrow_mut() = many.iter().map(|a| a.id.clone()).collect());
            let names: Vec<SharedString> = many.iter().map(|a| a.name.as_str().into()).collect();
            let pages = ui.global::<Pages>();
            pages.set_artist_names(ModelRc::new(VecModel::from(names)));
            pages.set_artists_open(true);
        }
    }
}

fn tab(t: i32) {
    with(|s| {
        let Some(ui) = s.ui.upgrade() else { return };
        if t == 0 {
            s.stack.clear();
            s.tab = 0;
            ui.set_tab(0);
            ui.global::<Pages>().set_shown(false);
            return;
        }
        if s.tab == t && s.stack.len() > 1 {
            s.stack.truncate(1);
            s.show();
            return;
        }
        s.tab = t;
        ui.set_tab(t);
        s.stack.clear();
        let key = match t {
            1 => Key::Home,
            2 => Key::Concerts,
            _ => Key::Collection,
        };
        s.open(key);
    });
}

fn push(key: Key) {
    with(|s| s.open(key));
}

fn back() {
    with(|s| {
        if let Some(ui) = s.ui.upgrade() {
            ui.invoke_refocus();
        }
        if s.stack.len() > 1 {
            s.stack.pop();
            s.show();
        } else {
            s.stack.clear();
            s.tab = 0;
            if let Some(ui) = s.ui.upgrade() {
                ui.set_tab(0);
                ui.global::<Pages>().set_shown(false);
            }
        }
    });
}

impl St {
    fn open(&mut self, key: Key) {
        let Some(ui) = self.ui.upgrade() else { return };
        if let Some(top) = self.stack.last_mut() {
            top.scroll = ui.global::<Pages>().get_scroll();
        }
        if key == Key::Search {
            ui.global::<Pages>().set_query("".into());
        }
        self.serial += 1;
        let id = self.serial;
        let cached = self.cache.get(&key).cloned();
        self.stack.push(Entry { id, key: key.clone(), data: cached.clone(), error: String::new(), scroll: 0.0, blocks: Rc::new(VecModel::default()), parts: Vec::new() });
        self.show();
        if cached.is_some() && !matches!(key, Key::Collection | Key::Likes | Key::LikedArtists | Key::Dislikes | Key::Playlists | Key::Albums | Key::History) {
            return;
        }
        let (api, uid) = (self.api.clone(), now::uid());
        std::thread::spawn(move || {
            let res = fetch(&api, &uid, &key);
            let _ = slint::invoke_from_event_loop(move || {
                with(|s| s.loaded(id, key, res));
            });
        });
    }

    fn loaded(&mut self, id: u64, key: Key, res: Result<PageData, String>) {
        let Some(e) = self.stack.iter_mut().find(|e| e.id == id) else {
            if let Ok(d) = res {
                self.cache.insert(key, d);
            }
            return;
        };
        match res {
            Ok(d) => {
                self.cache.insert(key, d.clone());
                e.data = Some(d);
                e.error.clear();
            }
            Err(err) => {
                log(format!("страница {key:?}: {err}"));
                if e.data.is_none() {
                    e.error = format!("Не удалось загрузить: {err}");
                }
            }
        }
        if self.stack.last().is_some_and(|e| e.id == id) {
            self.show();
        }
    }

    /// Показать верх стека: собрать модели и докачать картинки.
    fn show(&mut self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let pages = ui.global::<Pages>();
        let is_root = self.stack.len() == 1 && self.tab != 0;
        let Some(e) = self.stack.last_mut() else {
            pages.set_shown(false);
            return;
        };
        // Лайк исполнителя знает только UI-поток.
        if let Some(d) = e.data.as_mut() {
            if let (Entity::Artist(id), Some(h)) = (&d.entity, d.head.first_mut()) {
                if h.kind == ARTIST_HEAD {
                    h.liked = now::artist_liked(id);
                    h.disliked = now::artist_disliked(id);
                }
            }
        }
        let icons = ui.get_icons();
        let visible = e.data.as_ref().map(|d| d.visible()).unwrap_or_default();
        e.parts = visible.iter().enumerate().flat_map(|(i, b)| parts_of(i, b)).collect();
        let blocks: Vec<PBlock> = e.parts.iter().map(|p| pblock(visible[p.block], p.rows.clone(), &icons)).collect();
        e.blocks = Rc::new(VecModel::from(blocks));
        let d = e.data.as_ref();
        pages.set_page(PPage {
            title: d.map(|d| d.title.clone()).unwrap_or_else(|| title_of(&e.key)).into(),
            overlay: d.is_some_and(|d| d.overlay),
            search: matches!(e.key, Key::Search),
            has_search: d.is_some_and(|d| d.has_search),
            has_sort: d.is_some_and(|d| !d.playlists.is_empty()),
            root: is_root,
            loading: e.data.is_none() && e.error.is_empty(),
            error: e.error.clone().into(),
            blocks: ModelRc::from(e.blocks.clone()),
        });
        pages.set_scroll(e.scroll);
        pages.set_shown(true);
    }

    fn top(&self) -> Option<&Entry> {
        self.stack.last()
    }

    /// Блок страницы и сдвиг его треков для блока на экране.
    fn part(&self, shown: usize) -> (usize, usize) {
        self.top().and_then(|e| e.parts.get(shown)).map_or((shown, 0), |p| (p.block, p.rows.start))
    }

    /// Картинки части грузятся, когда она впервые попала на экран.
    fn load_part(&mut self, shown: usize) {
        let Some(ui) = self.ui.upgrade() else { return };
        let scale = ui.window().scale_factor();
        let Some(e) = self.stack.last_mut() else { return };
        let Some(p) = e.parts.get_mut(shown).filter(|p| !p.loaded) else { return };
        let Some(block) = e.data.as_ref().and_then(|d| d.visible().get(p.block).copied()) else { return };
        p.loaded = true;
        load_block_images(&e.blocks, shown, block, p.rows.clone(), scale);
    }

    fn block(&self, b: usize) -> Option<Block> {
        self.top()?.data.as_ref()?.visible().get(b).map(|b| (*b).clone())
    }

    fn play(&self, tracks: Vec<Track>, start: usize, kind: ListKind, id: String, title: String) {
        if tracks.is_empty() {
            return;
        }
        self.player.send(Cmd::PlayList { source: Source::List { kind, id, title }, tracks, start, after: After::Wave(Source::Wave) });
        // На паузе трек иначе только встаёт текущим и ждёт второго нажатия (#124).
        self.player.send(Cmd::Resume);
    }

    /// Источник для плеера по открытой странице.
    fn list_source(&self) -> (ListKind, String, String) {
        let Some(e) = self.top() else { return (ListKind::Tracks, String::new(), String::new()) };
        let title = e.data.as_ref().map(|d| d.title.clone()).unwrap_or_default();
        match &e.key {
            Key::Album(id) => (ListKind::Album, id.clone(), title),
            Key::Playlist(u, k) => (ListKind::Playlist, format!("{u}:{k}"), title),
            Key::Artist(id) | Key::ArtistTracks(id) => (ListKind::Artist, id.clone(), title),
            Key::Likes => (ListKind::Likes, "likes".into(), "Мне нравится".into()),
            _ => (ListKind::Tracks, String::new(), title),
        }
    }

    fn rebuild_block(&mut self, b: usize) {
        let Some(ui) = self.ui.upgrade() else { return };
        let icons = ui.get_icons();
        let scale = ui.window().scale_factor();
        let Some(e) = self.stack.last() else { return };
        let Some(block) = e.data.as_ref().and_then(|d| d.visible().get(b).copied()) else { return };
        for (shown, p) in e.parts.iter().enumerate().filter(|(_, p)| p.block == b) {
            e.blocks.set_row_data(shown, pblock(block, p.rows.clone(), &icons));
            load_block_images(&e.blocks, shown, block, p.rows.clone(), scale);
        }
    }
}

fn title_of(key: &Key) -> String {
    match key {
        Key::Home => "Что послушать".into(),
        Key::Concerts => "Концерты рядом".into(),
        Key::Collection => "Коллекция".into(),
        Key::ArtistTracks(_) => "Все треки".into(),
        Key::Likes => "Мне нравится".into(),
        Key::LikedArtists => "Любимые исполнители".into(),
        Key::Dislikes => "Ваши дизлайки".into(),
        Key::Playlists => "Плейлисты".into(),
        Key::Albums => "Альбомы".into(),
        Key::History => "История".into(),
        Key::Landing(_, _, t) | Key::Block(_, _, t) => t.clone(),
        _ => String::new(),
    }
}

// ---------- клики ----------

/// Блок страницы и сдвиг треков для номера блока на экране.
fn part(shown: i32) -> (usize, usize) {
    let shown = shown as usize;
    with(|s| s.part(shown)).unwrap_or((shown, 0))
}

fn card(b: usize, i: usize) {
    let target = with(|s| {
        let block = s.block(b)?;
        let c = block.shown_cards().get(i)?.clone();
        Some((c.target, c.title))
    })
    .flatten();
    if let Some((target, title)) = target {
        open_target(&target, &title);
    }
}

/// Открыть страницу поверх волны или плеера: album:ID, artist:ID, playlists.
pub fn open(target: &str) {
    open_target(target, "");
}

fn open_target(target: &str, title: &str) {
    let (kind, arg) = target.split_once(':').unwrap_or((target, ""));
    let key = match kind {
        "album" => Key::Album(arg.into()),
        "playlist" => match arg.split_once(':') {
            Some((u, k)) => Key::Playlist(u.into(), k.into()),
            None => return,
        },
        "artist" => Key::Artist(arg.into()),
        "likes" => Key::Likes,
        "history" => Key::History,
        "playlists" => Key::Playlists,
        "albums" => Key::Albums,
        "landing" => {
            let mut p = arg.splitn(2, ':');
            let skel = p.next().unwrap_or("main").to_string();
            let tab = p.next().and_then(|t| t.parse().ok()).unwrap_or(0);
            Key::Landing(skel, tab, title.into())
        }
        "tag" => Key::Tag(arg.into()),
        "query" => {
            let q = arg;
            with(|s| {
                s.open(Key::Search);
                if let Some(ui) = s.ui.upgrade() {
                    ui.global::<Pages>().set_query(q.into());
                }
            });
            search_edited(q.to_string(), Duration::ZERO);
            return;
        }
        "station" => {
            with(|s| {
                s.player.send(Cmd::Wave(Source::Seed { station: arg.into(), header: "Моя волна".into(), title: title.into() }));
                s.player.send(Cmd::Resume);
            });
            return;
        }
        "url" => return sys::open_url(arg),
        "concert" => return sys::open_url(&format!("https://music.yandex.ru/concert/{arg}")),
        _ => return,
    };
    push(key);
}

fn track(b: usize, i: usize) {
    with(|s| {
        let Some(block) = s.block(b) else { return };
        let current = s.player.state().track.map(|t| t.id);
        if current.is_some() && block.tracks.get(i).map(|t| &t.id) == current.as_ref() {
            s.player.send(Cmd::PlayPause);
            return;
        }
        let (kind, id, title) = s.list_source();
        let title = if title.is_empty() { block.title.clone() } else { title };
        s.play(block.tracks, i, kind, id, title);
    });
}

fn menu(b: usize, i: usize) {
    with(|s| {
        let Some(t) = s.block(b).and_then(|b| b.tracks.get(i).cloned()) else { return };
        let Some(ui) = s.ui.upgrade() else { return };
        let pages = ui.global::<Pages>();
        let mut row = prow(&t, None);
        pages.set_menu_track(row.clone());
        pages.set_menu_liked(s.player.is_liked(&t.id));
        pages.set_menu_disliked(s.player.is_disliked(&t.id));
        pages.set_menu_open(true);
        if let Some(uri) = &t.cover {
            let px = ((80.0 * ui.window().scale_factor()).round() as u32).min(1000);
            let weak = s.ui.clone();
            IMAGES.with(|images| {
                images.get(&api::pic(uri, px), px, move |img| {
                    if let Some(ui) = weak.upgrade() {
                        row.cover = img.clone();
                        ui.global::<Pages>().set_menu_track(row.clone());
                    }
                })
            });
        }
        s.menu_track = Some(t);
    });
}

fn menu_action(n: i32) {
    let Some((t, player, api)) = with(|s| s.menu_track.clone().map(|t| (t, s.player.clone(), s.api.clone()))).flatten() else { return };
    match n {
        0 => {
            player.send(Cmd::Wave(Source::Seed { station: format!("track:{}", t.id), header: "Моя волна по треку".into(), title: t.title.clone() }));
            player.send(Cmd::Resume);
        }
        1 => {
            let liked = !player.is_liked(&t.id);
            player.send(Cmd::LikeTrack(t, None));
            with(|s| {
                if let Some(ui) = s.ui.upgrade() {
                    ui.global::<Pages>().set_menu_liked(liked);
                }
            });
        }
        2 | 3 => player.send(Cmd::Enqueue { tracks: vec![t], next: n == 2 }),
        4 => sys::share(&crate::track_link(&t)),
        6 => {
            if let Some(a) = &t.album_id {
                open(&format!("album:{a}"));
            }
        }
        7 => open_artists(&t.artist_list),
        8 => player.send(Cmd::DislikeTrack(t, None)),
        9 => crate::download::start(api, "", vec![t]),
        10 => about(&t),
        _ => {}
    }
}

/// Шторка «О треке»: шапка сразу, участники записи — когда придут.
pub fn about(t: &Track) {
    let Some((Some(ui), api)) = with(|s| (s.ui.upgrade(), s.api.clone())) else { return };
    let pages = ui.global::<Pages>();
    let title = if t.version.is_empty() { t.title.clone() } else { format!("{} ({})", t.title, t.version) };
    pages.set_about_title(title.into());
    pages.set_about_artists(t.artists.clone().into());
    pages.set_about_roles(ModelRc::default());
    pages.set_about_names(ModelRc::default());
    pages.set_about_status("Загрузка…".into());
    pages.set_about_open(true);
    let id = t.id.clone();
    std::thread::spawn(move || {
        let res = api.track_credits(&id);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = crate::UI.get().and_then(|w| w.upgrade()) else { return };
            let pages = ui.global::<Pages>();
            match res {
                Ok(credits) if credits.is_empty() => pages.set_about_status("Нет данных об участниках записи".into()),
                Ok(credits) => {
                    let (roles, names): (Vec<SharedString>, Vec<SharedString>) = credits.into_iter().map(|(r, n)| (r.into(), n.into())).unzip();
                    pages.set_about_roles(ModelRc::new(VecModel::from(roles)));
                    pages.set_about_names(ModelRc::new(VecModel::from(names)));
                }
                Err(e) => pages.set_about_status(format!("Не удалось загрузить: {e}").into()),
            }
        });
    });
}

fn pick(i: usize) {
    let Some((t, api)) = with(|s| s.menu_track.clone().map(|t| (t, s.api.clone()))).flatten() else { return };
    let Some((uid, list)) = now::playlist(i) else { return };
    std::thread::spawn(move || match api.add_to_playlist(&uid, &list.kind, &[t]) {
        Ok(()) => {
            let _ = slint::invoke_from_event_loop(now::reload_playlists);
        }
        Err(e) => log(format!("добавить в плейлист: {e}")),
    });
}

fn more(b: usize) {
    if let Some(key) = with(|s| s.block(b).and_then(|b| b.more)).flatten() {
        push(key);
    }
}

/// Порядок «Моих плейлистов» из шторки «Сортировка»; живёт до закрытия приложения.
static PLAYLIST_SORT: AtomicI32 = AtomicI32::new(0);

fn sorted_playlists(tabs: &[(String, Vec<api::Playlist>)], sort: i32) -> Vec<(String, Vec<Block>)> {
    tabs.iter()
        .map(|(title, list)| {
            let mut list: Vec<&api::Playlist> = list.iter().collect();
            match sort {
                1 => list.sort_by(|a, b| b.modified.cmp(&a.modified)),
                2 => list.sort_by_cached_key(|p| p.title.to_lowercase()),
                _ => {}
            }
            let cards = list.into_iter().map(|p| lcard_from(&api::playlist_card(p))).collect();
            (title.clone(), vec![Block::cards(ROWS, "", cards)])
        })
        .collect()
}

fn sort_picked(sort: i32) {
    PLAYLIST_SORT.store(sort, Ordering::Relaxed);
    with(|s| {
        let Some(d) = s.stack.last_mut().and_then(|e| e.data.as_mut()) else { return };
        if d.playlists.is_empty() {
            return;
        }
        d.tabs = sorted_playlists(&d.playlists, sort);
        s.show();
    });
}

fn chip(b: usize, i: usize) {
    with(|s| {
        let Some(e) = s.stack.last_mut() else { return };
        let Some(d) = e.data.as_mut() else { return };
        let Some(block) = d.visible_mut(b) else { return };
        if block.kind == CHIPS {
            // Вкладки всей страницы: блоки ниже полосы меняются целиком.
            block.chip = i;
            d.tab = i;
            s.show();
            return;
        }
        block.chip = i;
        s.rebuild_block(b);
    });
}

fn action(b: usize, n: i32) {
    with(|s| {
        let Some(e) = s.top() else { return };
        let Some(d) = e.data.clone() else { return };
        let Some(block) = d.visible().get(b).map(|b| (*b).clone()) else { return };
        match (n, &d.entity) {
            // «Моя волна по исполнителю» и «Слушать» у исполнителя.
            (_, Entity::Album(id)) if block.kind == WAVE => {
                s.player.send(Cmd::Wave(Source::Seed { station: format!("album:{id}"), header: "Моя волна по альбому".into(), title: d.title.clone() }));
                s.player.send(Cmd::Resume);
            }
            (4, _) => {
                let Some(ui) = s.ui.upgrade() else { return };
                let pages = ui.global::<Pages>();
                let info = block.cards.first().cloned().unwrap_or_default();
                pages.set_viewer_title(block.title.clone().into());
                pages.set_viewer_artist(info.title.into());
                pages.set_viewer_year(info.subtitle.into());
                pages.set_viewer_bg(color(block.bg, Color::from_rgb_u8(0x55, 0x55, 0x55)));
                pages.set_viewer(Image::default());
                pages.set_viewer_open(true);
                if let Some(uri) = &block.image {
                    let weak = s.ui.clone();
                    IMAGES.with(|images| {
                        images.get(&api::pic(uri, 1000), 1000, move |img| {
                            if let Some(ui) = weak.upgrade() {
                                ui.global::<Pages>().set_viewer(img.clone());
                            }
                        })
                    });
                }
            }
            (5, Entity::Album(_)) if block.artists.len() > 1 => open_artists(&block.artists),
            (5, _) => {
                if let Some(key) = block.more.clone() {
                    s.open(key);
                }
            }
            (_, Entity::Artist(id)) if block.kind == WAVE || n == 2 => {
                let name = d.title.clone();
                s.player.send(Cmd::Wave(Source::Seed { station: format!("artist:{id}"), header: "Моя волна по исполнителю".into(), title: name }));
                s.player.send(Cmd::Resume);
            }
            (0 | 3, Entity::Artist(id)) => {
                if n == 0 {
                    now::toggle_artist(id);
                } else {
                    now::toggle_artist_dislike(id);
                }
                let (liked, disliked) = (now::artist_liked(id), now::artist_disliked(id));
                if let Some(e) = s.stack.last_mut() {
                    if let Some(bl) = e.data.as_mut().and_then(|d| d.visible_mut(b)) {
                        bl.liked = liked;
                        bl.disliked = disliked;
                    }
                }
                s.rebuild_block(b);
            }
            (1, Entity::Artist(id)) => {
                let (api, p) = (s.api.clone(), s.player.clone());
                let id = id.clone();
                std::thread::spawn(move || match api.trailer(&format!("artists/{id}")) {
                    Ok((title, tracks)) => {
                        p.send(Cmd::PlayList { source: Source::List { kind: ListKind::Artist, id, title }, tracks, start: 0, after: After::Wave(Source::Wave) });
                        p.send(Cmd::Resume);
                    }
                    Err(e) => log(format!("трейлер: {e}")),
                });
            }
            (2, _) => {
                let tracks: Vec<Track> = d.visible().iter().find(|b| matches!(b.kind, TRACKS | NUMBERED)).map(|b| b.tracks.clone()).unwrap_or_default();
                // Уже играет трек с этой страницы: кнопка ставит на паузу и снимает с неё.
                let current = s.player.state().track.map(|t| t.id);
                if current.is_some_and(|id| tracks.iter().any(|t| t.id == id)) {
                    s.player.send(Cmd::PlayPause);
                    return;
                }
                let (kind, id, title) = s.list_source();
                s.play(tracks, 0, kind, id, title);
            }
            (0, Entity::Album(id)) | (0, Entity::Playlist(_, id)) => {
                let what = if matches!(d.entity, Entity::Album(_)) { "albums" } else { "playlists" };
                let key = match &d.entity {
                    Entity::Playlist(u, k) => format!("{u}:{k}"),
                    _ => id.clone(),
                };
                let remove = block.liked;
                let (api, uid) = (s.api.clone(), now::uid());
                std::thread::spawn(move || {
                    if let Err(e) = api.like_entity(&uid, what, &key, remove) {
                        log(format!("лайк: {e}"));
                    }
                });
                if let Some(e) = s.stack.last_mut() {
                    if let Some(bl) = e.data.as_mut().and_then(|d| d.visible_mut(b)) {
                        bl.liked = !remove;
                    }
                }
                s.rebuild_block(b);
            }
            (6, _) => {
                let tracks: Vec<Track> = d.visible().iter().find(|b| matches!(b.kind, TRACKS | NUMBERED)).map(|b| b.tracks.clone()).unwrap_or_default();
                crate::download::start(s.api.clone(), &d.title, tracks);
            }
            (3, Entity::Album(id)) => sys::share(&format!("https://music.yandex.ru/album/{id}")),
            (3, Entity::Playlist(u, k)) => sys::share(&format!("https://music.yandex.ru/users/{u}/playlists/{k}")),
            _ => {}
        }
    });
}

// ---------- поиск ----------

fn search_edited(text: String, delay: Duration) {
    with(|s| {
        s.search_seq += 1;
        let seq = s.search_seq;
        let text = text.trim().to_string();
        if text.is_empty() {
            s.search_timer.stop();
            if let Some(e) = s.stack.last_mut().filter(|e| e.key == Key::Search) {
                e.data = s.cache.get(&Key::Search).cloned();
                s.show();
            }
            return;
        }
        let api = s.api.clone();
        let run = move || {
            let (api, text) = (api.clone(), text.clone());
            std::thread::spawn(move || {
                let res = fetch(&api, "", &Key::Results(text));
                let _ = slint::invoke_from_event_loop(move || {
                    with(|s| {
                        if s.search_seq != seq {
                            return;
                        }
                        let Some(e) = s.stack.last_mut().filter(|e| e.key == Key::Search) else { return };
                        match res {
                            Ok(d) => e.data = Some(d),
                            Err(err) => e.error = format!("Поиск не удался: {err}"),
                        }
                        e.scroll = 0.0;
                        s.show();
                    });
                });
            });
        };
        if delay.is_zero() {
            s.search_timer.stop();
            run();
        } else {
            s.search_timer.start(slint::TimerMode::SingleShot, delay, run);
        }
    });
}

// ---------- модели для Slint ----------

fn color(c: Option<[u8; 3]>, fallback: Color) -> Color {
    c.map_or(fallback, |[r, g, b]| Color::from_rgb_u8(r, g, b))
}

fn icon_for(name: &str, icons: &Icons) -> Image {
    match name {
        "chart" => icons.chart.clone(),
        "book" => icons.book.clone(),
        "kids" => icons.kids.clone(),
        "podcast" => icons.podcast.clone(),
        "clip" => icons.clip.clone(),
        "playlist" => icons.playlist.clone(),
        "album" => icons.album.clone(),
        _ => Image::default(),
    }
}

fn pcard(c: &LCard, icons: &Icons) -> PCard {
    let (month, day, weekday) = c.date.clone().unwrap_or_default();
    let icon = c.extra.strip_prefix("icon:").map(|n| icon_for(n, icons));
    PCard {
        title: c.title.clone().into(),
        subtitle: c.subtitle.clone().into(),
        sub2: if icon.is_some() { "icon".into() } else { c.sub2.clone().into() },
        cover: icon.unwrap_or_default(),
        cover2: Image::default(),
        round: c.round,
        explicit: c.explicit,
        bg: color(c.bg, Color::from_rgb_u8(0x33, 0x33, 0x33)),
        fg: color(c.fg, Color::from_rgb_u8(0xff, 0xff, 0xff)),
        month: month.into(),
        day: day.into(),
        weekday: weekday.into(),
    }
}

fn prow(t: &Track, pos: Option<&String>) -> PRow {
    PRow {
        id: t.id.clone().into(),
        title: if t.version.is_empty() { t.title.clone() } else { format!("{} {}", t.title, t.version) }.into(),
        subtitle: t.artists.clone().into(),
        cover: Image::default(),
        explicit: t.explicit,
        pos: pos.cloned().unwrap_or_default().into(),
    }
}

/// Блок на экране с треками `rows`; у продолжения длинного списка нет заголовка.
fn pblock(b: &Block, rows: Range<usize>, icons: &Icons) -> PBlock {
    let first = rows.start == 0;
    let cards: Vec<PCard> = b.shown_cards().iter().map(|c| pcard(c, icons)).collect();
    let mut rows: Vec<PRow> = rows.map(|i| prow(&b.tracks[i], b.pos.get(i))).collect();
    // В альбоме оригинал подписывает только треки с другим составом исполнителей.
    if b.kind == NUMBERED {
        rows.iter_mut().filter(|r| r.subtitle.as_str() == b.subtitle).for_each(|r| r.subtitle = "".into());
    }
    let chips: Vec<SharedString> = b.tabs.iter().map(|t| SharedString::from(t.0.as_str())).collect();
    PBlock {
        kind: b.kind,
        title: if first { b.title.clone().into() } else { SharedString::default() },
        subtitle: b.subtitle.clone().into(),
        more: first && b.more.is_some(),
        cards: ModelRc::from(Rc::new(VecModel::from(cards))),
        rows: ModelRc::from(Rc::new(VecModel::from(rows))),
        chips: ModelRc::from(Rc::new(VecModel::from(chips))),
        chip: b.chip as i32,
        image: Image::default(),
        text: b.text.clone().into(),
        bg: color(b.bg, Color::from_rgb_u8(0x55, 0x55, 0x55)),
        liked: b.liked,
        disliked: b.disliked,
    }
}

/// Размер картинки в точках экрана для блока.
fn card_dp(kind: i32) -> f32 {
    match kind {
        MIXES => 96.0,
        PROMO | BANNERS | ABOUT | RELEASES => 400.0,
        WAVE => 56.0,
        CONCERTS | GRID => 200.0,
        PAIR | ROWS | TILES => 72.0,
        VIBES => 180.0,
        _ => 140.0,
    }
}

fn load_block_images(blocks: &Rc<VecModel<PBlock>>, bi: usize, b: &Block, shown_rows: Range<usize>, scale: f32) {
    let Some(pb) = blocks.row_data(bi) else { return };
    let px = |dp: f32| ((dp * scale).round() as u32).clamp(1, 1000);
    IMAGES.with(|images| {
        if let Some(uri) = &b.image {
            let size = px(if b.kind == LIKES_HEAD { 84.0 } else { 420.0 });
            let (m, url) = (blocks.clone(), api::pic(uri, size));
            images.get(&url, size, move |img| {
                if let Some(mut row) = m.row_data(bi) {
                    row.image = img.clone();
                    m.set_row_data(bi, row);
                }
            });
        }
        let cards = pb.cards.clone();
        let size = px(card_dp(b.kind));
        for (i, c) in b.shown_cards().iter().enumerate() {
            if c.extra.starts_with("icon:") {
                continue;
            }
            for (slot, uri) in c.covers.iter().filter(|u| !u.is_empty()).take(2).enumerate() {
                let m = cards.clone();
                images.get(&api::pic(uri, size), size, move |img| {
                    if let Some(mut row) = m.row_data(i) {
                        if slot == 0 {
                            row.cover = img.clone();
                        } else {
                            row.cover2 = img.clone();
                        }
                        m.set_row_data(i, row);
                    }
                });
            }
        }
        let rows = pb.rows.clone();
        let size = px(52.0);
        for (i, t) in b.tracks[shown_rows].iter().enumerate() {
            let Some(uri) = &t.cover else { continue };
            let m = rows.clone();
            images.get(&api::pic(uri, size), size, move |img| {
                if let Some(mut row) = m.row_data(i) {
                    row.cover = img.clone();
                    m.set_row_data(i, row);
                }
            });
        }
    });
}

// ---------- загрузка ----------

fn str_at<'a>(v: &'a Value, p: &str) -> &'a str {
    v.pointer(p).and_then(|x| x.as_str()).unwrap_or("")
}

fn lcard_from(c: &api::Card) -> LCard {
    LCard {
        title: c.title.clone(),
        subtitle: if c.year.is_empty() { c.subtitle.clone() } else { c.year.clone() },
        covers: c.covers.clone(),
        round: c.round,
        explicit: c.explicit,
        target: c.target.clone(),
        ..Default::default()
    }
}

/// Блок лендинга мобильного клиента → блок страницы.
fn landing_block(kind: &str, title: &str, more: bool, uri: &str, b: LBlock) -> Block {
    let more_key = more.then(|| Key::Block(kind.to_string(), uri.to_string(), title.to_string()));
    let mut out = match kind {
        "LIKES_AND_HISTORY" => Block::cards(PAIR, "", b.cards),
        "ALBUM_PROMO" | "SIMPLE_ALBUM_PROMO" => Block::cards(PROMO, title, b.cards),
        "SETS_BY_WAVES" | "WAVES" => Block { tabs: b.tabs.into_iter().map(|(t, _, c)| (t, c)).collect(), ..Block::new(VIBES, title) },
        "IN_STYLE" => Block { tabs: b.tabs.into_iter().map(|(t, _, c)| (t, c)).collect(), ..Block::new(CARDS, title) },
        "NEW_RELEASES" | "EDITORIAL_NEW_RELEASES" => Block::cards(
            RELEASES,
            title,
            b.cards
                .into_iter()
                .map(|mut c| {
                    c.covers.reverse();
                    c
                })
                .collect(),
        ),
        "CONCERTS_PERSONAL" | "CONCERTS_TOP" | "VIEWED_CONCERTS" => Block::cards(CONCERTS, title, b.cards.into_iter().take(4).collect()),
        "SMART_OPEN_PLAYLIST" | "OPEN_PLAYLIST" | "NON_MUSIC_OPEN_PLAYLIST" => {
            let mut x = Block::tracks(title, b.tracks.into_iter().take(5).collect());
            if let Some(t) = b.target.as_ref().and_then(|t| t.strip_prefix("playlist:")).and_then(|t| t.split_once(':')) {
                x.more = Some(Key::Playlist(t.0.into(), t.1.into()));
            }
            return x;
        }
        "CHART_TRACKS" => {
            let pos = b.chart.iter().map(|(p, _)| p.to_string()).collect();
            // Блок лендинга отдаёт только топ-10, весь чарт — /landing3/chart, как на ПК.
            let more = Some(Key::Block(kind.to_string(), "/landing3/chart".into(), title.to_string()));
            return Block { pos, more, ..Block::tracks(title, b.tracks.into_iter().take(5).collect()) };
        }
        "DONATIONS" | "EDITORIAL_ARTISTS" => Block::cards(ARTISTS, title, b.cards),
        "EDITORIAL_PROMOTIONS" => Block::cards(BANNERS, title, b.cards),
        "SPECIAL" => match b.banner {
            // Платное промо с пометкой «Реклама» не показываем.
            Some(bn) if !bn.ad => Block::cards(
                BANNERS,
                title,
                vec![LCard { title: bn.title, subtitle: bn.subtitle, covers: bn.image.into_iter().collect(), target: format!("url:{}", bn.url), ..Default::default() }],
            ),
            _ => Block::default(),
        },
        "MIXES" | "MIXES_MUSIC" | "MIXES_GRID" => Block::cards(MIXES, if title.is_empty() { "Подборки музыки" } else { title }, b.cards),
        "CLIPS" | "EDITORIAL_CLIPS" | "DYNAMIC_BANNER" | "STATIC_DIV" | "NON_MUSIC_BANNER" | "KIDS_EASY_LOGIN_PROMO" => Block::default(),
        "HISTORY" => Block::cards(ROWS, title, b.cards),
        _ => {
            if !b.tracks.is_empty() {
                Block::tracks(title, b.tracks)
            } else {
                let all_round = !b.cards.is_empty() && b.cards.iter().all(|c| c.round);
                Block::cards(if all_round { ARTISTS } else { CARDS }, title, b.cards)
            }
        }
    };
    if out.more.is_none() {
        out.more = more_key;
    }
    out
}

/// Скелет мобильного клиента: блоки над вкладками и блоки каждой вкладки.
fn skeleton(api: &Api, id: &str) -> Result<(Vec<landing::BlockDef>, Vec<(String, Vec<landing::BlockDef>)>), String> {
    let v = api.get_raw(&format!("/landing/skeleton/{id}?showWizard=false"))?;
    let sk = landing::parse_skeleton(&v, None);
    Ok((sk.head, sk.tabs.into_iter().map(|t| (t.title, t.blocks)).collect()))
}

/// Все блоки грузятся параллельно, порядок — как в скелете.
fn load_blocks(api: &Api, defs: &[landing::BlockDef]) -> Vec<Block> {
    std::thread::scope(|sc| {
        let handles: Vec<_> = defs
            .iter()
            .map(|d| {
                sc.spawn(move || {
                    let uri = d.uri.as_deref()?;
                    let v = api.get_raw(uri).map_err(|e| log(format!("блок {}: {e}", d.kind))).ok()?;
                    let b = landing_block(&d.kind, &d.title, d.more, uri, landing::parse_block(&d.kind, &v));
                    if b.is_empty() {
                        log(format!("пустой блок {} {uri}: {}", d.kind, v.to_string().chars().take(1500).collect::<String>()));
                    }
                    Some(b)
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok().flatten()).filter(|b| !b.is_empty() && b.kind != 0).collect()
    })
}

fn shortcut(title: &str, icon: &str, target: &str, bg: [u8; 3], fg: [u8; 3]) -> LCard {
    LCard { title: title.into(), extra: format!("icon:{icon}"), target: target.into(), bg: Some(bg), fg: Some(fg), ..Default::default() }
}

pub fn tracks_word(n: u64) -> String {
    crate::net::tracks_word(n)
}

fn fetch(api: &Api, uid: &str, key: &Key) -> Result<PageData, String> {
    let mut d = match key {
        Key::Home => {
            let (_, tabs) = skeleton(api, "main")?;
            let defs = tabs.into_iter().next().map(|t| t.1).unwrap_or_default();
            let mut head = vec![Block::cards(
                SHORTCUTS,
                "",
                vec![
                    shortcut("Тренды", "chart", "landing:main:1", [0xff, 0x2d, 0x7a], [0xff, 0x8a, 0x3d]),
                    shortcut("Книги", "book", "landing:non_music:0", [0x2b, 0x8c, 0xff], [0x7a, 0xd3, 0xff]),
                    shortcut("Детям", "kids", "landing:kids:0", [0xff, 0x8a, 0x00], [0xff, 0xd0, 0x3d]),
                    shortcut("Подкасты", "podcast", "landing:non_music:1", [0x1f, 0xb5, 0x4a], [0x9b, 0xe8, 0x5a]),
                ],
            )];
            head.extend(load_blocks(api, &defs));
            PageData { has_search: true, ..PageData::new("Что послушать", head) }
        }
        Key::Landing(skel, tab, title) => {
            let (head_defs, tabs) = skeleton(api, skel)?;
            let mut head = load_blocks(api, &head_defs);
            let defs = tabs.get(*tab).or(tabs.first()).map(|t| t.1.clone()).unwrap_or_default();
            head.extend(load_blocks(api, &defs));
            PageData { has_search: true, ..PageData::new(title, head) }
        }
        Key::Tag(id) => {
            let (title, cards) = api.tag_playlists(id)?;
            PageData::new(&title, vec![Block::cards(GRID, "", cards.iter().map(lcard_from).collect())])
        }
        Key::Concerts => {
            let v = api.get_raw(FEED)?;
            let b = landing::parse_block("CONCERTS_PERSONAL", &v);
            PageData { has_search: true, ..PageData::new("Концерты рядом", vec![Block::cards(CONCERTS, "Концерты для вас", b.cards)]) }
        }
        Key::Collection => collection(api, uid)?,
        Key::Search => {
            let sug = api.get_raw("/search/suggestions/q2v").unwrap_or_default();
            let suggest: Vec<LCard> = sug
                .get("items")
                .and_then(|i| i.as_array())
                .into_iter()
                .flatten()
                .filter_map(|x| x.get("query")?.as_str())
                .map(|q| LCard { title: q.into(), target: format!("query:{q}"), ..Default::default() })
                .collect();
            let (_, tabs) = skeleton(api, "Search")?;
            let mut d = PageData::new("", vec![Block::cards(SUGGEST, "", suggest)]);
            let titles: Vec<(String, Vec<LCard>)> = tabs.iter().map(|t| (t.0.clone(), Vec::new())).collect();
            d.head.push(Block { tabs: titles, ..Block::new(CHIPS, "") });
            d.tabs = tabs.iter().map(|(t, defs)| (t.clone(), load_blocks(api, defs))).collect();
            d
        }
        Key::Results(q) => search(api, q)?,
        Key::Artist(id) => artist(api, id)?,
        Key::ArtistTracks(id) => PageData::new("Все треки", vec![Block::new(PLAY, ""), Block::tracks("", api.artist_tracks(id)?)]),
        Key::ArtistAlbums(id) => {
            let cards = api.artist_albums(id, "direct-albums")?.iter().map(lcard_from).collect();
            PageData::new("Альбомы", vec![Block::cards(GRID, "", cards)])
        }
        Key::ArtistPlaylists(id) => {
            let v = api.get_raw(&format!("/artists/{id}/blocks/artist-playlists"))?;
            PageData::new("Плейлисты", vec![Block::cards(GRID, "", landing::parse_block("", &v).cards)])
        }
        Key::Similar(id) => {
            let cards = api.similar_artists(id)?.iter().map(lcard_from).collect();
            PageData::new("Похожие исполнители", vec![Block::cards(GRID, "", cards)])
        }
        Key::About(id) => {
            let v = api.get_raw(&format!("/artists/{id}/brief-info"))?;
            let text = str_at(&v, "/artist/description/text").to_string();
            let text = if text.is_empty() { "Описания пока нет.".to_string() } else { text };
            PageData::new("Об исполнителе", vec![Block { text, ..Block::new(TEXT, "") }])
        }
        Key::Album(id) => {
            let (a, tracks) = api.album(id)?;
            let year = a.year.map(|y| y.to_string()).unwrap_or_default();
            let sub = [a.artists.clone(), year.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            let bg = tracks.iter().find_map(|t| t.color);
            // Карточка шапки несёт аватар исполнителя, а для просмотра обложки — исполнителей и год.
            let avatar = LCard { title: a.artists.clone(), subtitle: year, covers: a.artist_cover.iter().cloned().collect(), ..Default::default() };
            let liked = api.liked_album_ids(uid).is_ok_and(|ids| ids.contains(&a.id));
            let head = Block {
                subtitle: sub,
                image: a.cover.clone(),
                text: likes_text(a.likes),
                liked,
                bg,
                cards: vec![avatar],
                more: a.artist_list.first().map(|ar| Key::Artist(ar.id.clone())),
                artists: a.artist_list.clone(),
                ..Block::new(ALBUM_HEAD, &a.title)
            };
            let pos = (1..=tracks.len()).map(|n| n.to_string()).collect();
            let list = Block { kind: NUMBERED, pos, subtitle: a.artists.clone(), ..Block::tracks("", tracks) };
            let cover = LCard { covers: a.cover.iter().cloned().collect(), ..Default::default() };
            let wave = Block { subtitle: a.title.clone(), bg, cards: vec![cover], ..Block::new(WAVE, "Моя волна по альбому") };
            PageData { overlay: true, has_search: true, entity: Entity::Album(a.id.clone()), ..PageData::new(&a.title, vec![head, list, wave]) }
        }
        Key::Playlist(u, k) => {
            let (p, tracks) = api.playlist(u, k)?;
            let sub = [p.owner.clone(), tracks_word(tracks.len() as u64)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            let liked = api.liked_playlist_keys(uid).is_ok_and(|keys| keys.contains(&format!("playlist:{u}:{k}")));
            let head = Block { subtitle: sub, image: p.covers.first().cloned(), text: likes_text(p.likes), liked, ..Block::new(LIST_HEAD, &p.title) };
            PageData { entity: Entity::Playlist(p.uid.clone(), p.kind.clone()), ..PageData::new(&p.title, vec![head, Block::tracks("", tracks)]) }
        }
        Key::Likes => {
            let tracks = api.liked_tracks(uid)?;
            PageData::new("Мне нравится", vec![Block::new(PLAY, ""), Block::tracks("", tracks)])
        }
        Key::LikedArtists => {
            let cards = api
                .liked_artists(uid)?
                .into_iter()
                .map(|a| LCard { title: a.name, covers: a.cover.into_iter().collect(), round: true, target: format!("artist:{}", a.id), ..Default::default() })
                .collect();
            PageData::new("Любимые исполнители", vec![Block::cards(GRID, "", cards)])
        }
        Key::Dislikes => {
            let v = api.get_raw(&format!("/users/{uid}/dislikes/tracks"))?;
            let ids: Vec<String> = v
                .pointer("/library/tracks")
                .and_then(|t| t.as_array())
                .into_iter()
                .flatten()
                .filter_map(|t| t.get("id").map(|id| id.as_str().map(String::from).unwrap_or_else(|| id.to_string())))
                .take(300)
                .collect();
            let tracks = if ids.is_empty() { Vec::new() } else { api.tracks(&ids)? };
            let artists = api
                .disliked_artists(uid)?
                .into_iter()
                .map(|a| LCard { title: a.name, covers: a.cover.into_iter().collect(), round: true, target: format!("artist:{}", a.id), ..Default::default() })
                .collect();
            let or_empty = |b: Block| if b.is_empty() { Block { text: "Здесь пока пусто".into(), ..Block::new(EMPTY, "") } } else { b };
            let tabs = vec![
                ("Треки".to_string(), vec![or_empty(Block::tracks("", tracks))]),
                ("Исполнители".to_string(), vec![or_empty(Block::cards(GRID, "", artists))]),
            ];
            let mut d = PageData::new("Ваши дизлайки", vec![]);
            d.head.push(Block { tabs: tabs.iter().map(|t| (t.0.clone(), Vec::new())).collect(), ..Block::new(CHIPS, "") });
            d.tabs = tabs;
            d
        }
        Key::Playlists => {
            let playlists = api.collection_playlists()?;
            let mut d = PageData::new("Плейлисты", vec![]);
            d.head.push(Block { tabs: playlists.iter().map(|t| (t.0.clone(), Vec::new())).collect(), ..Block::new(CHIPS, "") });
            d.tabs = sorted_playlists(&playlists, PLAYLIST_SORT.load(Ordering::Relaxed));
            d.playlists = playlists;
            d
        }
        Key::Albums => {
            let tabs: Vec<(String, Vec<LCard>)> = api.collection_tabs("albums-liked-and-albums-presaved")?.into_iter().map(|(t, c)| (t, c.iter().map(lcard_from).collect())).collect();
            let mut d = PageData::new("Альбомы", vec![]);
            d.head.push(Block { tabs: tabs.iter().map(|t| (t.0.clone(), Vec::new())).collect(), ..Block::new(CHIPS, "") });
            d.tabs = tabs.into_iter().map(|(t, c)| (t, vec![Block::cards(GRID, "", c)])).collect();
            d
        }
        Key::History => {
            let days = crate::history::load(api)?;
            let blocks = days
                .iter()
                .map(|day| {
                    let cards = day
                        .groups
                        .iter()
                        .map(|g| LCard { title: g.title.clone(), subtitle: g.subtitle.clone(), covers: g.cover.iter().cloned().collect(), target: g.target.clone(), ..Default::default() })
                        .collect();
                    Block::cards(ROWS, &day.date, cards)
                })
                .collect();
            PageData::new("История", blocks)
        }
        Key::Block(kind, uri, title) => {
            let v = api.get_raw(uri)?;
            let b = landing::parse_block(kind, &v);
            let block = if !b.tracks.is_empty() {
                Block { pos: b.chart.iter().map(|(p, _)| p.to_string()).collect(), ..Block::tracks("", b.tracks) }
            } else if kind.contains("CONCERT") {
                Block::cards(CONCERTS, "", b.cards)
            } else {
                let mut cards = b.cards;
                if kind.contains("NEW_RELEASES") {
                    cards.iter_mut().for_each(|c| c.covers.reverse());
                }
                Block::cards(GRID, "", cards)
            };
            PageData::new(title, vec![block])
        }
    };
    if d.head.is_empty() && d.tabs.is_empty() {
        d.head.push(Block { text: "Здесь пока пусто".into(), ..Block::new(EMPTY, "") });
    }
    Ok(d)
}

/// «3 часа», «40 минут» — как подпись исполнителя в коллекции оригинала.
fn listened(seconds: u64) -> String {
    let word = |n: u64, forms: [&str; 3]| {
        let (m10, m100) = (n % 10, n % 100);
        let f = if m10 == 1 && m100 != 11 {
            forms[0]
        } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
            forms[1]
        } else {
            forms[2]
        };
        format!("{n} {f}")
    };
    if seconds >= 3600 {
        word(seconds / 3600, ["час", "часа", "часов"])
    } else {
        word((seconds / 60).max(1), ["минута", "минуты", "минут"])
    }
}

fn likes_text(n: u64) -> String {
    if n == 0 {
        String::new()
    } else {
        crate::net::group_digits(n)
    }
}

fn collection(api: &Api, uid: &str) -> Result<PageData, String> {
    let c = api.collection(uid)?;
    let sk = api.get_raw("/landing/skeleton/collection?showWizard=false").unwrap_or_default();
    let heart = sk
        .pointer("/blocks/0/data/tabs/0/blocks")
        .and_then(|b| b.as_array())
        .and_then(|a| a.iter().find(|b| b.get("type").and_then(|t| t.as_str()) == Some("COLLECTION_FAVOURITE_PLAYLIST")))
        .and_then(|b| b.pointer("/data/cover/uri"))
        .and_then(|u| u.as_str())
        .map(String::from);
    let likes = Block { subtitle: tracks_word(c.likes_count), image: heart, more: Some(Key::Likes), ..Block::new(LIKES_HEAD, "Мне нравится") };
    let preview = Block::tracks("", c.likes_preview.into_iter().take(3).collect());
    let first_names = |names: Vec<String>| names.into_iter().take(3).collect::<Vec<_>>().join(", ");
    let pls: Vec<_> = c.playlist_tabs.first().map(|t| t.1.clone()).unwrap_or_default();
    let als: Vec<_> = c.album_tabs.first().map(|t| t.1.clone()).unwrap_or_default();
    let tiles = vec![
        LCard {
            title: "Плейлисты".into(),
            subtitle: first_names(pls.iter().map(|p| p.title.clone()).collect()),
            covers: pls.first().and_then(|p| p.covers.first().cloned()).into_iter().collect(),
            target: "playlists".into(),
            ..Default::default()
        },
        LCard {
            title: "Альбомы".into(),
            subtitle: first_names(als.iter().map(|a| a.title.clone()).collect()),
            covers: als.first().and_then(|a| a.cover.clone()).into_iter().collect(),
            target: "albums".into(),
            ..Default::default()
        },
        LCard { title: "Клипы".into(), extra: "icon:clip".into(), ..Default::default() },
        LCard { title: "Книги".into(), extra: "icon:book".into(), target: "landing:non_music:0".into(), ..Default::default() },
        LCard { title: "Подкасты".into(), extra: "icon:podcast".into(), target: "landing:non_music:1".into(), ..Default::default() },
        LCard { title: "Детям".into(), extra: "icon:kids".into(), target: "landing:kids:0".into(), ..Default::default() },
    ];
    let mut head = vec![likes, preview];
    if !c.top_artists.is_empty() {
        let cards = c
            .top_artists
            .iter()
            .map(|t| LCard {
                title: t.artist.name.clone(),
                subtitle: format!("{} · {}", t.position, listened(t.seconds)),
                covers: t.artist.cover.iter().cloned().collect(),
                round: true,
                target: format!("artist:{}", t.artist.id),
                ..Default::default()
            })
            .collect();
        head.push(Block::cards(ARTISTS, "Любимые исполнители", cards).more(Key::LikedArtists));
    }
    head.push(Block::cards(TILES, "Ещё у вас в Коллекции", tiles));
    head.push(Block::new(HEADING, "Ваши дизлайки").more(Key::Dislikes));
    Ok(PageData { has_search: true, ..PageData::new("Коллекция", head) })
}

fn search(api: &Api, q: &str) -> Result<PageData, String> {
    let r = api.search(q, "all")?;
    let artists = r.artists.iter().map(|a| LCard { title: a.name.clone(), covers: a.cover.iter().cloned().collect(), round: true, target: format!("artist:{}", a.id), ..Default::default() }).collect();
    let albums = r.albums.iter().map(|a| lcard_from(&api::album_card(a))).collect();
    let playlists = r.playlists.iter().map(|p| lcard_from(&api::playlist_card(p))).collect();
    let mut head = vec![Block::tracks("Треки", r.tracks.into_iter().take(10).collect()), Block::cards(ARTISTS, "Исполнители", artists), Block::cards(CARDS, "Альбомы", albums), Block::cards(CARDS, "Плейлисты", playlists)];
    head.retain(|b| !b.is_empty());
    if head.is_empty() {
        head.push(Block { text: format!("По запросу «{q}» ничего не нашлось"), ..Block::new(EMPTY, "") });
    }
    Ok(PageData { head, ..Default::default() })
}

fn artist(api: &Api, id: &str) -> Result<PageData, String> {
    let a = api.artist_full(id)?;
    let brief = api.get_raw(&format!("/artists/{id}/brief-info")).unwrap_or_default();
    let likes = brief.pointer("/artist/likesCount").and_then(|x| x.as_u64()).unwrap_or(0);
    let about = str_at(&brief, "/artist/description/text").to_string();
    let photo = a.cover.clone();
    let head = Block {
        subtitle: a.listeners.map(|n| format!("{} за месяц", crate::net::group_digits(n))).unwrap_or_default(),
        image: photo.clone(),
        bg: a.color,
        text: likes_text(likes),
        liked: false,
        ..Block::new(ARTIST_HEAD, &a.name)
    };
    let mut blocks = vec![
        head,
        Block::tracks("Популярные треки", a.popular.clone()).more(Key::ArtistTracks(id.into())),
        Block { subtitle: a.name.clone(), ..Block::new(WAVE, "Моя волна по артисту") },
    ];
    let albums: Vec<LCard> = if a.albums.is_empty() { &a.studio } else { &a.albums }.iter().map(lcard_from).collect();
    blocks.push(Block::cards(CARDS, "Популярные альбомы", albums).more(Key::ArtistAlbums(id.into())));
    if let Some(r) = &a.release {
        blocks.push(Block::cards(CARDS, "Новый релиз", vec![lcard_from(r)]));
    }
    if !a.playlists.is_empty() {
        blocks.push(Block { subtitle: a.name.clone(), ..Block::new(LINK, "Плейлисты, где встречается") }.more(Key::ArtistPlaylists(id.into())));
    }
    if !about.is_empty() {
        blocks.push(Block { image: photo, ..Block::new(ABOUT, "Об исполнителе") }.more(Key::About(id.into())));
    }
    if !a.compilations.is_empty() {
        blocks.push(Block::cards(CARDS, "Сборники", a.compilations.iter().map(lcard_from).collect()));
    }
    blocks.push(Block::cards(ARTISTS, "Похожие исполнители", a.similar.iter().map(lcard_from).collect()).more(Key::Similar(id.into())));
    Ok(PageData { overlay: true, has_search: true, entity: Entity::Artist(id.into()), ..PageData::new(&a.name, blocks) })
}
