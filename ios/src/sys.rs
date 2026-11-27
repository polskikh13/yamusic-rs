//! То, что на iOS даёт система: папки песочницы, модель телефона, браузер, «Поделиться»,
//! тёмная тема и отступ под «чёлку». UIKit зовётся только с главного потока.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, ClassType, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSArray, NSDictionary, NSNotification, NSNotificationCenter, NSOperationQueue, NSString, NSURL};
use objc2_ui_kit::{
    UIActivityViewController, UIApplication, UIApplicationDidReceiveMemoryWarningNotification, UITraitCollection, UIUserInterfaceStyle,
    UIWindow,
};
use std::path::PathBuf;
use std::time::Duration;

/// Зовётся на главном потоке до цикла событий.
pub fn init() {
    // Системе не хватает памяти: обложки скачаются заново, когда понадобятся (на Android это
    // делает обработчик LowMemory в lib.rs).
    let block = block2::RcBlock::new(|_n: std::ptr::NonNull<NSNotification>| crate::low_memory());
    unsafe {
        let center = NSNotificationCenter::defaultCenter();
        let observer = center.addObserverForName_object_queue_usingBlock(
            Some(UIApplicationDidReceiveMemoryWarningNotification),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &block,
        );
        // Наблюдатель нужен до конца жизни процесса.
        std::mem::forget(observer);
    }
}

/// Корень песочницы: iOS кладёт его в HOME каждому приложению.
fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// Настройки в Library/Application Support: их сохраняет резервная копия iCloud.
pub fn config_base() -> PathBuf {
    home().join("Library").join("Application Support")
}

/// Library/Caches система чистит сама, когда мало места.
pub fn cache_base() -> PathBuf {
    home().join("Library").join("Caches")
}

/// Модель телефона (`iPhone15,2`): Яндекс показывает её в списке устройств аккаунта.
pub fn hostname() -> Option<String> {
    let mut buf = [0u8; 64];
    let mut len = buf.len();
    // SAFETY: буфер и длина свои, sysctlbyname пишет не больше len байт.
    let rc = unsafe { libc::sysctlbyname(c"hw.machine".as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) };
    if rc != 0 {
        return None;
    }
    let s = std::ffi::CStr::from_bytes_until_nul(&buf[..len]).ok()?.to_str().ok()?;
    (!s.is_empty()).then(|| s.to_string())
}

/// Выполнить на главном потоке: сразу, если мы на нём, иначе через цикл событий Slint.
fn on_main(f: impl FnOnce(MainThreadMarker) + Send + 'static) {
    if let Some(mtm) = MainThreadMarker::new() {
        return f(mtm);
    }
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(mtm) = MainThreadMarker::new() {
            f(mtm);
        }
    });
}

/// UIApplication появляется только в UIApplicationMain, а `run` до него строит окно Slint.
/// Обёртка objc2 считает `sharedApplication` ненулевым и паникует на nil, поэтому зовём сами.
fn shared_app(_mtm: MainThreadMarker) -> Option<Retained<UIApplication>> {
    unsafe { msg_send![UIApplication::class(), sharedApplication] }
}

#[allow(deprecated)]
fn key_window(mtm: MainThreadMarker) -> Option<Retained<UIWindow>> {
    shared_app(mtm)?.keyWindow()
}

/// Открыть адрес в Safari.
pub fn open_url(url: &str) {
    let Some(url) = crate::client::browser_url(url) else { return };
    on_main(move |mtm| {
        let Some(u) = NSURL::URLWithString(&NSString::from_str(&url)) else {
            crate::config::log(format!("не удалось открыть {url}"));
            return;
        };
        let Some(app) = shared_app(mtm) else { return };
        unsafe { app.openURL_options_completionHandler(&u, &NSDictionary::new(), None) };
    });
}

/// Отдать ссылку системному «Поделиться».
pub fn share(text: &str) {
    let text = text.to_string();
    on_main(move |mtm| {
        let Some(root) = key_window(mtm).and_then(|w| w.rootViewController()) else {
            crate::config::log("не удалось открыть «Поделиться»: нет окна");
            return;
        };
        let item: Retained<AnyObject> = NSString::from_str(&text).into();
        let items = NSArray::from_retained_slice(&[item]);
        let vc = unsafe {
            UIActivityViewController::initWithActivityItems_applicationActivities(UIActivityViewController::alloc(mtm), &items, None)
        };
        // На iPad шторка открывается поповером: без опорного вида UIKit падает.
        if let (Some(pop), Some(view)) = (vc.popoverPresentationController(), root.view()) {
            pop.setSourceView(Some(&view));
        }
        root.presentViewController_animated_completion(&vc, true, None);
    });
}

/// Установленную Яндекс Музыку из песочницы не прочесть: шрифты и иконки идут с music.yandex.ru.
pub fn package_path(_package: &str) -> Option<PathBuf> {
    None
}

/// Скачанное ложится в Documents: с UIFileSharingEnabled папку видно в «Файлах».
pub fn music_dir() -> Option<PathBuf> {
    Some(home().join("Documents"))
}

/// Медиатеки, как MediaScanner на Android, у iOS для чужих файлов нет.
pub fn scan_file(_path: &std::path::Path) {}

/// В свою папку Documents писать можно всегда.
pub fn storage_permission() -> bool {
    true
}

/// Включена ли тёмная тема системы. Вне главного потока UIKit не спрашиваем.
pub fn night_mode() -> bool {
    MainThreadMarker::new()
        .is_some_and(|_mtm| unsafe { UITraitCollection::currentTraitCollection().userInterfaceStyle() } == UIUserInterfaceStyle::Dark)
}

/// Отступ сверху под статус-бар и «чёлку», в точках (то же, что логические пиксели Slint).
pub fn status_bar_height() -> Option<f32> {
    let mtm = MainThreadMarker::new()?;
    let top = key_window(mtm)?.safeAreaInsets().top as f32;
    (top > 0.0).then_some(top)
}

/// Окно UIKit создаётся уже в цикле событий: ждём его и ставим отступ, как на Android.
pub fn watch_safe_area(ui: &crate::App) {
    use slint::ComponentHandle;
    let weak = ui.as_weak();
    let timer = std::rc::Rc::new(slint::Timer::default());
    let t = timer.clone();
    let mut tries = 0;
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), move || {
        tries += 1;
        let Some(ui) = weak.upgrade() else { return t.stop() };
        if let Some(top) = status_bar_height() {
            ui.global::<crate::T>().set_top(top);
            t.stop();
        } else if tries > 50 {
            t.stop();
        }
    });
    // Замыкание держит таймер через `t`, поэтому он живёт и после выхода из функции.
}
