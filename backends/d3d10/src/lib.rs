use weft_render::{
    Backend, BackendDesc, BufferId, Capture, CommandList, ComputeDesc, Error, FrameInfo, PipelineDesc, PipelineId, TextureDesc, TextureId,
};

pub struct D3d10Backend {
    width: u32,
    height: u32,
}

impl D3d10Backend {
    pub fn new(desc: &BackendDesc) -> Result<Self, Error> {
        Ok(Self { width: desc.width, height: desc.height })
    }
}

impl Backend for D3d10Backend {
    fn name(&self) -> &str {
        "d3d10-unimplemented"
    }

    fn supports_compute(&self) -> bool {
        false
    }

    fn device_name(&self) -> &str {
        "D3D10 backend scaffold; Ironlake DEV_0046 hardware unverified"
    }

    fn frames_in_flight(&self) -> usize {
        1
    }

    fn surface_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn resize(&mut self, width: u32, height: u32) {
        (self.width, self.height) = (width, height);
    }

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
        Ok(PipelineId(0))
    }

    fn create_compute(&mut self, _desc: &ComputeDesc<'_>) -> Result<PipelineId, Error> {
        Err(Error("Direct3D 10 does not support compute shaders".into()))
    }

    fn destroy_pipeline(&mut self, _pipeline: PipelineId) {}

    fn begin_frame(&mut self) -> Option<FrameInfo> {
        Some(FrameInfo { width: self.width, height: self.height, slot: 0 })
    }

    fn end_frame(&mut self, _cmds: &CommandList) {}

    fn request_capture(&mut self) {}

    fn take_capture(&mut self) -> Option<Capture> {
        None
    }

    fn wait_idle(&mut self) {}
}
