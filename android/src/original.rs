//! Шрифты и шейдер волны берутся из установленной Яндекс Музыки, как на десктопе из
//! app.asar: в репозитории их нет. APK — обычный zip, читаем из него только нужные файлы.

use crate::config::log;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const PACKAGE: &str = "ru.yandex.music";

struct Entry {
    header: u64,
    method: u16,
    packed: u64,
    size: u64,
}

pub struct Apk {
    file: File,
    entries: HashMap<String, Entry>,
}

impl Apk {
    /// APK официального приложения, если оно установлено.
    pub fn open_installed() -> Option<Apk> {
        let path = crate::sys::package_path(PACKAGE)?;
        let apk = Apk::open(&path);
        if apk.is_none() {
            log(format!("не прочитался {}", path.display()));
        }
        apk
    }

    /// Сплит с растровыми картинками под плотность экрана: в base.apk их нет.
    pub fn open_density_split() -> Option<Apk> {
        let base = crate::sys::package_path(PACKAGE)?;
        let dir = base.parent()?;
        ["xxhdpi", "xxxhdpi", "xhdpi", "hdpi", "mdpi"]
            .iter()
            .find_map(|d| Apk::open(&dir.join(format!("split_config.{d}.apk"))))
    }

    fn open(path: &Path) -> Option<Apk> {
        let mut file = File::open(path).ok()?;
        let len = file.metadata().ok()?.len();
        // Конец центрального каталога — в последних 64 КБ (плюс 22 байта самой записи).
        let tail_len = len.min(65_557);
        file.seek(SeekFrom::Start(len - tail_len)).ok()?;
        let mut tail = vec![0u8; tail_len as usize];
        file.read_exact(&mut tail).ok()?;
        let eocd = tail.windows(4).rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])?;
        let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let u32_at = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let count = u16_at(&tail, eocd + 10) as usize;
        let dir_size = u32_at(&tail, eocd + 12) as usize;
        let dir_offset = u32_at(&tail, eocd + 16) as u64;

        let mut dir = vec![0u8; dir_size];
        file.seek(SeekFrom::Start(dir_offset)).ok()?;
        file.read_exact(&mut dir).ok()?;
        let mut entries = HashMap::with_capacity(count);
        let mut at = 0;
        while at + 46 <= dir.len() && u32_at(&dir, at) == 0x0201_4b50 {
            let name_len = u16_at(&dir, at + 28) as usize;
            let extra_len = u16_at(&dir, at + 30) as usize;
            let comment_len = u16_at(&dir, at + 32) as usize;
            let name = String::from_utf8_lossy(dir.get(at + 46..at + 46 + name_len)?).into_owned();
            entries.insert(
                name,
                Entry {
                    header: u32_at(&dir, at + 42) as u64,
                    method: u16_at(&dir, at + 10),
                    packed: u32_at(&dir, at + 20) as u64,
                    size: u32_at(&dir, at + 24) as u64,
                },
            );
            at += 46 + name_len + extra_len + comment_len;
        }
        Some(Apk { file, entries })
    }

    pub fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        let e = self.entries.get(name)?;
        let mut head = [0u8; 30];
        self.file.seek(SeekFrom::Start(e.header)).ok()?;
        self.file.read_exact(&mut head).ok()?;
        let skip = u16::from_le_bytes([head[26], head[27]]) as u64 + u16::from_le_bytes([head[28], head[29]]) as u64;
        self.file.seek(SeekFrom::Start(e.header + 30 + skip)).ok()?;
        let mut packed = vec![0u8; e.packed as usize];
        self.file.read_exact(&mut packed).ok()?;
        match e.method {
            0 => Some(packed),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(&packed, e.size as usize).ok(),
            m => {
                log(format!("{name}: неизвестное сжатие {m}"));
                None
            }
        }
    }

    /// Векторная иконка `res/drawable*/<name>.xml` в виде SVG.
    pub fn icon(&mut self, name: &str) -> Option<String> {
        let suffix = format!("/{name}.xml");
        let path = self.entries.keys().find(|k| k.starts_with("res/drawable") && k.ends_with(&suffix))?.clone();
        let svg = crate::vector::to_svg(&self.read(&path)?);
        if svg.is_none() {
            log(format!("иконка {name} не разобралась"));
        }
        svg
    }

    /// Растровая картинка `res/drawable*/<name>.webp` или `.png`.
    pub fn raster(&mut self, name: &str) -> Option<Vec<u8>> {
        let (webp, png) = (format!("/{name}.webp"), format!("/{name}.png"));
        let path = self
            .entries
            .keys()
            .find(|k| k.starts_with("res/drawable") && (k.ends_with(&webp) || k.ends_with(&png)))?
            .clone();
        self.read(&path)
    }

    fn dex_names(&self) -> Vec<String> {
        let mut names: Vec<String> =
            self.entries.keys().filter(|n| n.starts_with("classes") && n.ends_with(".dex")).cloned().collect();
        // В старших dex код приложения, а не библиотек: шейдер найдётся быстрее.
        names.sort_by_key(|n| std::cmp::Reverse(n.len()));
        names
    }
}

const FONTS: &[&str] = &[
    "res/font/ys_text_regular.ttf",
    "res/font/ys_text_medium.ttf",
    "res/font/ys_text_bold.ttf",
    "res/font/ys_display_medium.ttf",
    "res/font/ys_display_bold.ttf",
    "res/font/ym_headline_bold_font.ttf",
    "res/font/ym_headline_black_font.otf",
];

/// Семейства «YS Text», «YS Display» и «YS Music Headline» становятся доступны в Slint по имени.
pub fn register_fonts(apk: &mut Apk) -> bool {
    use slint::fontique_012::fontique;
    let mut collection = slint::fontique_012::shared_collection();
    let mut ok = false;
    for name in FONTS {
        match apk.read(name) {
            Some(data) => {
                collection.register_fonts(fontique::Blob::new(std::sync::Arc::new(data)), None);
                ok = true;
            }
            None => log(format!("шрифт {name} не найден")),
        }
    }
    ok
}

/// Фрагментный шейдер «Моей волны» из байткода приложения. Строки dex хранятся как есть,
/// поэтому шейдер ищется по своему тексту. Найденный кладётся в кэш, чтобы не разжимать
/// мегабайты dex при каждом запуске.
pub fn wave_shader(apk: &mut Apk) -> Option<String> {
    let stamp = apk.entries.values().map(|e| e.size).sum::<u64>();
    let cached = crate::config::cache_dir().join(format!("wave-{stamp:x}.frag"));
    if let Ok(s) = std::fs::read_to_string(&cached) {
        return Some(s);
    }
    for name in apk.dex_names() {
        let Some(dex) = apk.read(&name) else { continue };
        if let Some(src) = find_shader(&dex) {
            let _ = std::fs::create_dir_all(crate::config::cache_dir());
            let _ = std::fs::write(&cached, &src);
            return Some(src);
        }
    }
    log("шейдер волны в приложении не найден");
    None
}

/// Новый вариант шейдера (с vBlobsAlpha и градиентом фона) — его оригинал и включает.
fn find_shader(dex: &[u8]) -> Option<String> {
    const MARK: &[u8] = b"uniform float vBlobsAlpha;";
    const START: &[u8] = b"precision highp float;";
    let mark = find(dex, MARK)?;
    let from = mark.saturating_sub(256);
    let start = from + rfind(&dex[from..mark], START)?;
    let end = mark + dex[mark..].iter().position(|&b| b == 0)?;
    let src = std::str::from_utf8(&dex[start..end]).ok()?;
    (src.contains("void main()") && src.contains("vColor[6]")).then(|| src.to_string())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}
