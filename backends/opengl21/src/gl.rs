use std::ffi::{c_void, CStr};

use windows::core::PCSTR;
use windows::Win32::Graphics::OpenGL::wglGetProcAddress;

pub const ARRAY_BUFFER: u32 = 0x8892;
pub const ELEMENT_ARRAY_BUFFER: u32 = 0x8893;
pub const STATIC_DRAW: u32 = 0x88E4;
pub const VERTEX_SHADER: u32 = 0x8B31;
pub const FRAGMENT_SHADER: u32 = 0x8B30;
pub const COMPILE_STATUS: u32 = 0x8B81;
pub const LINK_STATUS: u32 = 0x8B82;
pub const INFO_LOG_LENGTH: u32 = 0x8B84;
pub const TEXTURE0: u32 = 0x84C0;
pub const CLAMP_TO_EDGE: u32 = 0x812F;

unsafe fn find<T: Copy>(name: &CStr) -> Option<T> {
    let proc = unsafe { wglGetProcAddress(PCSTR(name.as_ptr().cast())) }?;
    let address = proc as *const c_void;
    // WGL drivers can return these sentinels instead of NULL for missing symbols.
    if (address as usize) <= 4 || address as isize == -1 {
        return None;
    }
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of_val(&address));
    Some(unsafe { std::mem::transmute_copy(&address) })
}

pub fn swap_interval(vsync: bool) -> bool {
    let set = unsafe { find::<unsafe extern "system" fn(i32) -> i32>(c"wglSwapIntervalEXT") };
    set.is_some_and(|set| unsafe { set(i32::from(vsync)) != 0 })
}

macro_rules! table {
    ($($name:ident($($arg:ty),*) -> $ret:ty;)*) => {
        #[allow(non_snake_case)]
        pub struct Gl { $(pub $name: unsafe extern "system" fn($($arg),*) -> $ret,)* }

        impl Gl {
            pub fn load() -> Result<Self, String> {
                Ok(Self {
                    $($name: unsafe {
                        find(CStr::from_bytes_with_nul(concat!(stringify!($name), "\0").as_bytes()).unwrap())
                    }.ok_or_else(|| format!("missing OpenGL entry point {}", stringify!($name)))?,)*
                })
            }
        }
    };
}

table! {
    glGenBuffers(i32, *mut u32) -> ();
    glBindBuffer(u32, u32) -> ();
    glBufferData(u32, isize, *const c_void, u32) -> ();
    glDeleteBuffers(i32, *const u32) -> ();
    glCreateShader(u32) -> u32;
    glShaderSource(u32, i32, *const *const i8, *const i32) -> ();
    glCompileShader(u32) -> ();
    glGetShaderiv(u32, u32, *mut i32) -> ();
    glGetShaderInfoLog(u32, i32, *mut i32, *mut i8) -> ();
    glDeleteShader(u32) -> ();
    glCreateProgram() -> u32;
    glAttachShader(u32, u32) -> ();
    glBindAttribLocation(u32, u32, *const i8) -> ();
    glLinkProgram(u32) -> ();
    glGetProgramiv(u32, u32, *mut i32) -> ();
    glGetProgramInfoLog(u32, i32, *mut i32, *mut i8) -> ();
    glUseProgram(u32) -> ();
    glDeleteProgram(u32) -> ();
    glGetUniformLocation(u32, *const i8) -> i32;
    glUniform1i(i32, i32) -> ();
    glUniform2f(i32, f32, f32) -> ();
    glUniform3f(i32, f32, f32, f32) -> ();
    glUniformMatrix4fv(i32, i32, u8, *const f32) -> ();
    glEnableVertexAttribArray(u32) -> ();
    glDisableVertexAttribArray(u32) -> ();
    glVertexAttribPointer(u32, i32, u32, u8, i32, *const c_void) -> ();
    glActiveTexture(u32) -> ();
}
