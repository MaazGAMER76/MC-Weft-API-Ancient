//! Direct3D 10 (feature level 10_0) backend: the API level the Ironlake driver actually
//! implements, with no Vulkan, DX11 or shader-model-5 dependencies.
//!
//! Hardware device and swap chain at 10_0, render target and depth views, viewport and scissor,
//! clears, presentation, resize, and CPU readback for screenshots. World records use the same
//! CPU decoding and texture atlas as the OpenGL 2.1 backend, drawn with HLSL 4.0 shaders.

use std::collections::HashMap;
use std::ffi::c_void;

use raw_window_handle::RawWindowHandle;
use weft_render::{
    Backend, BackendDesc, BufferId, Capture, Cmd, CommandList, ComputeDesc, Error, FrameInfo, PipelineDesc, PipelineId, Target,
    TextureDesc, TextureId,
};
use windows::core::BOOL;
use windows::Win32::Foundation::{HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct3D10::{
    D3D10CreateDeviceAndSwapChain, ID3D10DepthStencilView, ID3D10Device, ID3D10RenderTargetView, ID3D10Texture2D, D3D10_BIND_DEPTH_STENCIL,
    D3D10_CLEAR_DEPTH, D3D10_CLEAR_STENCIL, D3D10_CPU_ACCESS_READ, D3D10_CREATE_DEVICE_DEBUG, D3D10_DRIVER_TYPE_HARDWARE, D3D10_MAP_READ,
    D3D10_SDK_VERSION, D3D10_TEXTURE2D_DESC, D3D10_USAGE_DEFAULT, D3D10_USAGE_STAGING, D3D10_VIEWPORT,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_D24_UNORM_S8_UINT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_MODE_DESC, DXGI_MODE_SCALING_UNSPECIFIED,
    DXGI_MODE_SCANLINE_ORDER_UNSPECIFIED, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter, IDXGIFactory, IDXGISwapChain, DXGI_PRESENT, DXGI_PRESENT_DO_NOT_WAIT, DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_EFFECT_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

use crate::world::World;

pub struct D3d10Backend {
    device: ID3D10Device,
    swapchain: IDXGISwapChain,
    back: Option<ID3D10Texture2D>,
    color: Option<ID3D10RenderTargetView>,
    depth_tex: Option<ID3D10Texture2D>,
    depth: Option<ID3D10DepthStencilView>,
    width: u32,
    height: u32,
    vsync: bool,
    adapter: String,
    /// Last clear colour seen on a surface pass. The UI pass clears with `None`, and presenting
    /// black there is indistinguishable from a dead renderer.
    clear: [f32; 4],
    world: World,
    buffers: HashMap<BufferId, Vec<u8>>,
    next_buffer: u32,
    textures: HashMap<TextureId, TextureDesc>,
    next_texture: u32,
    capture_requested: bool,
    capture: Option<Capture>,
}

// The immediate device is used only from the thread that owns the swap chain; D3D10 objects are
// not themselves thread-safe, and `Backend: Send` asks only that the owner may move here.
unsafe impl Send for D3d10Backend {}

impl D3d10Backend {
    pub fn new(desc: &BackendDesc) -> Result<Self, Error> {
        let handle = desc
            .window
            .window_handle()
            .map_err(|e| Error(format!("window handle: {e}")))?;
        let hwnd = match handle.as_raw() {
            RawWindowHandle::Win32(win32) => HWND(win32.hwnd.get() as *mut c_void),
            other => return Err(Error(format!("Direct3D 10 needs a Win32 window, got {other:?}"))),
        };
        let (width, height) = (desc.width.max(1), desc.height.max(1));
        let chain = DXGI_SWAP_CHAIN_DESC {
            BufferDesc: DXGI_MODE_DESC {
                Width: width,
                Height: height,
                RefreshRate: DXGI_RATIONAL { Numerator: 0, Denominator: 1 },
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                ScanlineOrdering: DXGI_MODE_SCANLINE_ORDER_UNSPECIFIED,
                Scaling: DXGI_MODE_SCALING_UNSPECIFIED,
            },
            // DISCARD with one buffer is what a DXGI 1.0 driver offers; Ironlake has no flip model.
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 1,
            OutputWindow: hwnd,
            Windowed: BOOL(1),
            SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
            Flags: 0,
        };
        let flags = if desc.debug { D3D10_CREATE_DEVICE_DEBUG.0 as u32 } else { 0 };
        let mut device: Option<ID3D10Device> = None;
        let mut swapchain: Option<IDXGISwapChain> = None;
        // This entry point is fixed at feature level 10_0, exactly the Ironlake ceiling; the
        // `..Device1` variants ask the driver for more than it can give.
        let created = unsafe {
            D3D10CreateDeviceAndSwapChain(
                None::<&IDXGIAdapter>,
                D3D10_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                flags,
                D3D10_SDK_VERSION,
                Some(&chain),
                Some(&mut swapchain),
                Some(&mut device),
            )
        };
        created.map_err(|e| Error(format!("Direct3D 10 hardware device at feature level 10_0 failed: {e}")))?;
        let device = device.ok_or_else(|| Error("Direct3D 10 returned no device".into()))?;
        let swapchain = swapchain.ok_or_else(|| Error("Direct3D 10 returned no swap chain".into()))?;
        let adapter = adapter_name(&swapchain);
        let world = World::new(device.clone()).map_err(|e| Error(format!("D3D10 renderer: {e}")))?;
        let mut this = Self {
            device,
            swapchain,
            back: None,
            color: None,
            width,
            height,
            vsync: desc.vsync,
            adapter,
            clear: SKY,
            depth_tex: None,
            depth: None,
            world,
            buffers: HashMap::new(),
            next_buffer: 1,
            textures: HashMap::new(),
            next_texture: 1,
            capture_requested: false,
            capture: None,
        };
        this.make_views()?;
        Ok(this)
    }

    /// (Re)builds the back-buffer view and the depth surface for the current size.
    fn make_views(&mut self) -> Result<(), Error> {
        unsafe {
            // ResizeBuffers fails while the old back buffer is still bound.
            self.device
                .OMSetRenderTargets(None, None::<&ID3D10DepthStencilView>);
            self.color = None;
            self.back = None;
            self.depth = None;
            self.depth_tex = None;
            let back: ID3D10Texture2D = self
                .swapchain
                .GetBuffer(0)
                .map_err(|e| Error(format!("back buffer: {e}")))?;
            let mut color = None;
            self.device
                .CreateRenderTargetView(&back, None, Some(&mut color))
                .map_err(|e| Error(format!("render target view: {e}")))?;
            let depth_desc = D3D10_TEXTURE2D_DESC {
                Width: self.width,
                Height: self.height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_D24_UNORM_S8_UINT,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D10_USAGE_DEFAULT,
                BindFlags: D3D10_BIND_DEPTH_STENCIL.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let depth_tex = self
                .device
                .CreateTexture2D(&depth_desc, None)
                .map_err(|e| Error(format!("depth surface: {e}")))?;
            // A null view description takes format and mip range from the resource.
            let mut depth = None;
            self.device
                .CreateDepthStencilView(&depth_tex, None, Some(&mut depth))
                .map_err(|e| Error(format!("depth view: {e}")))?;
            self.back = Some(back);
            self.color = color;
            self.depth_tex = Some(depth_tex);
            self.depth = depth;
        }
        Ok(())
    }

    fn surface_clear(&self, cmds: &CommandList) -> [f32; 4] {
        cmds.cmds
            .iter()
            .rev()
            .find_map(|cmd| match cmd {
                Cmd::BeginPass { color: Some((Target::Surface, Some(color))), .. } => Some(*color),
                _ => None,
            })
            .unwrap_or(self.clear)
    }

    /// Copies the back buffer into a `STAGING` texture; mapping it stalls until the copy lands.
    fn capture_now(&mut self) {
        self.capture_requested = false;
        let Some(back) = self.back.clone() else { return };
        let (width, height) = (self.width as usize, self.height as usize);
        let desc = D3D10_TEXTURE2D_DESC {
            Width: self.width,
            Height: self.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D10_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D10_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let staged = match unsafe { self.device.CreateTexture2D(&desc, None) } {
            Ok(staged) => staged,
            Err(error) => {
                eprintln!("D3D10 renderer: staging texture: {error}");
                return;
            }
        };
        unsafe {
            self.device.CopyResource(&staged, &back);
            self.device.Flush();
        }
        let mapped = match unsafe { staged.Map(0, D3D10_MAP_READ, 0) } {
            Ok(mapped) => mapped,
            Err(error) => {
                eprintln!("D3D10 renderer: readback: {error}");
                return;
            }
        };
        let row = mapped.RowPitch as usize;
        let mut rgba = vec![0u8; width * height * 4];
        let src = unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, row * height) };
        for y in 0..height {
            let Some(line) = src.get(y * row..y * row + width * 4) else { continue };
            let to = y * width * 4;
            rgba[to..to + width * 4].copy_from_slice(line);
        }
        unsafe { staged.Unmap(0) };
        self.capture = Some(Capture { width: self.width, height: self.height, rgba });
    }
}

const SKY: [f32; 4] = [0.53, 0.81, 0.92, 1.0];

fn adapter_name(swapchain: &IDXGISwapChain) -> String {
    let read = || -> windows::core::Result<String> {
        let factory: IDXGIFactory = unsafe { swapchain.GetParent()? };
        let adapter: IDXGIAdapter = unsafe { factory.EnumAdapters(0)? };
        let desc = unsafe { adapter.GetDesc()? };
        let label = String::from_utf16_lossy(&desc.Description)
            .split('\0')
            .next()
            .unwrap_or("")
            .to_string();
        Ok(format!(
            "{label} [VEN_{:04X}&DEV_{:04X} rev {:04X}, {} MB]",
            desc.VendorId,
            desc.DeviceId,
            desc.Revision,
            desc.DedicatedVideoMemory / 1024 / 1024
        ))
    };
    read().unwrap_or_else(|_| "Direct3D 10 adapter unnamed".to_string())
}

impl Backend for D3d10Backend {
    fn name(&self) -> &str {
        "d3d10"
    }

    fn supports_compute(&self) -> bool {
        false
    }

    fn device_name(&self) -> &str {
        &self.adapter
    }

    fn frames_in_flight(&self) -> usize {
        1
    }

    fn surface_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.width, self.height) {
            return;
        }
        self.width = width;
        self.height = height;
        unsafe {
            self.device
                .OMSetRenderTargets(None, None::<&ID3D10DepthStencilView>);
            self.color = None;
            self.back = None;
            self.depth = None;
            self.depth_tex = None;
            if let Err(error) = self
                .swapchain
                .ResizeBuffers(1, width, height, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SWAP_CHAIN_FLAG(0))
            {
                eprintln!("D3D10 renderer: resize: {error}");
            }
        }
        if let Err(error) = self.make_views() {
            eprintln!("D3D10 renderer: {error}");
        }
    }

    fn set_vsync(&mut self, vsync: bool) {
        self.vsync = vsync;
    }

    fn create_buffer(&mut self, size: u64, _usage: u32, _label: &str) -> BufferId {
        let id = BufferId(self.next_buffer);
        self.next_buffer += 1;
        self.buffers
            .insert(id, vec![0; usize::try_from(size).unwrap_or(0)]);
        id
    }

    fn mapped(&mut self, buffer: BufferId) -> &mut [u8] {
        match self.buffers.get_mut(&buffer) {
            Some(bytes) => bytes.as_mut_slice(),
            None => &mut [],
        }
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) {
        let Some(bytes) = self.buffers.get_mut(&buffer) else { return };
        let Ok(start) = usize::try_from(offset) else { return };
        let Some(end) = start.checked_add(data.len()) else { return };
        if let Some(dst) = bytes.get_mut(start..end) {
            dst.copy_from_slice(data);
        }
    }

    fn destroy_buffer(&mut self, buffer: BufferId) {
        self.buffers.remove(&buffer);
    }

    fn create_texture(&mut self, desc: &TextureDesc, _label: &str) -> TextureId {
        let id = TextureId(self.next_texture);
        self.next_texture += 1;
        self.textures.insert(id, desc.clone());
        id
    }

    fn write_texture(&mut self, texture: TextureId, mip: u32, data: &[u8]) {
        if mip != 0 {
            return;
        }
        let Some(desc) = self.textures.get(&texture) else { return };
        if let Err(error) = self.world.texture(texture.0, desc.width, desc.height, data) {
            eprintln!("D3D10 renderer: {error}");
        }
    }

    fn texture_index(&self, texture: TextureId) -> u32 {
        texture.0
    }

    fn destroy_texture(&mut self, texture: TextureId) {
        self.textures.remove(&texture);
        self.world.remove_texture(texture.0);
    }

    fn create_pipeline(&mut self, desc: &PipelineDesc<'_>) -> Result<PipelineId, Error> {
        Ok(self.world.pipeline(desc))
    }

    fn create_compute(&mut self, _desc: &ComputeDesc<'_>) -> Result<PipelineId, Error> {
        Err(Error("Direct3D 10 has no compute shaders; the engine falls back to CPU terrain".into()))
    }

    fn destroy_pipeline(&mut self, pipeline: PipelineId) {
        self.world.destroy_pipeline(pipeline);
    }

    fn begin_frame(&mut self) -> Option<FrameInfo> {
        Some(FrameInfo { width: self.width, height: self.height, slot: 0 })
    }

    fn end_frame(&mut self, cmds: &CommandList) {
        self.clear = self.surface_clear(cmds);
        let targets = [self.color.clone()];
        unsafe {
            self.device
                .OMSetRenderTargets(Some(&targets), self.depth.as_ref());
            self.device
                .ClearRenderTargetView(self.color.as_ref(), &self.clear);
            self.device
                .ClearDepthStencilView(self.depth.as_ref(), (D3D10_CLEAR_DEPTH.0 | D3D10_CLEAR_STENCIL.0) as u32, 0.0, 0);
            self.device.RSSetViewports(Some(&[D3D10_VIEWPORT {
                TopLeftX: 0,
                TopLeftY: 0,
                Width: self.width,
                Height: self.height,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            self.device
                .RSSetScissorRects(Some(&[RECT { left: 0, top: 0, right: self.width as i32, bottom: self.height as i32 }]));
            self.world.draw(cmds, &self.buffers, self.width, self.height, self.color.as_ref(), self.depth.as_ref());
            if self.capture_requested {
                self.capture_now();
            }
            let (interval, flags) = if self.vsync { (1, DXGI_PRESENT(0)) } else { (0, DXGI_PRESENT_DO_NOT_WAIT) };
            // Present answers with a bare HRESULT rather than a Result.
            let presented = self.swapchain.Present(interval, flags);
            if presented.is_err() {
                eprintln!("D3D10 renderer: present failed: {presented:?}");
            }
        }
    }

    fn request_capture(&mut self) {
        self.capture_requested = true;
    }

    fn take_capture(&mut self) -> Option<Capture> {
        self.capture.take()
    }

    fn wait_idle(&mut self) {
        unsafe { self.device.Flush() };
    }
}
