use std::collections::HashMap;
use std::ffi::{c_void, CStr};

use weft_render::mesh::{self, globals, scene, world_bind, Cam, Geometry, Kind, Resolve};
use weft_render::{Atlas, Blend, BufferId, Cmd, CommandList, DepthState, Error, F32S_PER_VERTEX, PipelineDesc, PipelineId, Rect, Target};
use windows::Win32::Graphics::OpenGL::*;

use crate::gl::{self, Gl};

const VS: &str = r#"#version 120
attribute vec3 a_pos;
attribute vec2 a_uv;
attribute vec3 a_color;
attribute vec4 a_light;
uniform mat4 view_proj;
varying vec2 uv;
varying vec3 color;
varying vec4 light;
varying float dist;
void main() {
    gl_Position = view_proj * vec4(a_pos, 1.0);
    gl_Position.z = 2.0 * gl_Position.z - gl_Position.w;
    uv = a_uv;
    color = a_color;
    light = a_light;
    dist = length(a_pos);
}
"#;

const FS: &str = r#"#version 120
uniform sampler2D atlas;
uniform int alpha_mode;
uniform vec3 fog_color;
uniform vec2 fog_range;
uniform vec3 weather;
varying vec2 uv;
varying vec3 color;
varying vec4 light;
varying float dist;
float curve(float v) {
    return v / (4.0 - 3.0 * v);
}
vec3 lightmap(vec4 l) {
    float d = clamp((weather.x - 0.12) / 0.88, 0.0, 1.0);
    float sky = curve(l.x) * mix(0.24, 1.0, d);
    vec3 c = max(vec3(0.04), vec3(curve(weather.y))) + mix(vec3(0.478, 0.478, 1.0), vec3(1.0), d) * sky;
    vec3 b = l.yzw;
    if (max(max(b.x, b.y), b.z) > 0.0) {
        vec3 p = (2.0 * b - 1.0) * (2.0 * b - 1.0);
        vec3 warm = mix(vec3(1.0, 0.847, 0.549), vec3(1.0), 0.9 * p);
        c += min(warm * vec3(curve(b.x), curve(b.y), curve(b.z)) * 1.4, vec3(0.85));
        c /= max(1.0, max(max(c.x, c.y), c.z));
    }
    c = clamp(c, vec3(0.0), vec3(1.0));
    float m = max(max(c.x, c.y), c.z);
    if (m > 0.0001) {
        float o = 1.0 - m;
        float m2 = o * o;
        c = mix(c, c * (1.0 - m2 * m2) / m, weather.z);
    }
    return c;
}
void main() {
    vec4 a = texture2D(atlas, uv);
    if (alpha_mode == 1 && a.a <= 0.1) discard;
    vec3 lit = pow(a.rgb, vec3(2.2)) * pow(color * lightmap(light), vec3(2.2));
    float fog = clamp((dist - fog_range.x) / max(fog_range.y - fog_range.x, 1.0), 0.0, 1.0);
    gl_FragColor = vec4(pow(mix(lit, fog_color, fog), vec3(1.0 / 2.2)), alpha_mode == 2 ? a.a : 1.0);
}
"#;

struct Program {
    id: u32,
    view_proj: i32,
    atlas: i32,
    alpha_mode: i32,
    fog_color: i32,
    fog_range: i32,
    weather: i32,
}

#[derive(Clone, Copy)]
struct Pipe {
    kind: Kind,
    alpha_mode: i32,
    blend: Blend,
    depth: Option<DepthState>,
    cull_back: bool,
}

#[derive(Clone, Copy, Default)]
struct Binding {
    buffer: Option<BufferId>,
    offset: u64,
    size: u64,
}

impl Binding {
    fn bytes(self, buffers: &HashMap<BufferId, Vec<u8>>) -> &[u8] {
        let Some(bytes) = self.buffer.and_then(|id| buffers.get(&id)) else { return &[] };
        let Ok(start) = usize::try_from(self.offset) else { return &[] };
        let Some(bytes) = bytes.get(start..) else { return &[] };
        &bytes[..self.size.min(bytes.len() as u64) as usize]
    }
}

#[derive(Clone, Copy)]
struct Tables<'a> {
    g: &'a [u8],
    scene: &'a [u8],
    biomes: &'a [u8],
    tex_info: &'a [u8],
    frames: &'a [u8],
}

fn word(bytes: &[u8], at: usize) -> u32 {
    let Some(start) = at.checked_mul(4) else { return 0 };
    let Some(end) = start.checked_add(4) else { return 0 };
    bytes.get(start..end).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
}

fn float(bytes: &[u8], at: usize) -> f32 {
    f32::from_bits(word(bytes, at))
}

impl Tables<'_> {
    fn frame(&self, name: u32) -> u32 {
        let base = word(self.tex_info, name as usize * 2);
        let info = word(self.tex_info, name as usize * 2 + 1);
        if info == 0 {
            return base;
        }
        let (count, total) = (info & 0xFFFF, ((info >> 16) & 0x7FFF) as f32);
        let t = float(self.scene, scene::LIGHT / 4 + 2).rem_euclid(total);
        let mut acc = 0.0;
        for i in 0..count {
            let e = word(self.frames, (base + i) as usize);
            acc += (e >> 16) as f32;
            if t < acc || i + 1 == count {
                return e & 0xFFFF;
            }
        }
        base
    }
}

struct Sheet<'a> {
    atlas: &'a Atlas,
    tables: Tables<'a>,
}

impl Resolve for Sheet<'_> {
    fn rect(&mut self, texture: u32) -> Option<Rect> {
        self.atlas.rect(self.tables.frame(texture))
    }

    fn tint(&mut self, kind: u32, biome: u32, block: [i32; 3]) -> [f32; 3] {
        let kinds = word(self.tables.g, globals::EXTRA / 4 + 2) as usize;
        let tiles = word(self.tables.g, globals::IDS / 4 + 3) as i32;
        let mut sum = [0.0; 3];
        let mut n = 0.0;
        if tiles > 0 {
            for k in 0..4 {
                let (x, z) = (block[0] - 1 + (k & 1), block[2] - 1 + (k >> 1));
                let tile = (z >> 4).rem_euclid(tiles) * tiles + (x >> 4).rem_euclid(tiles);
                let at = word(self.tables.g, globals::IDS / 4 + 2) as usize
                    + tile as usize * 256 + ((z & 15) * 16 + (x & 15)) as usize;
                let c = word(self.tables.biomes, at * kinds + kind as usize - 1);
                if c != 0 {
                    let c = rgb(c);
                    for i in 0..3 { sum[i] += c[i]; }
                    n += 1.0;
                }
            }
        }
        if n > 0.0 {
            sum.map(|v| v / n)
        } else {
            rgb(word(self.tables.biomes, biome as usize * kinds + kind as usize - 1))
        }
    }
}

fn rgb(c: u32) -> [f32; 3] {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]
}

pub struct World {
    gl: Gl,
    program: Program,
    pipes: HashMap<PipelineId, Pipe>,
    next_pipeline: u32,
    atlas: Atlas,
    max_sheet: u32,
    sheet: u32,
    dirty: bool,
    vbo: u32,
    ibo: u32,
    geo: Geometry,
}

impl World {
    pub fn new(gl: Gl) -> Result<Self, String> {
        let vs = compile(&gl, gl::VERTEX_SHADER, VS)?;
        let fs = match compile(&gl, gl::FRAGMENT_SHADER, FS) {
            Ok(fs) => fs,
            Err(e) => {
                unsafe { (gl.glDeleteShader)(vs) };
                return Err(e);
            }
        };
        let id = link(&gl, vs, fs)?;
        let location = |name: &CStr| unsafe { (gl.glGetUniformLocation)(id, name.as_ptr()) };
        let program = Program {
            id,
            view_proj: location(c"view_proj"),
            atlas: location(c"atlas"),
            alpha_mode: location(c"alpha_mode"),
            fog_color: location(c"fog_color"),
            fog_range: location(c"fog_range"),
            weather: location(c"weather"),
        };
        let (mut sheet, mut buffers, mut max_sheet) = (0, [0; 2], 0);
        unsafe {
            glGetIntegerv(GL_MAX_TEXTURE_SIZE, &mut max_sheet);
            glGenTextures(1, &mut sheet);
            (gl.glGenBuffers)(2, buffers.as_mut_ptr());
            (gl.glActiveTexture)(gl::TEXTURE0);
            glBindTexture(GL_TEXTURE_2D, sheet);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST as i32);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST as i32);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
        }
        let max_sheet = (max_sheet as u32).min(8192);
        Ok(Self {
            gl, program, pipes: HashMap::new(), next_pipeline: 1,
            atlas: Atlas::new(1024.min(max_sheet)), max_sheet, sheet, dirty: true,
            vbo: buffers[0], ibo: buffers[1], geo: Geometry::default(),
        })
    }

    pub fn destroy(&mut self) {
        unsafe {
            (self.gl.glDeleteProgram)(self.program.id);
            glDeleteTextures(1, &self.sheet);
            (self.gl.glDeleteBuffers)(2, [self.vbo, self.ibo].as_ptr());
        }
    }

    pub fn pipeline(&mut self, desc: &PipelineDesc<'_>) -> PipelineId {
        let id = PipelineId(self.next_pipeline);
        self.next_pipeline += 1;
        let kind = match desc.vertex {
            "vs_face" => Kind::Faces,
            "vs_quad" => Kind::Quads,
            _ => {
                eprintln!("GL21: unsupported pipeline {} ({})", desc.label, desc.vertex);
                return id;
            }
        };
        let alpha_mode = match desc.fragment {
            Some("fs_cutout") => 1,
            Some("fs_translucent") => 2,
            _ => 0,
        };
        self.pipes.insert(id, Pipe { kind, alpha_mode, blend: desc.blend, depth: desc.depth, cull_back: desc.cull_back });
        id
    }

    pub fn destroy_pipeline(&mut self, id: PipelineId) {
        self.pipes.remove(&id);
    }

    pub fn texture(&mut self, id: u32, width: u32, height: u32, rgba: &[u8]) -> Result<(), Error> {
        if width == 0 || height == 0 || width.saturating_add(2) > self.max_sheet || height.saturating_add(2) > self.max_sheet {
            return Err(Error(format!("texture {id}: {width}x{height} exceeds atlas limit {}", self.max_sheet)));
        }
        loop {
            if self.atlas.insert(id, width, height, rgba).is_ok() {
                self.dirty = true;
                return Ok(());
            }
            if self.atlas.sheet() >= self.max_sheet || rgba.len() < width as usize * height as usize * 4 {
                return Err(Error(format!("texture {id}: atlas full or invalid pixel data")));
            }
            // grow consumes the old atlas, but the limit check guarantees a larger sheet exists.
            self.atlas = std::mem::replace(&mut self.atlas, Atlas::new(256)).grow().unwrap();
            self.dirty = true;
        }
    }

    pub fn remove_texture(&mut self, id: u32) {
        self.atlas.remove(id);
        self.dirty = true;
    }

    pub fn draw(&mut self, cmds: &CommandList, buffers: &HashMap<BufferId, Vec<u8>>, width: u32, height: u32) {
        if self.dirty { self.upload_sheet(); }
        let mut bound = [Binding::default(); 12];
        let mut pipe = None;
        let mut surface = false;
        unsafe { glViewport(0, 0, width as i32, height as i32); }
        for cmd in &cmds.cmds {
            match *cmd {
                Cmd::BeginPass { color, depth } => {
                    surface = matches!(color, Some((Target::Surface, _)));
                    pipe = None;
                    if surface {
                        unsafe {
                            glDisable(GL_SCISSOR_TEST);
                            if let Some((_, Some(c))) = color {
                                glClearColor(c[0], c[1], c[2], c[3]);
                                glClear(GL_COLOR_BUFFER_BIT);
                            }
                            if let Some((_, Some(d))) = depth {
                                glDepthMask(1);
                                glClearDepth(d as f64);
                                glClear(GL_DEPTH_BUFFER_BIT);
                            }
                        }
                    }
                }
                Cmd::EndPass => { surface = false; pipe = None; }
                Cmd::SetPipeline(id) => pipe = self.pipes.get(&id).copied(),
                Cmd::BindBuffer { binding, buffer, offset, size } => {
                    if let Some(slot) = bound.get_mut(binding as usize) {
                        *slot = Binding { buffer: Some(buffer), offset, size };
                    }
                }
                Cmd::Scissor { x, y, w, h } if surface => unsafe {
                    glEnable(GL_SCISSOR_TEST);
                    glScissor(x, height as i32 - y - h as i32, w as i32, h as i32);
                },
                Cmd::DrawIndexedIndirect { buffer, offset, count } if surface => {
                    let Some(pipe) = pipe else { continue };
                    let args = Binding { buffer: Some(buffer), offset, size: count as u64 * 20 }.bytes(buffers);
                    self.draw_indirect(pipe, &bound, args, buffers);
                }
                _ => {}
            }
        }
        unsafe {
            for i in 0..4 { (self.gl.glDisableVertexAttribArray)(i); }
            (self.gl.glUseProgram)(0);
            (self.gl.glBindBuffer)(gl::ARRAY_BUFFER, 0);
            (self.gl.glBindBuffer)(gl::ELEMENT_ARRAY_BUFFER, 0);
        }
    }

    fn draw_indirect(&mut self, pipe: Pipe, bound: &[Binding; 12], args: &[u8], buffers: &HashMap<BufferId, Vec<u8>>) {
        let bytes = |binding: u32| bound[binding as usize].bytes(buffers);
        let tables = Tables {
            g: bytes(world_bind::G), scene: bytes(world_bind::SCENE), biomes: bytes(world_bind::BIOMES),
            tex_info: bytes(world_bind::TEX_INFO), frames: bytes(world_bind::FRAMES),
        };
        if tables.g.len() < globals::SIZE || tables.scene.len() < scene::SIZE { return; }
        let cam = Cam {
            cube: std::array::from_fn(|i| word(tables.g, globals::CAM_CUBE / 4 + i) as i32),
            frac: std::array::from_fn(|i| float(tables.g, globals::CAM_FRAC / 4 + i)),
        };
        self.geo.clear();
        let mut sheet = Sheet { atlas: &self.atlas, tables };
        for i in 0..args.len() / 20 {
            let Some(draw) = mesh::draw_at(args, i) else { continue };
            for instance in 0..word(args, i * 5 + 1) {
                let mut draw = draw;
                draw.origin += instance as usize;
                self.geo.push_run(pipe.kind, draw, bytes(world_bind::RECORDS), bytes(world_bind::ORIGINS), bytes(world_bind::OFFSETS), cam, &mut sheet);
            }
        }
        if self.geo.indices.is_empty() { return; }
        let gl = &self.gl;
        let p = &self.program;
        let matrix: [f32; 16] = std::array::from_fn(|i| float(tables.scene, i));
        unsafe {
            (gl.glUseProgram)(p.id);
            (gl.glUniformMatrix4fv)(p.view_proj, 1, 0, matrix.as_ptr());
            (gl.glUniform1i)(p.atlas, 0);
            (gl.glUniform1i)(p.alpha_mode, pipe.alpha_mode);
            (gl.glUniform2f)(p.fog_range, float(tables.scene, scene::PARAMS / 4), float(tables.scene, scene::PARAMS / 4 + 1));
            (gl.glUniform3f)(p.fog_color, float(tables.scene, scene::FOG_COLOR / 4), float(tables.scene, scene::FOG_COLOR / 4 + 1), float(tables.scene, scene::FOG_COLOR / 4 + 2));
            (gl.glUniform3f)(p.weather, float(tables.scene, scene::PARAMS / 4 + 2), float(tables.scene, scene::LIGHT / 4), float(tables.scene, scene::LIGHT / 4 + 1));
            (gl.glActiveTexture)(gl::TEXTURE0);
            glBindTexture(GL_TEXTURE_2D, self.sheet);
            (gl.glBindBuffer)(gl::ARRAY_BUFFER, self.vbo);
            (gl.glBufferData)(gl::ARRAY_BUFFER, (self.geo.vertices.len() * 4) as isize, self.geo.vertices.as_ptr().cast(), gl::STREAM_DRAW);
            (gl.glBindBuffer)(gl::ELEMENT_ARRAY_BUFFER, self.ibo);
            (gl.glBufferData)(gl::ELEMENT_ARRAY_BUFFER, (self.geo.indices.len() * 4) as isize, self.geo.indices.as_ptr().cast(), gl::STREAM_DRAW);
            for (i, (size, offset)) in [(3, 0), (2, 12), (3, 20), (4, 32)].into_iter().enumerate() {
                (gl.glEnableVertexAttribArray)(i as u32);
                (gl.glVertexAttribPointer)(i as u32, size, GL_FLOAT, 0, (F32S_PER_VERTEX * 4) as i32, offset as *const c_void);
            }
            set_state(pipe);
            glDrawElements(GL_TRIANGLES, self.geo.indices.len() as i32, GL_UNSIGNED_INT, std::ptr::null());
        }
    }

    fn upload_sheet(&mut self) {
        unsafe {
            (self.gl.glActiveTexture)(gl::TEXTURE0);
            glBindTexture(GL_TEXTURE_2D, self.sheet);
            glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
            glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA as i32, self.atlas.sheet() as i32, self.atlas.sheet() as i32, 0,
                GL_RGBA, GL_UNSIGNED_BYTE, self.atlas.pixels().as_ptr().cast());
        }
        self.dirty = false;
    }
}

unsafe fn set_state(pipe: Pipe) {
    match pipe.blend {
        Blend::None => unsafe { glDisable(GL_BLEND) },
        blend => unsafe {
            glEnable(GL_BLEND);
            let (src, dst) = match blend {
                Blend::Premultiplied => (GL_ONE, GL_ONE_MINUS_SRC_ALPHA),
                Blend::Additive => (GL_SRC_ALPHA, GL_ONE),
                _ => (GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA),
            };
            glBlendFunc(src, dst);
        },
    }
    if let Some(depth) = pipe.depth {
        unsafe {
            glEnable(GL_DEPTH_TEST);
            glDepthMask(u8::from(depth.write));
            glDepthFunc(match depth.compare {
                weft_render::Compare::Always => GL_ALWAYS,
                weft_render::Compare::Greater => GL_GREATER,
                weft_render::Compare::GreaterEqual => GL_GEQUAL,
                weft_render::Compare::Less => GL_LESS,
                weft_render::Compare::LessEqual => GL_LEQUAL,
            });
        }
    } else {
        unsafe { glDisable(GL_DEPTH_TEST); glDepthMask(0); }
    }
    unsafe {
        if pipe.cull_back { glEnable(GL_CULL_FACE); } else { glDisable(GL_CULL_FACE); }
        glCullFace(GL_BACK);
        glFrontFace(GL_CCW);
    }
}

fn compile(gl: &Gl, kind: u32, source: &str) -> Result<u32, String> {
    let shader = unsafe { (gl.glCreateShader)(kind) };
    let text = source.as_ptr().cast();
    unsafe {
        (gl.glShaderSource)(shader, 1, &text, &(source.len() as i32));
        (gl.glCompileShader)(shader);
    }
    let mut ok = 0;
    unsafe { (gl.glGetShaderiv)(shader, gl::COMPILE_STATUS, &mut ok); }
    if ok != 0 { return Ok(shader); }
    let mut length = 0;
    unsafe { (gl.glGetShaderiv)(shader, gl::INFO_LOG_LENGTH, &mut length); }
    let mut log = vec![0u8; length.max(1) as usize];
    unsafe {
        (gl.glGetShaderInfoLog)(shader, log.len() as i32, std::ptr::null_mut(), log.as_mut_ptr().cast());
        (gl.glDeleteShader)(shader);
    }
    Err(String::from_utf8_lossy(&log).trim_end_matches('\0').to_string())
}

fn link(gl: &Gl, vs: u32, fs: u32) -> Result<u32, String> {
    let program = unsafe { (gl.glCreateProgram)() };
    unsafe {
        (gl.glAttachShader)(program, vs);
        (gl.glAttachShader)(program, fs);
        for (i, name) in [c"a_pos", c"a_uv", c"a_color", c"a_light"].into_iter().enumerate() {
            (gl.glBindAttribLocation)(program, i as u32, name.as_ptr());
        }
        (gl.glLinkProgram)(program);
        (gl.glDeleteShader)(vs);
        (gl.glDeleteShader)(fs);
    }
    let mut ok = 0;
    unsafe { (gl.glGetProgramiv)(program, gl::LINK_STATUS, &mut ok); }
    if ok != 0 { return Ok(program); }
    let mut length = 0;
    unsafe { (gl.glGetProgramiv)(program, gl::INFO_LOG_LENGTH, &mut length); }
    let mut log = vec![0u8; length.max(1) as usize];
    unsafe {
        (gl.glGetProgramInfoLog)(program, log.len() as i32, std::ptr::null_mut(), log.as_mut_ptr().cast());
        (gl.glDeleteProgram)(program);
    }
    Err(String::from_utf8_lossy(&log).trim_end_matches('\0').to_string())
}
