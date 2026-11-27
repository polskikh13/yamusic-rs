//! Эквалайзер из «Настроек звука»: предусиление и 10 пиковых фильтров ±12 дБ.

use rodio::source::SeekError;
use rodio::Source;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const FREQS: [f32; 10] = [60.0, 170.0, 310.0, 600.0, 1000.0, 3000.0, 6000.0, 12000.0, 14000.0, 16000.0];

/// Пресеты в порядке выпадающего списка. Первый — «Своя настройка».
pub const PRESETS: [(&str, [f32; 10]); 19] = [
    ("Своя настройка", [0.0; 10]),
    ("По умолчанию", [0.0; 10]),
    ("Классическая музыка", [-0.5, -0.5, -0.5, -0.5, -0.5, -0.5, -3.5, -3.5, -3.5, -4.5]),
    ("Клубная музыка", [-0.5, -0.5, 4.0, 2.5, 2.5, 2.5, 1.5, -0.5, -0.5, -0.5]),
    ("Танцевальная музыка", [4.5, 3.5, 1.0, -0.5, -0.5, -2.5, -3.5, -3.5, -0.5, -0.5]),
    ("Усиление НЧ", [4.0, 4.5, 4.5, 2.5, 0.5, -2.0, -4.0, -5.0, -5.5, -5.5]),
    ("Усиление НЧ и ВЧ", [3.5, 2.5, -0.5, -3.5, -2.0, 0.5, 4.0, 5.5, 6.0, 6.0]),
    ("Усиление ВЧ", [-4.5, -4.5, -4.5, -2.0, 1.0, 5.5, 8.0, 8.0, 8.0, 8.0]),
    ("Колонки ноутбука", [2.0, 5.5, 2.5, -1.5, -1.0, 0.5, 2.0, 4.5, 6.0, 7.0]),
    ("Большой зал", [5.0, 5.0, 2.5, 2.5, -0.5, -2.0, -2.0, -2.0, -0.5, -0.5]),
    ("Концерт", [-2.0, -0.5, 2.0, 2.5, 2.5, 2.5, 2.0, 1.0, 1.0, 1.0]),
    ("Вечеринка", [3.5, 3.5, -0.5, -0.5, -0.5, -0.5, -0.5, -0.5, 3.5, 3.5]),
    ("Поп", [-0.5, 2.0, 3.5, 4.0, 2.5, -0.5, -1.0, -1.0, -0.5, -0.5]),
    ("Регги", [-0.5, -0.5, -0.5, -2.5, -0.5, 3.0, 3.0, -0.5, -0.5, -0.5]),
    ("Рок", [4.0, 2.0, -2.5, -4.0, -1.5, 2.0, 4.0, 5.5, 5.5, 5.5]),
    ("Ска", [-1.0, -2.0, -2.0, -0.5, 2.0, 2.5, 4.0, 4.5, 5.5, 4.5]),
    ("Мягкое звучание", [2.0, 0.5, -0.5, -1.0, -0.5, 2.0, 4.0, 4.5, 5.5, 6.0]),
    ("Софт-рок", [2.0, 2.0, 1.0, -0.5, -2.0, -2.5, -1.5, -0.5, 1.0, 4.0]),
    ("Техно", [4.0, 2.5, -0.5, -2.5, -2.0, -0.5, 4.0, 4.5, 4.5, 4.0]),
];

/// Настройки, общие для UI и аудиопотока.
pub struct EqParams {
    on: AtomicBool,
    preamp: AtomicU32,
    gains: [AtomicU32; 10],
    version: AtomicU64,
}

impl Default for EqParams {
    fn default() -> Self {
        EqParams {
            on: AtomicBool::new(false),
            preamp: AtomicU32::new(1.0f32.to_bits()),
            gains: Default::default(),
            version: AtomicU64::new(1),
        }
    }
}

impl EqParams {
    /// `preamp` — множитель громкости 0,5…1,5, `gains` — дБ по полосам.
    pub fn set(&self, on: bool, preamp: f32, gains: &[f32; 10]) {
        self.on.store(on, Ordering::Relaxed);
        self.preamp.store(preamp.to_bits(), Ordering::Relaxed);
        for (a, g) in self.gains.iter().zip(gains) {
            a.store(g.to_bits(), Ordering::Relaxed);
        }
        self.version.fetch_add(1, Ordering::Release);
    }
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Biquad {
    /// Пиковый фильтр по RBJ Audio EQ Cookbook, добротность 1,4 — примерно октава.
    fn peaking(freq: f32, gain_db: f32, rate: f32) -> Biquad {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * (freq.min(rate * 0.45)) / rate;
        let alpha = w0.sin() / (2.0 * 1.4);
        let cos = w0.cos();
        let a0 = 1.0 + alpha / a;
        Biquad {
            b0: (1.0 + alpha * a) / a0,
            b1: -2.0 * cos / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }
}

/// Предусиление меняется, как у оригинала: каждое применение настроек эквалайзера —
/// `preamp.gain.linearRampToValueAtTime(e, currentTime + .3)`. Такая рампа Web Audio идёт не от
/// текущего момента, а от предыдущего события: после паузы в правках громкость почти сразу
/// оказывается у нового значения, а частые правки (ползунок тянут) догоняет с отставанием 0,3 с.
struct Ramp {
    /// Предыдущее событие: кадр и значение. Без кадра — событие было до этого трека, давно.
    from: (Option<u64>, f32),
    /// Запланированные события: кадр, к которому значение должно дойти, и само значение.
    to: VecDeque<(u64, f32)>,
}

impl Ramp {
    fn new(value: f32) -> Ramp {
        Ramp { from: (None, value), to: VecDeque::new() }
    }

    fn at(&mut self, frame: u64) -> f32 {
        while let Some(&(t, v)) = self.to.front() {
            if t > frame {
                break;
            }
            self.from = (Some(t), v);
            self.to.pop_front();
        }
        match (self.from, self.to.front()) {
            (_, None) => self.from.1,
            // Предыдущее событие было давно: значение сразу новое.
            ((None, _), Some(&(_, v1))) => v1,
            ((Some(t0), v0), Some(&(t1, v1))) => v0 + (v1 - v0) * (frame - t0) as f32 / (t1 - t0) as f32,
        }
    }
}

pub struct Eq<S> {
    inner: S,
    params: Arc<EqParams>,
    version: u64,
    on: bool,
    preamp: Ramp,
    /// Кадров с начала трека: часы для рампы предусиления, перемотка их не двигает.
    frame: u64,
    bands: Vec<(usize, Biquad)>,
    /// Состояние фильтров: [канал][полоса] = (x1, x2, y1, y2).
    state: Vec<[[f32; 4]; 10]>,
    ch: usize,
    count: u32,
}

impl<S: Source<Item = f32>> Eq<S> {
    pub fn new(inner: S, params: Arc<EqParams>) -> Self {
        let mut eq = Eq { inner, params, version: 0, on: false, preamp: Ramp::new(1.0), frame: 0, bands: Vec::new(), state: Vec::new(), ch: 0, count: 0 };
        eq.refresh();
        // Новый трек начинает с уже установленного значения, без рампы.
        eq.preamp = Ramp::new(f32::from_bits(eq.params.preamp.load(Ordering::Relaxed)));
        eq
    }

    fn refresh(&mut self) {
        let v = self.params.version.load(Ordering::Acquire);
        if v == self.version {
            return;
        }
        self.version = v;
        self.on = self.params.on.load(Ordering::Relaxed);
        let rate = self.inner.sample_rate().max(8000) as f32;
        // Оригинал применяет настройки, только пока эквалайзер включён.
        if self.on {
            let preamp = f32::from_bits(self.params.preamp.load(Ordering::Relaxed));
            self.preamp.to.push_back((self.frame + (0.3 * rate) as u64, preamp));
        }
        self.bands = (0..10)
            .filter_map(|i| {
                let g = f32::from_bits(self.params.gains[i].load(Ordering::Relaxed));
                (g.abs() > 0.01).then(|| (i, Biquad::peaking(FREQS[i], g, rate)))
            })
            .collect();
        let channels = self.inner.channels().max(1) as usize;
        if self.state.len() != channels {
            self.state = vec![[[0.0; 4]; 10]; channels];
        }
    }
}

impl<S: Source<Item = f32>> Iterator for Eq<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let x = self.inner.next()?;
        self.count += 1;
        if self.count >= 2048 && self.ch == 0 {
            self.count = 0;
            self.refresh();
        }
        let channels = self.state.len().max(1);
        let ch = self.ch % channels;
        self.ch = (self.ch + 1) % channels;
        if ch == 0 {
            self.frame += 1;
        }
        if !self.on {
            return Some(x);
        }
        let mut y = x * self.preamp.at(self.frame);
        let st = &mut self.state[ch];
        for &(i, f) in &self.bands {
            let s = &mut st[i];
            let out = f.b0 * y + f.b1 * s[0] + f.b2 * s[1] - f.a1 * s[2] - f.a2 * s[3];
            s[1] = s[0];
            s[0] = y;
            s[3] = s[2];
            s[2] = out;
            y = out;
        }
        Some(y.clamp(-1.0, 1.0))
    }
}

impl<S: Source<Item = f32>> Source for Eq<S> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.state.fill([[0.0; 4]; 10]);
        self.ch = 0;
        self.inner.try_seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Те же правки, что в OfflineAudioContext Chromium с `linearRampToValueAtTime(v, currentTime + .3)`
    /// (кадр — миллисекунда); ожидаемые значения сняты с него.
    #[test]
    fn preamp_ramp_matches_web_audio() {
        let mut r = Ramp { from: (Some(300), 0.5), to: VecDeque::new() };
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        // Правка через 0,7 с после конца прошлой рампы: сразу 70% пути.
        r.to.push_back((1300, 1.5));
        for (t, v) in [(1000, 1.2), (1100, 1.3), (1300, 1.5), (1990, 1.5)] {
            assert!(close(r.at(t), v), "{t}");
        }
        r.to.push_back((2300, 1.0));
        for (t, v) in [(2000, 1.15), (2150, 1.075), (2300, 1.0)] {
            assert!(close(r.at(t), v), "{t}");
        }
        // Ползунок тянут: правка каждые 20 мс, значение идёт за ними с отставанием 0,3 с.
        for k in 0..10u64 {
            r.to.push_back((3300 + 20 * k, 0.5 + 0.05 * k as f32));
            if k == 0 {
                assert!(close(r.at(3000), 0.65));
            }
        }
        for (t, v) in [(3100, 0.6), (3200, 0.55), (3300, 0.5), (3400, 0.75), (3500, 0.95)] {
            assert!(close(r.at(t), v), "{t}");
        }
        assert!(r.to.is_empty());
    }

    #[test]
    fn new_track_starts_at_current_preamp() {
        let params = Arc::new(EqParams::default());
        params.set(true, 0.5, &[0.0; 10]);
        let src = rodio::source::SineWave::new(440.0).take_duration(Duration::from_millis(50));
        let mut eq = Eq::new(src, params);
        assert_eq!(eq.preamp.at(eq.frame), 0.5);
        assert!(eq.preamp.to.is_empty());
        let _ = eq.next();
    }
}
