//! Runs one real frame through the backend: a window, a clear submitted as a command list, a
//! present, and the pixels read back off the GPU. This is how the present path gets checked on
//! Ironlake without building the whole game.
//!
//! `cargo run -p weft_backend_d3d10 --example present`

fn main() {
    #[cfg(windows)]
    run::check();
    #[cfg(not(windows))]
    println!("the present check only means something on Windows");
}

#[cfg(windows)]
mod run {
    use std::sync::Arc;

    use weft_backend_d3d10::D3d10Backend;
    use weft_render::{Backend, BackendDesc, Cmd, CommandList, Target};
    use winit::application::ApplicationHandler;
    use winit::dpi::PhysicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::window::{Window, WindowId};

    /// The sky the report says a working present should show.
    const SKY: [f32; 4] = [0.53, 0.81, 0.92, 1.0];

    struct Check;

    impl ApplicationHandler for Check {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            let window = Arc::new(
                el.create_window(
                    Window::default_attributes()
                        .with_title("Weft Direct3D 10 present check")
                        .with_inner_size(PhysicalSize::new(320, 200)),
                )
                .expect("a window to present into"),
            );
            let size = window.inner_size();
            let surface: Arc<dyn weft_render::Window> = window.clone();
            let mut gpu = match D3d10Backend::new(&BackendDesc {
                window: surface,
                width: size.width,
                height: size.height,
                vsync: false,
                debug: false,
            }) {
                Ok(gpu) => gpu,
                Err(error) => {
                    println!("present check: the backend refused to start: {error}");
                    el.exit();
                    return;
                }
            };
            println!("device: {}", gpu.device_name());

            let mut cmds = CommandList::default();
            cmds.push(Cmd::BeginPass { color: Some((Target::Surface, Some(SKY))), depth: None });
            cmds.push(Cmd::EndPass);

            // The first frame only draws; the capture is taken on the following one.
            let drawn = gpu.begin_frame().is_some();
            gpu.end_frame(&cmds);
            gpu.request_capture();
            let drawn = drawn && gpu.begin_frame().is_some();
            gpu.end_frame(&cmds);

            match gpu.take_capture() {
                Some(shot) => {
                    let centre = (shot.height / 2) as usize * (shot.width as usize) * 4 + (shot.width / 2) as usize * 4;
                    let pixel = [shot.rgba[centre], shot.rgba[centre + 1], shot.rgba[centre + 2], shot.rgba[centre + 3]];
                    let near = |p: &[u8]| {
                        (p[0] as i32 - pixel[0] as i32).abs() < 3
                            && (p[1] as i32 - pixel[1] as i32).abs() < 3
                            && (p[2] as i32 - pixel[2] as i32).abs() < 3
                    };
                    let off: usize = shot.rgba.chunks_exact(4).filter(|p| !near(p)).count();
                    println!(
                        "present check: {}x{} captured, centre rgba {pixel:?}, {} pixels of {} differ from it",
                        shot.width,
                        shot.height,
                        off,
                        shot.rgba.len() / 4
                    );
                    println!("expected roughly rgba [135, 207, 235, 255]; all zero means the present is black");
                }
                None => println!("present check: no frame was read back{}", if drawn { "" } else { " - begin_frame returned None" }),
            }
            el.exit();
        }

        fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
            if let WindowEvent::CloseRequested = event {
                el.exit();
            }
        }
    }

    pub fn check() {
        let el = EventLoop::new().expect("an event loop");
        let _ = el.run_app(&mut Check);
    }
}
