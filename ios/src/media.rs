//! Экран блокировки, Пункт управления и кнопки наушников: MPNowPlayingInfoCenter и
//! MPRemoteCommandCenter. AVAudioSession с категорией Playback держит звук в фоне
//! (нужен UIBackgroundModes = audio в Info.plist) и не даёт беззвучному режиму глушить музыку.

use crate::config::log;
use crate::player::{Cmd, Player};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, AnyThread, MainThreadMarker};
use objc2_avf_audio::{AVAudioSession, AVAudioSessionCategoryPlayback};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSData, NSMutableDictionary, NSNumber, NSString};
use objc2_media_player::{
    MPChangePlaybackPositionCommandEvent, MPMediaItemArtwork, MPMediaItemPropertyArtist, MPMediaItemPropertyArtwork,
    MPMediaItemPropertyPlaybackDuration, MPMediaItemPropertyTitle, MPNowPlayingInfoCenter,
    MPNowPlayingInfoPropertyElapsedPlaybackTime, MPNowPlayingInfoPropertyPlaybackRate, MPRemoteCommand,
    MPRemoteCommandCenter, MPRemoteCommandEvent, MPRemoteCommandHandlerStatus,
};
use objc2_ui_kit::UIImage;
use std::ptr::NonNull;
use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static WAKE: OnceLock<Sender<()>> = OnceLock::new();

/// Что сейчас показано на экране блокировки.
struct Shown {
    id: String,
    playing: bool,
    duration: f32,
    position: f32,
    at: Instant,
}

impl Shown {
    fn position_now(&self) -> f32 {
        if self.playing {
            self.position + self.at.elapsed().as_secs_f32()
        } else {
            self.position
        }
    }
}

/// Плеер что-то поменял.
pub fn changed() {
    if let Some(tx) = WAKE.get() {
        let _ = tx.send(());
    }
}

/// Зовётся из `run` на главном потоке, до цикла событий.
pub fn start(player: Player) {
    let Some(mtm) = MainThreadMarker::new() else {
        log("медиа: start не на главном потоке");
        return;
    };
    audio_session();
    remote_commands(mtm, player.clone());

    let (tx, rx) = channel();
    let _ = WAKE.set(tx);
    std::thread::spawn(move || {
        let mut shown: Option<Shown> = None;
        let mut cover: (String, Option<Vec<u8>>) = Default::default();
        while rx.recv().is_ok() {
            // Плеер зовёт пачками, хватит одного обновления на пачку.
            std::thread::sleep(Duration::from_millis(150));
            while rx.try_recv().is_ok() {}
            let s = player.state();
            let Some(t) = s.track else {
                if shown.take().is_some() {
                    let _ = slint::invoke_from_event_loop(|| unsafe { MPNowPlayingInfoCenter::defaultCenter().setNowPlayingInfo(None) });
                }
                continue;
            };
            if shown.is_none() && !s.playing {
                continue;
            }
            let same = shown.as_ref().is_some_and(|w| {
                w.id == t.id && w.playing == s.playing && w.duration == s.duration && (w.position_now() - s.position).abs() < 2.0
            });
            if same {
                continue;
            }
            if cover.0 != t.id {
                cover = (t.id.clone(), t.cover_url(400).and_then(|u| download(&u)));
            }
            let info = Info {
                title: t.title.clone(),
                artists: t.artists.clone(),
                cover: cover.1.clone(),
                duration: s.duration,
                position: s.position,
                playing: s.playing,
            };
            let _ = slint::invoke_from_event_loop(move || info.publish());
            shown = Some(Shown { id: t.id, playing: s.playing, duration: s.duration, position: s.position, at: Instant::now() });
        }
    });
}

fn download(url: &str) -> Option<Vec<u8>> {
    let resp = reqwest::blocking::get(url).ok()?.error_for_status().ok()?;
    resp.bytes().ok().map(|b| b.to_vec())
}

fn audio_session() {
    let session = unsafe { AVAudioSession::sharedInstance() };
    let Some(category) = (unsafe { AVAudioSessionCategoryPlayback }) else { return };
    if let Err(e) = unsafe { session.setCategory_error(category) } {
        log(format!("медиа: категория аудио: {e:?}"));
    }
    if let Err(e) = unsafe { session.setActive_error(true) } {
        log(format!("медиа: аудиосессия: {e:?}"));
    }
}

/// Кнопки экрана блокировки, Пункта управления, наушников и CarPlay.
fn remote_commands(_mtm: MainThreadMarker, player: Player) {
    let center = unsafe { MPRemoteCommandCenter::sharedCommandCenter() };
    let on = |cmd: Retained<MPRemoteCommand>, f: Box<dyn Fn(&Player) + 'static>| {
        let p = player.clone();
        let block = RcBlock::new(move |_e: NonNull<MPRemoteCommandEvent>| {
            f(&p);
            MPRemoteCommandHandlerStatus::Success
        });
        unsafe {
            cmd.setEnabled(true);
            let _ = cmd.addTargetWithHandler(&block);
        }
    };
    unsafe {
        on(center.playCommand(), Box::new(|p| if !p.state().playing { p.send(Cmd::PlayPause) }));
        on(center.pauseCommand(), Box::new(|p| if p.state().playing { p.send(Cmd::PlayPause) }));
        on(center.togglePlayPauseCommand(), Box::new(|p| p.send(Cmd::PlayPause)));
        on(center.nextTrackCommand(), Box::new(|p| p.send(Cmd::Next)));
        on(center.previousTrackCommand(), Box::new(|p| p.send(Cmd::Prev)));
    }
    let p = player.clone();
    let seek = RcBlock::new(move |e: NonNull<MPRemoteCommandEvent>| {
        // SAFETY: changePlaybackPositionCommand шлёт только MPChangePlaybackPositionCommandEvent.
        let e = unsafe { e.cast::<MPChangePlaybackPositionCommandEvent>().as_ref() };
        p.send(Cmd::Seek(unsafe { e.positionTime() } as f32));
        MPRemoteCommandHandlerStatus::Success
    });
    unsafe {
        let cmd = center.changePlaybackPositionCommand();
        cmd.setEnabled(true);
        let _ = cmd.addTargetWithHandler(&seek);
    }
}

struct Info {
    title: String,
    artists: String,
    cover: Option<Vec<u8>>,
    duration: f32,
    position: f32,
    playing: bool,
}

impl Info {
    /// Только с главного потока.
    fn publish(self) {
        let dict: Retained<NSMutableDictionary<NSString, AnyObject>> = NSMutableDictionary::new();
        let set = |key: &NSString, value: &AnyObject| dict.insert(key, value);
        unsafe {
            set(MPMediaItemPropertyTitle, &NSString::from_str(&self.title));
            set(MPMediaItemPropertyArtist, &NSString::from_str(&self.artists));
            set(MPMediaItemPropertyPlaybackDuration, &NSNumber::new_f64(self.duration as f64));
            set(MPNowPlayingInfoPropertyElapsedPlaybackTime, &NSNumber::new_f64(self.position as f64));
            set(MPNowPlayingInfoPropertyPlaybackRate, &NSNumber::new_f64(if self.playing { 1.0 } else { 0.0 }));
        }
        if let Some(image) = self.cover.as_deref().and_then(|b| UIImage::imageWithData(&NSData::with_bytes(b))) {
            let size = unsafe { image.size() };
            // Блок держит картинку, поэтому указатель на неё живёт столько же, сколько обложка.
            let handler = RcBlock::new(move |_size: CGSize| -> NonNull<UIImage> { NonNull::from(&*image) });
            // У objc2-media-player этот init сгенерирован только для macOS (NSImage), на iOS зовём сами.
            let art: Retained<MPMediaItemArtwork> =
                unsafe { msg_send![MPMediaItemArtwork::alloc(), initWithBoundsSize: size, requestHandler: &*handler] };
            unsafe { set(MPMediaItemPropertyArtwork, &art) };
        }
        unsafe { MPNowPlayingInfoCenter::defaultCenter().setNowPlayingInfo(Some(&dict)) };
    }
}
