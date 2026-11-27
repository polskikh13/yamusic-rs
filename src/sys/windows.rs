//! Windows: тонкие обёртки над Win32 — окно, фокус, мониторы, диалоги, автозапуск, один экземпляр.

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentProcess, GetCurrentThreadId, SetProcessWorkingSetSize};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

fn h(x: isize) -> HWND {
    x as HWND
}

/// HWND окна Slint; 0, пока winit его не создал.
fn handle(w: &slint::Window) -> isize {
    use slint::winit_030::{winit::raw_window_handle::{HasWindowHandle, RawWindowHandle}, WinitWindowAccessor};
    w.with_winit_window(|w| match w.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    })
    .unwrap_or(0)
}

/// Окно уже создано: winit делает это не сразу после `show()`.
pub fn created(w: &slint::Window) -> bool {
    handle(w) != 0
}

/// Один раз после создания окна: скруглённые углы, значок, HWND для второго запуска.
pub fn prepare(w: &slint::Window) {
    let hwnd = handle(w);
    round_corners(hwnd);
    let _ = std::fs::write(crate::config::dir().join("hwnd"), hwnd.to_string());
    set_icon(hwnd);
}

pub fn raise(w: &slint::Window) {
    let hwnd = handle(w);
    show(hwnd);
    force_foreground(hwnd);
}

pub fn is_shown(w: &slint::Window) -> bool {
    let hwnd = handle(w);
    hwnd != 0 && is_visible(hwnd)
}

pub fn minimize(w: &slint::Window) {
    w.set_minimized(true);
}

pub fn is_minimized(w: &slint::Window) -> bool {
    is_iconic(handle(w))
}

/// Настоящее состояние окна, включая Windows Snap и Win+стрелки.
pub fn is_maximized(w: &slint::Window) -> bool {
    unsafe { IsZoomed(h(handle(w))) != 0 }
}

/// Мини-плееру не нужны Snap, разворачивание и системные края изменения размера.
/// Меняем и флаги winit, и уже созданное окно, чтобы следующая смена свойств их не вернула.
pub fn set_mini_mode(w: &slint::Window, mini: bool) {
    use slint::winit_030::{winit::window::WindowButtons, WinitWindowAccessor};
    w.with_winit_window(|ww| {
        if mini {
            ww.set_maximized(false);
        }
        ww.set_resizable(!mini);
        ww.set_enabled_buttons(if mini { WindowButtons::CLOSE | WindowButtons::MINIMIZE } else { WindowButtons::all() });
    });
    let hwnd = handle(w);
    if hwnd == 0 {
        return;
    }
    unsafe {
        let style = GetWindowLongPtrW(h(hwnd), GWL_STYLE) as u32;
        let snap = WS_THICKFRAME | WS_MAXIMIZEBOX;
        let wanted = if mini { style & !snap } else { style | snap };
        if wanted != style {
            SetWindowLongPtrW(h(hwnd), GWL_STYLE, wanted as isize);
            SetWindowPos(h(hwnd), null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
}

/// Системное перетаскивание без внутреннего флага winit, который застревает после Snap.
/// Развёрнутое окно восстанавливаем под курсором перед началом движения.
pub fn start_move(w: &slint::Window) {
    let hwnd = handle(w);
    if hwnd == 0 {
        return;
    }
    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        if IsZoomed(h(hwnd)) != 0 {
            let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
            wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
            let mut rect: RECT = std::mem::zeroed();
            if GetWindowPlacement(h(hwnd), &mut wp) != 0 && GetWindowRect(h(hwnd), &mut rect) != 0 {
                let normal = wp.rcNormalPosition;
                let (width, height) = (normal.right - normal.left, normal.bottom - normal.top);
                if width > 0 && height > 0 {
                    let ratio = ((pt.x - rect.left) as f64 / (rect.right - rect.left).max(1) as f64).clamp(0.0, 1.0);
                    let x = pt.x - (ratio * width as f64) as i32;
                    let y = pt.y - 16;
                    ShowWindow(h(hwnd), SW_RESTORE);
                    SetWindowPos(h(hwnd), null_mut(), x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE);
                }
            }
        }
        ReleaseCapture();
        let lp = (((pt.y as u32) & 0xFFFF) << 16) | ((pt.x as u32) & 0xFFFF);
        PostMessageW(h(hwnd), WM_NCLBUTTONDOWN, HTCAPTION as usize, lp as isize);
    }
}

/// Направления 0..7 соответствуют краям и углам в интерфейсе.
pub fn start_resize(w: &slint::Window, dir: i32) {
    let hwnd = handle(w);
    if hwnd == 0 {
        return;
    }
    let code = [HTTOP, HTTOPRIGHT, HTRIGHT, HTBOTTOMRIGHT, HTBOTTOM, HTBOTTOMLEFT, HTLEFT, HTTOPLEFT]
        [dir.clamp(0, 7) as usize];
    unsafe {
        ReleaseCapture();
        PostMessageW(h(hwnd), WM_NCLBUTTONDOWN, code as usize, 0);
    }
}

/// Масштаб монитора, на котором окно.
pub fn scale(w: &slint::Window) -> f32 {
    dpi_scale(handle(w))
}

/// Сколько логических пикселей даёт щелчок колёсика — как у Chromium оригинала
/// (`WebMouseWheelEventBuilder`): 100/3 px на строку, строк за щелчок — из настройки Windows
/// (по умолчанию 3, то есть 100 px), по горизонтали — знаков за щелчок (по умолчанию 1).
/// `None` при настройке «на один экран»: Chromium тогда листает страницами, здесь шаг остаётся
/// шагом Slint.
pub fn wheel_px(horizontal: bool) -> Option<f32> {
    let (action, default) = if horizontal { (SPI_GETWHEELSCROLLCHARS, 1) } else { (SPI_GETWHEELSCROLLLINES, 3) };
    let mut n: u32 = default;
    if unsafe { SystemParametersInfoW(action, 0, &mut n as *mut u32 as *mut c_void, 0) } == 0 {
        n = default;
    }
    // WHEEL_PAGESCROLL
    (n != u32::MAX).then(|| n as f32 * 100.0 / 3.0)
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn is_visible(hwnd: isize) -> bool {
    unsafe { IsWindowVisible(h(hwnd)) != 0 }
}

fn is_iconic(hwnd: isize) -> bool {
    unsafe { IsIconic(h(hwnd)) != 0 }
}

fn show(hwnd: isize) {
    unsafe {
        if IsIconic(h(hwnd)) != 0 {
            ShowWindow(h(hwnd), SW_RESTORE);
        } else {
            ShowWindow(h(hwnd), SW_SHOW);
        }
    }
}

pub fn hide(w: &slint::Window) {
    unsafe {
        ShowWindow(h(handle(w)), SW_HIDE);
    }
}

/// SetForegroundWindow с обходом ограничений Windows на кражу фокуса.
fn force_foreground(hwnd: isize) {
    unsafe {
        let target = h(hwnd);
        if SetForegroundWindow(target) != 0 && GetForegroundWindow() == target {
            return;
        }
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, null_mut());
        let me = GetCurrentThreadId();
        if fg_thread != 0 && fg_thread != me {
            AttachThreadInput(me, fg_thread, 1);
            BringWindowToTop(target);
            SetForegroundWindow(target);
            AttachThreadInput(me, fg_thread, 0);
        } else {
            BringWindowToTop(target);
        }
    }
}

fn dpi_scale(hwnd: isize) -> f32 {
    let dpi = unsafe { GetDpiForWindow(h(hwnd)) };
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

/// Значок из ресурсов нашего exe (кладёт build.rs), иначе стандартный.
pub fn app_icon(size: i32) -> HICON {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    unsafe {
        let icon = LoadImageW(GetModuleHandleW(null()), 1 as _, IMAGE_ICON, size, size, LR_DEFAULTCOLOR);
        if !icon.is_null() {
            return icon as HICON;
        }
        LoadIconW(null_mut(), IDI_APPLICATION)
    }
}

/// Значок окна в заголовке, на панели задач и в Alt+Tab.
fn set_icon(hwnd: isize) {
    unsafe {
        let big = app_icon(GetSystemMetrics(SM_CXICON));
        let small = app_icon(GetSystemMetrics(SM_CXSMICON));
        SendMessageW(h(hwnd), WM_SETICON, ICON_BIG as usize, big as isize);
        SendMessageW(h(hwnd), WM_SETICON, ICON_SMALL as usize, small as isize);
    }
}

/// Скруглённые углы окна на Windows 11 (на 10 просто ничего не делает).
fn round_corners(hwnd: isize) {
    let pref = DWMWCP_ROUND;
    unsafe {
        DwmSetWindowAttribute(
            h(hwnd),
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &pref as *const _ as *const c_void,
            std::mem::size_of_val(&pref) as u32,
        );
    }
}

/// Системный диалог выбора картинки (JPEG, PNG). `None` — пользователь отказался.
pub fn pick_image(owner: &slint::Window, title: &str) -> Option<String> {
    use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW};
    let filter: Vec<u16> = "Изображения (*.jpg, *.png)\0*.jpg;*.jpeg;*.png\0\0".encode_utf16().collect();
    let title = wide(title);
    let mut buf = vec![0u16; 1024];
    unsafe {
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = h(handle(owner));
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = buf.as_mut_ptr();
        ofn.nMaxFile = buf.len() as u32;
        ofn.lpstrTitle = title.as_ptr();
        ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
        if GetOpenFileNameW(&mut ofn) == 0 {
            return None;
        }
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// Системный диалог выбора папки, открывается на `start`. `None` — пользователь отказался.
pub fn pick_folder(owner: &slint::Window, title: &str, start: &str) -> Option<String> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        SHBrowseForFolderW, SHGetPathFromIDListW, BFFM_INITIALIZED, BFFM_SETSELECTIONW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW,
    };
    unsafe extern "system" fn on_init(hwnd: HWND, msg: u32, _: isize, data: isize) -> i32 {
        if msg == BFFM_INITIALIZED {
            SendMessageW(hwnd, BFFM_SETSELECTIONW, 1, data);
        }
        0
    }
    let title = wide(title);
    let start = wide(start);
    let mut buf = vec![0u16; 1024];
    unsafe {
        let mut bi: BROWSEINFOW = std::mem::zeroed();
        bi.hwndOwner = h(handle(owner));
        bi.lpszTitle = title.as_ptr();
        bi.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
        bi.lpfn = Some(on_init);
        bi.lParam = start.as_ptr() as isize;
        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return None;
        }
        let ok = SHGetPathFromIDListW(pidl, buf.as_mut_ptr());
        CoTaskMemFree(pidl as *const c_void);
        if ok == 0 {
            return None;
        }
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// Рабочая область монитора с окном (без панели задач), в физических пикселях: лево, верх, право, низ.
pub fn work_area(w: &slint::Window) -> (i32, i32, i32, i32) {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromWindow(h(handle(w)), MONITOR_DEFAULTTONEAREST), &mut mi);
        let r = mi.rcWork;
        (r.left, r.top, r.right, r.bottom)
    }
}

pub fn open_url(url: &str) {
    let op = wide("open");
    let u = wide(url);
    unsafe {
        ShellExecuteW(null_mut(), op.as_ptr(), u.as_ptr(), null(), null(), SW_SHOWNORMAL);
    }
}

/// Отдать системе память, которую процесс сейчас не трогает (в основном —
/// буферы драйвера видеокарты). Вызываем, когда окно спрятано в трей.
pub fn trim_memory() {
    unsafe {
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

/// Отдать системе свободные блоки кучи (после выброса обложек) и ужать рабочий набор.
pub fn release_memory() {
    use windows_sys::Win32::System::Memory::{GetProcessHeap, HeapCompact};
    unsafe {
        HeapCompact(GetProcessHeap(), 0);
    }
    trim_memory();
}

/// Предпочитаемые языки интерфейса Windows по порядку («en-US», «ru-RU»): из них Chromium
/// оригинала собирает `navigator.languages`.
pub fn preferred_languages() -> Vec<String> {
    use windows_sys::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    let (mut count, mut len) = (0u32, 0u32);
    // SAFETY: сначала спрашиваем длину списка, потом даём буфер ровно такой длины.
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, null_mut(), &mut len) == 0 || len == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u16; len as usize];
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buf.as_mut_ptr(), &mut len) == 0 {
            return Vec::new();
        }
        buf.split(|c| *c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
    }
}

/// Светлая ли тема Windows для приложений.
pub fn system_light_theme() -> bool {
    let path = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide("AppsUseLightTheme");
    let mut value: u32 = 1;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, path.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, null_mut(), &mut value as *mut u32 as *mut c_void, &mut size);
    }
    value != 0
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "yamusic-rs";

/// Команда автозапуска: этот exe, сразу в трее.
fn autostart_command() -> Option<String> {
    Some(format!("\"{}\" --tray", std::env::current_exe().ok()?.display()))
}

/// Записан ли автозапуск вместе с Windows.
pub fn autostart_enabled() -> bool {
    let (key, name) = (wide(RUN_KEY), wide(RUN_NAME));
    unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, null_mut(), null_mut(), null_mut()) == 0 }
}

/// Включить или выключить автозапуск (HKCU\…\Run, права администратора не нужны).
pub fn set_autostart(on: bool) -> bool {
    let (key, name) = (wide(RUN_KEY), wide(RUN_NAME));
    unsafe {
        if !on {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
            return r == 0 || r == ERROR_FILE_NOT_FOUND;
        }
        let Some(cmd) = autostart_command() else { return false };
        let data = wide(&cmd);
        RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) == 0
    }
}

/// Exe перенесли в другую папку — поправить путь в автозапуске, если он включён.
pub fn refresh_autostart() {
    if autostart_enabled() {
        set_autostart(true);
    }
}

fn is_window(hwnd: isize) -> bool {
    unsafe { IsWindow(h(hwnd)) != 0 }
}

/// Занять мьютекс единственного экземпляра. Если он уже чей-то — отпустить свою ссылку,
/// чтобы мьютекс исчез вместе с тем процессом.
pub fn claim_instance() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name = wide(r"Local\yamusic_rs_single_instance");
    unsafe {
        let m = CreateMutexW(null(), 0, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(m);
            return false;
        }
        true
    }
}

/// Второй запуск: показать окно работающего экземпляра.
pub fn wake_existing() {
    if crate::tray::wake_existing() {
        return;
    }
    let hwnd: isize = std::fs::read_to_string(crate::config::dir().join("hwnd")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    if hwnd != 0 && is_window(hwnd) {
        show(hwnd);
        force_foreground(hwnd);
    }
}

/// Перед выходом: убрать значок из трея и HWND для второго запуска.
pub fn cleanup() {
    crate::tray::remove();
    let _ = std::fs::remove_file(crate::config::dir().join("hwnd"));
}

pub fn config_base() -> PathBuf {
    PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into()))
}

pub fn music_dir() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into())).join("Music")
}

pub fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

/// Скопировать текст в буфер обмена.
pub fn copy_text(text: &str) {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_UNICODETEXT: u32 = 13;
    let data = wide(text);
    unsafe {
        // Буфер может быть на миг занят другой программой.
        let opened = (0..10).any(|_| {
            let ok = OpenClipboard(null_mut()) != 0;
            if !ok {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            ok
        });
        if !opened {
            return;
        }
        EmptyClipboard();
        let mem = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2);
        if !mem.is_null() {
            let dst = GlobalLock(mem);
            // После удачного SetClipboardData памятью владеет система.
            if dst.is_null() {
                GlobalFree(mem);
            } else {
                std::ptr::copy_nonoverlapping(data.as_ptr(), dst.cast::<u16>(), data.len());
                GlobalUnlock(mem);
                if SetClipboardData(CF_UNICODETEXT, mem).is_null() {
                    GlobalFree(mem);
                }
            }
        }
        CloseClipboard();
    }
}
