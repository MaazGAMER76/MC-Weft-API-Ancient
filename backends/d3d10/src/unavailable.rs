use weft_render::{
    Backend, BackendDesc, BufferId, Capture, CommandList, ComputeDesc, Error, FrameInfo, PipelineDesc, PipelineId, TextureDesc, TextureId,
};

pub struct D3d10Backend;

impl D3d10Backend {
    pub fn new(_desc: &BackendDesc) -> Result<Self, Error> {
        Err(Error("Direct3D 10 is only available on Windows".into()))
    }
}

impl Backend for D3d10Backend {
    fn name(&self) -> &str {
        "d3d10"
    }

    fn supports_compute(&self) -> bool {
        false
    }

    fn supports_ui(&self) -> bool {
        false
    }

    fn frames_in_flight(&self) -> usize {
        1
    }

    fn surface_size(&self) -> (u32, u32) {
        (1, 1)
    }

    fn resize(&mut self, _width: u32, _height: u32) {}
    fn set_vsync(&mut self, _vsync: bool) {}
    fn create_buffer(&mut self, _size: u64, _usage: u32, _label: &str) -> BufferId {
        BufferId(0)
    }
    fn mapped(&mut self, _buffer: BufferId) -> &mut [u8] {
        &mut []
    }
    fn write_buffer(&mut self, _buffer: BufferId, _offset: u64, _data: &[u8]) {}
    fn destroy_buffer(&mut self, _buffer: BufferId) {}
    fn create_texture(&mut self, _desc: &TextureDesc, _label: &str) -> TextureId {
        TextureId(0)
    }
    fn write_texture(&mut self, _texture: TextureId, _mip: u32, _data: &[u8]) {}
    fn texture_index(&self, _texture: TextureId) -> u32 {
        0
    }
    fn destroy_texture(&mut self, _texture: TextureId) {}
    fn create_pipeline(&mut self, _desc: &PipelineDesc<'_>) -> Result<PipelineId, Error> {
        Err(Error("Direct3D 10 is only available on Windows".into()))
    }
    fn create_compute(&mut self, _desc: &ComputeDesc<'_>) -> Result<PipelineId, Error> {
        Err(Error("Direct3D 10 is only available on Windows".into()))
    }
    fn destroy_pipeline(&mut self, _pipeline: PipelineId) {}
    fn begin_frame(&mut self) -> Option<FrameInfo> {
        None
    }
    fn end_frame(&mut self, _cmds: &CommandList) {}
    fn request_capture(&mut self) {}
    fn take_capture(&mut self) -> Option<Capture> {
        None
    }
    fn wait_idle(&mut self) {}
}
