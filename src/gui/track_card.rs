//! Карточка трека справа, как у оригинала по клику на название трека в альбоме (#189):
//! шапка сразу, текст и похожие треки — когда придут.

use super::*;

thread_local! {
    /// Трек в карточке и похожие на него: по ним работают кнопки и строки.
    static TC: RefCell<(Option<Track>, Vec<Track>)> = const { RefCell::new((None, Vec::new())) };
}

impl Ctx {
    pub(super) fn open_track_card(self: &Rc<Self>, t: Track) {
        let Some(net) = self.st.borrow().net.clone() else { return };
        let ui = self.ui();
        let app = ui.global::<App>();
        let title = if t.version.is_empty() { t.title.clone() } else { format!("{} {}", t.title, t.version) };
        app.set_track_card_title(title.into());
        app.set_track_card_artists(t.artists.clone().into());
        app.set_track_card_liked(self.player.is_liked(&t.id));
        app.set_track_card_lyrics(ModelRc::default());
        app.set_track_card_lyrics_preview(ModelRc::default());
        app.set_track_card_similar(ModelRc::default());
        app.set_track_card_album_cover(Image::default());
        // Альбом открытой страницы знает число треков; иначе только название.
        let album = self.list().and_then(|d| match &*d {
            Data::List(l) => l.album.clone().filter(|a| Some(&a.id) == t.album_id.as_ref()).map(|a| (a, l.tracks.len())),
            _ => None,
        });
        let sub = album.as_ref().map(|(_, n)| tracks_word(*n as u64)).unwrap_or_default();
        app.set_track_card_album(if t.album_id.is_some() { t.album_title.clone() } else { String::new() }.into());
        app.set_track_card_album_sub(sub.into());
        if let Some(uri) = album.and_then(|(a, _)| a.cover).or_else(|| t.cover.clone()) {
            let size = self.size_for(56.0);
            let weak = self.ui.clone();
            let id = t.id.clone();
            self.images.get(&crate::api::pic(&uri, size), size, move |img| {
                if TC.with(|c| c.borrow().0.as_ref().is_some_and(|t| t.id == id)) {
                    if let Some(ui) = weak.upgrade() {
                        ui.global::<App>().set_track_card_album_cover(img.clone());
                    }
                }
            });
        }
        app.set_track_card_open(true);
        TC.with(|c| *c.borrow_mut() = (Some(t.clone()), Vec::new()));
        let api = net.api.clone();
        std::thread::spawn(move || {
            let (lyrics, similar) = std::thread::scope(|s| {
                let lyrics = t.lyrics.then(|| s.spawn(|| api.lyrics(&t.id)));
                let similar = api.similar_tracks(&t.id);
                (lyrics.and_then(|h| h.join().ok()).and_then(Result::ok), similar.unwrap_or_default())
            });
            let _ = slint::invoke_from_event_loop(move || {
                with_ctx(|c| {
                    if !TC.with(|tc| tc.borrow().0.as_ref().is_some_and(|x| x.id == t.id)) {
                        return;
                    }
                    let similar: Vec<Track> = similar.into_iter().take(5).collect();
                    let ui = c.ui();
                    let app = ui.global::<App>();
                    if let Some(l) = lyrics {
                        let lines: Vec<SharedString> = if l.text.is_empty() {
                            l.synced.iter().map(|s| s.text.as_str()).filter(|s| !s.is_empty()).map(SharedString::from).collect()
                        } else {
                            l.text.iter().map(|s| SharedString::from(s.as_str())).collect()
                        };
                        let preview: Vec<SharedString> = lines.iter().take(4).cloned().collect();
                        app.set_track_card_lyrics_preview(ModelRc::from(Rc::new(VecModel::from(preview))));
                        app.set_track_card_lyrics(ModelRc::from(Rc::new(VecModel::from(lines))));
                    }
                    app.set_track_card_similar(c.track_model(&similar));
                    TC.with(|tc| tc.borrow_mut().1 = similar);
                })
            });
        });
    }

    /// Страница альбома догрузилась: у его трека в карточке — число треков.
    pub(super) fn track_card_list_loaded(&self, l: &ListData) {
        let Some(a) = &l.album else { return };
        let ui = self.ui();
        let app = ui.global::<App>();
        if app.get_track_card_open() && self.track_card_track().is_some_and(|t| t.album_id.as_ref() == Some(&a.id)) {
            app.set_track_card_album_sub(tracks_word(l.tracks.len() as u64).into());
        }
    }

    pub(super) fn track_card_track(&self) -> Option<Track> {
        TC.with(|c| c.borrow().0.clone())
    }

    pub(super) fn track_card_similar(&self, i: usize) -> Option<Track> {
        TC.with(|c| c.borrow().1.get(i).cloned())
    }

    pub(super) fn track_card_action(self: &Rc<Self>, action: &str) {
        let Some(t) = self.track_card_track() else { return };
        match action {
            "play" => {
                let title = t.title.clone();
                self.play_tracks(vec![t], 0, ListKind::Tracks, "track-card", &title);
            }
            "like" => {
                self.ui().global::<App>().set_track_card_liked(!self.player.is_liked(&t.id));
                self.like(t);
            }
            "album" => {
                if let Some(id) = &t.album_id {
                    self.ui().global::<App>().set_track_card_open(false);
                    self.navigate(Page::List, Some(format!("album:{id}")), true);
                }
            }
            _ => {}
        }
    }

    pub(super) fn track_card_row(self: &Rc<Self>, i: usize, like: bool) {
        if like {
            if let Some(t) = self.track_card_similar(i) {
                self.like(t);
            }
            return;
        }
        let tracks = TC.with(|c| c.borrow().1.clone());
        self.play_tracks(tracks, i, ListKind::Tracks, "track-card", "Похожие треки");
    }
}
