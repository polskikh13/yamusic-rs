//! Живое пятно «Моей волны»: шейдер официального приложения на процессоре.
//!
//! У оригинала это web-worker с WebGL (`vibe-animation-worker`, строкой base64 в чанке
//! `4683-….js` app.asar): три шумовых круга, искры по краю, цвета из обложки трека, отклик на
//! звук и на лайк. Состояние воркера считается здесь в своём потоке 25 раз в секунду, как
//! тикер оригинала; формулы, константы и порядок действий перенесены без изменений (тест
//! `state_matches_original_worker` сверяет с записью настоящего воркера).
//!
//! Чем отличается счёт. Холст считается в половину стороны: пятно размытое, а браузер и так
//! растягивает холст 650 px на 250% ширины блока. Шум плавный и берётся с сетки в 8 пикселей
//! холста (оригинал в режиме ShaderV3 тоже считает его с половинной текстуры), искра, которая
//! зависит лишь от угла, — из таблицы по углу. Тест `half_canvas_looks_like_full` сравнивает
//! такой кадр с полным холстом, сверенным с GLSL оригинала (расхождение там до 4 из 255). На паузе пятно доходит до покоя по кривым воркера и замирает:
//! кадры больше не считаются и окно не перерисовывается.

use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::output::{TAP, TAP_LEN};

/// Сторона холста: `desktopSizePx` оригинала. Браузер растягивает его на 250% ширины блока.
const SIZE: usize = 650;
/// Сторона холста, которую считает процессор: вчетверо меньше пикселей.
pub const RES: usize = SIZE / 2;
/// `desktopScale`: во сколько раз сцена меньше холста.
const SCALE: f32 = 0.35;
/// Частота тикера воркера.
const FPS: u64 = 25;
/// `.VibeWidgetAnimation_root_visible { opacity: .8 }`.
const OPACITY: f32 = 0.8;
/// `transition: opacity 1s ease-in-out`.
const FADE_MS: f32 = 1000.0;
/// Шаг сетки шума в пикселях полного холста.
const GRID_PX: usize = 8;
/// Пиксели прозрачнее 3/255 не рисуются: на любом фоне они меняют цвет не больше чем на 2/255,
/// а без бледных кончиков лучей прямоугольник кадра, который окно перерисовывает, на треть меньше.
const MIN_ALPHA: u8 = 3;
/// Отсчётов искры на оборот и долей между ними в таблице углов.
const LUT: usize = 2048;
const LUT_SUB: usize = 32;

const CIRCLE_WIDTH_BASE: f32 = 0.8;
const CIRCLE_WIDTH_STEP: f32 = 0.2;
const SPARK_STRENGTH_BASE: f32 = 1.0;
const SPARK_STRENGTH_STEP: f32 = 0.3;
const CIRCLE_RADIUS_BASE: f32 = 0.95;
const CIRCLE_RADIUS_STEP: f32 = 0.15;
const CIRCLE_OFFSET_BASE: f32 = 0.0;
const CIRCLE_OFFSET_STEP: f32 = 1.57;
/// Пи шейдера: в оригинале именно 3.1415, от этого зависит шов искр.
#[allow(clippy::approx_constant)]
const PI_GL: f32 = 3.1415;
/// `rotation` из состояния воркера: смещение шума каждого круга и скорость его вращения.
const ROTATION: [[f32; 3]; 3] = [[-0.3, 0.3, 0.2], [-0.3, -0.3, -0.2], [-0.3, -0.3, 0.2]];

// ---------- звук ----------

/// Размер окна анализатора: `analyser.fftSize = 32`.
const FFT: usize = TAP_LEN;

/// Средние уровни трёх полос, как `getAverageFrequencies` оригинала: 0–250, 500–2000 и
/// 2000–4000 Гц. Ширину полосы оригинал считает как `sampleRate / frequencyBinCount`,
/// то есть вдвое шире настоящей, — так и оставлено, иначе отклик был бы другим.
fn bands(x: &[f32; FFT], rate: u32) -> [f64; 3] {
    let bytes = byte_frequency_data(x);
    let width = rate.max(8000) as f64 / (FFT / 2) as f64;
    let mut out = [0f64; 3];
    for (o, (low, high)) in out.iter_mut().zip([(0.0, 250.0), (500.0, 2000.0), (2000.0, 4000.0)]) {
        let (a, b) = ((low / width).floor() as usize, (high / width).floor() as usize);
        let sum: f64 = (a..=b).map(|i| bytes.get(i).copied().unwrap_or(0) as f64 / 256.0).sum();
        *o = sum / (b - a + 1) as f64;
    }
    out
}

/// `AnalyserNode.getByteFrequencyData` при `smoothingTimeConstant = 0`: окно Блэкмана, БПФ,
/// модуль, делённый на размер окна, децибелы от −100 до −30 в байт.
fn byte_frequency_data(x: &[f32; FFT]) -> [u8; FFT / 2] {
    use std::f64::consts::PI;
    let mut w = [0f64; FFT];
    for (i, v) in w.iter_mut().enumerate() {
        let p = i as f64 / FFT as f64;
        let window = 0.42 - 0.5 * (2.0 * PI * p).cos() + 0.08 * (4.0 * PI * p).cos();
        *v = x[i] as f64 * window;
    }
    let mut out = [0u8; FFT / 2];
    for (k, o) in out.iter_mut().enumerate() {
        let (mut re, mut im) = (0f64, 0f64);
        for (n, v) in w.iter().enumerate() {
            let a = 2.0 * PI * (k * n) as f64 / FFT as f64;
            re += v * a.cos();
            im -= v * a.sin();
        }
        let mag = (re * re + im * im).sqrt() / FFT as f64;
        let db = if mag == 0.0 { -100.0 } else { 20.0 * mag.log10() };
        *o = (255.0 * (db + 100.0) / 70.0).clamp(0.0, 255.0) as u8;
    }
    out
}

/// Чтение отвода звука ([`TAP`]): если звуковой поток не писал дольше 150 мс (выход закрыт
/// или переоткрывается), в кольце старые сэмплы — для анализатора это тишина.
struct Listener {
    writes: u32,
    changed: Instant,
}

impl Listener {
    fn bands(&mut self, now: Instant) -> [f64; 3] {
        let (writes, samples, rate) = TAP.snapshot();
        if writes != self.writes {
            self.writes = writes;
            self.changed = now;
        }
        let fresh = now.duration_since(self.changed) < Duration::from_millis(150);
        bands(if fresh { &samples } else { &[0.0; FFT] }, rate)
    }
}

// ---------- состояние воркера ----------

/// Случайные числа воркера: начало времени и разброс оттенков. Точность не важна.
struct Rng {
    state: u64,
    /// Постоянное значение вместо случайного: так порт сверяется с воркером оригинала.
    fixed: Option<f64>,
}

impl Rng {
    fn new() -> Rng {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        Rng { state: nanos | 1, fixed: None }
    }

    #[cfg(test)]
    fn fixed(value: f64) -> Rng {
        Rng { state: 1, fixed: Some(value) }
    }

    fn unit(&mut self) -> f64 {
        if let Some(v) = self.fixed {
            return v;
        }
        let s = &mut self.state;
        *s ^= *s << 13;
        *s ^= *s >> 7;
        *s ^= *s << 17;
        (*s >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `Math.floor(Math.random() * (max - min + 1)) + min`.
    fn range(&mut self, min: f64, max: f64) -> f64 {
        (self.unit() * (max.floor() - min + 1.0)).floor() + min
    }
}

/// Плавное значение воркера: каждый кадр подходит к цели на долю, растущую со временем.
/// Когда время перехода вышло, значение встаёт точно на цель.
#[derive(Clone, Copy)]
struct Tween {
    cur: f64,
    target: f64,
    elapsed: f64,
    duration: f64,
}

impl Tween {
    fn new(v: f64, duration: f64) -> Tween {
        Tween { cur: v, target: v, elapsed: 0.0, duration }
    }

    fn update(&mut self, target: f64) {
        self.target = target;
        self.elapsed = 0.0;
    }

    fn next(&mut self, dt: f64) {
        let k = (self.elapsed / self.duration).clamp(0.0, 1.0);
        self.elapsed += dt;
        self.cur += (self.target - self.cur) * k;
    }

    fn done(&self) -> bool {
        self.cur == self.target
    }
}

/// HSL в RGB, формула воркера. `h` в градусах, `s` и `l` от 0 до 1.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> [f64; 3] {
    let f = |n: f64| {
        let k = (n + h / 30.0) % 12.0;
        let a = s * l.min(1.0 - l);
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

/// Цвет, все три канала которого идут к цели за 3 секунды.
#[derive(Clone, Copy)]
struct Tint([Tween; 3]);

impl Tint {
    fn new(hue: f64) -> Tint {
        Tint(hsl_to_rgb(hue, 1.0, 0.5).map(|c| Tween::new(c, 3000.0)))
    }

    fn update(&mut self, h: f64, s: f64, l: f64) {
        for (t, c) in self.0.iter_mut().zip(hsl_to_rgb(h, s, l)) {
            t.update(c);
        }
    }

    fn value(&self) -> [f32; 3] {
        self.0.map(|t| t.cur as f32)
    }
}

/// Оттенок, насыщенность и светлота круга: `customColors` оригинала.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hsl {
    pub h: f64,
    pub s: f64,
    pub l: f64,
}

/// Цвета трёх кругов: верхнего (малого), среднего и нижнего (большого).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Palette {
    pub top: Hsl,
    pub middle: Hsl,
    pub bottom: Hsl,
}

impl Palette {
    /// Цвета «Моей волны», пока трек не играет: жёлтый, розовый и пурпурный (`W` в чанке 2179).
    pub const DEFAULT: Palette = Palette {
        top: Hsl { h: 50.0, s: 1.0, l: 0.5 },
        middle: Hsl { h: 330.0, s: 1.0, l: 0.5 },
        bottom: Hsl { h: 300.0, s: 1.0, l: 0.5 },
    };

    /// Цвета по основному цвету трека: чем меньше круг, тем он ярче и насыщеннее.
    pub fn from_rgb(c: [u8; 3]) -> Palette {
        let (h, s, l) = rgb_to_hsl(c);
        let hsl = |ds: f64, dl: f64| Hsl { h: h.clamp(0.0, 360.0), s: ((s + ds) / 100.0).clamp(0.0, 1.0), l: ((l + dl) / 100.0).clamp(0.0, 1.0) };
        Palette { top: hsl(15.0, 10.0), middle: hsl(15.0, 5.0), bottom: hsl(0.0, 0.0) }
    }
}

/// Оттенок целым числом градусов, насыщенность и светлота в процентах с одним знаком —
/// так считает оригинал.
fn rgb_to_hsl(c: [u8; 3]) -> (f64, f64, f64) {
    let [r, g, b] = c.map(|v| v as f64 / 255.0);
    let (min, max) = (r.min(g).min(b), r.max(g).max(b));
    let d = max - min;
    let l = (min + max) / 2.0;
    let mut h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d) % 6.0
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h = (h * 60.0).round();
    if h < 0.0 {
        h += 360.0;
    }
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()) };
    let tenth = |v: f64| (v * 1000.0).round() / 10.0;
    (h, tenth(s), tenth(l))
}

/// Шесть цветов шейдера: начало и конец градиента каждого круга.
struct Colors {
    top: [Tint; 2],
    middle: [Tint; 2],
    bottom: [Tint; 2],
}

impl Colors {
    /// Начальные цвета воркера: `collectionHue` по умолчанию 10.
    fn new(rng: &mut Rng) -> Colors {
        let shifted = |hue: f64| (hue + 280.0) % 360.0;
        let wrap = |t: f64, e: f64| if (280.0..360.0).contains(&e) { t % 360.0 } else { t };
        let e = shifted(10.0);
        let i = wrap(e + rng.range(30.0, 40.0), e);
        Colors { top: [Tint::new(50.0), Tint::new(50.0)], middle: [Tint::new(300.0), Tint::new(320.0)], bottom: [Tint::new(e), Tint::new(i)] }
    }

    fn tints(&mut self) -> impl Iterator<Item = &mut Tween> {
        [&mut self.top, &mut self.middle, &mut self.bottom].into_iter().flatten().flat_map(|t| t.0.iter_mut())
    }

    fn update(&mut self, p: Palette) {
        for (pair, c) in [(&mut self.top, p.top), (&mut self.middle, p.middle), (&mut self.bottom, p.bottom)] {
            pair.iter_mut().for_each(|t| t.update(c.h, c.s, c.l));
        }
    }

    fn next(&mut self, dt: f64) {
        self.tints().for_each(|t| t.next(dt));
    }

    fn done(&mut self) -> bool {
        self.tints().all(|t| t.done())
    }

    /// Порядок `vColor`: начала нижнего, среднего и верхнего круга, затем их концы.
    fn value(&self) -> [[f32; 3]; 6] {
        [self.bottom[0].value(), self.middle[0].value(), self.top[0].value(), self.bottom[1].value(), self.middle[1].value(), self.top[1].value()]
    }
}

/// Юниформы шейдера на один кадр.
#[derive(Clone, Copy, Debug)]
struct Uniforms {
    time: f32,
    colors: [[f32; 3]; 6],
    audio: [f32; 3],
    react: [f32; 3],
}

/// Состояние воркера между кадрами.
struct State {
    energy: Tween,
    time: f64,
    colors: Colors,
    audio: [f64; 3],
    audio_ratio: Tween,
    freq: Option<[f64; 3]>,
    react: [Tween; 3],
    /// Отложенные шаги анимации лайка: (когда по часам воркера в мс, какой круг, к какому значению).
    timers: Vec<(f64, usize, f64)>,
}

impl State {
    fn new(rng: &mut Rng) -> State {
        State {
            energy: Tween::new(0.2, 1000.0),
            time: (3600.0 * rng.unit()).floor(),
            colors: Colors::new(rng),
            audio: [0.0; 3],
            audio_ratio: Tween::new(0.0, 1000.0),
            freq: None,
            react: [Tween::new(0.0, 600.0); 3],
            timers: Vec::new(),
        }
    }

    /// `playAnimation({})`: энергия по умолчанию 0,6, в шейдер идёт `0.4 * (energy + 1)`.
    fn play(&mut self) {
        self.audio_ratio.update(1.0);
        self.energy.update(0.4 * (0.6 + 1.0));
    }

    fn idle(&mut self) {
        self.energy.update(0.2);
        self.audio_ratio.update(0.0);
    }

    /// Лайк: круги вспыхивают по очереди и гаснут через 0,85–1,05 секунды.
    fn like(&mut self, now: f64) {
        self.react[0].update(0.7);
        for (ms, circle, value) in [(100.0, 1, 0.7), (150.0, 2, 0.7), (850.0, 0, 0.0), (950.0, 1, 0.0), (1050.0, 2, 0.0)] {
            self.timers.push((now + ms, circle, value));
        }
    }

    /// Сработать таймерам, чьё время пришло: у оригинала это `setTimeout` между кадрами.
    fn fire_timers(&mut self, now: f64) {
        let mut due = Vec::new();
        self.timers.retain(|&(at, circle, value)| {
            let fire = at <= now;
            if fire {
                due.push((circle, value));
            }
            !fire
        });
        for (circle, value) in due {
            self.react[circle].update(value);
        }
    }

    fn update(&mut self, dt: f64) {
        self.energy.next(dt);
        self.colors.next(dt);
        self.react.iter_mut().for_each(|t| t.next(dt));
        self.time = (self.time + self.energy.cur * dt / 1000.0) % 86400.0;
        if let Some(freq) = self.freq {
            self.audio_ratio.next(dt);
            for (a, f) in self.audio.iter_mut().zip(freq) {
                *a = audio_param(*a, f) * self.audio_ratio.cur;
            }
        }
    }

    /// Все переходы пришли к цели, звук и вспышки погасли: дальше меняется только время.
    fn resting(&mut self) -> bool {
        self.energy.done() && self.audio_ratio.done() && self.react.iter().all(Tween::done) && self.colors.done() && self.timers.is_empty() && self.audio == [0.0; 3]
    }

    fn uniforms(&self) -> Uniforms {
        Uniforms { time: self.time as f32, colors: self.colors.value(), audio: self.audio.map(|v| v as f32), react: self.react.map(|t| t.cur as f32) }
    }
}

/// Уровень полосы растёт сразу, а спадает не быстрее 0,02 за кадр.
fn audio_param(prev: f64, new: f64) -> f64 {
    let (s, n) = (prev.clamp(0.0, 1.0), new.clamp(0.0, 1.0));
    if n > s {
        n
    } else if n < s {
        s - (s - n).min(0.02)
    } else {
        s
    }
}

// ---------- шейдер ----------

#[inline]
fn fract(x: f32) -> f32 {
    x - x.floor()
}

#[inline]
fn mod289(x: f32) -> f32 {
    x - 289.0 * (x / 289.0).floor()
}

#[inline]
fn permute(x: f32) -> f32 {
    mod289((x * 34.0 + 1.0) * x)
}

/// `smoothstep` GLSL. Края могут идти в обратном порядке — шейдер этим пользуется.
#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Симплекс-шум Ashima из шейдера.
fn snoise3(v: [f32; 3]) -> f32 {
    const CX: f32 = 0.166_666_7;
    const CY: f32 = 0.333_333_3;
    let s = (v[0] + v[1] + v[2]) * CY;
    let mut i = [(v[0] + s).floor(), (v[1] + s).floor(), (v[2] + s).floor()];
    let t = (i[0] + i[1] + i[2]) * CX;
    let x0 = [v[0] - i[0] + t, v[1] - i[1] + t, v[2] - i[2] + t];

    let step = |edge: f32, x: f32| if x < edge { 0.0f32 } else { 1.0 };
    let g = [step(x0[1], x0[0]), step(x0[2], x0[1]), step(x0[0], x0[2])];
    let l = [1.0 - g[0], 1.0 - g[1], 1.0 - g[2]];
    let i1 = [g[0].min(l[2]), g[1].min(l[0]), g[2].min(l[1])];
    let i2 = [g[0].max(l[2]), g[1].max(l[0]), g[2].max(l[1])];

    let x1 = [x0[0] - i1[0] + CX, x0[1] - i1[1] + CX, x0[2] - i1[2] + CX];
    let x2 = [x0[0] - i2[0] + 2.0 * CX, x0[1] - i2[1] + 2.0 * CX, x0[2] - i2[2] + 2.0 * CX];
    let x3 = [x0[0] - 1.0 + 3.0 * CX, x0[1] - 1.0 + 3.0 * CX, x0[2] - 1.0 + 3.0 * CX];

    i = [mod289(i[0]), mod289(i[1]), mod289(i[2])];
    let oz = [0.0, i1[2], i2[2], 1.0];
    let oy = [0.0, i1[1], i2[1], 1.0];
    let ox = [0.0, i1[0], i2[0], 1.0];

    const N: f32 = 0.142_857_15;
    let ns = [2.0 * N, 0.5 * N - 1.0, N];
    let corners = [x0, x1, x2, x3];
    let mut sum = 0.0;
    for k in 0..4 {
        let p = permute(permute(permute(i[2] + oz[k]) + i[1] + oy[k]) + i[0] + ox[k]);
        let j = p - 49.0 * (p * ns[2] * ns[2]).floor();
        let x_ = (j * ns[2]).floor();
        let y_ = (j - 7.0 * x_).floor();
        let x = x_ * ns[0] + ns[1];
        let y = y_ * ns[0] + ns[1];
        let h = 1.0 - x.abs() - y.abs();
        let sh = if h > 0.0 { 0.0 } else { -1.0 };
        let mut grad = [x + (x.floor() * 2.0 + 1.0) * sh, y + (y.floor() * 2.0 + 1.0) * sh, h];
        let norm = 1.792_842_9 - 0.853_734_73 * (grad[0] * grad[0] + grad[1] * grad[1] + grad[2] * grad[2]);
        grad = [grad[0] * norm, grad[1] * norm, grad[2] * norm];
        let c = corners[k];
        let m = (0.6 - (c[0] * c[0] + c[1] * c[1] + c[2] * c[2])).max(0.0);
        let m = m * m;
        sum += m * m * (grad[0] * c[0] + grad[1] * c[1] + grad[2] * c[2]);
    }
    42.0 * sum
}

#[inline]
fn tri(x: f32) -> f32 {
    (fract(x) - 0.5).abs()
}

fn tri_noise(mut p: [f32; 3], time: f32) -> f32 {
    let mut z = 0.4f32;
    let mut rz = 0.1f32;
    let mut bp = p;
    // `vTime * .1 * spd` при spd = 0.1.
    let shift = time * 0.1 * 0.1;
    for _ in 0..5 {
        let q = [bp[0] * 0.01, bp[1] * 0.01, bp[2] * 0.01];
        let dg = [tri(q[2] + tri(q[1] * 20.0)), tri(q[2] + tri(q[0])), tri(q[1] + tri(q[0]))];
        for a in 0..3 {
            p[a] += dg[a] + shift;
            bp[a] *= 4.0;
        }
        z *= 0.9;
        for v in &mut p {
            *v *= 1.6;
        }
        rz += tri(p[2] + tri(0.6 * p[0] + 0.1 * tri(p[1]))) / z;
    }
    smoothstep(0.0, 8.0, rz + (rz + z.sin() * 2.8).sin() * 2.2)
}

/// Искра в направлении `angle`: зависит только от угла и времени.
fn spark(angle: f32, time: f32) -> f32 {
    let idx = (angle / PI_GL) / 2.0;
    // Тот же шум, повёрнутый на 3.1415: у шва угла первый шум рвётся, второй нет.
    let (s, c) = PI_GL.sin_cos();
    let (x, y) = (angle.cos(), angle.sin());
    let pa1 = (x * s + y * c).atan2(x * c - y * s);
    let idx1 = (pa1 / PI_GL) / 2.0;
    let idx21 = (pa1 / PI_GL + 1.0) / 2.0 * PI_GL;
    let mut v = tri_noise([idx, 0.0, 0.0], time);
    let seam = smoothstep(0.9, 1.0, idx21.sin());
    if seam > 0.0 {
        v += (tri_noise([idx1, 0.0, idx1], time) - v) * seam;
    }
    v = v * 0.2 + v.powi(10);
    smoothstep(0.0, v.max(1e-5), 0.3) * v
}

/// Кривая времени CSS: `cubic-bezier(x1, y1, x2, y2)`.
fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let at = |a: f32, b: f32, t: f32| ((1.0 - 3.0 * b + 3.0 * a) * t + (3.0 * b - 6.0 * a)) * t * t + 3.0 * a * t;
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut t = x;
    for _ in 0..24 {
        let v = at(x1, x2, t);
        if (v - x).abs() < 1e-5 {
            break;
        }
        if v < x {
            lo = t;
        } else {
            hi = t;
        }
        t = (lo + hi) / 2.0;
    }
    at(y1, y2, t)
}

/// Таблицы, которые не меняются от кадра к кадру, и рабочие буферы.
struct Renderer {
    /// Сторона холста в пикселях счёта и шаг сетки шума в них же.
    res: usize,
    grid: usize,
    /// Координата сцены по центру пикселя: по x слева направо, по y снизу вверх.
    axis: Vec<f32>,
    /// Место пикселя в таблице искры: отсчёт × `LUT_SUB` + доля.
    angle: Vec<u16>,
    lut: Vec<f32>,
    grid_n: usize,
    /// Общий шум и шум трёх кругов в узлах сетки. NaN — узел в этом кадре ещё не считали.
    grids: [Vec<f32>; 4],
    /// Холст целиком и прямоугольник, закрашенный прошлым кадром: чистить нужно только его.
    canvas: Vec<Rgba8Pixel>,
    painted: [usize; 4],
}

/// Кадр: закрашенный прямоугольник холста (x, y, ширина, высота в пикселях [`RES`]) и его
/// пиксели с умноженной альфой.
pub struct Frame {
    pub rect: [usize; 4],
    pub pixels: SharedPixelBuffer<Rgba8Pixel>,
}

/// Постоянные одного круга на кадр.
#[derive(Clone, Copy, Default)]
struct Layer {
    radius: f32,
    width: f32,
    spark_k: f32,
    like: f32,
    react_k: f32,
    strength_base: f32,
    offset: f32,
    scale: f32,
    shift: [f32; 2],
    sin_t: f32,
    c1: [f32; 3],
    dc: [f32; 3],
}

impl Renderer {
    fn new(res: usize) -> Renderer {
        Renderer::with_grid(res, (GRID_PX * res / SIZE).max(1))
    }

    fn with_grid(res: usize, grid: usize) -> Renderer {
        let axis: Vec<f32> = (0..res).map(|i| ((i as f32 + 0.5) / res as f32 * 2.0 - 1.0) / SCALE).collect();
        let mut angle = vec![0u16; res * res];
        let span = (LUT * LUT_SUB) as f64;
        for row in 0..res {
            let y = axis[res - 1 - row] as f64;
            for col in 0..res {
                let a = y.atan2(axis[col] as f64);
                let pos = ((a + std::f64::consts::PI) / std::f64::consts::TAU * span).round();
                angle[row * res + col] = (pos as usize % (LUT * LUT_SUB)) as u16;
            }
        }
        let grid_n = (res - 1) / grid + 2;
        Renderer {
            res,
            grid,
            axis,
            angle,
            lut: vec![0.0; LUT + 1],
            grid_n,
            grids: std::array::from_fn(|_| vec![f32::NAN; grid_n * grid_n]),
            canvas: vec![Rgba8Pixel { r: 0, g: 0, b: 0, a: 0 }; res * res],
            painted: [0; 4],
        }
    }

    /// Нарисовать кадр и отдать только его закрашенную часть с прозрачной каймой в 2 пикселя
    /// (чтобы растянутый край сходил на нет): интерфейсу тогда перерисовывать не весь холст,
    /// а одно пятно.
    fn frame(&mut self, u: &Uniforms, circles: usize, opacity: f32, crop: [usize; 4]) -> Frame {
        let [x0, y0, x1, y1] = self.render(u, circles, opacity, crop);
        if x1 <= x0 || y1 <= y0 {
            return Frame { rect: [0, 0, 1, 1], pixels: SharedPixelBuffer::new(1, 1) };
        }
        let res = self.res;
        let (x0, y0) = (x0.saturating_sub(2), y0.saturating_sub(2));
        let (x1, y1) = ((x1 + 2).min(res), (y1 + 2).min(res));
        let (w, h) = (x1 - x0, y1 - y0);
        let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
        for (row, line) in pixels.make_mut_slice().chunks_exact_mut(w).enumerate() {
            let at = (y0 + row) * res + x0;
            line.copy_from_slice(&self.canvas[at..at + w]);
        }
        Frame { rect: [x0, y0, w, h], pixels }
    }

    /// Шум круга `which` (0 — общий) в точке пикселя, билинейно по узлам сетки.
    #[inline]
    fn noise(&mut self, which: usize, col: usize, row: usize, layer: &Layer, time: f32) -> f32 {
        let (grid, res) = (self.grid, self.res);
        let (gx, gy) = (col / grid, row / grid);
        let (tx, ty) = ((col % grid) as f32 / grid as f32, (row % grid) as f32 / grid as f32);
        let mut v = [0f32; 4];
        for (k, (dx, dy)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
            let at = (gy + dy) * self.grid_n + gx + dx;
            let mut n = self.grids[which][at];
            if n.is_nan() {
                // Узел стоит на центре своего пикселя; за краем холста координата продолжается.
                let node = |i: usize| (((i * grid) as f32 + 0.5) / res as f32 * 2.0 - 1.0) / SCALE;
                let (ux, uy) = (node(gx + dx), -node(gy + dy));
                n = if which == 0 {
                    snoise3([ux * 1.2, uy * 1.2, time * 0.5])
                } else {
                    let (lx, ly) = (ux * layer.scale + layer.shift[0], uy * layer.scale + layer.shift[1]);
                    snoise3([lx * 1.2 + layer.offset, ly * 1.2 + layer.offset, time * 0.5 + layer.offset]) * 0.5 + 0.5
                };
                self.grids[which][at] = n;
            }
            v[k] = n;
        }
        let top = v[0] + (v[1] - v[0]) * tx;
        let bottom = v[2] + (v[3] - v[2]) * tx;
        top + (bottom - top) * ty
    }

    /// Нарисовать кадр на холсте (`res`×`res`, альфа уже умножена). Вне `crop`
    /// (x0, y0, x1, y1 в пикселях холста) и вне кругов пиксели остаются прозрачными.
    /// Возвращает закрашенный прямоугольник: x0, y0, x1, y1.
    fn render(&mut self, u: &Uniforms, circles: usize, opacity: f32, crop: [usize; 4]) -> [usize; 4] {
        let res = self.res;
        let [px0, py0, px1, py1] = self.painted;
        for row in py0..py1 {
            self.canvas[row * res + px0..row * res + px1].fill(Rgba8Pixel { r: 0, g: 0, b: 0, a: 0 });
        }
        let mut painted = [res, res, 0, 0];
        let time = u.time;
        for (k, v) in self.lut.iter_mut().take(LUT).enumerate() {
            let a = -std::f32::consts::PI + std::f32::consts::TAU * k as f32 / LUT as f32;
            *v = spark(a, time);
        }
        self.lut[LUT] = self.lut[0];
        for g in &mut self.grids {
            g.fill(f32::NAN);
        }

        let mut layers = [Layer::default(); 3];
        for (i, l) in layers.iter_mut().enumerate() {
            let fi = i as f32;
            let like = u.react[i];
            let audio = u.audio[i];
            let offset = CIRCLE_OFFSET_BASE + CIRCLE_OFFSET_STEP * fi;
            let (s, c) = (time * ROTATION[i][2]).sin_cos();
            let strength = like.max(audio);
            *l = Layer {
                radius: CIRCLE_RADIUS_BASE - CIRCLE_RADIUS_STEP * fi,
                width: CIRCLE_WIDTH_BASE - CIRCLE_WIDTH_STEP * fi,
                spark_k: SPARK_STRENGTH_BASE - SPARK_STRENGTH_STEP * fi,
                like,
                react_k: like.max(audio * 0.6) * 50.0,
                strength_base: 0.15 * (1.0 + 1.5 * (-(time * 2.0 + offset * 0.5).sin() * 0.5)) + 0.3 * strength,
                offset,
                scale: 1.0 - like * 0.5,
                shift: [ROTATION[i][0] * c - ROTATION[i][1] * s, ROTATION[i][0] * s + ROTATION[i][1] * c],
                sin_t: (time + offset).sin(),
                c1: u.colors[i],
                dc: [u.colors[i + 3][0] - u.colors[i][0], u.colors[i + 3][1] - u.colors[i][1], u.colors[i + 3][2] - u.colors[i][2]],
            };
        }
        let circles = circles.min(3);
        let [x0, y0, x1, y1] = crop.map(|v| v.min(res));

        for row in y0..y1 {
            let uy = self.axis[res - 1 - row];
            for col in x0..x1 {
                let ux = self.axis[col];
                let len = (ux * ux + uy * uy).sqrt();
                let pos = self.angle[row * res + col] as usize;
                let (k, f) = (pos / LUT_SUB, (pos % LUT_SUB) as f32 / LUT_SUB as f32);
                let spark = self.lut[k] + (self.lut[k + 1] - self.lut[k]) * f;

                // Край круга без общего шума: шум добавит не больше 0,3.
                let mut edge = [0f32; 3];
                let mut reach = 0f32;
                for i in 0..circles {
                    let l = &layers[i];
                    let base = l.spark_k * spark;
                    edge[i] = l.radius + l.width * 0.5 + base * (1.0 + l.react_k * base);
                    reach = reach.max(edge[i]);
                }
                if len >= reach + 0.3 {
                    continue;
                }
                let n0 = self.noise(0, col, row, &layers[0], time);

                let mut rgb = [0f32; 3];
                let mut alpha = 0f32;
                for i in 0..circles {
                    let l = layers[i];
                    let outer = edge[i] + 0.3 * n0;
                    if len >= outer {
                        continue;
                    }
                    let (lx, ly) = (ux * l.scale + l.shift[0], uy * l.scale + l.shift[1]);
                    let llen = (lx * lx + ly * ly).sqrt();
                    let r0 = self.noise(i + 1, col, row, &l, time);
                    let v0 = smoothstep(r0 + 0.1 + (l.sin_t + 1.0), r0, llen);
                    let a = v0 * smoothstep(outer, 0.5, len);
                    if a <= 0.0 {
                        continue;
                    }
                    let d0 = (llen - r0).abs();
                    let glow = l.strength_base / (1.0 + d0 + d0 * 10.0);
                    let flash = 0.6 * l.like * (1.0 - smoothstep(0.2, outer * 0.8, len));
                    let m = ly * 2.0;
                    for ch in 0..3 {
                        let col = (l.c1[ch] + l.dc[ch] * m + glow).clamp(0.0, 1.0) + flash;
                        rgb[ch] += (col - rgb[ch]) * a;
                    }
                    alpha = alpha.max(a);
                }
                if alpha <= 0.0 {
                    continue;
                }
                // Холст оригинала без premultipliedAlpha: браузер сам умножает цвет на альфу.
                let a = alpha.min(1.0) * opacity;
                let px = |c: f32| (c.clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
                let alpha8 = (a * 255.0 + 0.5) as u8;
                if alpha8 < MIN_ALPHA {
                    continue;
                }
                self.canvas[row * res + col] = Rgba8Pixel { r: px(rgb[0]), g: px(rgb[1]), b: px(rgb[2]), a: alpha8 };
                painted = [painted[0].min(col), painted[1].min(row), painted[2].max(col + 1), painted[3].max(row + 1)];
            }
        }
        self.painted = if painted[2] > painted[0] { painted } else { [0; 4] };
        self.painted
    }
}

// ---------- поток анимации ----------

pub enum Msg {
    /// Страница волны показана или скрыта. При показе пятно проявляется заново за секунду.
    Mount(bool),
    /// Окно видно или свёрнуто: `enable` и `disable` оригинала.
    Visible(bool),
    Playing(bool),
    Like,
    Palette(Palette),
    /// Видимая часть холста в пикселях [`RES`]: x0, y0, x1, y1.
    Crop([usize; 4]),
}

/// Что поток анимации сообщает интерфейсу.
pub enum Event {
    Frame(Frame),
    /// Кадры не успевают даже с двумя кругами: дальше показывать неподвижную картинку.
    Fallback,
}

#[derive(Clone)]
pub struct Vibe {
    tx: Sender<Msg>,
    /// Интерфейс ещё не забрал прошлый кадр: новый рисовать незачем.
    busy: Arc<AtomicBool>,
}

impl Vibe {
    pub fn spawn(emit: impl Fn(Event) + Send + 'static) -> Vibe {
        let (tx, rx) = channel();
        let busy = Arc::new(AtomicBool::new(false));
        let flag = busy.clone();
        std::thread::Builder::new().name("vibe".into()).spawn(move || run(rx, flag, emit)).expect("vibe thread");
        Vibe { tx, busy }
    }

    pub fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
    }

    /// Интерфейс показал кадр.
    pub fn frame_shown(&self) {
        self.busy.store(false, Ordering::Release);
    }
}

fn run(rx: Receiver<Msg>, busy: Arc<AtomicBool>, emit: impl Fn(Event)) {
    let mut rng = Rng::new();
    let mut state = State::new(&mut rng);
    let mut renderer: Option<Renderer> = None;
    let (mut mounted, mut visible, mut playing) = (false, true, false);
    let mut palette: Option<Palette> = None;
    let mut crop = [0, 0, RES, RES];
    let mut circles = 3;
    let mut fallback = false;
    // Пятно на паузе пришло к покою и последний кадр уже отдан: новых не нужно.
    let mut rested = false;
    let frame = Duration::from_millis(1000 / FPS);
    // Часы воркера в миллисекундах: по ним идут отложенные шаги лайка.
    let clock = Instant::now();
    let ms = |t: Instant| t.duration_since(clock).as_secs_f64() * 1000.0;
    let mut last = clock;
    let mut next = clock;
    let mut shown_at = clock;
    let mut listener = Listener { writes: 0, changed: clock };
    // Замер скорости, как у оригинала: среднее за 10 секунд, три плохих замера подряд.
    let (mut spent, mut frames, mut slow) = (Duration::ZERO, 0u32, 0u32);

    loop {
        let active = mounted && visible && !fallback && !rested;
        // Звук нужен анализатору, только пока пятно играет на экране.
        TAP.listen(active && playing);
        let msg = if active {
            match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            }
        };
        if let Some(msg) = msg {
            let now = Instant::now();
            if !active {
                // Пока кадры стояли, время воркера не шло: тикер начинает с этого момента.
                last = now;
                next = now;
            }
            rested = false;
            match msg {
                Msg::Mount(on) => {
                    if on && !mounted {
                        // Оригинал при каждом показе страницы создаёт воркер заново: случайное
                        // начало времени и исходные цвета, из которых пятно переходит к нужным.
                        state = State::new(&mut rng);
                        if let Some(p) = palette {
                            state.colors.update(p);
                        }
                        if playing {
                            state.play();
                        } else {
                            state.idle();
                        }
                        shown_at = now;
                        fallback = false;
                    }
                    if !on {
                        // Таблицы и холст — около 1 МБ: без страницы волны они не нужны.
                        renderer = None;
                    }
                    mounted = on;
                }
                Msg::Visible(on) => visible = on,
                Msg::Playing(on) => {
                    playing = on;
                    if on {
                        state.play();
                    } else {
                        state.idle();
                    }
                }
                Msg::Like => state.like(ms(now)),
                Msg::Palette(p) => {
                    palette = Some(p);
                    state.colors.update(p);
                }
                Msg::Crop(c) => crop = c,
            }
            continue;
        }

        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f64() * 1000.0;
        last = now;
        next += frame;
        if next < now {
            next = now + frame;
        }
        // Страница оригинала шлёт воркеру уровни звука, только пока трек играет.
        if playing {
            state.freq = Some(listener.bands(now));
        }
        state.fire_timers(ms(now));
        state.update(dt);
        if busy.swap(true, Ordering::AcqRel) {
            continue;
        }
        let fade = (now.duration_since(shown_at).as_secs_f32() * 1000.0 / FADE_MS).min(1.0);
        let opacity = OPACITY * cubic_bezier(0.42, 0.0, 0.58, 1.0, fade);
        let frame = renderer.get_or_insert_with(|| Renderer::new(RES)).frame(&state.uniforms(), circles, opacity, crop);
        emit(Event::Frame(frame));
        // На паузе пятно доходит до покоя и замирает: у оригинала оно и дальше медленно
        // плывёт, но ради этого окно пришлось бы перерисовывать без конца.
        rested = !playing && fade >= 1.0 && state.resting();

        spent += now.elapsed();
        frames += 1;
        if frames >= 250 {
            let per_frame = spent / frames;
            (spent, frames) = (Duration::ZERO, 0);
            // Меньше 20 кадров в секунду — кадр считается дольше 50 мс.
            if per_frame > Duration::from_millis(50) {
                slow += 1;
            } else {
                slow = 0;
            }
            if slow >= 3 {
                slow = 0;
                if circles == 3 {
                    circles = 2;
                } else {
                    fallback = true;
                    renderer = None;
                    emit(Event::Fallback);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_palette_matches_original() {
        // Жёлтый hsl(50, 100%, 50%) — #ffd500.
        let [r, g, b] = hsl_to_rgb(50.0, 1.0, 0.5);
        assert!((r - 1.0).abs() < 1e-9 && (g - 0.8333).abs() < 1e-3 && b.abs() < 1e-9);
        // #7C35C0 (plum) — hsl(271, 56.7%, 48%).
        assert_eq!(rgb_to_hsl([0x7C, 0x35, 0xC0]), (271.0, 56.7, 48.0));
    }

    #[test]
    fn noise_stays_in_range() {
        let mut max = 0f32;
        for i in 0..4000 {
            let f = i as f32 * 0.137;
            let v = snoise3([f.sin() * 3.0, f.cos() * 3.0, f * 0.31]);
            assert!(v.is_finite());
            max = max.max(v.abs());
        }
        assert!(max > 0.5 && max <= 1.01, "{max}");
    }

    #[test]
    fn silence_and_full_scale() {
        assert_eq!(byte_frequency_data(&[0.0; FFT]), [0; FFT / 2]);
        // Постоянный уровень 1: нулевая полоса на пределе шкалы.
        assert_eq!(byte_frequency_data(&[1.0; FFT])[0], 255);
        assert_eq!(bands(&[0.0; FFT], 48_000), [0.0; 3]);
    }

    #[test]
    fn bezier_ends() {
        assert!(cubic_bezier(0.42, 0.0, 0.58, 1.0, 0.0).abs() < 1e-4);
        assert!((cubic_bezier(0.42, 0.0, 0.58, 1.0, 1.0) - 1.0).abs() < 1e-4);
        assert!((cubic_bezier(0.42, 0.0, 0.58, 1.0, 0.5) - 0.5).abs() < 1e-3);
    }

    /// Тот же сценарий, что проигран настоящему воркеру оригинала (кадры монитора 60 Гц,
    /// тикер 25 кадров/с, `Math.random() = 0.5`): покой, игра со звуком, лайк, снова покой.
    /// Возвращает юниформы каждого кадра.
    fn replay() -> Vec<Uniforms> {
        const TRACK: [u8; 3] = [0x7C, 0x35, 0xC0];
        let mut rng = Rng::fixed(0.5);
        let mut state = State::new(&mut rng);
        let mut out = Vec::new();
        let (mut now, mut ticked) = (0f64, 0f64);
        let mut run = |state: &mut State, ms: f64, now: &mut f64, out: &mut Vec<Uniforms>| {
            let end = *now + ms;
            while *now + 1000.0 / 60.0 <= end + 1e-9 {
                *now += 1000.0 / 60.0;
                state.fire_timers(*now);
                let n = *now - ticked;
                if n >= 40.0 - 0.1 {
                    ticked = *now - n % 40.0;
                    state.update(n);
                    out.push(state.uniforms());
                }
            }
        };
        state.colors.update(Palette::DEFAULT);
        state.idle();
        run(&mut state, 2000.0, &mut now, &mut out);
        state.colors.update(Palette::from_rgb(TRACK));
        state.play();
        for i in 0..100 {
            let v = 0.5 + 0.5 * (i as f64 * 0.37).sin();
            state.freq = Some([v, v * 0.8, v * 0.5]);
            run(&mut state, 40.0, &mut now, &mut out);
            if i == 40 {
                state.like(now);
            }
        }
        state.idle();
        state.colors.update(Palette::DEFAULT);
        run(&mut state, 3000.0, &mut now, &mut out);
        out
    }

    /// Контрольные кадры из воркера оригинала 5.128.0: номер кадра, `vTime`, первый цвет
    /// `vColor`, `vAudio[0]` и `vReact[0]`.
    const ORIGINAL: &[(usize, f32, [f32; 3], f32, f32)] = &[
        (10, 1800.1046, [0.93474954, 0.0, 1.0], 0.0, 0.0),
        (30, 1800.2966, [0.99997926, 0.0, 1.0], 0.0, 0.0),
        (70, 1800.9153, [0.5379114, 0.18797266, 0.77585095], 0.8395811, 0.0),
        (90, 1801.5339, [0.48907268, 0.20783973, 0.7521603], 0.97871494, 0.0),
        (110, 1802.1462, [0.489072, 0.20784, 0.75216], 0.91876316, 0.7),
        (130, 1802.7712, [0.489072, 0.20784, 0.75216], 0.8589911, 3.0908723e-6),
        (150, 1803.3721, [0.49701977, 0.20460694, 0.7560153], 0.76190007, 0.0),
        (170, 1803.6339, [0.99111104, 0.0036159316, 0.99568814], 3.62753e-11, 0.0),
        (210, 1804.0072, [1.0, 3.1552876e-24, 1.0], 0.0, 0.0),
    ];

    #[test]
    fn state_matches_original_worker() {
        let frames = replay();
        for &(i, time, color, audio, react) in ORIGINAL {
            let u = frames[i];
            let close = |a: f32, b: f32| (a - b).abs() < 2e-5 * b.abs().max(1.0);
            assert!(close(u.time, time), "кадр {i}: время {} вместо {time}", u.time);
            for ch in 0..3 {
                assert!(close(u.colors[0][ch], color[ch]), "кадр {i}: цвет {:?} вместо {color:?}", u.colors[0]);
            }
            assert!(close(u.audio[0], audio), "кадр {i}: звук {} вместо {audio}", u.audio[0]);
            assert!(close(u.react[0], react), "кадр {i}: лайк {} вместо {react}", u.react[0]);
        }
    }

    /// После паузы пятно приходит к покою за время переходов воркера (цвета — 3 с) и
    /// дальше кадров не просит.
    #[test]
    fn pause_comes_to_rest() {
        let mut rng = Rng::fixed(0.5);
        let mut state = State::new(&mut rng);
        state.colors.update(Palette::from_rgb([0x7C, 0x35, 0xC0]));
        state.play();
        state.freq = Some([0.9, 0.7, 0.5]);
        for _ in 0..100 {
            state.update(40.0);
        }
        state.like(0.0);
        state.idle();
        state.colors.update(Palette::DEFAULT);
        let mut ms = 0.0;
        while !state.resting() {
            ms += 40.0;
            state.fire_timers(ms);
            state.update(40.0);
            assert!(ms < 5000.0, "пятно не успокоилось за 5 с");
        }
        assert!(ms >= 3000.0, "успокоилось раньше переходов: {ms} мс");
    }

    /// Юниформы кадра сверки: цвета трека #7C35C0, как в tools/ref/ref.py форка.
    fn dump_uniforms(time: f32, audio: f32, react: f32) -> Uniforms {
        let palette = Palette::from_rgb([0x7C, 0x35, 0xC0]);
        let tint = |c: Hsl| hsl_to_rgb(c.h, c.s, c.l).map(|v| v as f32);
        let (top, middle, bottom) = (tint(palette.top), tint(palette.middle), tint(palette.bottom));
        Uniforms { time, colors: [bottom, middle, top, bottom, middle, top], audio: [audio; 3], react: [react; 3] }
    }

    /// Холст поверх фона страницы #121212 — как в браузере.
    fn over_page(r: &Renderer) -> Vec<[f32; 3]> {
        r.canvas.iter().map(|p| [p.r, p.g, p.b].map(|c| c as f32 + 18.0 * (1.0 - p.a as f32 / 255.0))).collect()
    }

    /// Билинейно растянуть картинку `side`×`side` до `to`×`to`, как это делает окно.
    fn upscale(img: &[[f32; 3]], side: usize, to: usize) -> Vec<[f32; 3]> {
        let at = |x: usize, y: usize| img[y.min(side - 1) * side + x.min(side - 1)];
        let mut out = Vec::with_capacity(to * to);
        for y in 0..to {
            let fy = ((y as f32 + 0.5) * side as f32 / to as f32 - 0.5).max(0.0);
            for x in 0..to {
                let fx = ((x as f32 + 0.5) * side as f32 / to as f32 - 0.5).max(0.0);
                let (ix, iy, tx, ty) = (fx as usize, fy as usize, fract(fx), fract(fy));
                let mut px = [0f32; 3];
                for (c, p) in px.iter_mut().enumerate() {
                    let top = at(ix, iy)[c] * (1.0 - tx) + at(ix + 1, iy)[c] * tx;
                    let bottom = at(ix, iy + 1)[c] * (1.0 - tx) + at(ix + 1, iy + 1)[c] * tx;
                    *p = top * (1.0 - ty) + bottom * ty;
                }
                out.push(px);
            }
        }
        out
    }

    /// Холст в половину стороны с шумом на сетке 8 px, растянутый на экране, почти не
    /// отличается от полного холста с сеткой 4 px — того, что сверен с GLSL оригинала по
    /// tools/ref форка: окно и так увеличивает холст оригинала в 2,5–4 раза.
    #[test]
    fn half_canvas_looks_like_full() {
        for (time, audio, react) in [(1234.5, 0.0, 0.0), (3141.7, 0.4, 0.7), (60.2, 1.0, 0.0)] {
            let u = dump_uniforms(time, audio, react);
            let (mut full, mut half) = (Renderer::with_grid(SIZE, 4), Renderer::new(RES));
            full.render(&u, 3, OPACITY, [0, 0, SIZE, SIZE]);
            half.render(&u, 3, OPACITY, [0, 0, RES, RES]);
            // Холст на экране: 650 → 1950 (окно 1900 px в ширину при масштабе 100%).
            let to = SIZE * 3;
            let (a, b) = (upscale(&over_page(&full), SIZE, to), upscale(&over_page(&half), RES, to));
            let diffs: Vec<f32> = a.iter().zip(&b).flat_map(|(p, q)| (0..3).map(move |c| (p[c] - q[c]).abs())).collect();
            let mean = diffs.iter().sum::<f32>() / diffs.len() as f32;
            let mut sorted = diffs;
            sorted.sort_by(f32::total_cmp);
            let p999 = sorted[sorted.len() * 999 / 1000];
            println!("время {time}: средняя разница {mean:.2}, 99,9% пикселей не дальше {p999:.0} из 255");
            assert!(mean < 0.5 && p999 < 10.0, "время {time}: средняя {mean}, 99,9% — {p999}");
        }
    }

    /// Кадр для сверки с шейдером оригинала (tools/ref форка) и замер скорости:
    /// `VIBE_DUMP=out.png cargo test --release vibe::tests::dump -- --nocapture`.
    #[test]
    fn dump() {
        let Ok(path) = std::env::var("VIBE_DUMP") else { return };
        let get = |name: &str, default: f32| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
        let u = dump_uniforms(get("VIBE_TIME", 1234.5), get("VIBE_AUDIO", 0.0), get("VIBE_REACT", 0.0));
        let res = get("VIBE_RES", RES as f32) as usize;
        let mut r = Renderer::new(res);
        let started = Instant::now();
        let rounds = 20;
        let mut rect = [0; 4];
        for _ in 0..rounds {
            rect = r.frame(&u, 3, 1.0, [0, 0, res, res]).rect;
        }
        println!("кадр {res}×{res}: {:?}, пятно {rect:?}", started.elapsed() / rounds);
        let page = upscale(&over_page(&r), res, SIZE);
        let mut img = image::RgbImage::new(SIZE as u32, SIZE as u32);
        for (p, o) in page.iter().zip(img.pixels_mut()) {
            *o = image::Rgb(p.map(|c| (c + 0.5) as u8));
        }
        img.save(path).unwrap();
    }
}
