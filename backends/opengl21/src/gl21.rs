//! OpenGL 2.1 (the highest GL level the Ironlake driver exposes) backend, built on the plain
//! Windows GL API: no Vulkan, DX11 or shader-model-5 dependency anywhere.
//!
//! World records use CPU decoding and a texture atlas instead of bindless WGSL.

use std::collections::HashMap;
use std::ffi::c_void;

use raw_window_handle::RawWindowHandle;
use weft_render::{
    Backend, BackendDesc, BufferId, Capture, Cmd, CommandList, ComputeDesc, Error, FrameInfo, PipelineDesc, PipelineId, Target,
    TextureDesc, TextureId,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC, HDC};
use windows::Win32::Graphics::OpenGL::{
    glClear, glClearColor, glFinish, glGetString, glReadPixels, glViewport, wglCreateContext, wglDeleteContext, wglGetCurrentContext,
    wglMakeCurrent, ChoosePixelFormat, SetPixelFormat, SwapBuffers, GL_COLOR_BUFFER_BIT, GL_RENDERER, GL_RGBA, GL_UNSIGNED_BYTE,
    GL_VERSION, HGLRC, PFD_DOUBLEBUFFER, PFD_DRAW_TO_WINDOW, PFD_FLAGS, PFD_MAIN_PLANE, PFD_SUPPORT_OPENGL, PFD_TYPE_RGBA,
    PIXELFORMATDESCRIPTOR,
};

use crate::gl::{self, Gl};
use crate::world::World;

const SKY: [f32; 4] = [0.53, 0.81, 0.92, 1.0];

pub struct Gl21Backend {
    hwnd: HWND,
    hdc: HDC,
    context: HGLRC,
    width: u32,
    height: u32,
    adapter: String,
    clear: [f32; 4],
    world: World,
    buffers: HashMap<BufferId, Vec<u8>>,
    next_buffer: u32,
    textures: HashMap<TextureId, TextureDesc>,
    next_texture: u32,
    capture_requested: bool,
    capture: Option<Capture>,
}

// The context is made current on the frame thread and used only there; `Backend: Send` asks only
// that the owner may move here.
unsafe impl Send for Gl21Backend {}

impl Gl21Backend {
    pub fn new(desc: &BackendDesc) -> Result<Self, Error> {
        let handle = desc
            .window
            .window_handle()
            .map_err(|e| Error(format!("window handle: {e}")))?;
        let hwnd = match handle.as_raw() {
            RawWindowHandle::Win32(win32) => HWND(win32.hwnd.get() as *mut c_void),
            other => return Err(Error(format!("OpenGL needs a Win32 window, got {other:?}"))),
        };
        let hdc = unsafe { GetDC(Some(hwnd)) };
        if hdc.is_invalid() {
            return Err(Error("GetDC returned no device context for the window".into()));
        }
        // winit registers its window class without a pixel format, and `wglCreateContext` answers
        // E_INVALIDPIXELFORMAT until one is set on this DC.
        let pfd = PIXELFORMATDESCRIPTOR {
            nSize: std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16,
            nVersion: 1,
            dwFlags: PFD_FLAGS(PFD_DRAW_TO_WINDOW.0 | PFD_SUPPORT_OPENGL.0 | PFD_DOUBLEBUFFER.0),
            iPixelType: PFD_TYPE_RGBA,
            cColorBits: 32,
            cAlphaBits: 8,
            cDepthBits: 24,
            cStencilBits: 8,
            iLayerType: PFD_MAIN_PLANE.0 as u8,
            ..Default::default()
        };
        let format = unsafe { ChoosePixelFormat(hdc, &pfd) };
        if format == 0 {
            unsafe { ReleaseDC(Some(hwnd), hdc) };
            return Err(Error("the driver offers no OpenGL pixel format for this window".into()));
        }
        if let Err(error) = unsafe { SetPixelFormat(hdc, format, &pfd) } {
            unsafe { ReleaseDC(Some(hwnd), hdc) };
            return Err(Error(format!("setting the OpenGL pixel format: {error}")));
        }
        // A plain wglCreateContext gives the driver's own pixel format and its highest compatibility
        // profile, which on this hardware is 2.1 - enough for what this backend does.
        let context = unsafe { wglCreateContext(hdc) }.map_err(|e| Error(format!("WGL context on the window failed: {e}")))?;
        unsafe { wglMakeCurrent(hdc, context) }.map_err(|e| Error(format!("making the WGL context current: {e}")))?;
        let (version, renderer) = (text(GL_VERSION), text(GL_RENDERER));
        let adapter = format!("OpenGL {version} - {renderer}");
        let world = Gl::load()
            .map_err(|e| Error(format!("GL21 entry points: {e}")))
            .and_then(|gl| World::new(gl).map_err(|e| Error(format!("GL21 renderer: {e}"))));
        let world = match world {
            Ok(world) => world,
            Err(error) => {
                unsafe {
                    let _ = wglMakeCurrent(hdc, HGLRC(std::ptr::null_mut()));
                    let _ = wglDeleteContext(context);
                    ReleaseDC(Some(hwnd), hdc);
                }
                return Err(error);
            }
        };
        Ok(Self {
            hwnd,
            hdc,
            context,
            width: desc.width.max(1),
            height: desc.height.max(1),
            adapter,
            clear: SKY,
            world,
            buffers: HashMap::new(),
            next_buffer: 1,
            textures: HashMap::new(),
            next_texture: 1,
            capture_requested: false,
            capture: None,
        })
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

    /// Reads the back buffer; GL rows come bottom-up, so they are flipped here.
    fn capture_now(&mut self) {
        self.capture_requested = false;
        let (width, height) = (self.width as usize, self.height as usize);
        let mut rgba = vec![0u8; width * height * 4];
        unsafe {
            glReadPixels(0, 0, self.width as i32, self.height as i32, GL_RGBA, GL_UNSIGNED_BYTE, rgba.as_mut_ptr() as *mut c_void);
        }
        let row = width * 4;
        for y in 0..height / 2 {
            let top = y * row;
            let (low, bottom) = rgba.split_at_mut((height - 1 - y) * row);
            low[top..top + row].swap_with_slice(&mut bottom[..row]);
        }
        self.capture = Some(Capture { width: self.width, height: self.height, rgba });
    }
}

fn text(name: u32) -> String {
    let pointer = unsafe { glGetString(name) };
    if pointer.is_null() {
        return "unknown".to_string();
    }
    unsafe { std::ffi::CStr::from_ptr(pointer as *const i8) }
        .to_string_lossy()
        .into_owned()
}

impl Drop for Gl21Backend {
    fn drop(&mut self) {
        unsafe {
            if wglGetCurrentContext() != self.context {
                let _ = wglMakeCurrent(self.hdc, self.context);
            }
            self.world.destroy();
            if wglGetCurrentContext() == self.context {
                let _ = wglMakeCurrent(self.hdc, HGLRC(std::ptr::null_mut()));
            }
            let _ = wglDeleteContext(self.context);
            ReleaseDC(Some(self.hwnd), self.hdc);
        }
    }
}

impl Backend for Gl21Backend {
    fn name(&self) -> &str {
        "gl21"
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
        // The driver resizes the window's default framebuffer; only the viewport has to follow.
        self.width = width.max(1);
        self.height = height.max(1);
    }

    fn set_vsync(&mut self, vsync: bool) {
        if !gl::swap_interval(vsync) {
            eprintln!("GL21 renderer: no WGL swap control; the driver decides vsync");
        }
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
        // One sheet, no mip chain: the nearest filter keeps block textures crisp.
        if mip != 0 {
            return;
        }
        let Some(desc) = self.textures.get(&texture) else { return };
        let (width, height) = (desc.width, desc.height);
        if let Err(error) = self.world.texture(texture.0, width, height, data) {
            eprintln!("GL21 renderer: {error}");
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
        Err(Error("OpenGL 2.1 has no compute shaders; the engine falls back to CPU terrain".into()))
    }

    fn destroy_pipeline(&mut self, pipeline: PipelineId) {
        self.world.destroy_pipeline(pipeline);
    }

    fn begin_frame(&mut self) -> Option<FrameInfo> {
        // A WGL context belongs to one thread at a time, so re-assert it every frame.
        if unsafe { wglGetCurrentContext() != self.context } {
            if let Err(error) = unsafe { wglMakeCurrent(self.hdc, self.context) } {
                eprintln!("GL21 renderer: context: {error}");
                return None;
            }
        }
        Some(FrameInfo { width: self.width, height: self.height, slot: 0 })
    }

    fn end_frame(&mut self, cmds: &CommandList) {
        self.clear = self.surface_clear(cmds);
        unsafe {
            glViewport(0, 0, self.width as i32, self.height as i32);
            glClearColor(self.clear[0], self.clear[1], self.clear[2], self.clear[3]);
            glClear(GL_COLOR_BUFFER_BIT);
        }
        self.world.draw(cmds, &self.buffers, self.width, self.height);
        if self.capture_requested {
            self.capture_now();
        }
        if let Err(error) = unsafe { SwapBuffers(self.hdc) } {
            eprintln!("GL21 renderer: swap: {error:?}");
        }
    }

    fn request_capture(&mut self) {
        self.capture_requested = true;
    }

    fn take_capture(&mut self) -> Option<Capture> {
        self.capture.take()
    }

    fn wait_idle(&mut self) {
        unsafe {
            if wglGetCurrentContext() != self.context {
                let _ = wglMakeCurrent(self.hdc, self.context);
            }
            glFinish();
        }
    }
}