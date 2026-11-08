use std::num::NonZeroU32;
use std::sync::Arc;

use softbuffer::{Context, Surface};
use weft_render::Renderer;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

pub(crate) struct RendererSelector {
    window: Arc<Window>,
    _context: Context<Arc<Window>>,
    surface: Surface<Arc<Window>, Arc<Window>>,
    cursor: (f64, f64),
}

/// One line of the list. `None` in `renderer` means the row is only a statement about the
/// hardware, not a choice: Ironlake has no Vulkan or DX12 driver, and no GL2.1 backend exists.
/// Both selectable rows clear the window to the pass colour but rasterise nothing, and the
/// labels say so rather than promising a game.
struct Row {
    label: &'static str,
    renderer: Option<Renderer>,
    recommended: bool,
}

const ROWS: [Row; 5] = [
    Row { label: "1 VULKAN - NOT AVAILABLE", renderer: None, recommended: false },
    Row { label: "2 DX12 - NOT AVAILABLE", renderer: None, recommended: false },
    Row { label: "3 D3D10 - WORLD RENDERS", renderer: Some(Renderer::DirectX10), recommended: true },
    Row { label: "4 GL21 - WORLD RENDERS", renderer: Some(Renderer::OpenGl21), recommended: false },
    Row { label: "5 CPU - SKY AND GREEN FRAME", renderer: Some(Renderer::Cpu), recommended: false },
];

const ROW_TOP: u32 = 55;
const ROW_STEP: u32 = 33;
const ROW_HEIGHT: u32 = 27;

fn row_at(y: f64) -> Option<usize> {
    (0..ROWS.len()).find(|&i| {
        let top = ROW_TOP as f64 + i as f64 * ROW_STEP as f64;
        y >= top && y < top + ROW_HEIGHT as f64
    })
}

fn default_choice() -> Option<Renderer> {
    ROWS.iter()
        .find(|row| row.recommended)
        .and_then(|row| row.renderer)
}

impl RendererSelector {
    pub(crate) fn new(event_loop: &ActiveEventLoop) -> Result<Self, String> {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Which render? - Ironlake Compatible")
                        .with_inner_size(PhysicalSize::new(420, 300)),
                )
                .map_err(|e| e.to_string())?,
        );
        let context = Context::new(window.clone()).map_err(|e| e.to_string())?;
        let mut surface = Surface::new(&context, window.clone()).map_err(|e| e.to_string())?;
        surface
            .resize(NonZeroU32::new(420).unwrap(), NonZeroU32::new(300).unwrap())
            .map_err(|e| e.to_string())?;
        Ok(Self { window, _context: context, surface, cursor: (0.0, 0.0) })
    }

    pub(crate) fn id(&self) -> WindowId {
        self.window.id()
    }

    pub(crate) fn request_redraw(&self) {
        self.window.request_redraw();
    }

    pub(crate) fn event(&mut self, event: &WindowEvent) -> Option<Renderer> {
        match event {
            WindowEvent::CloseRequested => Some(Renderer::Cpu),
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    let width = NonZeroU32::new(size.width).unwrap();
                    let height = NonZeroU32::new(size.height).unwrap();
                    if let Err(error) = self.surface.resize(width, height) {
                        eprintln!("renderer selector: {error}");
                    }
                }
                None
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                None
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                row_at(self.cursor.1).and_then(|row| ROWS[row].renderer)
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => match event.physical_key {
                PhysicalKey::Code(KeyCode::Digit1) => ROWS[0].renderer,
                PhysicalKey::Code(KeyCode::Digit2) => ROWS[1].renderer,
                PhysicalKey::Code(KeyCode::Digit3) => ROWS[2].renderer,
                PhysicalKey::Code(KeyCode::Digit4) => ROWS[3].renderer,
                PhysicalKey::Code(KeyCode::Digit5) => ROWS[4].renderer,
                PhysicalKey::Code(KeyCode::Enter | KeyCode::Space) => default_choice(),
                PhysicalKey::Code(KeyCode::Escape) => Some(Renderer::Cpu),
                _ => None,
            },
            WindowEvent::RedrawRequested => {
                self.draw();
                None
            }
            _ => None,
        }
    }

    fn draw(&mut self) {
        let Ok(mut buffer) = self.surface.buffer_mut() else { return };
        let (width, height) = (buffer.width().get(), buffer.height().get());
        for pixel in buffer.iter_mut() {
            *pixel = rgb(15, 20, 27);
        }
        fill_rect(&mut buffer, width, height, 12, 12, width.saturating_sub(24), 34, rgb(29, 42, 50));
        text(&mut buffer, width, height, 23, 22, "WHICH RENDER?", 2, rgb(238, 242, 240));

        for (index, row) in ROWS.iter().enumerate() {
            let y = ROW_TOP + index as u32 * ROW_STEP;
            let (background, foreground) = match (row.renderer, row.recommended) {
                (Some(_), true) => (rgb(34, 91, 73), rgb(235, 248, 239)),
                (Some(_), false) => (rgb(24, 57, 84), rgb(224, 239, 251)),
                (None, _) => (rgb(58, 64, 68), rgb(145, 151, 153)),
            };
            fill_rect(&mut buffer, width, height, 16, y, width.saturating_sub(32), ROW_HEIGHT, background);
            text(&mut buffer, width, height, 25, y + 7, row.label, 2, foreground);
        }
        text(&mut buffer, width, height, 17, 235, "CLICK A ROW OR PRESS ENTER", 1, rgb(196, 209, 207));
        text(&mut buffer, width, height, 17, 253, "GPU ROWS DRAW THE WORLD - CPU SHOWS A TEST FRAME", 1, rgb(206, 176, 120));
        if let Err(error) = buffer.present() {
            eprintln!("renderer selector: {error}");
        }
    }
}

fn rgb(red: u8, green: u8, blue: u8) -> u32 {
    (u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue)
}

fn fill_rect(pixels: &mut [u32], width: u32, height: u32, x: u32, y: u32, w: u32, h: u32, color: u32) {
    let right = x.saturating_add(w).min(width);
    let bottom = y.saturating_add(h).min(height);
    for row in y.min(height)..bottom {
        for column in x.min(width)..right {
            pixels[(row * width + column) as usize] = color;
        }
    }
}

fn text(pixels: &mut [u32], width: u32, height: u32, x: u32, y: u32, value: &str, scale: u32, color: u32) {
    let mut left = x;
    for byte in value.bytes() {
        for (row, bits) in glyph(byte).into_iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) != 0 {
                    fill_rect(pixels, width, height, left + column * scale, y + row as u32 * scale, scale, scale, color);
                }
            }
        }
        left += 6 * scale;
    }
}

fn glyph(byte: u8) -> [u8; 7] {
    match byte {
        b'A' => [14, 17, 17, 31, 17, 17, 17],
        b'B' => [30, 17, 17, 30, 17, 17, 30],
        b'C' => [14, 17, 16, 16, 16, 17, 14],
        b'D' => [30, 17, 17, 17, 17, 17, 30],
        b'E' => [31, 16, 16, 30, 16, 16, 31],
        b'F' => [31, 16, 16, 30, 16, 16, 16],
        b'G' => [14, 17, 16, 23, 17, 17, 15],
        b'H' => [17, 17, 17, 31, 17, 17, 17],
        b'I' => [14, 4, 4, 4, 4, 4, 14],
        b'J' => [7, 2, 2, 2, 18, 18, 12],
        b'K' => [17, 18, 20, 24, 20, 18, 17],
        b'L' => [16, 16, 16, 16, 16, 16, 31],
        b'M' => [17, 27, 21, 21, 17, 17, 17],
        b'N' => [17, 25, 21, 19, 17, 17, 17],
        b'O' => [14, 17, 17, 17, 17, 17, 14],
        b'P' => [30, 17, 17, 30, 16, 16, 16],
        b'Q' => [14, 17, 17, 17, 21, 18, 13],
        b'R' => [30, 17, 17, 30, 20, 18, 17],
        b'S' => [15, 16, 16, 14, 1, 1, 30],
        b'T' => [31, 4, 4, 4, 4, 4, 4],
        b'U' => [17, 17, 17, 17, 17, 17, 14],
        b'V' => [17, 17, 17, 17, 17, 10, 4],
        b'W' => [17, 17, 17, 21, 21, 21, 10],
        b'X' => [17, 17, 10, 4, 10, 17, 17],
        b'Y' => [17, 17, 10, 4, 4, 4, 4],
        b'Z' => [31, 1, 2, 4, 8, 16, 31],
        b'0' => [14, 17, 19, 21, 25, 17, 14],
        b'1' => [4, 12, 4, 4, 4, 4, 14],
        b'2' => [14, 17, 1, 2, 4, 8, 31],
        b'3' => [30, 1, 1, 14, 1, 1, 30],
        b'4' => [2, 6, 10, 18, 31, 2, 2],
        b'5' => [31, 16, 16, 30, 1, 1, 30],
        b'?' => [14, 17, 1, 2, 4, 0, 4],
        b'-' => [0, 0, 0, 31, 0, 0, 0],
        b'.' => [0, 0, 0, 0, 0, 12, 12],
        _ => [0; 7],
    }
}
