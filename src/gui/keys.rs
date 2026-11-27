//! Горячие клавиши, как в официальном приложении (`ShortcutsProvider`).
//!
//! - Клавиша узнаётся по физическому коду (`KeyboardEvent.code`), раскладка не важна.
//! - С любым модификатором (Ctrl, Shift, Alt, Win) не срабатывает.
//! - Пока фокус в поле ввода, все клавиши достаются полю.
//! - При удержании повторяются только громкость, переходы и перемотка.
//! - Действие то же, что у кнопки: клавиши вызывают те же колбэки.
//! - Клавиши можно переназначить в окне «Горячие клавиши», они хранятся в config.json.

use super::*;
use std::collections::BTreeMap;
use winit::keyboard::{KeyCode, ModifiersState};

/// Шаг громкости клавишами: `increase/decreaseExponentVolume(.05)`.
const VOLUME_STEP: f64 = 0.05;
/// Шаг перемотки, с: `slideForward(2)`.
const SEEK_STEP: f32 = 2.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    TogglePlay,
    ToggleMute,
    VolumeUp,
    VolumeDown,
    Like,
    Dislike,
    Next,
    Prev,
    Forward,
    Backward,
    Repeat,
    Shuffle,
    Fullscreen,
    Close,
}

impl Action {
    fn repeatable(self) -> bool {
        matches!(self, Action::VolumeUp | Action::VolumeDown | Action::Next | Action::Prev | Action::Forward | Action::Backward)
    }
}

/// Строки окна «Горячие клавиши» в порядке оригинала: действие, имя в config.json, подпись и
/// клавиши по умолчанию. Сколько клавиш по умолчанию, столько ячеек в строке. Esc не меняется.
const LISTED: [(Action, &str, &str, &[KeyCode]); 13] = [
    (Action::TogglePlay, "play", "Включить музыку/поставить на паузу", &[KeyCode::KeyK]),
    (Action::ToggleMute, "mute", "Отключить/включить звук", &[KeyCode::KeyM]),
    (Action::Forward, "forward", "Промотать вперед", &[KeyCode::ArrowRight, KeyCode::KeyL]),
    (Action::Backward, "backward", "Промотать назад", &[KeyCode::ArrowLeft, KeyCode::KeyJ]),
    (Action::VolumeUp, "volume_up", "Увеличить громкость", &[KeyCode::ArrowUp]),
    (Action::VolumeDown, "volume_down", "Уменьшить громкость", &[KeyCode::ArrowDown]),
    (Action::Like, "like", "Лайк", &[KeyCode::KeyF]),
    (Action::Dislike, "dislike", "Дизлайк", &[KeyCode::KeyD]),
    (Action::Repeat, "repeat", "Переключение режима повтора", &[KeyCode::KeyR]),
    (Action::Shuffle, "shuffle", "Переключение режима («в случайном порядке»)", &[KeyCode::KeyS]),
    (Action::Next, "next", "Переключить на следующий трек", &[KeyCode::KeyN]),
    (Action::Prev, "prev", "Переключить на предыдущий трек", &[KeyCode::KeyP]),
    (Action::Fullscreen, "fullscreen", "Открыть / закрыть фулскрин плеер", &[KeyCode::KeyW]),
];

/// Клавиши, которые можно назначить: имя в config.json и подпись на кнопке.
const KEYS: &[(KeyCode, &str, &str)] = &[
    (KeyCode::KeyA, "KeyA", "A"),
    (KeyCode::KeyB, "KeyB", "B"),
    (KeyCode::KeyC, "KeyC", "C"),
    (KeyCode::KeyD, "KeyD", "D"),
    (KeyCode::KeyE, "KeyE", "E"),
    (KeyCode::KeyF, "KeyF", "F"),
    (KeyCode::KeyG, "KeyG", "G"),
    (KeyCode::KeyH, "KeyH", "H"),
    (KeyCode::KeyI, "KeyI", "I"),
    (KeyCode::KeyJ, "KeyJ", "J"),
    (KeyCode::KeyK, "KeyK", "K"),
    (KeyCode::KeyL, "KeyL", "L"),
    (KeyCode::KeyM, "KeyM", "M"),
    (KeyCode::KeyN, "KeyN", "N"),
    (KeyCode::KeyO, "KeyO", "O"),
    (KeyCode::KeyP, "KeyP", "P"),
    (KeyCode::KeyQ, "KeyQ", "Q"),
    (KeyCode::KeyR, "KeyR", "R"),
    (KeyCode::KeyS, "KeyS", "S"),
    (KeyCode::KeyT, "KeyT", "T"),
    (KeyCode::KeyU, "KeyU", "U"),
    (KeyCode::KeyV, "KeyV", "V"),
    (KeyCode::KeyW, "KeyW", "W"),
    (KeyCode::KeyX, "KeyX", "X"),
    (KeyCode::KeyY, "KeyY", "Y"),
    (KeyCode::KeyZ, "KeyZ", "Z"),
    (KeyCode::Digit0, "Digit0", "0"),
    (KeyCode::Digit1, "Digit1", "1"),
    (KeyCode::Digit2, "Digit2", "2"),
    (KeyCode::Digit3, "Digit3", "3"),
    (KeyCode::Digit4, "Digit4", "4"),
    (KeyCode::Digit5, "Digit5", "5"),
    (KeyCode::Digit6, "Digit6", "6"),
    (KeyCode::Digit7, "Digit7", "7"),
    (KeyCode::Digit8, "Digit8", "8"),
    (KeyCode::Digit9, "Digit9", "9"),
    (KeyCode::ArrowUp, "ArrowUp", "↑"),
    (KeyCode::ArrowDown, "ArrowDown", "↓"),
    (KeyCode::ArrowLeft, "ArrowLeft", "←"),
    (KeyCode::ArrowRight, "ArrowRight", "→"),
    (KeyCode::Space, "Space", "Пробел"),
    (KeyCode::Comma, "Comma", ","),
    (KeyCode::Period, "Period", "."),
    (KeyCode::Slash, "Slash", "/"),
    (KeyCode::Semicolon, "Semicolon", ";"),
    (KeyCode::Quote, "Quote", "'"),
    (KeyCode::BracketLeft, "BracketLeft", "["),
    (KeyCode::BracketRight, "BracketRight", "]"),
    (KeyCode::Minus, "Minus", "-"),
    (KeyCode::Equal, "Equal", "="),
    (KeyCode::Backquote, "Backquote", "`"),
    (KeyCode::Backslash, "Backslash", "\\"),
    (KeyCode::Insert, "Insert", "Insert"),
    (KeyCode::Delete, "Delete", "Delete"),
    (KeyCode::Home, "Home", "Home"),
    (KeyCode::End, "End", "End"),
    (KeyCode::PageUp, "PageUp", "PgUp"),
    (KeyCode::PageDown, "PageDown", "PgDn"),
    (KeyCode::F1, "F1", "F1"),
    (KeyCode::F2, "F2", "F2"),
    (KeyCode::F3, "F3", "F3"),
    (KeyCode::F4, "F4", "F4"),
    (KeyCode::F5, "F5", "F5"),
    (KeyCode::F6, "F6", "F6"),
    (KeyCode::F7, "F7", "F7"),
    (KeyCode::F8, "F8", "F8"),
    (KeyCode::F9, "F9", "F9"),
    (KeyCode::F10, "F10", "F10"),
    (KeyCode::F11, "F11", "F11"),
    (KeyCode::F12, "F12", "F12"),
    (KeyCode::Numpad0, "Numpad0", "Num 0"),
    (KeyCode::Numpad1, "Numpad1", "Num 1"),
    (KeyCode::Numpad2, "Numpad2", "Num 2"),
    (KeyCode::Numpad3, "Numpad3", "Num 3"),
    (KeyCode::Numpad4, "Numpad4", "Num 4"),
    (KeyCode::Numpad5, "Numpad5", "Num 5"),
    (KeyCode::Numpad6, "Numpad6", "Num 6"),
    (KeyCode::Numpad7, "Numpad7", "Num 7"),
    (KeyCode::Numpad8, "Numpad8", "Num 8"),
    (KeyCode::Numpad9, "Numpad9", "Num 9"),
    (KeyCode::NumpadAdd, "NumpadAdd", "Num +"),
    (KeyCode::NumpadSubtract, "NumpadSubtract", "Num -"),
    (KeyCode::NumpadMultiply, "NumpadMultiply", "Num *"),
    (KeyCode::NumpadDivide, "NumpadDivide", "Num /"),
    (KeyCode::NumpadDecimal, "NumpadDecimal", "Num ."),
];

fn key_name(code: KeyCode) -> Option<&'static str> {
    KEYS.iter().find(|k| k.0 == code).map(|k| k.1)
}

fn key_label(code: KeyCode) -> &'static str {
    KEYS.iter().find(|k| k.0 == code).map_or("?", |k| k.2)
}

fn key_by_name(name: &str) -> Option<KeyCode> {
    KEYS.iter().find(|k| k.1 == name).map(|k| k.0)
}

/// Клавиши строк `LISTED` по ячейкам; `None` — ячейка пустая.
type Bindings = Vec<Vec<Option<KeyCode>>>;

fn default_bindings() -> Bindings {
    LISTED.iter().map(|l| l.3.iter().map(|k| Some(*k)).collect()).collect()
}

/// Назначения из config.json поверх стандартных.
fn bindings_from(saved: &BTreeMap<String, Vec<String>>) -> Bindings {
    let mut b = default_bindings();
    for (row, l) in LISTED.iter().enumerate() {
        let Some(keys) = saved.get(l.1) else { continue };
        for (slot, name) in keys.iter().enumerate().take(b[row].len()) {
            b[row][slot] = key_by_name(name);
        }
    }
    b
}

fn bindings_to_config(b: &Bindings) -> BTreeMap<String, Vec<String>> {
    LISTED
        .iter()
        .zip(b)
        .map(|(l, keys)| (l.1.to_string(), keys.iter().map(|k| k.and_then(key_name).unwrap_or("").to_string()).collect()))
        .collect()
}

/// Поставить клавишу в ячейку. У другого действия эта клавиша снимается.
fn assign(b: &mut Bindings, row: usize, slot: usize, code: KeyCode) {
    for k in b.iter_mut().flatten() {
        if *k == Some(code) {
            *k = None;
        }
    }
    if let Some(k) = b.get_mut(row).and_then(|keys| keys.get_mut(slot)) {
        *k = Some(code);
    }
}

/// Действие для нажатия. `None` — клавиша не наша, её получает интерфейс.
fn action(code: KeyCode, mods: ModifiersState, repeat: bool) -> Option<Action> {
    if !mods.is_empty() {
        return None;
    }
    let a = if code == KeyCode::Escape {
        Action::Close
    } else {
        let row = BINDINGS.with_borrow(|b| b.iter().position(|keys| keys.contains(&Some(code))))?;
        LISTED[row].0
    };
    (a.repeatable() || !repeat).then_some(a)
}

/// Загрузить назначения из настроек и показать их в окне «Горячие клавиши».
pub(super) fn init(app: &App, saved: &BTreeMap<String, Vec<String>>) {
    BINDINGS.set(bindings_from(saved));
    show(app);
    app.on_shortcuts_reset(|| {
        BINDINGS.set(default_bindings());
        with_ctx(|c| {
            let ui = c.ui();
            let app = ui.global::<App>();
            app.set_shortcut_row(-1);
            show(&app);
            let mut st = c.st.borrow_mut();
            st.cfg.hotkeys.clear();
            st.cfg.save();
        });
    });
}

/// Строки окна «Горячие клавиши» с текущими клавишами, подписи на текущем языке.
pub(super) fn show(app: &App) {
    let items: Vec<ShortcutItem> = BINDINGS.with_borrow(|b| {
        LISTED
            .iter()
            .zip(b)
            .map(|(l, keys)| ShortcutItem {
                title: crate::i18n::tr(l.2).into(),
                keys: ModelRc::from(Rc::new(VecModel::from(
                    keys.iter().map(|k| SharedString::from(k.map_or("—", key_label))).collect::<Vec<_>>(),
                ))),
            })
            .collect()
    });
    app.set_shortcuts(ModelRc::from(Rc::new(VecModel::from(items))));
}

/// Окно «Горячие клавиши» ждёт клавишу для ячейки. `true` — нажатие забрано: назначено или
/// это Esc, который отменяет ожидание.
fn capture(code: KeyCode) -> bool {
    let mut taken = false;
    with_ctx(|c| {
        let ui = c.ui();
        let app = ui.global::<App>();
        let row = app.get_shortcut_row();
        if !app.get_shortcuts_open() || row < 0 {
            return;
        }
        taken = true;
        if code == KeyCode::Escape {
            app.set_shortcut_row(-1);
            return;
        }
        if !MODS.get().is_empty() || key_name(code).is_none() {
            return;
        }
        let slot = app.get_shortcut_slot().max(0) as usize;
        let saved = BINDINGS.with_borrow_mut(|b| {
            assign(b, row as usize, slot, code);
            bindings_to_config(b)
        });
        app.set_shortcut_row(-1);
        show(&app);
        let mut st = c.st.borrow_mut();
        st.cfg.hotkeys = saved;
        st.cfg.save();
    });
    taken
}

/// Громкость на шаг выше: `1 − s > t ? t + s : 1`.
fn volume_up(v: f32) -> f32 {
    let v = v as f64;
    (if 1.0 - VOLUME_STEP > v { v + VOLUME_STEP } else { 1.0 }) as f32
}

/// Громкость на шаг ниже: `t > s ? t − s : 0`.
fn volume_down(v: f32) -> f32 {
    let v = v as f64;
    (if v > VOLUME_STEP { v - VOLUME_STEP } else { 0.0 }) as f32
}

/// Новое место после перемотки на `step` секунд (`slideForward` / `slideBackward`).
fn slide(pos: f32, duration: f32, step: f32) -> f32 {
    if step > 0.0 {
        if duration - step > pos { pos + step } else { duration }
    } else if pos > -step {
        pos + step
    } else {
        0.0
    }
}

thread_local! {
    static MODS: Cell<ModifiersState> = const { Cell::new(ModifiersState::empty()) };
    static CURSOR: Cell<slint::LogicalPosition> = const { Cell::new(slint::LogicalPosition::new(0.0, 0.0)) };
    /// Клавиши, нажатие которых забрали себе: их повтор и отпускание интерфейсу тоже не нужны.
    static TAKEN: RefCell<Vec<KeyCode>> = const { RefCell::new(Vec::new()) };
    static BINDINGS: RefCell<Bindings> = RefCell::new(default_bindings());
}

pub(super) fn set_modifiers(m: ModifiersState) {
    MODS.set(m);
}

pub(super) fn set_cursor(at: slint::LogicalPosition) {
    CURSOR.set(at);
}

/// Где курсор, в логических пикселях окна.
pub(super) fn cursor() -> slint::LogicalPosition {
    CURSOR.get()
}

/// Нажата левая кнопка мыши. Если у поля ввода фокус, а клик пришёлся мимо него, фокус
/// снимается, чтобы клавиши снова работали. Проверка идёт после того, как Slint обработал
/// нажатие: если кликнули по другому полю, фокус уже там.
pub(super) fn on_mouse_down() {
    let at = CURSOR.get();
    Timer::single_shot(Duration::ZERO, move || {
        with_ctx(|c| {
            let ui = c.ui();
            let app = ui.global::<App>();
            if !app.get_text_focused() {
                return;
            }
            let (x, y, w, h) = (app.get_text_x(), app.get_text_y(), app.get_text_w(), app.get_text_h());
            if !(at.x >= x && at.x <= x + w && at.y >= y && at.y <= y + h) {
                // Поле снимет фокус само. Если его уже нет на экране, флаг сбрасывается здесь.
                app.set_blur_tick(app.get_blur_tick() + 1);
                app.set_text_focused(false);
            }
        })
    });
}

/// Событие клавиатуры окна. `true` — клавиша обработана, интерфейсу её не передавать.
pub(super) fn on_key(code: KeyCode, pressed: bool, repeat: bool) -> bool {
    if !pressed {
        return TAKEN.with(|t| {
            let mut t = t.borrow_mut();
            let had = t.iter().position(|c| *c == code).map(|i| t.swap_remove(i));
            had.is_some()
        });
    }
    // Удерживают клавишу без повтора (K, W, Esc…), нажатие которой уже забрали. Иначе
    // удержанный Esc, закрыв меню, дошёл бы до полноэкранного плеера и закрыл бы и его.
    if repeat && action(code, MODS.get(), true).is_none() && TAKEN.with(|t| t.borrow().contains(&code)) {
        return true;
    }
    let mut done = capture(code);
    with_ctx(|c| {
        if done || c.ui().global::<App>().get_text_focused() {
            return;
        }
        if let Some(a) = action(code, MODS.get(), repeat) {
            done = c.hotkey(a);
        }
    });
    if done {
        TAKEN.with(|t| {
            let mut t = t.borrow_mut();
            if !t.contains(&code) {
                t.push(code);
            }
        });
    }
    done
}

impl Ctx {
    /// Выполнить действие клавиши. `false` — сейчас нечего делать (нет трека и т. п.), и
    /// клавиша уходит интерфейсу.
    fn hotkey(self: &Rc<Self>, a: Action) -> bool {
        let ui = self.ui();
        let app = ui.global::<App>();
        // На экране входа плеера нет, а в мини-плеере не видно ни полноэкранного плеера, ни меню.
        if app.get_logged_out() || (app.get_mini() && matches!(a, Action::Fullscreen | Action::Close)) {
            return false;
        }
        let s = self.player.state();
        if s.track.is_none() && !matches!(a, Action::TogglePlay | Action::ToggleMute | Action::VolumeUp | Action::VolumeDown | Action::Close) {
            return false;
        }
        match a {
            Action::TogglePlay => app.invoke_play_pause(),
            Action::ToggleMute => app.invoke_toggle_mute(),
            Action::VolumeUp | Action::VolumeDown => {
                let v = if a == Action::VolumeUp { volume_up(app.get_volume()) } else { volume_down(app.get_volume()) };
                app.set_volume(v);
                app.invoke_set_volume(v);
            }
            Action::Like => app.invoke_like(),
            Action::Dislike => app.invoke_dislike(),
            Action::Next => app.invoke_next(),
            Action::Prev => app.invoke_prev(),
            Action::Forward | Action::Backward => {
                if s.duration <= 0.0 {
                    return false;
                }
                let step = if a == Action::Forward { SEEK_STEP } else { -SEEK_STEP };
                app.invoke_seek(slide(s.position, s.duration, step));
            }
            // Перемешивания в волне нет, как у оригинала (`canShuffle`).
            Action::Shuffle if app.get_is_wave() => return false,
            Action::Repeat => app.invoke_cycle_repeat(),
            Action::Shuffle => app.invoke_toggle_shuffle(),
            Action::Fullscreen => {
                if app.get_fullscreen() {
                    app.set_fullscreen(false);
                } else {
                    // Как кнопка полноэкранного режима в плеере.
                    app.invoke_open_fullscreen(0);
                }
            }
            Action::Close => return self.close_top(),
        }
        true
    }

    /// Esc: закрывается самое верхнее — подменю, меню, подсказка, панель аккаунта, окно,
    /// настройки звука, «О треке»; под полноэкранным плеером — обложка, настройки и меню волны. Сам
    /// полноэкранный плеер закрывает его FocusScope (`ui/fullscreen.slint`): если закрывать
    /// поверх нечего, Esc уходит интерфейсу.
    fn close_top(&self) -> bool {
        let ui = self.ui();
        let app = ui.global::<App>();
        if app.get_submenu_open() >= 0 {
            app.set_submenu_open(-1);
        } else if app.get_menu_open() {
            app.set_menu_open(false);
        } else if app.get_hint_open() {
            app.set_hint_open(false);
        } else if app.get_account_sub() >= 0 {
            app.set_account_sub(-1);
        } else if app.get_account_open() {
            app.set_account_open(false);
        } else if app.get_shortcuts_open() {
            app.set_shortcuts_open(false);
        } else if app.get_about_open() {
            app.set_about_open(false);
        } else if app.get_overview_open() {
            app.set_overview_open(false);
        } else if app.get_eq_open() {
            app.set_eq_open(false);
        } else if app.get_track_about_open() {
            // «О треке» бывает и поверх полноэкранного плеера: закрывается первым.
            app.set_track_about_open(false);
        } else if app.get_fullscreen() {
            return false;
        } else if app.get_cover_open() {
            app.set_cover_open(false);
        } else if app.get_wave_settings_open() && app.get_page() == Page::Wave {
            app.set_wave_settings_open(false);
        } else if app.get_wave_menu_open() && app.get_page() == Page::Wave {
            app.set_wave_menu_open(false);
        } else {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_matches_original() {
        let none = ModifiersState::empty();
        let table = [
            (KeyCode::KeyK, Action::TogglePlay),
            (KeyCode::KeyM, Action::ToggleMute),
            (KeyCode::ArrowUp, Action::VolumeUp),
            (KeyCode::ArrowDown, Action::VolumeDown),
            (KeyCode::KeyF, Action::Like),
            (KeyCode::KeyD, Action::Dislike),
            (KeyCode::KeyN, Action::Next),
            (KeyCode::KeyP, Action::Prev),
            (KeyCode::ArrowRight, Action::Forward),
            (KeyCode::KeyL, Action::Forward),
            (KeyCode::ArrowLeft, Action::Backward),
            (KeyCode::KeyJ, Action::Backward),
            (KeyCode::KeyR, Action::Repeat),
            (KeyCode::KeyS, Action::Shuffle),
            (KeyCode::KeyW, Action::Fullscreen),
            (KeyCode::Escape, Action::Close),
        ];
        for (code, a) in table {
            assert_eq!(action(code, none, false), Some(a), "{code:?}");
        }
        // Пробел у оригинала не занят, с модификаторами ничего не срабатывает.
        assert_eq!(action(KeyCode::Space, none, false), None);
        for m in [ModifiersState::CONTROL, ModifiersState::SHIFT, ModifiersState::ALT, ModifiersState::SUPER] {
            assert_eq!(action(KeyCode::KeyK, m, false), None);
            assert_eq!(action(KeyCode::ArrowRight, m, false), None);
        }
    }

    #[test]
    fn only_some_keys_repeat() {
        let none = ModifiersState::empty();
        for code in [KeyCode::KeyN, KeyCode::KeyP, KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::ArrowLeft, KeyCode::KeyJ, KeyCode::ArrowRight, KeyCode::KeyL] {
            assert!(action(code, none, true).is_some(), "{code:?}");
        }
        for code in [KeyCode::KeyK, KeyCode::KeyM, KeyCode::KeyF, KeyCode::KeyD, KeyCode::KeyR, KeyCode::KeyS, KeyCode::KeyW, KeyCode::Escape] {
            assert_eq!(action(code, none, true), None, "{code:?}");
        }
    }

    #[test]
    fn twenty_steps_cover_the_scale() {
        let mut v = 1.0;
        for _ in 0..20 {
            v = volume_down(v);
        }
        assert_eq!(v, 0.0);
        for _ in 0..20 {
            v = volume_up(v);
        }
        assert_eq!(v, 1.0);
        assert_eq!(volume_up(0.97), 1.0);
        assert_eq!(volume_down(0.03), 0.0);
    }

    #[test]
    fn rebinding_moves_key_and_survives_config() {
        let mut b = default_bindings();
        // K у «Лайка» (строка 6) снимается с паузы (строка 0).
        assign(&mut b, 6, 0, KeyCode::KeyK);
        assert_eq!(b[0], vec![None]);
        assert_eq!(b[6], vec![Some(KeyCode::KeyK)]);
        let saved = bindings_to_config(&b);
        assert_eq!(saved["play"], vec![String::new()]);
        assert_eq!(bindings_from(&saved), b);
        // Чего нет в настройках, остаётся по умолчанию.
        assert_eq!(bindings_from(&BTreeMap::new()), default_bindings());
    }

    #[test]
    fn slide_clamps_to_track() {
        assert_eq!(slide(10.0, 100.0, 2.0), 12.0);
        assert_eq!(slide(99.0, 100.0, 2.0), 100.0);
        assert_eq!(slide(10.0, 100.0, -2.0), 8.0);
        assert_eq!(slide(1.5, 100.0, -2.0), 0.0);
    }
}
