//! Direct3D 10 backend. Only Windows has a D3D10 driver to talk to; everywhere else the
//! renderer reports itself unavailable so the app can fall back instead of failing to build.

#[cfg(windows)]
mod dx10;

#[cfg(not(windows))]
mod unavailable;

#[cfg(windows)]
pub use dx10::D3d10Backend;

#[cfg(not(windows))]
pub use unavailable::D3d10Backend;
