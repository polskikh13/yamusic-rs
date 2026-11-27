//! Фон «Моей волны»: шейдер рисуется под интерфейсом Slint неподвижной картинкой.
//! Картинка размытая, поэтому считается в текстуре втрое меньше экрана и растягивается:
//! пикселей в 9 раз меньше, а на глаз разницы нет. Оригинал считает каждый пиксель экрана.

use crate::api::Mood;
use crate::mood::{self, Palette};
use glow::HasContext;
use std::time::Instant;

/// Во сколько раз текстура волны меньше экрана по каждой стороне.
const DOWNSCALE: i32 = 3;
/// Сдвиг центра сцены: у оригинала пятно стоит над заголовком, а не посреди экрана.
pub(crate) const SCENE_OFFSET_Y: f32 = -0.225;
/// Плавность смены цветов и скорости.
pub(crate) const TRANSITION_MS: f32 = 1000.0;
/// Секунд времени шейдера за секунду, когда пятно движется.
pub(crate) const MOTION_SPEED: f32 = 0.5;

const VERTEX: &str = "attribute vec2 a_position;
varying vec2 v_uv;
void main() {
    v_uv = a_position * 0.5 + 0.5;
    gl_Position = vec4(a_position, 0.0, 1.0);
}
";

const BLIT: &str = "precision mediump float;
varying vec2 v_uv;
uniform sampler2D u_tex;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv);
}
";

/// Значение, плавно идущее к цели по AccelerateDecelerate, как анимации оригинала.
pub(crate) struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    ms: f32,
}

impl Tween {
    pub(crate) fn new(v: f32) -> Tween {
        Tween { from: v, to: v, start: Instant::now(), ms: 1.0 }
    }

    pub(crate) fn done(&self, now: Instant) -> bool {
        now.duration_since(self.start).as_secs_f32() * 1000.0 >= self.ms
    }

    pub(crate) fn go(&mut self, to: f32, ms: f32, now: Instant) {
        self.from = self.get(now);
        self.to = to;
        self.start = now;
        self.ms = ms.max(1.0);
    }

    pub(crate) fn get(&self, now: Instant) -> f32 {
        let t = (now.duration_since(self.start).as_secs_f32() * 1000.0 / self.ms).clamp(0.0, 1.0);
        let eased = ((t + 1.0) * std::f32::consts::PI).cos() / 2.0 + 0.5;
        self.from + (self.to - self.from) * eased
    }
}

struct Target {
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    width: i32,
    height: i32,
}

struct Uniforms {
    time: Option<glow::UniformLocation>,
    alpha: Option<glow::UniformLocation>,
    blobs_alpha: Option<glow::UniformLocation>,
    size: Option<glow::UniformLocation>,
    offset_x: Option<glow::UniformLocation>,
    offset_y: Option<glow::UniformLocation>,
    scale: Option<glow::UniformLocation>,
    audio: Option<glow::UniformLocation>,
    background: Option<glow::UniformLocation>,
    gradient: Option<glow::UniformLocation>,
    color: Option<glow::UniformLocation>,
    react: Option<glow::UniformLocation>,
    rotation: Option<glow::UniformLocation>,
    interaction: Option<glow::UniformLocation>,
    point: Option<glow::UniformLocation>,
    horizontal: Option<glow::UniformLocation>,
    volume: Option<glow::UniformLocation>,
}

pub struct Wave {
    gl: glow::Context,
    wave: glow::Program,
    blit: glow::Program,
    blit_tex: Option<glow::UniformLocation>,
    quad: glow::Buffer,
    vao: Option<glow::VertexArray>,
    target: Option<Target>,
    u: Uniforms,
    /// Без движения время шейдера стоит на месте: картинка неподвижна и не тратит CPU.
    /// Случайная точка даёт каждому запуску свой рисунок.
    time: f32,
    /// Прошлый кадр с движением: от него отсчитывается ход времени.
    last_tick: Option<Instant>,
    alpha: Tween,
    blobs: Tween,
    from: Palette,
    to: Palette,
    mix: Tween,
    mood: Option<Mood>,
}

/// Движется ли пятно волны.
#[derive(Clone, Copy, PartialEq)]
pub enum Motion {
    Still,
    Moving,
}

/// Состояние GL, которое Skia держит у себя в кэше: после волны его надо вернуть как было.
struct Saved {
    program: u32,
    array_buffer: u32,
    framebuffer: u32,
    texture: u32,
    active_texture: i32,
    vao: Option<u32>,
    sampler: Option<u32>,
    viewport: [i32; 4],
    blend: bool,
    scissor: bool,
    depth: bool,
    stencil: bool,
    cull: bool,
}

impl Wave {
    /// # Safety
    /// `gl` должен быть контекстом, который сейчас текущий в этом потоке.
    pub unsafe fn new(gl: glow::Context, fragment: &str) -> Result<Wave, String> {
        let wave = program(&gl, VERTEX, fragment)?;
        let blit = program(&gl, VERTEX, BLIT)?;
        let quad = gl.create_buffer()?;
        let saved_buffer = gl.get_parameter_i32(glow::ARRAY_BUFFER_BINDING);
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(quad));
        let verts: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
        let bytes: Vec<u8> = verts.iter().flat_map(|v| v.to_ne_bytes()).collect();
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &bytes, glow::STATIC_DRAW);
        gl.bind_buffer(glow::ARRAY_BUFFER, native(saved_buffer).map(glow::NativeBuffer));
        let vao = if gl.version().major >= 3 { Some(gl.create_vertex_array()?) } else { None };
        let loc = |name: &str| gl.get_uniform_location(wave, name);
        let u = Uniforms {
            time: loc("vTime"),
            alpha: loc("vAlpha"),
            blobs_alpha: loc("vBlobsAlpha"),
            size: loc("vScreenSize"),
            offset_x: loc("vSceneOffsetX"),
            offset_y: loc("vSceneOffsetY"),
            scale: loc("vSceneScale"),
            audio: loc("vAudio"),
            background: loc("vColorBackground"),
            gradient: loc("vColorBackgroundGradient"),
            color: loc("vColor"),
            react: loc("vReact"),
            rotation: loc("vRotation"),
            interaction: loc("vInteraction"),
            point: loc("vInteractionPoint"),
            horizontal: loc("vGradientHorizontal"),
            volume: loc("vVolumeBackgroundEnabled"),
        };
        let blit_tex = gl.get_uniform_location(blit, "u_tex");
        let now = Instant::now();
        let palette = mood::palette(None);
        let mut alpha = Tween::new(0.01);
        alpha.go(1.0, 500.0, now);
        let mut blobs = Tween::new(0.01);
        blobs.go(0.8, 600.0, now);
        Ok(Wave {
            gl,
            wave,
            blit,
            blit_tex,
            quad,
            vao,
            target: None,
            u,
            time: random_start(),
            last_tick: None,
            alpha,
            blobs,
            from: palette,
            to: palette,
            mix: Tween::new(1.0),
            mood: None,
        })
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

    /// Нарисовать кадр волны во весь экран `width`×`height` (в пикселях).
    ///
    /// # Safety
    /// Вызывать только из колбэка отрисовки Slint, где контекст GL текущий.
    /// `shift` сдвигает центр пятна по горизонтали, в пикселях экрана.
    pub unsafe fn draw(&mut self, width: u32, height: u32, shift: i32, motion: Motion) {
        if width == 0 || height == 0 {
            return;
        }
        let now = Instant::now();
        match motion {
            Motion::Moving => {
                if let Some(last) = self.last_tick {
                    // После паузы в отрисовке пятно не прыгает вперёд.
                    self.time += now.duration_since(last).as_secs_f32().min(0.1) * MOTION_SPEED;
                }
                self.last_tick = Some(now);
            }
            Motion::Still => self.last_tick = None,
        }

        let saved = self.save();
        let gl = &self.gl;
        // Со сдвигом кадр волны шире экрана, чтобы его центр встал куда надо, а края не обрезались.
        let (w, h) = (width as i32 + 2 * shift.abs(), height as i32);
        let left = shift.min(0) * 2;
        let (tw, th) = ((w / DOWNSCALE).max(1), (h / DOWNSCALE).max(1));
        if self.target.as_ref().is_none_or(|t| t.width != tw || t.height != th) {
            if let Some(old) = self.target.take() {
                gl.delete_framebuffer(old.fbo);
                gl.delete_texture(old.tex);
            }
            self.target = make_target(gl, tw, th).ok();
        }
        let Some(target) = &self.target else {
            self.restore(&saved);
            return;
        };

        gl.disable(glow::BLEND);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::STENCIL_TEST);
        gl.disable(glow::CULL_FACE);
        if let Some(vao) = self.vao {
            gl.bind_vertex_array(Some(vao));
        }
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.quad));
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 0, 0);

        // Волна в маленькую текстуру.
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.fbo));
        gl.viewport(0, 0, tw, th);
        gl.use_program(Some(self.wave));
        let u = &self.u;
        gl.uniform_1_f32(u.time.as_ref(), self.time);
        gl.uniform_1_f32(u.alpha.as_ref(), self.alpha.get(now));
        gl.uniform_1_f32(u.blobs_alpha.as_ref(), self.blobs.get(now));
        gl.uniform_2_f32(u.size.as_ref(), tw as f32, th as f32);
        gl.uniform_1_f32(u.offset_x.as_ref(), 0.0);
        gl.uniform_1_f32(u.offset_y.as_ref(), SCENE_OFFSET_Y);
        gl.uniform_1_f32(u.scale.as_ref(), 1.0);
        gl.uniform_1_f32_slice(u.audio.as_ref(), &[0.0; 3]);
        gl.uniform_3_f32(u.background.as_ref(), 0.0, 0.0, 0.0);
        gl.uniform_3_f32(u.gradient.as_ref(), 0.0, 0.0, 0.0);
        let colors: Vec<f32> = self.palette(now).iter().flatten().copied().collect();
        gl.uniform_3_f32_slice(u.color.as_ref(), &colors);
        gl.uniform_1_f32_slice(u.react.as_ref(), &[0.0; 3]);
        gl.uniform_3_f32_slice(u.rotation.as_ref(), &[-0.3, 0.3, 0.2, -0.3, -0.3, -0.2, -0.3, -0.3, 0.2]);
        gl.uniform_1_f32(u.interaction.as_ref(), 0.0);
        gl.uniform_2_f32(u.point.as_ref(), 0.0, 0.0);
        gl.uniform_1_f32(u.horizontal.as_ref(), 0.0);
        gl.uniform_1_i32(u.volume.as_ref(), 0);
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

        // Растянуть на экран с билинейной фильтрацией.
        gl.bind_framebuffer(glow::FRAMEBUFFER, native(saved.framebuffer as i32).map(glow::NativeFramebuffer));
        gl.viewport(left, 0, w, h);
        gl.use_program(Some(self.blit));
        gl.active_texture(glow::TEXTURE0);
        if saved.sampler.is_some() {
            gl.bind_sampler(0, None);
        }
        gl.bind_texture(glow::TEXTURE_2D, Some(target.tex));
        gl.uniform_1_i32(self.blit_tex.as_ref(), 0);
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

        self.restore(&saved);
    }

    unsafe fn save(&self) -> Saved {
        let gl = &self.gl;
        let int = |p| gl.get_parameter_i32(p);
        let mut viewport = [0; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
        let es3 = self.vao.is_some();
        let active_texture = int(glow::ACTIVE_TEXTURE);
        gl.active_texture(glow::TEXTURE0);
        let saved = Saved {
            program: int(glow::CURRENT_PROGRAM) as u32,
            array_buffer: int(glow::ARRAY_BUFFER_BINDING) as u32,
            framebuffer: int(glow::FRAMEBUFFER_BINDING) as u32,
            texture: int(glow::TEXTURE_BINDING_2D) as u32,
            active_texture,
            vao: es3.then(|| int(glow::VERTEX_ARRAY_BINDING) as u32),
            sampler: es3.then(|| int(glow::SAMPLER_BINDING) as u32),
            viewport,
            blend: gl.is_enabled(glow::BLEND),
            scissor: gl.is_enabled(glow::SCISSOR_TEST),
            depth: gl.is_enabled(glow::DEPTH_TEST),
            stencil: gl.is_enabled(glow::STENCIL_TEST),
            cull: gl.is_enabled(glow::CULL_FACE),
        };
        gl.active_texture(active_texture as u32);
        saved
    }

    unsafe fn restore(&self, s: &Saved) {
        let gl = &self.gl;
        let flag = |cap, on: bool| if on { gl.enable(cap) } else { gl.disable(cap) };
        flag(glow::BLEND, s.blend);
        flag(glow::SCISSOR_TEST, s.scissor);
        flag(glow::DEPTH_TEST, s.depth);
        flag(glow::STENCIL_TEST, s.stencil);
        flag(glow::CULL_FACE, s.cull);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, native(s.texture as i32).map(glow::NativeTexture));
        if let Some(sampler) = s.sampler {
            gl.bind_sampler(0, native(sampler as i32).map(glow::NativeSampler));
        }
        gl.active_texture(s.active_texture as u32);
        if let Some(vao) = s.vao {
            gl.bind_vertex_array(native(vao as i32).map(glow::NativeVertexArray));
        }
        gl.bind_buffer(glow::ARRAY_BUFFER, native(s.array_buffer as i32).map(glow::NativeBuffer));
        gl.use_program(native(s.program as i32).map(glow::NativeProgram));
        gl.bind_framebuffer(glow::FRAMEBUFFER, native(s.framebuffer as i32).map(glow::NativeFramebuffer));
        let [x, y, w, h] = s.viewport;
        gl.viewport(x, y, w, h);
    }
}

impl Drop for Wave {
    fn drop(&mut self) {
        // SAFETY: Slint зовёт RenderingTeardown, пока контекст ещё текущий.
        unsafe {
            if let Some(t) = self.target.take() {
                self.gl.delete_framebuffer(t.fbo);
                self.gl.delete_texture(t.tex);
            }
            if let Some(vao) = self.vao {
                self.gl.delete_vertex_array(vao);
            }
            self.gl.delete_buffer(self.quad);
            self.gl.delete_program(self.wave);
            self.gl.delete_program(self.blit);
        }
    }
}

fn native(id: i32) -> Option<std::num::NonZeroU32> {
    std::num::NonZeroU32::new(id as u32)
}

/// Оригинал берёт случайное начало от 0 до 600, иначе после каждого запуска волна одна и та же.
pub(crate) fn random_start() -> f32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    (nanos % 600) as f32
}

unsafe fn program(gl: &glow::Context, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
    let p = gl.create_program()?;
    let mut shaders = Vec::new();
    for (kind, src) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
        let s = gl.create_shader(kind)?;
        gl.shader_source(s, src);
        gl.compile_shader(s);
        if !gl.get_shader_compile_status(s) {
            let log = gl.get_shader_info_log(s);
            gl.delete_shader(s);
            gl.delete_program(p);
            return Err(format!("шейдер не собрался: {log}"));
        }
        gl.attach_shader(p, s);
        shaders.push(s);
    }
    gl.bind_attrib_location(p, 0, "a_position");
    gl.link_program(p);
    for s in shaders {
        gl.detach_shader(p, s);
        gl.delete_shader(s);
    }
    if !gl.get_program_link_status(p) {
        let log = gl.get_program_info_log(p);
        gl.delete_program(p);
        return Err(format!("шейдер не слинковался: {log}"));
    }
    Ok(p)
}

unsafe fn make_target(gl: &glow::Context, width: i32, height: i32) -> Result<Target, String> {
    let tex = gl.create_texture()?;
    gl.active_texture(glow::TEXTURE0);
    let prev_tex = gl.get_parameter_i32(glow::TEXTURE_BINDING_2D);
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA as i32,
        width,
        height,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(None),
    );
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
    gl.bind_texture(glow::TEXTURE_2D, native(prev_tex).map(glow::NativeTexture));
    let prev_fbo = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
    let fbo = gl.create_framebuffer()?;
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
    gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
    let ok = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
    gl.bind_framebuffer(glow::FRAMEBUFFER, native(prev_fbo).map(glow::NativeFramebuffer));
    if !ok {
        gl.delete_framebuffer(fbo);
        gl.delete_texture(tex);
        return Err("framebuffer волны не собрался".into());
    }
    Ok(Target { fbo, tex, width, height })
}
