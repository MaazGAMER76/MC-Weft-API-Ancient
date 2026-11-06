//! OpenGL 2.1 backend. Only Windows has the WGL entry points this uses; everywhere else the
//! renderer reports itself unavailable so the app can fall back instead of failing to build.

#[cfg(windows)]
mod gl21;

#[cfg(not(windows))]
mod unavailable;

#[cfg(windows)]
pub use gl21::Gl21Backend;

#[cfg(not(windows))]
pub use unavailable::Gl21Backend;
