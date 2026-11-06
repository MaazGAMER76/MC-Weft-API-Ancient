use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

use softbuffer::{Context, Surface};
use weft_render::{
    Backend, BackendDesc, BufferId, Capture, CommandList, ComputeDesc, Error, FrameInfo, PipelineDesc, PipelineId, TextureDesc, TextureId,
    Window,
};

type CpuSurface = Surface<Arc<dyn Window>, Arc<dyn Window>>;

/// The upper half of every CPU frame: sky blue.
const SKY: (u8, u8, u8) = (0x87, 0xCE, 0xEB);
/// The lower half: green. Two colours rather than one, so a frame that reached the screen proves
/// more than a single flat clear would.
const GRASS: (u8, u8, u8) = (0x22, 0x8B, 0x22);

struct CpuTexture {
    desc: TextureDesc,
    bytes: Vec<u8>,
}

pub struct CpuBackend {
    _context: Context<Arc<dyn Window>>,
    surface: CpuSurface,
    width: u32,
    height: u32,
    next_buffer: u32,
    next_texture: u32,
    buffers: HashMap<BufferId, Vec<u8>>,
    textures: HashMap<TextureId, CpuTexture>,
    capture_requested: bool,
    capture: Option<Capture>,
}

impl CpuBackend {
    pub fn new(desc: &BackendDesc) -> Result<Self, Error> {
        let context = Context::new(desc.window.clone()).map_err(|e| Error(format!("creating software display: {e}")))?;
        let surface = Surface::new(&context, desc.window.clone()).map_err(|e| Error(format!("creating software surface: {e}")))?;
        let mut backend = Self {
            _context: context,
            surface,
            width: desc.width.max(1),
            height: desc.height.max(1),
            next_buffer: 0,
            next_texture: 0,
            buffers: HashMap::new(),
            textures: HashMap::new(),
            capture_requested: false,
            capture: None,
        };
        backend.resize_surface()?;
        Ok(backend)
    }

    fn resize_surface(&mut self) -> Result<(), Error> {
        let width = NonZeroU32::new(self.width.max(1)).expect("nonzero width");
        let height = NonZeroU32::new(self.height.max(1)).expect("nonzero height");
        self.surface
            .resize(width, height)
            .map_err(|e| Error(format!("resizing software surface: {e}")))
    }

    fn texture_len(desc: &TextureDesc) -> usize {
        let bpp = desc.format.bytes_per_pixel() as usize;
        let (mut width, mut height, mut total) = (desc.width as usize, desc.height as usize, 0usize);
        for _ in 0..desc.mips.max(1) {
            let Some(level) = width.checked_mul(height).and_then(|n| n.checked_mul(bpp)) else { return 0 };
            let Some(next) = total.checked_add(level) else { return 0 };
            total = next;
            width = (width / 2).max(1);
            height = (height / 2).max(1);
        }
        total
    }

    fn pixel(y: u32, height: u32) -> (u8, u8, u8) {
        if y < height / 2 {
            SKY
        } else {
            GRASS
        }
    }
}

impl Backend for CpuBackend {
    fn name(&self) -> &str {
        "cpu-softbuffer"
    }

    fn supports_compute(&self) -> bool {
        false
    }

    fn device_name(&self) -> &str {
        "CPU softbuffer - Ironlake DEV_0046 fallback, presents a sky/green frame, draws no geometry"
    }

    fn frames_in_flight(&self) -> usize {
        1
    }

    fn surface_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        if let Err(error) = self.resize_surface() {
            eprintln!("CPU renderer: {error}");
        }
    }

    fn set_vsync(&mut self, _vsync: bool) {}

    fn create_buffer(&mut self, size: u64, _usage: u32, _label: &str) -> BufferId {
        let id = BufferId(self.next_buffer);
        self.next_buffer = self.next_buffer.wrapping_add(1);
        let size = usize::try_from(size).unwrap_or(0);
        self.buffers.insert(id, vec![0; size]);
        id
    }

    fn mapped(&mut self, buffer: BufferId) -> &mut [u8] {
        self.buffers
            .get_mut(&buffer)
            .expect("unknown CPU buffer")
            .as_mut_slice()
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
        self.next_texture = self.next_texture.wrapping_add(1);
        self.textures
            .insert(id, CpuTexture { desc: desc.clone(), bytes: vec![0; Self::texture_len(desc)] });
        id
    }

    fn write_texture(&mut self, texture: TextureId, mip: u32, data: &[u8]) {
        let Some(texture) = self.textures.get_mut(&texture) else { return };
        let mut offset = 0usize;
        let (mut width, mut height) = (texture.desc.width as usize, texture.desc.height as usize);
        for _ in 0..mip.min(texture.desc.mips.saturating_sub(1)) {
            let Some(level) = width
                .checked_mul(height)
                .and_then(|n| n.checked_mul(texture.desc.format.bytes_per_pixel() as usize))
            else {
                return;
            };
            let Some(next) = offset.checked_add(level) else { return };
            offset = next;
            width = (width / 2).max(1);
            height = (height / 2).max(1);
        }
        let Some(end) = offset.checked_add(data.len()) else { return };
        if let Some(dst) = texture.bytes.get_mut(offset..end) {
            dst.copy_from_slice(data);
        }
    }

    fn texture_index(&self, texture: TextureId) -> u32 {
        texture.0
    }

    fn destroy_texture(&mut self, texture: TextureId) {
        self.textures.remove(&texture);
    }

    fn create_pipeline(&mut self, _desc: &PipelineDesc<'_>) -> Result<PipelineId, Error> {
        Ok(PipelineId(0))
    }

    fn create_compute(&mut self, _desc: &ComputeDesc<'_>) -> Result<PipelineId, Error> {
        Err(Error("compute not supported - CPU terrain fallback active".into()))
    }

    fn destroy_pipeline(&mut self, _pipeline: PipelineId) {}

    fn begin_frame(&mut self) -> Option<FrameInfo> {
        Some(FrameInfo { width: self.width, height: self.height, slot: 0 })
    }

    fn end_frame(&mut self, _cmds: &CommandList) {
        let (width, height) = (self.width, self.height);
        let mut rgba = Vec::new();
        let Ok(pixel_count) = usize::try_from(width).and_then(|w| usize::try_from(height).map(|h| w.saturating_mul(h))) else {
            return;
        };
        if self.capture_requested {
            rgba.reserve(pixel_count.saturating_mul(4));
        }
        let mut buffer = match self.surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("CPU renderer: {error}");
                return;
            }
        };
        for (row, chunk) in buffer.chunks_mut(width.max(1) as usize).enumerate() {
            let (red, green, blue) = Self::pixel(row as u32, height);
            let pixel = (u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue);
            for dst in &mut *chunk {
                *dst = pixel;
            }
            if self.capture_requested {
                for _ in 0..chunk.len() {
                    rgba.extend_from_slice(&[red, green, blue, 255]);
                }
            }
        }
        if let Err(error) = buffer.present() {
            eprintln!("CPU renderer: {error}");
            return;
        }
        if self.capture_requested {
            self.capture = Some(Capture { width, height, rgba });
            self.capture_requested = false;
        }
    }

    fn request_capture(&mut self) {
        self.capture_requested = true;
    }

    fn take_capture(&mut self) -> Option<Capture> {
        self.capture.take()
    }

    fn wait_idle(&mut self) {}
}
