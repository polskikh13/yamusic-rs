//! Действия с карточки плейлиста без перехода на его страницу.

use super::*;

fn playlist_parts(target: &str) -> Option<(&str, &str)> {
    let (owner, kind) = target.strip_prefix("playlist:")?.split_once(':')?;
    (owner.parse::<u64>().is_ok() && kind.parse::<i64>().is_ok()).then_some((owner, kind))
}

pub(super) fn card_target(target: &str) -> SharedString {
    playlist_parts(target).map(|_| target).unwrap_or_default().into()
}

impl Ctx {
    pub(super) fn update_playlist_cards(&self) {
        let ui = self.ui();
        let app = ui.global::<App>();
        app.set_playlist_card_revision(app.get_playlist_card_revision().wrapping_add(1));
    }

    fn playlist_card_state(&self, target: &str) -> PlaylistCardState {
        let st = self.st.borrow();
        PlaylistCardState {
            pinned: st.pins.iter().any(|p| p.target == target),
            liked: st.liked_playlists.contains(target),
            can_like: !st.uid.is_empty() && playlist_parts(target).is_some_and(|(owner, _)| owner != st.uid),
        }
    }

    fn playlist_card_action(self: &Rc<Self>, action: PlaylistCardAction, target: &str, title: &str, x: f32, y: f32) {
        let Some((owner, kind)) = playlist_parts(target) else { return };
        match action {
            PlaylistCardAction::Play => self.play_target(target, title),
            PlaylistCardAction::Pin => self.toggle_pin(target.to_string()),
            PlaylistCardAction::Like => {
                if self.playlist_card_state(target).can_like {
                    self.toggle_playlist_like(target, title);
                }
            }
            PlaylistCardAction::Trailer => self.play_trailer(format!("users/{owner}/playlists/{kind}")),
            PlaylistCardAction::Menu => self.open_playlist_card_menu(target, title, x, y),
        }
    }

    fn open_playlist_card_menu(&self, target: &str, title: &str, x: f32, y: f32) {
        let state = self.playlist_card_state(target);
        let ui = self.ui();
        let icons = ui.global::<Icons>();
        let mut entries = vec![self.menu_entry(
            "playlist-card-pin",
            if state.pinned { "Открепить" } else { "Закрепить" },
            if state.pinned { icons.get_m_unpin() } else { icons.get_m_pin() },
            false,
        )];
        if state.can_like {
            entries.push(self.menu_entry(
                "playlist-card-like",
                if state.liked { "Удалить из коллекции" } else { "Нравится" },
                if state.liked { icons.get_m_liked() } else { icons.get_m_like() },
                false,
            ));
        }
        entries.push(self.menu_entry("playlist-card-trailer", "Трейлер", icons.get_m_trailer(), false));
        self.st.borrow_mut().playlist_card_menu = Some((target.to_string(), title.to_string()));
        self.show_menu(entries, x, y, true);
    }

    pub(super) fn playlist_card_menu_action(self: &Rc<Self>, action: &str) {
        let action = match action {
            "pin" => PlaylistCardAction::Pin,
            "like" => PlaylistCardAction::Like,
            "trailer" => PlaylistCardAction::Trailer,
            _ => return,
        };
        let selected = self.st.borrow_mut().playlist_card_menu.take();
        if let Some((target, title)) = selected {
            self.playlist_card_action(action, &target, &title, 0.0, 0.0);
        }
    }

    fn set_playlist_liked(&self, target: &str, liked: bool) {
        {
            let mut st = self.st.borrow_mut();
            if liked {
                st.liked_playlists.insert(target.to_string());
            } else {
                st.liked_playlists.remove(target);
            }
        }
        let ui = self.ui();
        let app = ui.global::<App>();
        if app.get_page() == Page::List && self.list_target().as_deref() == Some(target) {
            app.set_list_liked(liked);
        }
        self.update_playlist_cards();
    }

    pub(super) fn toggle_playlist_like(self: &Rc<Self>, target: &str, title: &str) {
        let Some((owner, kind)) = playlist_parts(target) else { return };
        let id = format!("{owner}:{kind}");
        let request = Arc::new(());
        let (net, was_liked) = {
            let mut st = self.st.borrow_mut();
            let Some(net) = st.net.clone() else { return };
            if st.pending_playlist_likes.contains_key(target) {
                return;
            }
            st.pending_playlist_likes.insert(target.to_string(), request.clone());
            (net, st.liked_playlists.contains(target))
        };
        self.set_playlist_liked(target, !was_liked);
        let (target, title) = (target.to_string(), title.to_string());
        std::thread::spawn(move || {
            let result = net.api.like_entity(&net.uid, "playlists", &id, was_liked);
            let _ = slint::invoke_from_event_loop(move || with_ctx(|c| {
                {
                    let mut st = c.st.borrow_mut();
                    if !st.pending_playlist_likes.get(&target).is_some_and(|pending| Arc::ptr_eq(pending, &request)) {
                        return;
                    }
                    st.pending_playlist_likes.remove(&target);
                }
                match result {
                    Ok(()) => {
                        net.refresh("collection");
                        c.toast(if was_liked { format!("«{title}» удалён из коллекции") } else { format!("«{title}» в коллекции") });
                    }
                    Err(e) => {
                        c.set_playlist_liked(&target, was_liked);
                        c.toast(format!("Не удалось сохранить: {e}"));
                    }
                }
            }));
        });
    }
}

pub(super) fn register(app: &App) {
    app.on_playlist_card_state(|target, _| {
        CTX.with(|ctx| ctx.borrow().as_ref().map(|c| c.playlist_card_state(&target)).unwrap_or_default())
    });
    app.on_playlist_card_action(|action, target, title, x, y| {
        with_ctx(|c| c.playlist_card_action(action, &target, &title, x, y));
    });
}
