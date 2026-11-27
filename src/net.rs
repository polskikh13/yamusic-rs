//! Данные страниц: грузятся в фоне по ключу, пока грузятся свежие — показываются старые.

use crate::api::{Album, Api, Artist, ArtistFull, Collection, Lyrics, Playlist, Res, Search, SearchLanding, Track};
use crate::config::log;
use crate::player::{After, Cmd, ListKind, Player, Source};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone)]
pub struct ListData {
    pub kind: ListKind,
    pub title: String,
    pub covers: Vec<String>,
    pub tracks: Vec<Track>,
    pub playlist: Option<Playlist>,
    pub album: Option<Album>,
    /// Полный список «Популярных треков» исполнителя.
    pub artist: Option<Artist>,
}

#[derive(Clone)]
pub enum Data {
    Search(Search),
    Artist(ArtistFull),
    SearchLanding(SearchLanding),
    List(ListData),
    Playlists(Vec<Playlist>),
    Lyrics(Lyrics),
    Collection(Collection),
    Skeleton(crate::landing::Skeleton),
    Block(crate::landing::LBlock),
    History(Vec<crate::history::Day>),
}

struct Slot {
    data: Option<Result<Arc<Data>, String>>,
    loading: bool,
    refresh_again: bool,
    at: Instant,
}

impl Slot {
    fn edit(&mut self) -> Option<&mut Data> {
        self.refresh_again |= self.loading;
        self.data.as_mut()?.as_mut().ok().map(Arc::make_mut)
    }
}

fn set_track_liked(tracks: &mut Vec<Track>, track: &Track, liked: bool) -> bool {
    match (liked, tracks.iter().position(|t| t.id == track.id)) {
        (true, None) => tracks.insert(0, track.clone()),
        (false, Some(i)) => { tracks.remove(i); }
        _ => return false,
    }
    true
}

type Store = Arc<Mutex<HashMap<String, Slot>>>;
pub type Toast = Arc<Mutex<Option<(String, Instant)>>>;
pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct Net {
    pub api: Api,
    pub uid: String,
    store: Store,
    notify: Notify,
    pub toast: Toast,
}

fn plural(n: u64, one: &str, few: &str, many: &str) -> String {
    let (m10, m100) = (n % 10, n % 100);
    let w = if m10 == 1 && m100 != 11 {
        one
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        few
    } else {
        many
    };
    format!("{} {w}", group_digits(n))
}

/// «6 441 232»: разряды через пунктуационный пробел U+2008 — он шире обычного
/// (0,26 em против 0,2 в YS Text), числа читаются легче.
pub fn group_digits(n: u64) -> String {
    let d = n.to_string();
    let mut out = String::with_capacity(d.len() + d.len() / 3 * 3);
    for (i, ch) in d.chars().enumerate() {
        if i > 0 && (d.len() - i).is_multiple_of(3) {
            out.push('\u{2008}');
        }
        out.push(ch);
    }
    out
}

pub fn tracks_word(n: u64) -> String {
    plural(n, "трек", "трека", "треков")
}

pub fn listeners_word(n: u64) -> String {
    format!("{} в месяц", plural(n, "слушатель", "слушателя", "слушателей"))
}

fn fetch(api: &Api, uid: &str, key: &str) -> Res<Data> {
    let (kind, arg) = key.split_once(':').unwrap_or((key, ""));
    match kind {
        "search" => {
            let (t, text) = arg.split_once(':').unwrap_or(("all", arg));
            api.search(text, t).map(Data::Search)
        }
        "artist" => api.artist_full(arg).map(Data::Artist),
        "search-landing" => api.search_landing().map(Data::SearchLanding),
        "album" => api.album(arg).map(|(a, tracks)| {
            Data::List(ListData {
                kind: ListKind::Album,
                title: a.title.clone(),
                covers: a.cover.iter().cloned().collect(),
                tracks,
                playlist: None,
                album: Some(a),
                artist: None,
            })
        }),
        "playlist" => {
            let (owner, k) = arg.split_once(':').ok_or("плохой ключ плейлиста")?;
            api.playlist(owner, k).map(|(p, tracks)| {
                Data::List(ListData {
                    kind: ListKind::Playlist,
                    title: p.title.clone(),
                    covers: p.covers.clone(),
                    tracks,
                    playlist: Some(p),
                    album: None,
                    artist: None,
                })
            })
        }
        "likes" => api.liked_tracks(uid).map(|tracks| {
            Data::List(ListData {
                kind: ListKind::Likes,
                title: "Мне нравится".into(),
                covers: api.likes_cover(uid).into_iter().collect(),
                tracks,
                playlist: None,
                album: None,
                artist: None,
            })
        }),
        "dislikes" => {
            let ids = api.disliked_ids(uid)?;
            let tracks = if ids.is_empty() { Vec::new() } else { api.tracks(&ids)? };
            Ok(Data::List(ListData {
                kind: ListKind::Tracks,
                title: "Мои дизлайки".into(),
                covers: Vec::new(),
                tracks,
                playlist: None,
                album: None,
                artist: None,
            }))
        }
        "new-episodes" => api.new_episodes().map(|(title, tracks)| {
            Data::List(ListData {
                kind: ListKind::Tracks,
                title: if title.is_empty() { "Новые выпуски".into() } else { title },
                covers: Vec::new(),
                tracks,
                playlist: None,
                album: None,
                artist: None,
            })
        }),
        "artist-tracks" | "familiar" => {
            let info = api.get_raw(&format!("/artists/{arg}/info"))?;
            let a = info.get("artist").and_then(crate::api::parse_artist).ok_or("нет данных об исполнителе")?;
            let familiar = kind == "familiar";
            Ok(Data::List(ListData {
                kind: ListKind::Artist,
                title: if familiar { "Знакомое вам".into() } else { a.name.clone() },
                covers: a.cover.iter().cloned().collect(),
                tracks: if familiar { api.artist_familiar(arg)? } else { api.artist_tracks(arg)? },
                playlist: None,
                album: None,
                artist: Some(a),
            }))
        }
        "playlists" => api.playlists(uid).map(Data::Playlists),
        "lyrics" => api.lyrics(arg).map(Data::Lyrics),
        "collection" => api.collection(uid).map(Data::Collection),
        "history" => crate::history::load(api).map(Data::History),
        "skeleton" => {
            let v = api.get_raw(&format!("/landing/skeleton/{arg}?showWizard=false"))?;
            let tabs = v
                .pointer("/blocks")
                .and_then(|b| b.as_array())
                .and_then(|a| a.iter().find_map(|b| b.pointer("/data/source/uri").and_then(|u| u.as_str()).filter(|u| u.ends_with("/tabs"))))
                .and_then(|u| api.get_raw(u).ok());
            Ok(Data::Skeleton(crate::landing::parse_skeleton(&v, tabs.as_ref())))
        }
        "lblock" => {
            let (kind, uri) = arg.split_once('|').ok_or("плохой ключ блока")?;
            api.get_raw(uri).map(|v| Data::Block(crate::landing::parse_block(kind, &v)))
        }
        _ => Err(format!("неизвестный ключ {key}")),
    }
}

impl Net {
    pub fn new(api: Api, uid: String, notify: Notify) -> Self {
        Net { api, uid, store: Default::default(), notify, toast: Default::default() }
    }

    /// None — ещё грузится.
    pub fn get(&self, key: &str) -> Option<Result<Arc<Data>, String>> {
        let mut st = self.store.lock().unwrap();
        if let Some(s) = st.get_mut(key) {
            s.at = Instant::now();
            return s.data.clone();
        }
        st.insert(key.to_string(), Slot { data: None, loading: true, refresh_again: false, at: Instant::now() });
        if st.len() > 80 {
            let oldest = st.iter().filter(|(_, s)| !s.loading).min_by_key(|(_, s)| s.at).map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                st.remove(&k);
            }
        }
        drop(st);
        self.load(key);
        None
    }

    /// Есть ли ключ в кэше (загружен или грузится), без запроса.
    pub fn peek(&self, key: &str) -> Option<()> {
        self.store.lock().unwrap().contains_key(key).then_some(())
    }

    pub fn refresh(&self, key: &str) {
        let mut st = self.store.lock().unwrap();
        match st.get_mut(key) {
            Some(s) if s.loading => { s.refresh_again = true; return; }
            Some(s) => s.loading = true,
            None => return,
        }
        drop(st);
        self.load(key);
    }

    /// Обновляем «Мне нравится» и коллекцию в том порядке, в каком сервер подтвердил лайки.
    pub fn like_saved(&self, track: &Track, liked: bool) {
        self.update_likes(track, liked, true);
    }

    /// Выкинуть ключ из кэша: при следующем открытии загрузится заново.
    pub fn forget(&self, key: &str) {
        self.store.lock().unwrap().remove(key);
    }

    pub(crate) fn dislike_saved(&self, track: &Track) {
        self.forget("dislikes");
        // Дизлайк мог относиться к треку, у которого вообще не было лайка.
        self.update_likes(track, false, false);
    }

    fn update_likes(&self, track: &Track, liked: bool, toggled_like: bool) {
        let mut store = self.store.lock().unwrap();
        for key in ["likes", "collection"] {
            if store.get(key).is_some_and(|s| !s.loading && matches!(s.data, Some(Err(_)))) {
                store.remove(key);
            }
        }
        let full = match store.get_mut("likes").and_then(Slot::edit) {
            Some(Data::List(list)) => {
                let changed = set_track_liked(&mut list.tracks, track, liked);
                Some((changed, list.tracks.iter().take(8).cloned().collect::<Vec<_>>()))
            }
            _ => None,
        };
        let mut refill = false;
        if let Some(Data::Collection(c)) = store.get_mut("collection").and_then(Slot::edit) {
            let was_visible = c.likes_preview.iter().any(|t| t.id == track.id);
            let changed = full.as_ref().map_or(toggled_like, |(changed, _)| *changed);
            if liked && !was_visible && changed {
                c.likes_count += 1;
            } else if !liked && (changed || was_visible) {
                c.likes_count = c.likes_count.saturating_sub(1);
            }
            if let Some((_, preview)) = full {
                c.likes_preview = preview;
            } else {
                set_track_liked(&mut c.likes_preview, track, liked);
                c.likes_preview.truncate(8);
                refill = !liked && (!was_visible || c.likes_preview.len() < c.likes_count.min(8) as usize);
            }
        }
        drop(store);
        (self.notify)();
        if refill {
            if let Ok((count, tracks)) = self.api.collection_likes() {
                let mut store = self.store.lock().unwrap();
                if let Some(Data::Collection(c)) = store.get_mut("collection").and_then(Slot::edit) {
                    c.likes_count = count;
                    c.likes_preview = tracks;
                }
                drop(store);
                (self.notify)();
            }
        }
    }

    fn load(&self, key: &str) {
        let (api, uid, store, notify, key) = (self.api.clone(), self.uid.clone(), self.store.clone(), self.notify.clone(), key.to_string());
        std::thread::spawn(move || loop {
            let res = fetch(&api, &uid, &key).map(Arc::new);
            if let Err(e) = &res {
                log(format!("{key}: {e}"));
            }
            let mut st = store.lock().unwrap();
            let slot = st.entry(key.clone()).or_insert(Slot { data: None, loading: true, refresh_again: false, at: Instant::now() });
            if std::mem::take(&mut slot.refresh_again) { continue; }
            match (&slot.data, res) {
                (Some(Ok(_)), Err(_)) => {}
                (_, res) => slot.data = Some(res),
            }
            slot.loading = false;
            drop(st);
            notify();
            break;
        });
    }

    pub fn say(&self, msg: impl Into<String>) {
        *self.toast.lock().unwrap() = Some((msg.into(), Instant::now()));
        (self.notify)();
    }

    /// «Слушать» у исполнителя: все его треки по кругу, начиная с `start`.
    pub fn play_artist(&self, player: &Player, id: &str, name: &str, start: Option<String>) {
        let (api, player, id, name, me) = (self.api.clone(), player.clone(), id.to_string(), name.to_string(), self.clone());
        std::thread::spawn(move || match api.artist_tracks(&id) {
            Ok(tracks) => {
                let start = start.and_then(|s| tracks.iter().position(|t| t.id == s)).unwrap_or(0);
                player.send(Cmd::PlayList {
                    source: Source::List { kind: ListKind::Artist, id, title: name },
                    tracks,
                    start,
                    after: After::Loop,
                });
            }
            Err(e) => me.say(format!("Не удалось загрузить треки исполнителя: {e}")),
        });
    }

    /// `what` — название для тоста: трек, альбом или плейлист целиком.
    pub fn add_to_playlist(&self, pl: &Playlist, tracks: Vec<Track>, what: String) {
        let (me, pl) = (self.clone(), pl.clone());
        std::thread::spawn(move || match me.api.add_to_playlist(&me.uid, &pl.kind, &tracks) {
            Ok(()) => {
                me.say(format!("«{what}» добавлен в «{}»", pl.title));
                me.refresh(&format!("playlist:{}:{}", pl.uid, pl.kind));
                me.refresh("playlists");
            }
            Err(e) => me.say(format!("Не удалось добавить в плейлист: {e}")),
        });
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_like_updates_open_and_cached_favorites() {
        let track = |i: u32| crate::api::parse_track(&serde_json::json!({"id": i, "title": format!("Track {i}")})).unwrap();
        let tracks: Vec<_> = (1..=12).map(track).collect();
        let net = Net::new(Api::new(""), "test".into(), Arc::new(|| {}));
        let slot = |data| Slot { data: Some(Ok(Arc::new(data))), loading: false, refresh_again: false, at: Instant::now() };
        let collection = Collection { likes_count: 12, likes_preview: tracks[..8].to_vec(), ..Default::default() };
        let list = ListData { kind: ListKind::Likes, title: String::new(), covers: vec![], tracks, playlist: None, album: None, artist: None };
        {
            let mut store = net.store.lock().unwrap();
            store.insert("collection".into(), slot(Data::Collection(collection)));
            store.insert("likes".into(), slot(Data::List(list)));
        }
        let snapshot = |key: &str| net.get(key).unwrap().unwrap();
        let open = snapshot("likes");
        for (liked, first, count) in [(true, "99", 13), (true, "99", 13), (false, "1", 12), (false, "1", 12)] {
            net.like_saved(&track(99), liked);
            let (list, collection) = (snapshot("likes"), snapshot("collection"));
            let (Data::List(l), Data::Collection(c)) = (&*list, &*collection) else { panic!("favorites cache") };
            assert!(!Arc::ptr_eq(&list, &open));
            assert_eq!(l.tracks.len(), count);
            assert_eq!(c.likes_count, count as u64);
            assert_eq!(c.likes_preview.len(), 8);
            assert_eq!(c.likes_preview[0].id, first);
            assert!(c.likes_preview.iter().zip(&l.tracks).all(|(a, b)| a.id == b.id));
        }
        net.dislike_saved(&track(2));
        net.dislike_saved(&track(2));
        let (list, collection) = (snapshot("likes"), snapshot("collection"));
        let (Data::List(list), Data::Collection(c)) = (&*list, &*collection) else { unreachable!() };
        assert!(!list.tracks.iter().chain(&c.likes_preview).any(|t| t.id == "2"));
        assert_eq!(c.likes_count, 11);
        let Data::List(original) = &*open else { unreachable!() };
        assert_eq!(original.tracks[0].id, "1");

        // Коллекцию уже загрузили, а «Мне нравится» ещё не открывали.
        net.store.lock().unwrap().remove("likes");
        net.like_saved(&track(77), true);
        let preview = snapshot("collection");
        let Data::Collection(c) = &*preview else { unreachable!() };
        assert_eq!((c.likes_count, c.likes_preview[0].id.as_str()), (12, "77"));
        assert!(net.peek("likes").is_none());
        net.store.lock().unwrap().get_mut("collection").unwrap().loading = true;
        net.like_saved(&track(78), true);
        assert!(net.store.lock().unwrap()["collection"].refresh_again);
    }

    #[test]
    fn words() {
        assert_eq!(tracks_word(1), "1 трек");
        assert_eq!(tracks_word(3), "3 трека");
        assert_eq!(tracks_word(11), "11 треков");
        assert_eq!(listeners_word(1964082), "1\u{2008}964\u{2008}082 слушателя в месяц");
    }
}
