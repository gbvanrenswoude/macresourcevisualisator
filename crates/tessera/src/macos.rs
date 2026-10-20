//! Thin wrappers around the AppKit / Foundation calls the app needs.

use std::ffi::c_int;
use std::path::{Path, PathBuf};

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{AnyThread, MainThreadMarker, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBitmapImageRep, NSCompositingOperation,
    NSDeviceRGBColorSpace, NSGraphicsContext, NSImage, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSData, NSNumber, NSPoint, NSRect, NSSize, NSString, NSURL,
    NSURLVolumeAvailableCapacityForImportantUsageKey, NSURLVolumeAvailableCapacityKey,
    NSURLVolumeLocalizedNameKey, NSURLVolumeTotalCapacityKey,
};
use tessera_core::metrics::Volume;

pub fn set_thread_qos_utility() {
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos: u32, relpri: c_int) -> c_int;
    }
    unsafe { pthread_set_qos_class_self_np(0x11, 0) };
}

/// Capacity of the startup volume, including purgeable space in
/// "available" the way Finder counts it.
pub fn volume_info() -> Option<Volume> {
    autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str("/"));
        let keys = unsafe {
            NSArray::from_slice(&[
                NSURLVolumeTotalCapacityKey,
                NSURLVolumeAvailableCapacityForImportantUsageKey,
                NSURLVolumeAvailableCapacityKey,
                NSURLVolumeLocalizedNameKey,
            ])
        };
        let values = url.resourceValuesForKeys_error(&keys).ok()?;
        let num = |key: &NSString| -> Option<u64> {
            let v = values.objectForKey(key)?;
            let n = v.downcast::<NSNumber>().ok()?;
            Some(n.longLongValue().max(0) as u64)
        };
        let total = num(unsafe { NSURLVolumeTotalCapacityKey })?;
        let free = num(unsafe { NSURLVolumeAvailableCapacityKey }).unwrap_or(0);
        let available = num(unsafe { NSURLVolumeAvailableCapacityForImportantUsageKey })
            .unwrap_or(free)
            .max(free);
        let name = values
            .objectForKey(unsafe { NSURLVolumeLocalizedNameKey })
            .and_then(|v| v.downcast::<NSString>().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Macintosh HD".into());
        Some(Volume {
            name,
            total,
            available,
            free,
        })
    })
}

/// Show or hide the Dock icon (and app menu) by switching activation policy.
pub fn set_dock_visible(visible: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let policy = if visible {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    };
    app.setActivationPolicy(policy);
}

/// The appearance forced with `TESSERA_THEME=dark` / `light`, if any.
pub fn forced_appearance() -> Option<Retained<objc2_app_kit::NSAppearance>> {
    use objc2_app_kit::{NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua};
    let name = match std::env::var("TESSERA_THEME").as_deref() {
        Ok("dark") => unsafe { NSAppearanceNameDarkAqua },
        Ok("light") => unsafe { NSAppearanceNameAqua },
        _ => return None,
    };
    NSAppearance::appearanceNamed(name)
}

/// Apply `TESSERA_THEME` to the whole app (window frame included) instead
/// of following the system.
pub fn apply_forced_appearance() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if let Some(appearance) = forced_appearance() {
        NSApplication::sharedApplication(mtm).setAppearance(Some(&appearance));
    }
}

pub fn activate() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
}

pub fn quit() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    NSApplication::sharedApplication(mtm).terminate(None);
}

pub fn reveal_in_finder(path: &Path) {
    let ws = NSWorkspace::sharedWorkspace();
    let p = NSString::from_str(&path.to_string_lossy());
    ws.selectFile_inFileViewerRootedAtPath(Some(&p), &NSString::from_str(""));
}

pub fn open_path(path: &Path) {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSWorkspace::sharedWorkspace().openURL(&url);
}

pub fn open_url(url: &str) {
    if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

pub fn open_full_disk_access_settings() {
    open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles");
}

/// Whether this process may read data protected by Full Disk Access.
pub fn has_full_disk_access() -> bool {
    let home = tessera_core::paths::home_dir();
    let probes = [
        PathBuf::from("/Library/Application Support/com.apple.TCC/TCC.db"),
        home.join("Library/Safari/Bookmarks.plist"),
    ];
    probes.iter().any(|p| std::fs::File::open(p).is_ok())
}

/// The Finder icon for a file or bundle, rendered to premultiplied RGBA.
pub fn icon_rgba(path: &str, px: usize) -> Option<(usize, usize, Vec<u8>)> {
    autoreleasepool(|_| unsafe {
        let image = NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(path));
        render_image(&image, px)
    })
}

unsafe fn render_image(image: &NSImage, px: usize) -> Option<(usize, usize, Vec<u8>)> {
    unsafe {
        let rep = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            px as isize,
            px as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )?;
        let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&ctx));
        image.drawInRect_fromRect_operation_fraction(
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(px as f64, px as f64)),
            NSRect::ZERO,
            NSCompositingOperation::Copy,
            1.0,
        );
        ctx.flushGraphics();
        NSGraphicsContext::restoreGraphicsState_class();

        let data = rep.bitmapData();
        if data.is_null() {
            return None;
        }
        let stride = rep.bytesPerRow() as usize;
        let mut out = Vec::with_capacity(px * px * 4);
        for row in 0..px {
            let line = std::slice::from_raw_parts(data.add(row * stride), px * 4);
            out.extend_from_slice(line);
        }
        Some((px, px, out))
    }
}

/// An `NSImage` from PNG bytes, sized in points (for the status item).
pub fn image_from_png(bytes: &[u8], points: f64, template: bool) -> Option<Retained<NSImage>> {
    let data = NSData::with_bytes(bytes);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(NSSize::new(points, points));
    image.setTemplate(template);
    Some(image)
}

/// Path of the enclosing `.app`, when running from a bundle.
pub fn bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe.parent()?.parent()?.parent()?;
    (bundle.extension().is_some_and(|e| e == "app")).then(|| bundle.to_path_buf())
}

// ---------------------------------------------------------------------------
// Login item, via SMAppService (macOS 13+). Called through the runtime so we
// do not need extra bindings.

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

fn main_app_service() -> Option<Retained<AnyObject>> {
    bundle_path()?;
    let cls = AnyClass::get(c"SMAppService")?;
    let svc: Option<Retained<AnyObject>> = unsafe { msg_send![cls, mainAppService] };
    svc
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginItem {
    Unavailable,
    Disabled,
    Enabled,
    NeedsApproval,
}

pub fn login_item_status() -> LoginItem {
    let Some(svc) = main_app_service() else {
        return LoginItem::Unavailable;
    };
    let status: isize = unsafe { msg_send![&svc, status] };
    match status {
        1 => LoginItem::Enabled,
        2 => LoginItem::NeedsApproval,
        _ => LoginItem::Disabled,
    }
}

pub fn set_login_item(enabled: bool) -> Result<(), String> {
    let Some(svc) = main_app_service() else {
        return Err("Install Tessera.app to use this".into());
    };
    let mut err: *mut AnyObject = std::ptr::null_mut();
    let ok: Bool = unsafe {
        if enabled {
            msg_send![&svc, registerAndReturnError: &mut err]
        } else {
            msg_send![&svc, unregisterAndReturnError: &mut err]
        }
    };
    if ok.as_bool() {
        Ok(())
    } else {
        let msg = if err.is_null() {
            "unknown error".to_string()
        } else {
            let desc: Retained<NSString> = unsafe { msg_send![err, localizedDescription] };
            desc.to_string()
        };
        Err(msg)
    }
}

/// True when macOS launched us as a login item (so we stay in the menu bar
/// instead of opening the dashboard). Only meaningful during launch.
pub fn launched_as_login_item() -> bool {
    const KEY_AE_PROP_DATA: u32 = u32::from_be_bytes(*b"prdt");
    const KEY_AE_LAUNCHED_AS_LOGIN_ITEM: u32 = u32::from_be_bytes(*b"lgit");
    autoreleasepool(|_| unsafe {
        let Some(cls) = AnyClass::get(c"NSAppleEventManager") else {
            return false;
        };
        let mgr: Option<Retained<AnyObject>> = msg_send![cls, sharedAppleEventManager];
        let Some(mgr) = mgr else { return false };
        let event: Option<Retained<AnyObject>> = msg_send![&mgr, currentAppleEvent];
        let Some(event) = event else { return false };
        let has: Bool = msg_send![&event, respondsToSelector: sel!(paramDescriptorForKeyword:)];
        if !has.as_bool() {
            return false;
        }
        let param: Option<Retained<AnyObject>> =
            msg_send![&event, paramDescriptorForKeyword: KEY_AE_PROP_DATA];
        let Some(param) = param else { return false };
        let code: u32 = msg_send![&param, enumCodeValue];
        code == KEY_AE_LAUNCHED_AS_LOGIN_ITEM
    })
}
