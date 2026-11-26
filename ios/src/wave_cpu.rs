//! Фон «Моей волны» без OpenGL: у Slint на iOS только Metal, поэтому тот же рисунок, что
//! в android/src/wave.frag, считается на процессоре в маленькую картинку, а Slint растягивает
//! её на экран со сглаживанием. Картинка размытая, так что на глаз разницы нет.
//! Тайминги, переходы цветов и палитры общие с wave_gl.rs.

use crate::api::Mood;
use crate::mood::{self, Palette, Rgb};
use crate::wave_gl::{random_start, Tween, MOTION_SPEED, SCENE_OFFSET_Y, TRANSITION_MS};
use slint::{Image, Rgb8Pixel, SharedPixelBuffer};
use std::time::Instant;

/// Во сколько раз картинка меньше окна (в логических пикселях) по каждой стороне.
const DOWNSCALE: f32 = 5.0;

pub struct Wave {
    /// Без движения время стоит на месте: картинка неподвижна и не тратит CPU.
    time: f32,
    last_tick: Option<Instant>,
    alpha: Tween,
    blobs: Tween,
    from: Palette,
    to: Palette,
    mix: Tween,
    mood: Option<Mood>,
}

impl Wave {
    pub fn new() -> Wave {
        let now = Instant::now();
        let palette = mood::palette(None);
        let mut alpha = Tween::new(0.01);
        alpha.go(1.0, 500.0, now);
        let mut blobs = Tween::new(0.01);
        blobs.go(0.8, 600.0, now);
        Wave {
            time: random_start(),
            last_tick: None,
            alpha,
            blobs,
            from: palette,
            to: palette,
            mix: Tween::new(1.0),
            mood: None,
        }
    }

    /// Сменился трек: цвета плавно переходят к новым. Вернёт true, если надо перерисовать.
    pub fn set_mood(&mut self, mood: Option<Mood>) -> bool {
        if mood == self.mood {
            return false;
        }
        let now = Instant::now();
        self.from = self.palette(now);
        self.to = mood::palette(mood);
        self.mix = Tween::new(0.0);
        self.mix.go(1.0, TRANSITION_MS, now);
        self.mood = mood;
        true
    }

    /// Идёт ли переход: только тогда нужны новые кадры.
    pub fn animating(&self) -> bool {
        let now = Instant::now();
        ![&self.mix, &self.alpha, &self.blobs].iter().all(|t| t.done(now))
    }

    fn palette(&self, now: Instant) -> Palette {
        let k = self.mix.get(now);
        let mut out = self.to;
        for (o, (a, b)) in out.iter_mut().zip(self.from.iter().zip(self.to.iter())) {
            for i in 0..3 {
                o[i] = a[i] + (b[i] - a[i]) * k;
            }
        }
        out
    }

    /// Кадр для окна `width`×`height` в логических пикселях.
    pub fn frame(&mut self, width: f32, height: f32, moving: bool) -> Image {
        let now = Instant::now();
        if moving {
            if let Some(last) = self.last_tick {
                // После паузы пятно не прыгает вперёд.
                self.time += now.duration_since(last).as_secs_f32().min(0.1) * MOTION_SPEED;
            }
            self.last_tick = Some(now);
        } else {
            self.last_tick = None;
        }
        let tw = ((width / DOWNSCALE) as u32).max(1);
        let th = ((height / DOWNSCALE) as u32).max(1);
        let gain = self.blobs.get(now) * self.alpha.get(now);
        Image::from_rgb8(render(tw, th, self.time, &self.palette(now), gain))
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: Rgb, b: Rgb, k: f32) -> Rgb {
    [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k]
}

fn blob(px: f32, py: f32, r: f32, t: f32, seed: f32) -> f32 {
    let a = py.atan2(px);
    let edge = 1.0 + 0.14 * (a * 3.0 + t * 0.8 + seed).sin() + 0.08 * (a * 5.0 - t * 1.1 + seed * 2.3).sin();
    let d = (px * px + py * py).sqrt() / (r * edge);
    (-d * d * 3.0).exp()
}

/// То же, что main() в wave.frag, для каждого пикселя. Строка 0 сверху, а у GL снизу.
fn render(tw: u32, th: u32, t: f32, colors: &Palette, gain: f32) -> SharedPixelBuffer<Rgb8Pixel> {
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(tw, th);
    let aspect = tw as f32 / th as f32;
    // Всё, что не зависит от пикселя: центры, повороты и размеры трёх пятен.
    let spots: [(f32, f32, f32, f32, f32, f32); 3] = std::array::from_fn(|i| {
        let fi = i as f32;
        let phase = fi * 2.094;
        let (cx, cy) = (0.05 * (t * 0.31 + phase).sin(), 0.05 * (t * 0.23 + phase * 1.3).cos());
        let ang = t * 0.12 + phase;
        (cx, cy, ang.cos(), ang.sin(), 0.27 - fi * 0.03, 0.75 - fi * 0.12)
    });
    let pixels = buf.make_mut_slice();
    for y in 0..th {
        let uv_y = (th - y) as f32 / th as f32 - 0.5 / th as f32;
        let py0 = uv_y - 0.5 + SCENE_OFFSET_Y;
        let glow = smoothstep(0.55, 0.0, uv_y) * 0.12;
        for x in 0..tw {
            let uv_x = (x as f32 + 0.5) / tw as f32;
            let px0 = (uv_x - 0.5) * aspect;
            let mut col = [0.0f32; 3];
            for (i, &(cx, cy, cos, sin, r, weight)) in spots.iter().enumerate() {
                let (dx, dy) = (px0 - cx, py0 - cy);
                // mat2(cos, -sin, sin, cos) в GLSL задан по столбцам.
                let qx = cos * dx + sin * dy;
                let qy = (-sin * dx + cos * dy) * 1.25;
                let b = blob(qx, qy, r, t, i as f32 * 1.7);
                let tint = mix(colors[i * 2 + 1], colors[i * 2], smoothstep(0.2, 0.9, b));
                for c in 0..3 {
                    col[c] += tint[c] * b * weight;
                }
            }
            let g = mix(colors[1], colors[3], uv_x);
            let out = &mut pixels[(y * tw + x) as usize];
            let ch = |c: usize| ((1.0 - (-(col[c] + g[c] * glow) * 1.6).exp()) * gain * 255.0).round().clamp(0.0, 255.0) as u8;
            *out = Rgb8Pixel { r: ch(0), g: ch(1), b: ch(2) };
        }
    }
    buf
}
