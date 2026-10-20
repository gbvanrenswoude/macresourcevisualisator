//! Development aids: render the dashboard or the menu to PNG files so the
//! design can be checked without screen-recording permissions.
//!
//! `TESSERA_SCREENSHOT=/tmp/dash.png` saves the dashboard after a few seconds
//! and quits; `TESSERA_MENU_SNAPSHOT=/tmp/menu.png` saves the dropdown's
//! stats view; `TESSERA_CAPTURE=/dir` saves the windows as they look on
//! screen.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;

use crate::state::Shared;

pub struct Screenshot {
    path: PathBuf,
    started: Instant,
    delay: Duration,
    requested: bool,
}

impl Screenshot {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("TESSERA_SCREENSHOT")?;
        let delay = std::env::var("TESSERA_SCREENSHOT_DELAY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(6.0);
        Some(Self {
            path: path.into(),
            started: Instant::now(),
            delay: Duration::from_secs_f64(delay),
            requested: false,
        })
    }

    pub fn tick(&mut self, ctx: &egui::Context) {
        if !self.requested && self.started.elapsed() >= self.delay {
            self.requested = true;
            let path = self.path.clone();
            let keep = std::env::var_os("TESSERA_SCREENSHOT_KEEP").is_some();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                egui::ScreenshotCallback::new(move |image| {
                    let [w, h] = image.size;
                    let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                    match write_png(&path, w as u32, h as u32, &rgba) {
                        Ok(()) => eprintln!("saved {}", path.display()),
                        Err(e) => eprintln!("screenshot failed: {e}"),
                    }
                    if !keep {
                        std::process::exit(0);
                    }
                }),
            ));
        }
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}

/// `TESSERA_SHOW_AFTER=5`: start hidden and ask for the dashboard from
/// another thread after that many seconds, exactly like the menu's
/// "Open Tessera" does. Returns whether it is in effect.
pub fn show_later_from_env(shared: &Arc<Shared>) -> bool {
    let Some(secs) = std::env::var("TESSERA_SHOW_AFTER")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
    else {
        return false;
    };
    let shared = shared.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs_f64(secs));
        shared.request_dashboard();
    });
    true
}

pub fn menu_snapshot_from_env(mtm: MainThreadMarker, shared: &Arc<Shared>) {
    let Some(path) = std::env::var_os("TESSERA_MENU_SNAPSHOT") else {
        return;
    };
    let path = PathBuf::from(path);
    let shared = shared.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(4));
        dispatch_main(move || {
            let mtm = MainThreadMarker::new().unwrap();
            let dark = std::env::var("TESSERA_MENU_APPEARANCE")
                .map(|v| v != "light")
                .unwrap_or(true);
            match crate::statusbar::render_stats_png(mtm, shared, dark) {
                Some(bytes) => {
                    let _ = std::fs::write(&path, bytes);
                    eprintln!("saved {}", path.display());
                }
                None => eprintln!("menu snapshot failed"),
            }
        });
    });
    let _ = mtm;
}

/// `TESSERA_CAPTURE=/dir`: save Tessera's own windows the way they look on
/// screen, frame and shadow included: `dashboard.png` a few seconds after the
/// dashboard opens and `menu.png` once the dropdown is open. An app may
/// always capture its own windows, so unlike `screencapture` this needs no
/// screen-recording permission. macOS leaves the dashboard's Metal-drawn
/// content out of the capture, so `dashboard.png` is the window frame and
/// shadow only; combine it with `TESSERA_SCREENSHOT` for the content.
pub fn capture_windows_from_env(shared: &Arc<Shared>) {
    let Some(dir) = std::env::var_os("TESSERA_CAPTURE").map(PathBuf::from) else {
        return;
    };
    for menu in [false, true] {
        let shared = shared.clone();
        let path = dir.join(if menu { "menu.png" } else { "dashboard.png" });
        std::thread::spawn(move || {
            let flag = if menu {
                &shared.menu_open
            } else {
                &shared.dashboard_visible
            };
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
            }
            // Let the window finish appearing and the charts animate in.
            std::thread::sleep(Duration::from_secs_f64(if menu { 1.5 } else { 4.0 }));
            // Pop-up menus sit at window level 101, the dashboard at 0.
            let window = own_windows()
                .into_iter()
                .filter(|w| if menu { w.layer >= 101 } else { w.layer == 0 })
                .max_by_key(|w| w.area);
            let result = match window {
                Some(w) => capture_window(w.id)
                    .ok_or_else(|| std::io::Error::other("capture failed"))
                    .and_then(|(width, height, rgba)| write_png(&path, width, height, &rgba)),
                None => Err(std::io::Error::other("window not found")),
            };
            match result {
                Ok(()) => eprintln!("saved {}", path.display()),
                Err(e) => eprintln!("{}: {e}", path.display()),
            }
        });
    }
}

struct OwnWindow {
    id: u32,
    layer: i64,
    area: i64,
}

/// This process's on-screen windows.
fn own_windows() -> Vec<OwnWindow> {
    use tessera_core::ffi::{CFRelease, cf_number_i64, dict_get};
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *const std::ffi::c_void;
        fn CFArrayGetCount(array: *const std::ffi::c_void) -> isize;
        fn CFArrayGetValueAtIndex(
            array: *const std::ffi::c_void,
            i: isize,
        ) -> *const std::ffi::c_void;
    }
    const ON_SCREEN_ONLY: u32 = 1;
    let pid = std::process::id() as i64;
    let mut out = Vec::new();
    unsafe {
        let list = CGWindowListCopyWindowInfo(ON_SCREEN_ONLY, 0);
        if list.is_null() {
            return out;
        }
        for i in 0..CFArrayGetCount(list) {
            let w = CFArrayGetValueAtIndex(list, i);
            if cf_number_i64(dict_get(w, "kCGWindowOwnerPID")) != Some(pid) {
                continue;
            }
            let bounds = dict_get(w, "kCGWindowBounds");
            let size = |k| cf_number_i64(dict_get(bounds, k)).unwrap_or(0);
            out.push(OwnWindow {
                id: cf_number_i64(dict_get(w, "kCGWindowNumber")).unwrap_or(0) as u32,
                layer: cf_number_i64(dict_get(w, "kCGWindowLayer")).unwrap_or(0),
                area: size("Width") * size("Height"),
            });
        }
        CFRelease(list);
    }
    out
}

/// One window with its shadow, at full resolution, as straight-alpha RGBA.
fn capture_window(id: u32) -> Option<(u32, u32, Vec<u8>)> {
    use std::ffi::c_void;
    use tessera_core::ffi::{CFDataGetBytePtr, CFDataGetLength, CFRelease};
    #[repr(C)]
    struct CGRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    unsafe extern "C" {
        fn CGWindowListCreateImage(bounds: CGRect, list: u32, id: u32, image: u32)
        -> *const c_void;
        fn CGImageGetWidth(image: *const c_void) -> usize;
        fn CGImageGetHeight(image: *const c_void) -> usize;
        fn CGImageGetBytesPerRow(image: *const c_void) -> usize;
        fn CGImageGetBitsPerPixel(image: *const c_void) -> usize;
        fn CGImageGetBitmapInfo(image: *const c_void) -> u32;
        fn CGImageGetDataProvider(image: *const c_void) -> *const c_void;
        fn CGDataProviderCopyData(provider: *const c_void) -> *const c_void;
    }
    const INCLUDING_WINDOW: u32 = 1 << 3;
    const BEST_RESOLUTION: u32 = 1 << 3;
    // CGRectNull: let the window's own bounds (plus shadow) decide.
    let null = CGRect {
        x: f64::INFINITY,
        y: f64::INFINITY,
        w: 0.0,
        h: 0.0,
    };
    unsafe {
        let image = CGWindowListCreateImage(null, INCLUDING_WINDOW, id, BEST_RESOLUTION);
        if image.is_null() {
            return None;
        }
        let (w, h) = (CGImageGetWidth(image), CGImageGetHeight(image));
        let stride = CGImageGetBytesPerRow(image);
        let info = CGImageGetBitmapInfo(image);
        let data = CGDataProviderCopyData(CGImageGetDataProvider(image));
        let ok = CGImageGetBitsPerPixel(image) == 32 && !data.is_null();
        let mut rgba = Vec::with_capacity(w * h * 4);
        if ok {
            let bytes =
                std::slice::from_raw_parts(CFDataGetBytePtr(data), CFDataGetLength(data) as usize);
            let alpha = info & 0x1f;
            let little = info & 0x7000 == 0x2000;
            let alpha_first = matches!(alpha, 2 | 4 | 6);
            let [r, g, b, a] = match (little, alpha_first) {
                (true, true) => [2, 1, 0, 3],
                (true, false) => [3, 2, 1, 0],
                (false, true) => [1, 2, 3, 0],
                (false, false) => [0, 1, 2, 3],
            };
            for y in 0..h {
                for px in bytes[y * stride..][..w * 4].as_chunks::<4>().0 {
                    let alpha_v = if matches!(alpha, 0 | 5 | 6) {
                        255
                    } else {
                        px[a]
                    };
                    let un = |c: u8| match alpha {
                        // Premultiplied: undo it for PNG's straight alpha.
                        1 | 2 if alpha_v > 0 => {
                            ((c as u32 * 255 + alpha_v as u32 / 2) / alpha_v as u32).min(255) as u8
                        }
                        _ => c,
                    };
                    rgba.extend_from_slice(&[un(px[r]), un(px[g]), un(px[b]), alpha_v]);
                }
            }
        }
        if !data.is_null() {
            CFRelease(data);
        }
        CFRelease(image);
        ok.then_some((w as u32, h as u32, rgba))
    }
}

/// Run a closure on the main thread via the main dispatch queue.
pub fn dispatch_main(f: impl FnOnce() + Send + 'static) {
    unsafe extern "C" {
        static _dispatch_main_q: std::ffi::c_void;
        fn dispatch_async_f(
            queue: *const std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }
    extern "C" fn trampoline(ctx: *mut std::ffi::c_void) {
        let f: Box<Box<dyn FnOnce()>> = unsafe { Box::from_raw(ctx.cast()) };
        f();
    }
    let boxed: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    unsafe {
        dispatch_async_f(
            &raw const _dispatch_main_q,
            Box::into_raw(boxed).cast(),
            trampoline,
        )
    };
}

/// Minimal PNG writer (stored deflate blocks), enough for debug output.
pub fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    fn crc32(data: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        let mut crc = 0xFFFF_FFFFu32;
        for b in data {
            crc = table[((crc ^ *b as u32) & 0xFF) as usize] ^ (crc >> 8);
        }
        crc ^ 0xFFFF_FFFF
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for row in rgba.chunks(w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    // zlib stream with stored blocks.
    let mut z = vec![0x78, 0x01];
    for (i, block) in raw.chunks(65_535).enumerate() {
        let last = (i + 1) * 65_535 >= raw.len();
        z.push(last as u8);
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + *byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    std::fs::File::create(path)?.write_all(&out)
}
