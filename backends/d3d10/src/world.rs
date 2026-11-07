//! The same world path as the OpenGL 2.1 backend, for feature level 10_0: CPU-decoded records,
//! one atlas sheet, HLSL 4.0 shaders compiled by the system's d3dcompiler. The engine's
//! projection is already in DirectX convention (y-up clip, z in 0..1, reverse-Z), so the vertex
//! shader only multiplies - no remap, no flip.

use std::collections::HashMap;
use std::ffi::c_void;

use weft_render::mesh::{self, globals, scene, world_bind, Cam, Geometry, Kind, Resolve};
use weft_render::{
    Atlas, Blend, BufferId, Cmd, CommandList, Error, F32S_PER_VERTEX, PipelineDesc, PipelineId, Rect, Target,
};
use windows::core::{w, HRESULT, PCSTR};
use windows::Win32::Foundation::{HMODULE, BOOL, RECT};
use windows::Win32::Graphics::Direct3D::{ID3DBlob, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST};
use windows::Win32::Graphics::Direct3D10::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_FORMAT_R32G32B32_FLOAT, DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R32_UINT,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const VS: &str = r#"
cbuffer Frame : register(b0) {
    // The matrix bytes are glam's column-major layout: each 16-byte register holds a column of
    // the mathematical matrix. Reading them as rows makes the HLSL matrix the transpose, which
    // is exactly what the row-vector multiply below needs.
    row_major float4x4 view_proj;
    float4 fog_color;
    float4 params;
    float4 light;
    int alpha_mode;
    int3 pad;
};
struct In {
    float3 pos : POSITION;
    float2 uv : TEXCOORD0;
    float3 color : COLOR0;
    float4 light : TEXCOORD1;
};
struct Out {
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
    float3 color : COLOR0;
    float4 light : TEXCOORD1;
    float dist : TEXCOORD2;
};
Out vs_main(In i) {
    Out o;
    // The matrix arrives column-major (glam to_cols_array_2d), so the vector goes on the left.
    o.pos = mul(float4(i.pos, 1.0), view_proj);
    o.uv = i.uv;
    o.color = i.color;
    o.light = i.light;
    o.dist = length(i.pos);
    return o;
}
"#;

const FS: &str = r#"
cbuffer Frame : register(b0) {
    row_major float4x4 view_proj;
    float4 fog_color;
    float4 params;
    float4 light;
    int alpha_mode;
    int3 pad;
};
Texture2D atlas : register(t0);
SamplerState point_clamp : register(s0);
struct Out {
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
    float3 color : COLOR0;
    float4 light : TEXCOORD1;
    float dist : TEXCOORD2;
};
float curve(float v) {
    return v / (4.0 - 3.0 * v);
}
float3 lightmap(float4 l) {
    float d = clamp((params.z - 0.12) / 0.88, 0.0, 1.0);
    float sky = curve(l.x) * lerp(0.24, 1.0, d);
    float3 c = max(float3(0.04, 0.04, 0.04), curve(light.x).xxx) + lerp(float3(0.478, 0.478, 1.0), float3(1.0, 1.0, 1.0), d) * sky;
    float3 b = l.yzw;
    if (max(max(b.x, b.y), b.z) > 0.0) {
        float3 p = (2.0 * b - 1.0) * (2.0 * b - 1.0);
        float3 warm = lerp(float3(1.0, 0.847, 0.549), float3(1.0, 1.0, 1.0), 0.9 * p);
        c += min(warm * float3(curve(b.x), curve(b.y), curve(b.z)) * 1.4, float3(0.85, 0.85, 0.85));
        c /= max(1.0, max(max(c.x, c.y), c.z));
    }
    c = clamp(c, 0.0, 1.0);
    float m = max(max(c.x, c.y), c.z);
    if (m > 0.0001) {
        float o = 1.0 - m;
        float m2 = o * o;
        c = lerp(c, c * (1.0 - m2 * m2) / m, light.y);
    }
    return c;
}
float4 ps_main(Out i) : SV_Target {
    float4 a = atlas.Sample(point_clamp, i.uv);
    if (alpha_mode == 1 && a.a <= 0.1) {
        discard;
    }
    float3 lit = pow(a.rgb, 2.2) * pow(i.color * lightmap(i.light), 2.2);
    float fog = clamp((i.dist - params.x) / max(params.y - params.x, 1.0), 0.0, 1.0);
    return float4(pow(lerp(lit, fog_color.rgb, fog), 1.0 / 2.2), alpha_mode == 2 ? a.a : 1.0);
}
"#;

type CompileFn = unsafe extern "system" fn(
    *const c_void,
    usize,
    PCSTR,
    *const c_void,
    *const c_void,
    PCSTR,
    PCSTR,
    u32,
    u32,
    *mut Option<ID3DBlob>,
    *mut Option<ID3DBlob>,
) -> HRESULT;

/// Windows 10 ships d3dcompiler_47; older DirectX runtimes leave _43. Either compiles SM 4.0.
fn compiler() -> Result<(HMODULE, CompileFn), String> {
    for name in [w!("d3dcompiler_47.dll"), w!("d3dcompiler_43.dll")] {
        let Ok(lib) = (unsafe { LoadLibraryW(name) }) else { continue };
        let Some(proc) = (unsafe { GetProcAddress(lib, windows::core::s!("D3DCompile")) }) else { continue };
        let f: CompileFn = unsafe { std::mem::transmute(proc) };
        return Ok((lib, f));
    }
    Err("no d3dcompiler DLL with D3DCompile on this system".into())
}

fn compile(f: CompileFn, source: &str, entry: PCSTR, target: PCSTR) -> Result<Vec<u8>, String> {
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    let hr = unsafe {
        f(source.as_ptr().cast(), source.len(), PCSTR::null(), std::ptr::null(), std::ptr::null(), entry, target, 0, 0, &mut code, &mut errors)
    };
    let blob_text = |blob: Option<ID3DBlob>| -> String {
        blob.map(|b| unsafe {
            let bytes = std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize());
            String::from_utf8_lossy(bytes).into_owned()
        })
        .unwrap_or_default()
    };
    if hr.is_err() {
        return Err(format!("{hr:?}: {}", blob_text(errors)));
    }
    let code = code.ok_or_else(|| "D3DCompile returned no bytecode".to_string())?;
    Ok(unsafe { std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, code.GetBufferSize()) }.to_vec())
}

struct Pipe {
    kind: Kind,
    alpha_mode: i32,
    blend: ID3D10BlendState,
    depth: ID3D10DepthStencilState,
    raster: ID3D10RasterizerState,
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
    device: ID3D10Device,
    _compiler: HMODULE,
    vs: ID3D10VertexShader,
    ps: ID3D10PixelShader,
    layout: ID3D10InputLayout,
    sampler: ID3D10SamplerState,
    no_depth: ID3D10DepthStencilState,
    cb: ID3D10Buffer,
    pipes: HashMap<PipelineId, Pipe>,
    next_pipeline: u32,
    atlas: Atlas,
    max_sheet: u32,
    sheet: Option<ID3D10Texture2D>,
    sheet_view: Option<ID3D10ShaderResourceView>,
    sheet_size: u32,
    dirty: bool,
    vbo: Option<ID3D10Buffer>,
    vbo_cap: usize,
    ibo: Option<ID3D10Buffer>,
    ibo_cap: usize,
    geo: Geometry,
}

/// D3D10 create calls answer through an out-param; a success HRESULT always fills it.
fn created<T>(value: Option<T>) -> windows::core::Result<T> {
    value.ok_or_else(|| windows::core::Error::from_hresult(HRESULT(0x80004005_u32 as i32)))
}

impl World {
    pub fn new(device: ID3D10Device) -> Result<Self, String> {
        let (lib, f) = compiler()?;
        let vs_bytes = compile(f, VS, windows::core::s!("vs_main"), windows::core::s!("vs_4_0"))?;
        let ps_bytes = compile(f, FS, windows::core::s!("ps_main"), windows::core::s!("ps_4_0"))?;
        let map = |e: windows::core::Error| format!("{e}");
        let mut vs = None;
        unsafe { device.CreateVertexShader(&vs_bytes, Some(&mut vs)) }.map_err(map)?;
        let vs = created(vs).map_err(map)?;
        let mut ps = None;
        unsafe { device.CreatePixelShader(&ps_bytes, Some(&mut ps)) }.map_err(map)?;
        let ps = created(ps).map_err(map)?;
        let element = |name: PCSTR, format, offset| D3D10_INPUT_ELEMENT_DESC {
            SemanticName: name,
            SemanticIndex: 0,
            Format: format,
            InputSlot: 0,
            AlignedByteOffset: offset,
            InputSlotClass: D3D10_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        };
        let elements = [
            element(windows::core::s!("POSITION"), DXGI_FORMAT_R32G32B32_FLOAT, 0),
            element(windows::core::s!("TEXCOORD"), DXGI_FORMAT_R32G32_FLOAT, 12),
            element(windows::core::s!("COLOR"), DXGI_FORMAT_R32G32B32_FLOAT, 20),
            element(windows::core::s!("TEXCOORD"), DXGI_FORMAT_R32G32B32A32_FLOAT, 32),
        ];
        // The two TEXCOORD entries need distinct indices: uv is 0, light is 1.
        let mut elements = elements;
        elements[3].SemanticIndex = 1;
        let mut layout = None;
        unsafe { device.CreateInputLayout(&elements, &vs_bytes, Some(&mut layout)) }.map_err(map)?;
        let layout = created(layout).map_err(map)?;
        let mut sampler = None;
        unsafe {
            device.CreateSamplerState(
                &D3D10_SAMPLER_DESC {
                    Filter: D3D10_FILTER_MIN_MAG_MIP_POINT,
                    AddressU: D3D10_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D10_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D10_TEXTURE_ADDRESS_CLAMP,
                    MipLODBias: 0.0,
                    MaxAnisotropy: 1,
                    ComparisonFunc: D3D10_COMPARISON_NEVER,
                    BorderColor: [0.0; 4],
                    MinLOD: 0.0,
                    MaxLOD: f32::MAX,
                },
                Some(&mut sampler),
            )
        }
        .map_err(map)?;
        let sampler = created(sampler).map_err(map)?;
        let mut no_depth = None;
        unsafe { device.CreateDepthStencilState(&depth_desc(false, false, weft_render::Compare::Always), Some(&mut no_depth)) }.map_err(map)?;
        let no_depth = created(no_depth).map_err(map)?;
        let mut cb = None;
        unsafe {
            device.CreateBuffer(
                &D3D10_BUFFER_DESC {
                    ByteWidth: 128,
                    Usage: D3D10_USAGE_DEFAULT,
                    BindFlags: D3D10_BIND_CONSTANT_BUFFER.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                },
                None,
                Some(&mut cb),
            )
        }
        .map_err(map)?;
        let cb = created(cb).map_err(map)?;
        Ok(Self {
            device,
            _compiler: lib,
            vs,
            ps,
            layout,
            sampler,
            no_depth,
            cb,
            pipes: HashMap::new(),
            next_pipeline: 1,
            // Feature level 10_0 requires 8192-wide textures; start small and grow on demand.
            atlas: Atlas::new(1024),
            max_sheet: 8192,
            sheet: None,
            sheet_view: None,
            sheet_size: 0,
            dirty: true,
            vbo: None,
            vbo_cap: 0,
            ibo: None,
            ibo_cap: 0,
            geo: Geometry::default(),
        })
    }

    pub fn pipeline(&mut self, desc: &PipelineDesc<'_>) -> PipelineId {
        let id = PipelineId(self.next_pipeline);
        self.next_pipeline += 1;
        let kind = match desc.vertex {
            "vs_face" => Kind::Faces,
            "vs_quad" => Kind::Quads,
            _ => {
                eprintln!("D3D10: unsupported pipeline {} ({})", desc.label, desc.vertex);
                return id;
            }
        };
        let alpha_mode = match desc.fragment {
            Some("fs_cutout") => 1,
            Some("fs_translucent") => 2,
            _ => 0,
        };
        let made = (|| -> windows::core::Result<Pipe> {
            let (src, dst) = match desc.blend {
                Blend::None => (D3D10_BLEND_ONE, D3D10_BLEND_ZERO),
                Blend::Premultiplied => (D3D10_BLEND_ONE, D3D10_BLEND_INV_SRC_ALPHA),
                Blend::Additive => (D3D10_BLEND_SRC_ALPHA, D3D10_BLEND_ONE),
                Blend::Alpha => (D3D10_BLEND_SRC_ALPHA, D3D10_BLEND_INV_SRC_ALPHA),
            };
            let mut blend = None;
            unsafe {
                self.device.CreateBlendState(
                    &D3D10_BLEND_DESC {
                        AlphaToCoverageEnable: BOOL(0),
                        BlendEnable: [BOOL(i32::from(desc.blend != Blend::None)); 8],
                        SrcBlend: src,
                        DestBlend: dst,
                        BlendOp: D3D10_BLEND_OP_ADD,
                        SrcBlendAlpha: D3D10_BLEND_ONE,
                        DestBlendAlpha: D3D10_BLEND_INV_SRC_ALPHA,
                        BlendOpAlpha: D3D10_BLEND_OP_ADD,
                        RenderTargetWriteMask: [0xF; 8],
                    },
                    Some(&mut blend),
                )
            }?;
            let blend = created(blend)?;
            let depth = match desc.depth {
                Some(d) => {
                    let mut depth = None;
                    unsafe { self.device.CreateDepthStencilState(&depth_desc(true, d.write, d.compare), Some(&mut depth)) }?;
                    created(depth)?
                }
                None => self.no_depth.clone(),
            };
            let mut raster = None;
            unsafe {
                self.device.CreateRasterizerState(
                    &D3D10_RASTERIZER_DESC {
                        FillMode: D3D10_FILL_SOLID,
                        CullMode: if desc.cull_back { D3D10_CULL_BACK } else { D3D10_CULL_NONE },
                        FrontCounterClockwise: BOOL(1),
                        DepthBias: 0,
                        DepthBiasClamp: 0.0,
                        SlopeScaledDepthBias: 0.0,
                        DepthClipEnable: BOOL(1),
                        ScissorEnable: BOOL(1),
                        MultisampleEnable: BOOL(0),
                        AntialiasedLineEnable: BOOL(0),
                    },
                    Some(&mut raster),
                )
            }?;
            let raster = created(raster)?;
            Ok(Pipe { kind, alpha_mode, blend, depth, raster })
        })();
        match made {
            Ok(pipe) => {
                self.pipes.insert(id, pipe);
            }
            Err(e) => eprintln!("D3D10: pipeline {}: {e}", desc.label),
        }
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
            self.atlas = std::mem::replace(&mut self.atlas, Atlas::new(256)).grow().unwrap();
            self.dirty = true;
        }
    }

    pub fn remove_texture(&mut self, id: u32) {
        self.atlas.remove(id);
        self.dirty = true;
    }

    pub fn draw(
        &mut self,
        cmds: &CommandList,
        buffers: &HashMap<BufferId, Vec<u8>>,
        width: u32,
        height: u32,
        rtv: Option<&ID3D10RenderTargetView>,
        dsv: Option<&ID3D10DepthStencilView>,
    ) {
        if self.dirty {
            self.upload_sheet();
        }
        let mut bound = [Binding::default(); 12];
        let mut pipe = None;
        let mut surface = false;
        for cmd in &cmds.cmds {
            match *cmd {
                Cmd::BeginPass { color, depth } => {
                    surface = matches!(color, Some((Target::Surface, _)));
                    pipe = None;
                    if surface {
                        unsafe {
                            self.device.RSSetScissorRects(Some(&[RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 }]));
                            if let (Some((_, Some(c))), Some(rtv)) = (color, rtv) {
                                self.device.ClearRenderTargetView(rtv, &c);
                            }
                            if let (Some((_, Some(d))), Some(dsv)) = (depth, dsv) {
                                self.device.ClearDepthStencilView(dsv, D3D10_CLEAR_DEPTH.0 as u32, d, 0);
                            }
                        }
                    }
                }
                Cmd::EndPass => {
                    surface = false;
                    pipe = None;
                }
                Cmd::SetPipeline(id) => pipe = self.pipes.get(&id),
                Cmd::BindBuffer { binding, buffer, offset, size } => {
                    if let Some(slot) = bound.get_mut(binding as usize) {
                        *slot = Binding { buffer: Some(buffer), offset, size };
                    }
                }
                Cmd::Scissor { x, y, w, h } if surface => unsafe {
                    self.device.RSSetScissorRects(Some(&[RECT { left: x, top: y, right: x + w as i32, bottom: y + h as i32 }]));
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
            self.device.IASetInputLayout(None::<&ID3D10InputLayout>);
            self.device.VSSetShader(None::<&ID3D10VertexShader>);
            self.device.PSSetShader(None::<&ID3D10PixelShader>);
            self.device.PSSetShaderResources(0, Some(&[None]));
        }
    }

    fn draw_indirect(&mut self, pipe: &Pipe, bound: &[Binding; 12], args: &[u8], buffers: &HashMap<BufferId, Vec<u8>>) {
        let bytes = |binding: u32| bound[binding as usize].bytes(buffers);
        let tables = Tables {
            g: bytes(world_bind::G),
            scene: bytes(world_bind::SCENE),
            biomes: bytes(world_bind::BIOMES),
            tex_info: bytes(world_bind::TEX_INFO),
            frames: bytes(world_bind::FRAMES),
        };
        if tables.g.len() < globals::SIZE || tables.scene.len() < scene::SIZE {
            return;
        }
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
        if self.geo.indices.is_empty() {
            return;
        }
        let Some(view) = self.sheet_view.clone() else { return };
        let vbytes = self.geo.vertices.len() * 4;
        let ibytes = self.geo.indices.len() * 4;
        if self.vbo_cap < vbytes {
            match self.make_buffer(vbytes, D3D10_BIND_VERTEX_BUFFER) {
                Ok(buffer) => {
                    self.vbo = Some(buffer);
                    self.vbo_cap = vbytes;
                }
                Err(e) => {
                    eprintln!("D3D10: vertex buffer: {e}");
                    return;
                }
            }
        }
        if self.ibo_cap < ibytes {
            match self.make_buffer(ibytes, D3D10_BIND_INDEX_BUFFER) {
                Ok(buffer) => {
                    self.ibo = Some(buffer);
                    self.ibo_cap = ibytes;
                }
                Err(e) => {
                    eprintln!("D3D10: index buffer: {e}");
                    return;
                }
            }
        }
        let (Some(vbo), Some(ibo)) = (self.vbo.clone(), self.ibo.clone()) else { return };
        // The constant buffer mirrors the Scene prefix the shaders read: matrix, fog colour,
        // params, light, then the alpha mode this pipeline draws with.
        let mut frame = [0u8; 128];
        frame[0..64].copy_from_slice(&tables.scene[0..64]);
        frame[64..80].copy_from_slice(&tables.scene[scene::FOG_COLOR..scene::FOG_COLOR + 16]);
        frame[80..96].copy_from_slice(&tables.scene[scene::PARAMS..scene::PARAMS + 16]);
        frame[96..112].copy_from_slice(&tables.scene[scene::LIGHT..scene::LIGHT + 16]);
        frame[112..116].copy_from_slice(&pipe.alpha_mode.to_le_bytes());
        unsafe {
            self.device.UpdateSubresource(&vbo, 0, None, self.geo.vertices.as_ptr().cast(), 0, 0);
            self.device.UpdateSubresource(&ibo, 0, None, self.geo.indices.as_ptr().cast(), 0, 0);
            self.device.UpdateSubresource(&self.cb, 0, None, frame.as_ptr().cast(), 0, 0);
            self.device.IASetInputLayout(Some(&self.layout));
            self.device.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            let vertex_buffers = [Some(vbo)];
            let strides = [(F32S_PER_VERTEX * 4) as u32];
            let offsets = [0u32];
            self.device
                .IASetVertexBuffers(0, 1, Some(vertex_buffers.as_ptr()), Some(strides.as_ptr()), Some(offsets.as_ptr()));
            self.device.IASetIndexBuffer(Some(&ibo), DXGI_FORMAT_R32_UINT, 0);
            self.device.VSSetShader(Some(&self.vs));
            self.device.PSSetShader(Some(&self.ps));
            self.device.VSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            self.device.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            self.device.PSSetShaderResources(0, Some(&[Some(view)]));
            self.device.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            self.device.OMSetBlendState(&pipe.blend, &[0.0; 4], 0xFFFFFFFF);
            self.device.OMSetDepthStencilState(Some(&pipe.depth), 0);
            self.device.RSSetState(Some(&pipe.raster));
            self.device.DrawIndexed(self.geo.indices.len() as u32, 0, 0);
        }
    }

    fn make_buffer(&self, bytes: usize, bind: D3D10_BIND_FLAG) -> windows::core::Result<ID3D10Buffer> {
        let mut buffer = None;
        unsafe {
            self.device.CreateBuffer(
                &D3D10_BUFFER_DESC {
                    ByteWidth: bytes as u32,
                    Usage: D3D10_USAGE_DEFAULT,
                    BindFlags: bind.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                },
                None,
                Some(&mut buffer),
            )
        }?;
        created(buffer)
    }

    fn upload_sheet(&mut self) {
        let size = self.atlas.sheet();
        if self.sheet_size != size || self.sheet.is_none() {
            let desc = D3D10_TEXTURE2D_DESC {
                Width: size,
                Height: size,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D10_USAGE_DEFAULT,
                BindFlags: D3D10_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let made = (|| -> windows::core::Result<(ID3D10Texture2D, Option<ID3D10ShaderResourceView>)> {
                let tex = unsafe { self.device.CreateTexture2D(&desc, None) }?;
                let mut view = None;
                unsafe { self.device.CreateShaderResourceView(&tex, None, Some(&mut view)) }?;
                Ok((tex, view))
            })();
            match made {
                Ok((tex, view)) => {
                    self.sheet = Some(tex);
                    self.sheet_view = view;
                    self.sheet_size = size;
                }
                Err(e) => {
                    eprintln!("D3D10: atlas sheet: {e}");
                    self.dirty = false;
                    return;
                }
            }
        }
        if let Some(sheet) = &self.sheet {
            unsafe { self.device.UpdateSubresource(sheet, 0, None, self.atlas.pixels().as_ptr().cast(), size * 4, 0) };
        }
        self.dirty = false;
    }
}

fn depth_desc(enable: bool, write: bool, compare: weft_render::Compare) -> D3D10_DEPTH_STENCIL_DESC {
    let keep = D3D10_DEPTH_STENCILOP_DESC {
        StencilFailOp: D3D10_STENCIL_OP_KEEP,
        StencilDepthFailOp: D3D10_STENCIL_OP_KEEP,
        StencilPassOp: D3D10_STENCIL_OP_KEEP,
        StencilFunc: D3D10_COMPARISON_ALWAYS,
    };
    D3D10_DEPTH_STENCIL_DESC {
        DepthEnable: BOOL(i32::from(enable)),
        DepthWriteMask: if write { D3D10_DEPTH_WRITE_MASK_ALL } else { D3D10_DEPTH_WRITE_MASK_ZERO },
        DepthFunc: match compare {
            weft_render::Compare::Always => D3D10_COMPARISON_ALWAYS,
            weft_render::Compare::Greater => D3D10_COMPARISON_GREATER,
            weft_render::Compare::GreaterEqual => D3D10_COMPARISON_GREATER_EQUAL,
            weft_render::Compare::Less => D3D10_COMPARISON_LESS,
            weft_render::Compare::LessEqual => D3D10_COMPARISON_LESS_EQUAL,
        },
        StencilEnable: BOOL(0),
        StencilReadMask: 0xFF,
        StencilWriteMask: 0xFF,
        FrontFace: keep,
        BackFace: keep,
    }
}