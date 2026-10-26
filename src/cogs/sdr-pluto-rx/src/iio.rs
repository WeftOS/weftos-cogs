//! libiio through `dlopen`, with no headers and no link-time dependency.
//!
//! The cog binds the C library's 0.x API (0.19 to 0.26) by hand: about twenty functions, looked
//! up when the radio is opened. A host without libiio still builds and runs `--simulate` and
//! `--replay`; only opening a radio reports "libiio not found". libiio is LGPL-2.1+ and is loaded
//! as a shared library, never copied into the binary. The `industrial-io` crate (MIT) was
//! considered and not used: it wants the libiio headers and bindgen at build time, which the
//! Ventuno and the cross-build containers do not have, and it tracks libiio 0.2x only loosely.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};

type Ptr = *mut c_void;

const LIB_NAMES: &[&str] = &[
    "libiio.so.0",
    "libiio.so",
    "libiio.dylib",
    "/opt/homebrew/lib/libiio.dylib",
    "/usr/local/lib/libiio.dylib",
    "/Library/Frameworks/iio.framework/iio",
];

macro_rules! syms {
    ($($name:ident : $ty:ty),* $(,)?) => {
        struct Syms { $($name: $ty),* }
        impl Syms {
            fn load(h: Ptr) -> Result<Self, String> {
                Ok(Self { $($name: {
                    let n = CString::new(stringify!($name)).unwrap_or_default();
                    // SAFETY: dlsym on a handle from dlopen with a valid C string.
                    let p = unsafe { libc::dlsym(h, n.as_ptr()) };
                    if p.is_null() {
                        return Err(format!("libiio has no {} (the 0.x API is required)", stringify!($name)));
                    }
                    // SAFETY: the symbol has the C signature declared in `$ty`.
                    unsafe { std::mem::transmute::<*mut c_void, $ty>(p) }
                }),* })
            }
        }
    };
}

syms! {
    iio_create_context_from_uri: unsafe extern "C" fn(*const c_char) -> Ptr,
    iio_context_destroy: unsafe extern "C" fn(Ptr),
    iio_context_set_timeout: unsafe extern "C" fn(Ptr, c_uint) -> c_int,
    iio_context_find_device: unsafe extern "C" fn(Ptr, *const c_char) -> Ptr,
    iio_device_find_channel: unsafe extern "C" fn(Ptr, *const c_char, bool) -> Ptr,
    iio_channel_attr_read: unsafe extern "C" fn(Ptr, *const c_char, *mut c_char, usize) -> isize,
    iio_channel_attr_write: unsafe extern "C" fn(Ptr, *const c_char, *const c_char) -> isize,
    iio_device_attr_write: unsafe extern "C" fn(Ptr, *const c_char, *const c_char) -> isize,
    iio_device_debug_attr_read: unsafe extern "C" fn(Ptr, *const c_char, *mut c_char, usize) -> isize,
    iio_device_debug_attr_write: unsafe extern "C" fn(Ptr, *const c_char, *const c_char) -> isize,
    iio_channel_enable: unsafe extern "C" fn(Ptr),
    iio_device_create_buffer: unsafe extern "C" fn(Ptr, usize, bool) -> Ptr,
    iio_buffer_destroy: unsafe extern "C" fn(Ptr),
    iio_buffer_refill: unsafe extern "C" fn(Ptr) -> isize,
    iio_buffer_start: unsafe extern "C" fn(Ptr) -> *mut c_void,
    iio_buffer_end: unsafe extern "C" fn(Ptr) -> *mut c_void,
    iio_strerror: unsafe extern "C" fn(c_int, *mut c_char, usize),
}

/// The loaded library.
pub struct Lib {
    s: Syms,
    _h: Ptr,
}

// SAFETY: the table is plain function pointers; the handle is never closed.
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

impl Lib {
    pub fn load() -> Result<&'static Lib, String> {
        static LIB: std::sync::OnceLock<Result<Lib, String>> = std::sync::OnceLock::new();
        LIB.get_or_init(|| {
            let mut names: Vec<String> = Vec::new();
            if let Ok(p) = std::env::var("LIBIIO_PATH") {
                names.push(p);
            }
            names.extend(LIB_NAMES.iter().map(|s| s.to_string()));
            let mut last = String::new();
            for n in &names {
                let c = CString::new(n.as_str()).unwrap_or_default();
                // SAFETY: dlopen with a valid C string.
                let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
                if h.is_null() {
                    continue;
                }
                match Syms::load(h) {
                    Ok(s) => return Ok(Lib { s, _h: h }),
                    Err(e) => last = format!("{n}: {e}"),
                }
            }
            if last.is_empty() {
                Err("libiio not found (install libiio, or set LIBIIO_PATH)".into())
            } else {
                Err(last)
            }
        })
        .as_ref()
        .map_err(|e| e.clone())
    }

    fn err(&self, code: isize) -> String {
        let mut b = [0 as c_char; 160];
        // SAFETY: libiio writes a NUL-terminated message of at most the length given.
        unsafe { (self.s.iio_strerror)((-code) as c_int, b.as_mut_ptr(), b.len()) };
        // SAFETY: NUL-terminated by the call above (the buffer was zeroed).
        unsafe { CStr::from_ptr(b.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }

    fn check(&self, rc: isize, what: &str) -> Result<isize, String> {
        if rc < 0 {
            Err(format!("{what}: {} ({rc})", self.err(rc)))
        } else {
            Ok(rc)
        }
    }
}

/// An IIO context (one connection to the radio).
pub struct Context {
    lib: &'static Lib,
    ctx: Ptr,
    pub uri: String,
}

// SAFETY: the context is used from one thread at a time (the cog's acquisition thread owns it).
unsafe impl Send for Context {}

impl Context {
    pub fn open(uri: &str, timeout_ms: u32) -> Result<Context, String> {
        let lib = Lib::load()?;
        let c = CString::new(uri).map_err(|_| "bad uri")?;
        // SAFETY: valid C string; a null result is an error.
        let ctx = unsafe { (lib.s.iio_create_context_from_uri)(c.as_ptr()) };
        if ctx.is_null() {
            return Err(format!(
                "cannot open {uri}: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: ctx is live.
        unsafe { (lib.s.iio_context_set_timeout)(ctx, timeout_ms) };
        Ok(Context {
            lib,
            ctx,
            uri: uri.to_string(),
        })
    }

    pub fn device(&self, name: &str) -> Result<Device, String> {
        let c = CString::new(name).map_err(|_| "bad name")?;
        // SAFETY: ctx is live; the result is checked.
        let d = unsafe { (self.lib.s.iio_context_find_device)(self.ctx, c.as_ptr()) };
        if d.is_null() {
            return Err(format!("no IIO device {name} on {}", self.uri));
        }
        Ok(Device {
            lib: self.lib,
            dev: d,
        })
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: destroying the context we created; devices and channels die with it and the
        // cog drops them first (they live in the same struct, declared after the context).
        unsafe { (self.lib.s.iio_context_destroy)(self.ctx) };
    }
}

/// A device handle. Valid while the `Context` it came from is alive; the owner keeps both.
#[derive(Clone, Copy)]
pub struct Device {
    lib: &'static Lib,
    dev: Ptr,
}

// SAFETY: see `Context`: one owner thread.
unsafe impl Send for Device {}

fn read_attr(
    lib: &Lib,
    f: unsafe extern "C" fn(Ptr, *const c_char, *mut c_char, usize) -> isize,
    h: Ptr,
    attr: &str,
) -> Result<String, String> {
    let a = CString::new(attr).map_err(|_| "bad attribute")?;
    let mut buf = vec![0u8; 4096];
    // SAFETY: h is live, a is a C string, buf holds the length passed.
    let n = unsafe { f(h, a.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    lib.check(n, &format!("read {attr}"))?;
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Ok(String::from_utf8_lossy(&buf[..end]).trim().to_string())
}

fn write_attr(
    lib: &Lib,
    f: unsafe extern "C" fn(Ptr, *const c_char, *const c_char) -> isize,
    h: Ptr,
    attr: &str,
    val: &str,
) -> Result<(), String> {
    let a = CString::new(attr).map_err(|_| "bad attribute")?;
    let v = CString::new(val).map_err(|_| "bad value")?;
    // SAFETY: h is live; both are C strings.
    let n = unsafe { f(h, a.as_ptr(), v.as_ptr()) };
    lib.check(n, &format!("write {attr}={val}")).map(|_| ())
}

impl Device {
    pub fn channel(&self, name: &str, output: bool) -> Result<Channel, String> {
        let c = CString::new(name).map_err(|_| "bad name")?;
        // SAFETY: dev is live; the result is checked.
        let ch = unsafe { (self.lib.s.iio_device_find_channel)(self.dev, c.as_ptr(), output) };
        if ch.is_null() {
            return Err(format!(
                "no {} channel {name}",
                if output { "output" } else { "input" }
            ));
        }
        Ok(Channel { lib: self.lib, ch })
    }

    pub fn set_attr(&self, a: &str, v: &str) -> Result<(), String> {
        write_attr(self.lib, self.lib.s.iio_device_attr_write, self.dev, a, v)
    }

    pub fn debug_attr(&self, a: &str) -> Result<String, String> {
        read_attr(self.lib, self.lib.s.iio_device_debug_attr_read, self.dev, a)
    }

    pub fn set_debug_attr(&self, a: &str, v: &str) -> Result<(), String> {
        write_attr(
            self.lib,
            self.lib.s.iio_device_debug_attr_write,
            self.dev,
            a,
            v,
        )
    }

    /// A non-cyclic capture buffer of `samples` over the channels enabled on this device.
    pub fn buffer(&self, samples: usize) -> Result<Buffer, String> {
        // SAFETY: dev is live; the result is checked.
        let b = unsafe { (self.lib.s.iio_device_create_buffer)(self.dev, samples, false) };
        if b.is_null() {
            return Err(format!(
                "cannot create a {samples}-sample buffer: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Buffer {
            lib: self.lib,
            buf: b,
        })
    }
}

#[derive(Clone, Copy)]
pub struct Channel {
    lib: &'static Lib,
    ch: Ptr,
}

// SAFETY: see `Context`.
unsafe impl Send for Channel {}

impl Channel {
    pub fn attr(&self, a: &str) -> Result<String, String> {
        read_attr(self.lib, self.lib.s.iio_channel_attr_read, self.ch, a)
    }

    pub fn set_attr(&self, a: &str, v: &str) -> Result<(), String> {
        write_attr(self.lib, self.lib.s.iio_channel_attr_write, self.ch, a, v)
    }

    pub fn enable(&self) {
        // SAFETY: ch is live.
        unsafe { (self.lib.s.iio_channel_enable)(self.ch) };
    }
}

/// A capture buffer. Dropping it destroys it, so drop it before the `Context`.
pub struct Buffer {
    lib: &'static Lib,
    buf: Ptr,
}

// SAFETY: see `Context`.
unsafe impl Send for Buffer {}

impl Buffer {
    /// Block until the buffer is full again; returns the bytes delivered.
    pub fn refill(&mut self) -> Result<usize, String> {
        // SAFETY: buf is live.
        let n = unsafe { (self.lib.s.iio_buffer_refill)(self.buf) };
        self.lib.check(n, "buffer refill").map(|n| n as usize)
    }

    /// The bytes of the last refill (interleaved little-endian 16-bit I and Q).
    pub fn data(&self, len: usize) -> &[u8] {
        // SAFETY: after a successful refill the region [start, end) holds `len` bytes owned by
        // the buffer, valid until the next refill, which needs `&mut self`.
        unsafe {
            let s = (self.lib.s.iio_buffer_start)(self.buf) as *const u8;
            let e = (self.lib.s.iio_buffer_end)(self.buf) as *const u8;
            let n = (e as usize).saturating_sub(s as usize).min(len);
            std::slice::from_raw_parts(s, n)
        }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: destroying the buffer we created.
        unsafe { (self.lib.s.iio_buffer_destroy)(self.buf) };
    }
}
