//! Плавная прокрутка колёсиком мыши, как у Chromium оригинала (Electron 38, Chromium 140).
//!
//! Щелчок колёсика не сдвигает страницу сразу, а запускает анимацию
//! `cc::ScrollOffsetAnimationCurve` для `ScrollType::kMouseWheel`: кривая ease-in-out
//! `cubic-bezier(0.42, 0, 0.58, 1)`, длительность `clamp(14 − |Δ|/60, 6, 12)` кадров по 1/60 с
//! (`kInverseDelta`: чем дальше, тем быстрее, 0,1–0,2 с). Щелчок во время анимации переносит
//! цель дальше и сохраняет скорость (`UpdateTarget`), поэтому быстрое вращение колёсика
//! даёт одно непрерывное движение.
//!
//! Страница едет за счёт мелких `PointerScrolled` в той точке, где начался жест: так
//! прокручивается тот же `Flickable`, что и без анимации, и знать о нём здесь ничего не нужно.
//! Шаг щелчка — по настройке Windows (`sys::wheel_px`), иначе как у Slint. Без анимации, как раньше, идут: «Лёгкий» режим,
//! тачпад на Linux (пиксели, `PixelDelta`) и дробные доли щелчка, которые на Windows присылают
//! тачпад и колёсики с плавным ходом, — Chromium их тоже не анимирует (crbug.com/545234).

use super::*;

/// `kEpsilon` кривой: и пиксели, и секунды.
const EPS: f64 = 0.01;

/// `gfx::CubicBezier`: кривая через (0,0), (x1,y1), (x2,y2), (1,1).
#[derive(Clone, Copy, Debug)]
struct Bezier {
    ax: f64,
    bx: f64,
    cx: f64,
    ay: f64,
    by: f64,
    cy: f64,
}

impl Bezier {
    fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Bezier {
        let (cx, cy) = (3.0 * x1, 3.0 * y1);
        let (bx, by) = (3.0 * (x2 - x1) - cx, 3.0 * (y2 - y1) - cy);
        Bezier { ax: 1.0 - cx - bx, bx, cx, ay: 1.0 - cy - by, by, cy }
    }

    /// `kEaseInOutControlPoints`, у продолжения — с начальным наклоном
    /// (`EaseInOutWithInitialSlope`): первая точка поднимается так, чтобы скорость не прыгнула.
    fn ease_in_out(slope: Option<f64>) -> Bezier {
        match slope {
            Some(s) => Bezier::new(0.42, 0.42 * s.clamp(-1000.0, 1000.0), 0.58, 1.0),
            None => Bezier::new(0.42, 0.0, 0.58, 1.0),
        }
    }

    fn x(&self, t: f64) -> f64 {
        ((self.ax * t + self.bx) * t + self.cx) * t
    }

    fn y(&self, t: f64) -> f64 {
        ((self.ay * t + self.by) * t + self.cy) * t
    }

    fn dx(&self, t: f64) -> f64 {
        (3.0 * self.ax * t + 2.0 * self.bx) * t + self.cx
    }

    fn dy(&self, t: f64) -> f64 {
        (3.0 * self.ay * t + 2.0 * self.by) * t + self.cy
    }

    /// Параметр кривой для `x`: сначала Ньютон, если не сошёлся — деление пополам.
    fn solve_x(&self, x: f64) -> f64 {
        let mut t = x;
        for _ in 0..8 {
            let err = self.x(t) - x;
            if err.abs() < 1e-7 {
                return t;
            }
            let d = self.dx(t);
            if d.abs() < 1e-6 {
                break;
            }
            t -= err / d;
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        t = x;
        while lo < hi {
            let v = self.x(t);
            if (v - x).abs() < 1e-7 {
                break;
            }
            if x > v {
                lo = t;
            } else {
                hi = t;
            }
            t = (hi - lo) / 2.0 + lo;
            if hi - lo < 1e-9 {
                break;
            }
        }
        t
    }

    fn value(&self, x: f64) -> f64 {
        self.y(self.solve_x(x.clamp(0.0, 1.0)))
    }

    /// Наклон dy/dx в точке `x` (`CubicBezier::Slope`).
    fn slope(&self, x: f64) -> f64 {
        let t = self.solve_x(x.clamp(0.0, 1.0));
        let (dx, dy) = (self.dx(t), self.dy(t));
        if dx == 0.0 && dy == 0.0 {
            0.0
        } else {
            dy / dx
        }
    }
}

/// Большее по модулю смещение из двух осей (`MaximumDimension`).
fn max_dim(d: [f64; 2]) -> f64 {
    if d[0].abs() > d[1].abs() {
        d[0]
    } else {
        d[1]
    }
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

/// `EaseInOutSegmentDuration` с `kInverseDelta`, секунды.
fn segment(delta: [f64; 2]) -> f64 {
    (14.0 - max_dim(delta).abs() / 60.0).clamp(6.0, 12.0) / 60.0
}

/// `VelocityBasedDurationBound`: за сколько дойдём до новой цели на нынешней скорости,
/// с запасом на замедление в конце.
fn velocity_bound(velocity: f64, new_delta: [f64; 2]) -> f64 {
    let d = max_dim(new_delta);
    if d.abs() < EPS {
        return 0.0;
    }
    if velocity.abs() < EPS {
        return f64::INFINITY;
    }
    let bound = d / velocity * 2.5;
    if bound < 0.0 {
        f64::INFINITY
    } else {
        bound
    }
}

/// `cc::ScrollOffsetAnimationCurve` для колёсика. Время — секунды от начала жеста, смещение —
/// пиксели от места, где жест начался.
#[derive(Clone, Debug)]
struct Curve {
    initial: [f64; 2],
    target: [f64; 2],
    timing: Bezier,
    total: f64,
    last_retarget: f64,
}

impl Curve {
    fn new(delta: [f64; 2]) -> Curve {
        Curve { initial: [0.0; 2], target: delta, timing: Bezier::ease_in_out(None), total: segment(delta), last_retarget: 0.0 }
    }

    fn duration(&self) -> f64 {
        self.total
    }

    fn target(&self) -> [f64; 2] {
        self.target
    }

    fn value(&self, t: f64) -> [f64; 2] {
        let duration = self.total - self.last_retarget;
        let t = t - self.last_retarget;
        if duration <= 0.0 || t >= duration {
            return self.target;
        }
        if t <= 0.0 {
            return self.initial;
        }
        let p = self.timing.value(t / duration);
        [self.initial[0] + (self.target[0] - self.initial[0]) * p, self.initial[1] + (self.target[1] - self.initial[1]) * p]
    }

    /// Пиксели в секунду по большей оси.
    fn velocity(&self, t: f64) -> f64 {
        let duration = self.total - self.last_retarget;
        let slope = self.timing.slope((t - self.last_retarget) / duration);
        slope * (max_dim(sub(self.target, self.initial)) / duration)
    }

    /// Новый щелчок: цель дальше, скорость та же (`UpdateTarget`).
    fn update_target(&mut self, t: f64, new_target: [f64; 2]) {
        let t = t.max(self.last_retarget);
        if max_dim(sub(self.target, new_target)).abs() < EPS {
            self.target = new_target;
            return;
        }
        let current = self.value(t);
        let new_delta = sub(new_target, current);
        if max_dim(new_delta).abs() < EPS {
            self.last_retarget = t;
            self.total = t;
            self.target = new_target;
            return;
        }
        if self.total - self.last_retarget <= 0.0 {
            self.total = t + segment(new_delta);
            self.target = new_target;
            return;
        }
        let velocity = self.velocity(t);
        let new_duration = segment(new_delta).min(velocity_bound(velocity, new_delta));
        if new_duration < EPS {
            self.target = new_target;
            self.total = t;
            return;
        }
        let slope = velocity * (new_duration / max_dim(new_delta));
        self.timing = Bezier::ease_in_out(Some(slope));
        self.initial = current;
        self.target = new_target;
        self.total = t + new_duration;
        self.last_retarget = t;
    }
}

struct Anim {
    curve: Curve,
    start: Instant,
    /// Где начался жест: туда же идут все сдвиги, чтобы ехал один и тот же список.
    at: slint::LogicalPosition,
    /// Сколько уже отдано странице.
    applied: [f64; 2],
}

#[derive(Default)]
struct Smooth {
    timer: Timer,
    anim: Option<Anim>,
}

thread_local! {
    static SMOOTH: RefCell<Smooth> = RefCell::default();
}

/// Шаг строки колёсика у Slint (`LineDelta` × 60): когда настройки системы нет.
const LINE_PX: f32 = 60.0;

/// Пикселей в щелчке: по настройке Windows, как у Chromium, иначе как у Slint.
fn line_px(horizontal: bool) -> f32 {
    crate::sys::wheel_px(horizontal).unwrap_or(LINE_PX)
}

/// Колёсико мыши (`LineDelta` от winit): `lx`, `ly` — щелчки, у обычного колёсика ±1.
/// `false` — анимировать не нужно, событие обработает Slint, как раньше.
pub(super) fn notch(lx: f32, ly: f32) -> bool {
    if lx.fract() != 0.0 || ly.fract() != 0.0 {
        return false;
    }
    let mut animate = false;
    with_ctx(|c| {
        animate = !c.lite.get();
        if animate {
            c.ui().global::<App>().set_wheel_notch(line_px(false));
        }
    });
    if animate {
        scroll(keys::cursor(), lx * line_px(true), ly * line_px(false));
    }
    animate
}

/// Шаг анимации уходит в окно. Пока он идёт, `App.wheel-smooth` поднят: обработчикам,
/// которым нужен целый щелчок (громкость), он говорит, что это доля щелчка.
fn dispatch(at: slint::LogicalPosition, dx: f32, dy: f32) {
    with_ctx(|c| {
        let ui = c.ui();
        ui.global::<App>().set_wheel_smooth(true);
        ui.window().dispatch_event(slint::platform::WindowEvent::PointerScrolled { position: at, delta_x: dx, delta_y: dy });
        ui.global::<App>().set_wheel_smooth(false);
    });
}

/// Колёсико сдвинуло на (`dx`, `dy`) логических пикселей в точке `at`.
fn scroll(at: slint::LogicalPosition, dx: f32, dy: f32) {
    let start_timer = SMOOTH.with(|s| {
        let mut s = s.borrow_mut();
        let now = Instant::now();
        match &mut s.anim {
            // Анимация ещё не закрыта шагом (даже если её время вышло, пока цикл событий
            // был занят): продолжаем её, иначе недоданный остаток пропал бы.
            Some(a) => {
                let t = now.duration_since(a.start).as_secs_f64();
                let target = a.curve.target();
                a.curve.update_target(t, [target[0] + dx as f64, target[1] + dy as f64]);
                false
            }
            None => {
                s.anim = Some(Anim { curve: Curve::new([dx as f64, dy as f64]), start: now, at, applied: [0.0; 2] });
                true
            }
        }
    });
    if start_timer {
        SMOOTH.with(|s| s.borrow().timer.start(TimerMode::Repeated, frame(), tick));
        tick();
    }
}

/// Как часто сдвигается страница: раз в кадр монитора, как Chromium сдвигает её на каждом
/// BeginFrame. Чаще не нужно: каждый шаг — новая отрисовка окна, а глаз её не увидит.
fn frame() -> Duration {
    let mut hz = 60.0;
    with_ctx(|c| {
        let rate = c.ui().window().with_winit_window(|w| w.current_monitor().and_then(|m| m.refresh_rate_millihertz()));
        if let Some(mhz) = rate.flatten() {
            hz = mhz as f64 / 1000.0;
        }
    });
    Duration::from_secs_f64(1.0 / hz.clamp(30.0, 240.0))
}

/// Шаг анимации: странице уходит разница между положением на кривой и уже отданным.
fn tick() {
    let step = SMOOTH.with(|s| {
        let mut s = s.borrow_mut();
        let a = s.anim.as_mut()?;
        let t = a.start.elapsed().as_secs_f64();
        let v = a.curve.value(t);
        let d = sub(v, a.applied);
        a.applied = v;
        let step = (a.at, d);
        if t >= a.curve.duration() {
            s.anim = None;
            s.timer.stop();
        }
        Some(step)
    });
    // Событие уходит уже без заёма: обработчики прокрутки могут прийти сюда же.
    if let Some((at, d)) = step {
        if d[0].abs() > 1e-3 || d[1].abs() > 1e-3 {
            dispatch(at, d[0] as f32, d[1] as f32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn duration_follows_chromium_inverse_delta() {
        // 100 px — 12 кадров, 266,7 px (8 строк по 100/3) — 9,56, от 480 px — 6.
        assert!(close(Curve::new([0.0, 100.0]).duration(), 12.0 / 60.0, 1e-12));
        assert!(close(Curve::new([0.0, -800.0 / 3.0]).duration(), (14.0 - 800.0 / 3.0 / 60.0) / 60.0, 1e-12));
        assert!(close(Curve::new([0.0, 600.0]).duration(), 6.0 / 60.0, 1e-12));
        assert!(close(Curve::new([0.0, 10.0]).duration(), 12.0 / 60.0, 1e-12));
    }

    #[test]
    fn ease_in_out_shape() {
        let e = Bezier::ease_in_out(None);
        assert!(close(e.value(0.5), 0.5, 1e-6));
        assert!(close(e.value(0.25) + e.value(0.75), 1.0, 1e-6));
        // cubic-bezier(0.42, 0, 0.58, 1) в 0,25 — медленный разгон.
        assert!(e.value(0.25) > 0.12 && e.value(0.25) < 0.14, "{}", e.value(0.25));
        let c = Curve::new([0.0, 100.0]);
        assert_eq!(c.value(0.0), [0.0, 0.0]);
        assert!(close(c.value(0.1)[1], 50.0, 1e-4));
        assert_eq!(c.value(0.2), [0.0, 100.0]);
        assert_eq!(c.value(1.0), [0.0, 100.0]);
    }

    #[test]
    fn retarget_keeps_position_and_velocity() {
        let mut c = Curve::new([0.0, 100.0]);
        let t = 0.08;
        let (before, v_before) = (c.value(t), c.velocity(t));
        c.update_target(t, [0.0, 200.0]);
        assert!(close(c.value(t)[1], before[1], 1e-9));
        assert!(close(c.velocity(t), v_before, v_before.abs() * 1e-3), "{} vs {v_before}", c.velocity(t));
        assert_eq!(c.target(), [0.0, 200.0]);
        assert_eq!(c.value(c.duration()), [0.0, 200.0]);
        // Движение не идёт назад и не перелетает цель.
        let mut prev = before[1];
        let mut x = t;
        while x <= c.duration() {
            let y = c.value(x)[1];
            assert!(y >= prev - 1e-9 && y <= 200.0 + 1e-6, "{x}: {y}");
            prev = y;
            x += 0.001;
        }
    }

    #[test]
    fn late_notch_continues_from_old_target() {
        // Цикл событий был занят: щелчок пришёл, когда время первой анимации уже вышло.
        let mut c = Curve::new([0.0, 100.0]);
        let t = c.duration() + 0.05;
        c.update_target(t, [0.0, 200.0]);
        assert_eq!(c.value(t), [0.0, 100.0]);
        assert_eq!(c.value(c.duration()), [0.0, 200.0]);
        assert!(close(c.duration() - t, segment([0.0, 100.0]), 1e-12));
    }

    #[test]
    fn same_target_does_not_restart() {
        let mut c = Curve::new([0.0, 100.0]);
        let total = c.duration();
        c.update_target(0.05, [0.0, 100.0]);
        assert_eq!(c.duration(), total);
    }
}
