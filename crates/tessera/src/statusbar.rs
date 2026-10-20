//! The menu bar item and its dropdown.
//!
//! A native `NSMenu`, so it opens instantly and behaves like every other
//! menu extra. The live numbers are one custom `NSView` drawn with AppKit
//! (semantic colours, so it follows light/dark and the menu's vibrancy).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAppearanceCustomization, NSBezierPath, NSColor, NSCompositingOperation,
    NSControlStateValueOff, NSControlStateValueOn, NSFont, NSFontAttributeName, NSFontWeightMedium,
    NSFontWeightRegular, NSFontWeightSemibold, NSForegroundColorAttributeName, NSGraphicsContext,
    NSImage, NSKernAttributeName, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar, NSStatusItem,
    NSStringDrawing, NSVariableStatusItemLength, NSView, NSWorkspace,
};
use objc2_foundation::{
    NSAttributedStringKey, NSDictionary, NSNumber, NSPoint, NSRect, NSRunLoop,
    NSRunLoopCommonModes, NSSize, NSString, NSTimer,
};
use tessera_core::disk::ScanPriority;
use tessera_core::format;
use tessera_core::metrics::{AppUsage, CoreKind, Pressure, Snapshot};

use crate::macos::{self, LoginItem};
use crate::state::{Busyness, History, Shared};

const WIDTH: f64 = 304.0;
const PAD: f64 = 16.0;
const APP_ROWS: usize = 4;

static MENUBAR_PNG: &[u8] = include_bytes!("../../../assets/menubar@2x.png");

/// Keeps the status item and everything it needs alive.
pub struct StatusBar {
    _item: Retained<NSStatusItem>,
    _menu: Retained<NSMenu>,
    _target: Retained<MenuTarget>,
    _timer: Retained<NSTimer>,
}

pub struct TargetIvars {
    shared: Arc<Shared>,
    item: RefCell<Option<Retained<NSStatusItem>>>,
    stats: RefCell<Option<Retained<StatsView>>>,
    cpu_toggle: RefCell<Option<Retained<NSMenuItem>>>,
    login_toggle: RefCell<Option<Retained<NSMenuItem>>>,
    rescan: RefCell<Option<Retained<NSMenuItem>>>,
    title: RefCell<String>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and we add no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TesseraMenuTarget"]
    #[ivars = TargetIvars]
    pub struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(openDashboard:))]
        fn open_dashboard(&self, _sender: Option<&AnyObject>) {
            self.ivars().shared.request_dashboard();
        }

        #[unsafe(method(rescan:))]
        fn rescan(&self, _sender: Option<&AnyObject>) {
            start_scan(&self.ivars().shared, ScanPriority::Interactive);
            self.refresh_items();
        }

        #[unsafe(method(toggleCpu:))]
        fn toggle_cpu(&self, _sender: Option<&AnyObject>) {
            let shared = &self.ivars().shared;
            {
                let mut s = shared.settings.lock().unwrap();
                s.cpu_in_menu_bar = !s.cpu_in_menu_bar;
                s.save();
            }
            self.update_title();
            self.refresh_items();
        }

        #[unsafe(method(toggleLogin:))]
        fn toggle_login(&self, _sender: Option<&AnyObject>) {
            let enable = macos::login_item_status() != LoginItem::Enabled;
            if let Err(e) = macos::set_login_item(enable) {
                eprintln!("tessera: login item: {e}");
            }
            self.refresh_items();
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            self.ivars().shared.request_quit();
        }

        #[unsafe(method(openMenu:))]
        fn open_menu(&self, _timer: &NSTimer) {
            let mtm = MainThreadMarker::from(self);
            let button = self.ivars().item.borrow().as_ref().and_then(|i| i.button(mtm));
            if let Some(button) = button {
                // SAFETY: `None` is a valid sender.
                unsafe { button.performClick(None) };
            }
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            self.update_title();
            if self.ivars().shared.menu_open.load(Ordering::Relaxed) {
                if let Some(v) = self.ivars().stats.borrow().as_ref() {
                    v.setNeedsDisplay(true);
                }
                self.refresh_items();
            }
        }
    }

    unsafe impl NSObjectProtocol for MenuTarget {}

    unsafe impl NSMenuDelegate for MenuTarget {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {
            self.ivars().shared.menu_open.store(true, Ordering::Relaxed);
            self.refresh_items();
            if let Some(v) = self.ivars().stats.borrow().as_ref() {
                v.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            self.ivars().shared.menu_open.store(false, Ordering::Relaxed);
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars {
            shared,
            item: RefCell::new(None),
            stats: RefCell::new(None),
            cpu_toggle: RefCell::new(None),
            login_toggle: RefCell::new(None),
            rescan: RefCell::new(None),
            title: RefCell::new(String::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn update_title(&self) {
        let iv = self.ivars();
        let show = iv
            .shared
            .settings
            .lock()
            .map(|s| s.cpu_in_menu_bar)
            .unwrap_or(false);
        let title = if show {
            let cpu = iv
                .shared
                .metrics
                .lock()
                .map(|m| m.latest.cpu.total)
                .unwrap_or(0.0);
            format!(" {:.0}%", cpu * 100.0)
        } else {
            String::new()
        };
        if *iv.title.borrow() == title {
            return;
        }
        if let Some(item) = iv.item.borrow().as_ref()
            && let Some(button) = item.button(MainThreadMarker::from(self))
        {
            button.setTitle(&NSString::from_str(&title));
        }
        *iv.title.borrow_mut() = title;
    }

    fn refresh_items(&self) {
        let iv = self.ivars();
        if let Some(t) = iv.cpu_toggle.borrow().as_ref() {
            let on = iv
                .shared
                .settings
                .lock()
                .map(|s| s.cpu_in_menu_bar)
                .unwrap_or(false);
            t.setState(if on {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        if let Some(t) = iv.login_toggle.borrow().as_ref() {
            let status = macos::login_item_status();
            t.setState(if status == LoginItem::Enabled {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            t.setEnabled(status != LoginItem::Unavailable);
        }
        if let Some(t) = iv.rescan.borrow().as_ref() {
            let view = iv.shared.disk.view();
            let title = match &view.scan {
                Some(scan) => format!(
                    "Scanning disk… {} files",
                    format::count(scan.progress.files.load(Ordering::Relaxed))
                ),
                None => "Rescan Disk".to_string(),
            };
            t.setTitle(&NSString::from_str(&title));
            t.setEnabled(view.scan.is_none());
        }
    }
}

pub fn start_scan(shared: &Arc<Shared>, priority: ScanPriority) {
    let exclude = shared
        .settings
        .lock()
        .map(|s| s.excluded.iter().map(std::path::PathBuf::from).collect())
        .unwrap_or_default();
    let waker = shared.clone();
    shared
        .disk
        .start_scan(priority, exclude, move || waker.wake());
    shared.wake();
}

impl StatusBar {
    pub fn new(mtm: MainThreadMarker, shared: Arc<Shared>) -> Self {
        let target = MenuTarget::new(mtm, shared.clone());

        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            if let Some(img) = macos::image_from_png(MENUBAR_PNG, 18.0, true) {
                button.setImage(Some(&img));
            }
            button.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageLeading);
            let font =
                NSFont::monospacedDigitSystemFontOfSize_weight(12.0, unsafe { NSFontWeightMedium });
            button.setFont(Some(&font));
            button.setToolTip(Some(&NSString::from_str("Tessera")));
        }

        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        // Menu bar menus follow the menu bar, not the app's appearance.
        if let Some(appearance) = macos::forced_appearance() {
            menu.setAppearance(Some(&appearance));
        }
        menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));

        let open = menu_item(
            mtm,
            "Open Tessera",
            Some(sel!(openDashboard:)),
            "o",
            &target,
        );
        menu.addItem(&open);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let stats = StatsView::new(mtm, shared.clone());
        let stats_item = NSMenuItem::new(mtm);
        stats_item.setView(Some(&stats));
        menu.addItem(&stats_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let rescan = menu_item(mtm, "Rescan Disk", Some(sel!(rescan:)), "", &target);
        menu.addItem(&rescan);
        let cpu_toggle = menu_item(
            mtm,
            "Show CPU in Menu Bar",
            Some(sel!(toggleCpu:)),
            "",
            &target,
        );
        menu.addItem(&cpu_toggle);
        let login_toggle = menu_item(mtm, "Open at Login", Some(sel!(toggleLogin:)), "", &target);
        menu.addItem(&login_toggle);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let quit = menu_item(mtm, "Quit Tessera", Some(sel!(quit:)), "q", &target);
        menu.addItem(&quit);

        item.setMenu(Some(&menu));

        {
            let iv = target.ivars();
            *iv.item.borrow_mut() = Some(item.clone());
            *iv.stats.borrow_mut() = Some(stats);
            *iv.cpu_toggle.borrow_mut() = Some(cpu_toggle);
            *iv.login_toggle.borrow_mut() = Some(login_toggle);
            *iv.rescan.borrow_mut() = Some(rescan);
        }
        target.update_title();
        target.refresh_items();

        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                1.0,
                &target,
                sel!(tick:),
                None,
                true,
            )
        };
        unsafe { NSRunLoop::currentRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };

        // TESSERA_OPEN_MENU=5 opens the dropdown after that many seconds, like
        // a click on the icon (development aid for screenshots).
        if let Some(secs) = std::env::var("TESSERA_OPEN_MENU")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
        {
            let once = unsafe {
                NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                    secs,
                    &target,
                    sel!(openMenu:),
                    None,
                    false,
                )
            };
            unsafe { NSRunLoop::currentRunLoop().addTimer_forMode(&once, NSRunLoopCommonModes) };
        }

        Self {
            _item: item,
            _menu: menu,
            _target: target,
            _timer: timer,
        }
    }
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
    target: &MenuTarget,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    };
    let target: &AnyObject = target.as_ref();
    unsafe { item.setTarget(Some(target)) };
    item
}

// ---------------------------------------------------------------------------
// The stats view

pub struct StatsIvars {
    shared: Arc<Shared>,
    icons: RefCell<HashMap<String, Retained<NSImage>>>,
    height: Cell<f64>,
}

define_class!(
    // SAFETY: NSView may be subclassed; we only override drawing hooks.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TesseraStatsView"]
    #[ivars = StatsIvars]
    pub struct StatsView;

    impl StatsView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }
    }
);

impl StatsView {
    fn new(mtm: MainThreadMarker, shared: Arc<Shared>) -> Retained<Self> {
        let height = content_height();
        let this = Self::alloc(mtm).set_ivars(StatsIvars {
            shared,
            icons: RefCell::new(HashMap::new()),
            height: Cell::new(height),
        });
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, height));
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn icon(&self, app: &AppUsage) -> Retained<NSImage> {
        let key = app
            .bundle
            .clone()
            .unwrap_or_else(|| "public.unix-executable".into());
        if let Some(img) = self.ivars().icons.borrow().get(&key) {
            return img.clone();
        }
        let ws = NSWorkspace::sharedWorkspace();
        let img = match &app.bundle {
            Some(path) => ws.iconForFile(&NSString::from_str(path)),
            None => ws.iconForFile(&NSString::from_str("/bin/zsh")),
        };
        self.ivars().icons.borrow_mut().insert(key, img.clone());
        img
    }

    fn draw(&self) {
        let shared = &self.ivars().shared;
        let (snap, history, apps) = match shared.metrics.lock() {
            Ok(m) => (
                m.latest.clone(),
                m.history.clone(),
                m.apps.iter().take(APP_ROWS).cloned().collect::<Vec<_>>(),
            ),
            Err(_) => return,
        };
        let cores = shared.info.cores.clone();
        let reclaimable = shared
            .disk
            .view()
            .overview
            .map(|o| o.reclaimable())
            .unwrap_or(0);
        let _ = self.ivars().height.get();
        let d = Draw::new();
        let w = WIDTH - 2.0 * PAD;
        let x = PAD;
        let mut y = 8.0;

        // Summary: one word for how busy the machine is.
        let busy = Busyness::of(&snap);
        let dot = match busy.severity() {
            0 => NSColor::systemGreenColor(),
            1 => NSColor::systemYellowColor(),
            2 => NSColor::systemOrangeColor(),
            _ => NSColor::systemRedColor(),
        };
        fill_oval(
            NSRect::new(NSPoint::new(x, y + 5.5), NSSize::new(9.0, 9.0)),
            &dot,
        );
        d.text(
            busy.label(),
            x + 16.0,
            y,
            &d.headline,
            &NSColor::labelColor(),
        );
        d.text_right(
            &format!("Up {}", format::duration(snap.uptime)),
            x + w,
            y + 3.0,
            &d.small,
            &NSColor::tertiaryLabelColor(),
        );
        y += 21.0;
        d.text_clipped(
            &Busyness::summary(&snap),
            x,
            y,
            w,
            &d.small,
            &NSColor::secondaryLabelColor(),
        );
        y += 26.0;

        // CPU
        d.section("CPU", x, y);
        d.text_right(
            &format::percent(snap.cpu.total),
            x + w,
            y - 2.0,
            &d.value,
            &NSColor::labelColor(),
        );
        y += 19.0;
        sparkline(
            NSRect::new(NSPoint::new(x, y), NSSize::new(w, 26.0)),
            &history,
            &NSColor::systemBlueColor(),
        );
        y += 32.0;
        core_bars(
            NSRect::new(NSPoint::new(x, y), NSSize::new(w, 24.0)),
            &snap,
            &cores,
        );
        y += 28.0;
        // Cluster captions under each group of bars.
        let groups = core_groups(&cores, &snap);
        let n = cores.len().max(1) as f64;
        let gap = 6.0;
        let groups_count = groups.len().max(1) as f64;
        let bar_w = (w - gap * (groups_count - 1.0)) / n;
        let mut gx = x;
        for (label, count, avg) in &groups {
            let gw = bar_w * *count as f64;
            let caption = format!("{label} {}", format::percent(*avg));
            let size = d.measure(&caption, &d.tiny);
            d.text(
                &caption,
                gx + (gw - size.width) / 2.0,
                y,
                &d.tiny,
                &NSColor::tertiaryLabelColor(),
            );
            gx += gw + gap;
        }
        y += 17.0;
        let detail = format!(
            "User {}   System {}   Load {:.1} · {:.1} · {:.1}",
            format::percent(snap.cpu.user),
            format::percent(snap.cpu.system),
            snap.load[0],
            snap.load[1],
            snap.load[2]
        );
        d.text(&detail, x, y, &d.small, &NSColor::secondaryLabelColor());
        y += 28.0;

        // Memory
        let m = &snap.memory;
        d.section("MEMORY", x, y);
        d.text_right(
            &format!(
                "{} of {}",
                format::bytes_short(m.used),
                format::bytes_short(m.total)
            ),
            x + w,
            y - 2.0,
            &d.value,
            &NSColor::labelColor(),
        );
        y += 20.0;
        let total = m.total.max(1) as f64;
        stacked_bar(
            NSRect::new(NSPoint::new(x, y), NSSize::new(w, 8.0)),
            &[
                (m.app as f64 / total, NSColor::systemBlueColor()),
                (m.wired as f64 / total, NSColor::systemOrangeColor()),
                (m.compressed as f64 / total, NSColor::systemPurpleColor()),
                (m.cached as f64 / total, NSColor::tertiaryLabelColor()),
            ],
        );
        y += 15.0;
        // 2 x 2 legend: label left, value right within each column.
        let legend = [
            ("App", m.app, NSColor::systemBlueColor()),
            ("Compressed", m.compressed, NSColor::systemPurpleColor()),
            ("Wired", m.wired, NSColor::systemOrangeColor()),
            ("Cached", m.cached, NSColor::tertiaryLabelColor()),
        ];
        let col_w = (w - 18.0) / 2.0;
        for (i, (label, bytes, color)) in legend.into_iter().enumerate() {
            let cx = x + (i % 2) as f64 * (col_w + 18.0);
            let cy = y + (i / 2) as f64 * 16.0;
            fill_oval(
                NSRect::new(NSPoint::new(cx, cy + 4.5), NSSize::new(6.0, 6.0)),
                &color,
            );
            d.text(
                label,
                cx + 10.0,
                cy,
                &d.small,
                &NSColor::secondaryLabelColor(),
            );
            d.text_right(
                &format::bytes_short(bytes),
                cx + col_w,
                cy,
                &d.small_value,
                &NSColor::labelColor(),
            );
        }
        y += 38.0;
        let (pcolor, plabel) = pressure_style(m.pressure);
        d.text("Pressure", x, y, &d.small, &NSColor::secondaryLabelColor());
        let ptext = format!("{} · {:.0}%", plabel, m.pressure_pct);
        let pw = d.measure(&ptext, &d.small_value).width;
        fill_oval(
            NSRect::new(
                NSPoint::new(x + w - pw - 12.0, y + 4.5),
                NSSize::new(7.0, 7.0),
            ),
            &pcolor,
        );
        d.text_right(&ptext, x + w, y, &d.small_value, &NSColor::labelColor());
        y += 19.0;
        d.text("Swap", x, y, &d.small, &NSColor::secondaryLabelColor());
        let swap_text = if snap.swap.total > 0 {
            format!(
                "{} of {}",
                format::bytes_short(snap.swap.used),
                format::bytes_short(snap.swap.total)
            )
        } else {
            "Not in use".to_string()
        };
        d.text_right(&swap_text, x + w, y, &d.small_value, &NSColor::labelColor());
        y += 18.0;
        let swap_frac = if snap.swap.total > 0 {
            snap.swap.used as f64 / snap.swap.total as f64
        } else {
            0.0
        };
        stacked_bar(
            NSRect::new(NSPoint::new(x, y), NSSize::new(w, 4.0)),
            &[(swap_frac, NSColor::systemPinkColor())],
        );
        y += 18.0;

        // Disk
        let v = &snap.volume;
        d.section("DISK", x, y);
        d.text_right(
            &v.name,
            x + w,
            y - 1.0,
            &d.small,
            &NSColor::secondaryLabelColor(),
        );
        y += 20.0;
        let used_frac = if v.total > 0 {
            v.used() as f64 / v.total as f64
        } else {
            0.0
        };
        let disk_color = if used_frac > 0.92 {
            NSColor::systemOrangeColor()
        } else {
            NSColor::systemTealColor()
        };
        stacked_bar(
            NSRect::new(NSPoint::new(x, y), NSSize::new(w, 8.0)),
            &[(used_frac, disk_color)],
        );
        y += 15.0;
        let avail = format!("{} available", format::bytes_short(v.available));
        let aw = d.text(&avail, x, y, &d.small_value, &NSColor::labelColor());
        d.text(
            &format!(" of {}", format::bytes_short(v.total)),
            x + aw,
            y,
            &d.small,
            &NSColor::secondaryLabelColor(),
        );
        if reclaimable > 0 {
            d.text_right(
                &format!("{} reclaimable", format::bytes_short(reclaimable)),
                x + w,
                y,
                &d.small,
                &NSColor::systemPinkColor(),
            );
        }
        y += 18.0;
        let io = format!(
            "Read {}   Write {}",
            format::rate(snap.disk_io.read_per_sec),
            format::rate(snap.disk_io.write_per_sec)
        );
        d.text(&io, x, y, &d.small, &NSColor::secondaryLabelColor());
        y += 28.0;

        // Network & GPU
        d.section("NETWORK", x, y);
        let net = format!(
            "↓ {}   ↑ {}",
            format::rate(snap.network.read_per_sec),
            format::rate(snap.network.write_per_sec)
        );
        d.text_right(&net, x + w, y - 1.0, &d.small_value, &NSColor::labelColor());
        y += 19.0;
        if let Some(gpu) = snap.gpu {
            d.section("GPU", x, y);
            d.text_right(
                &format::percent(gpu),
                x + w,
                y - 1.0,
                &d.small_value,
                &NSColor::labelColor(),
            );
        }
        y += 27.0;

        // Top apps
        d.section("TOP APPS", x, y);
        d.text_right(
            "CPU   MEMORY",
            x + w,
            y,
            &d.tiny_caps,
            &NSColor::tertiaryLabelColor(),
        );
        y += 20.0;
        for app in &apps {
            let icon = self.icon(app);
            unsafe {
                icon.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                    NSRect::new(NSPoint::new(x - 1.0, y), NSSize::new(17.0, 17.0)),
                    NSRect::ZERO,
                    NSCompositingOperation::SourceOver,
                    1.0,
                    true,
                    None,
                );
            }
            let name_w = w - 22.0 - 112.0;
            d.text_clipped(
                &app.name,
                x + 22.0,
                y + 0.5,
                name_w,
                &d.small,
                &NSColor::labelColor(),
            );
            d.text_right(
                &format!("{:.0}%", app.cpu),
                x + w - 64.0,
                y + 0.5,
                &d.small_value,
                &NSColor::labelColor(),
            );
            d.text_right(
                &format::bytes_short(app.memory),
                x + w,
                y + 0.5,
                &d.small,
                &NSColor::secondaryLabelColor(),
            );
            y += 21.0;
        }
    }
}

fn content_height() -> f64 {
    // Mirrors the vertical advances in `draw`.
    let summary = 8.0 + 21.0 + 26.0;
    let cpu = 19.0 + 32.0 + 28.0 + 17.0 + 28.0;
    let memory = 20.0 + 15.0 + 38.0 + 19.0 + 18.0 + 18.0;
    let disk = 20.0 + 15.0 + 18.0 + 28.0;
    let net = 19.0 + 27.0;
    let apps = 20.0 + 21.0 * APP_ROWS as f64 + 4.0;
    summary + cpu + memory + disk + net + apps
}

fn pressure_style(p: Pressure) -> (Retained<NSColor>, &'static str) {
    match p {
        Pressure::Normal => (NSColor::systemGreenColor(), "Normal"),
        Pressure::Warning => (NSColor::systemYellowColor(), "Elevated"),
        Pressure::Critical => (NSColor::systemRedColor(), "Critical"),
    }
}

fn core_groups(cores: &[CoreKind], snap: &Snapshot) -> Vec<(&'static str, usize, f32)> {
    let mut out: Vec<(&'static str, usize, f32)> = Vec::new();
    for (i, kind) in cores.iter().enumerate() {
        let label = match kind {
            CoreKind::Efficiency => "Efficiency",
            CoreKind::Performance => "Performance",
            CoreKind::Standard => "Cores",
        };
        let load = snap.cpu.per_core.get(i).copied().unwrap_or(0.0);
        match out.last_mut() {
            Some(last) if last.0 == label => {
                last.2 = (last.2 * last.1 as f32 + load) / (last.1 + 1) as f32;
                last.1 += 1;
            }
            _ => out.push((label, 1, load)),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Drawing helpers (AppKit, flipped coordinates)

struct Draw {
    headline: Retained<NSFont>,
    value: Retained<NSFont>,
    small: Retained<NSFont>,
    small_value: Retained<NSFont>,
    tiny: Retained<NSFont>,
    tiny_caps: Retained<NSFont>,
}

impl Draw {
    fn new() -> Self {
        unsafe {
            Self {
                headline: NSFont::systemFontOfSize_weight(14.0, NSFontWeightSemibold),
                value: NSFont::monospacedDigitSystemFontOfSize_weight(13.0, NSFontWeightSemibold),
                small: NSFont::monospacedDigitSystemFontOfSize_weight(11.0, NSFontWeightRegular),
                small_value: NSFont::monospacedDigitSystemFontOfSize_weight(
                    11.0,
                    NSFontWeightMedium,
                ),
                tiny: NSFont::monospacedDigitSystemFontOfSize_weight(9.5, NSFontWeightRegular),
                tiny_caps: NSFont::systemFontOfSize_weight(9.0, NSFontWeightSemibold),
            }
        }
    }

    fn attrs(
        font: &NSFont,
        color: &NSColor,
        kern: f64,
    ) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
        let kern = NSNumber::new_f64(kern);
        let keys: [&NSAttributedStringKey; 3] = unsafe {
            [
                NSFontAttributeName,
                NSForegroundColorAttributeName,
                NSKernAttributeName,
            ]
        };
        let values: [&AnyObject; 3] = [font.as_ref(), color.as_ref(), kern.as_ref()];
        NSDictionary::from_slices(&keys, &values)
    }

    fn measure(&self, s: &str, font: &NSFont) -> NSSize {
        let attrs = Self::attrs(font, &NSColor::labelColor(), 0.0);
        unsafe { NSString::from_str(s).sizeWithAttributes(Some(&attrs)) }
    }

    /// Draw text with its top-left at (x, y); returns its width.
    fn text(&self, s: &str, x: f64, y: f64, font: &NSFont, color: &NSColor) -> f64 {
        let ns = NSString::from_str(s);
        let attrs = Self::attrs(font, color, 0.0);
        unsafe {
            let size = ns.sizeWithAttributes(Some(&attrs));
            ns.drawAtPoint_withAttributes(NSPoint::new(x, y), Some(&attrs));
            size.width
        }
    }

    fn text_right(&self, s: &str, right: f64, y: f64, font: &NSFont, color: &NSColor) {
        let ns = NSString::from_str(s);
        let attrs = Self::attrs(font, color, 0.0);
        unsafe {
            let size = ns.sizeWithAttributes(Some(&attrs));
            ns.drawAtPoint_withAttributes(NSPoint::new(right - size.width, y), Some(&attrs));
        }
    }

    /// Single line, truncated with an ellipsis to `max_w`.
    fn text_clipped(&self, s: &str, x: f64, y: f64, max_w: f64, font: &NSFont, color: &NSColor) {
        let mut text = s.to_string();
        if self.measure(&text, font).width > max_w {
            while !text.is_empty() && self.measure(&format!("{text}…"), font).width > max_w {
                text.pop();
            }
            text = format!("{}…", text.trim_end());
        }
        self.text(&text, x, y, font, color);
    }

    fn section(&self, title: &str, x: f64, y: f64) {
        let ns = NSString::from_str(title);
        let attrs = Self::attrs(&self.tiny_caps, &NSColor::secondaryLabelColor(), 0.6);
        unsafe { ns.drawAtPoint_withAttributes(NSPoint::new(x, y + 1.0), Some(&attrs)) };
    }
}

fn fill_round(rect: NSRect, radius: f64, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius).fill();
}

fn fill_oval(rect: NSRect, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithOvalInRect(rect).fill();
}

fn track_color() -> Retained<NSColor> {
    NSColor::labelColor().colorWithAlphaComponent(0.09)
}

/// A rounded bar made of consecutive coloured segments over a faint track.
fn stacked_bar(rect: NSRect, segments: &[(f64, Retained<NSColor>)]) {
    let r = rect.size.height / 2.0;
    fill_round(rect, r, &track_color());
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, r, r);
    NSGraphicsContext::saveGraphicsState_class();
    path.addClip();
    let mut x = rect.origin.x;
    for (frac, color) in segments {
        let w = (frac.clamp(0.0, 1.0) * rect.size.width).max(0.0);
        if w <= 0.0 {
            continue;
        }
        color.setFill();
        NSBezierPath::fillRect(NSRect::new(
            NSPoint::new(x, rect.origin.y),
            NSSize::new(w, rect.size.height),
        ));
        x += w;
    }
    NSGraphicsContext::restoreGraphicsState_class();
}

fn sparkline(rect: NSRect, history: &History, color: &NSColor) {
    let values: Vec<f32> = history.cpu.iter().collect();
    let cap = 60usize;
    let start = values.len().saturating_sub(cap);
    let values = &values[start..];
    fill_round(
        rect,
        4.0,
        &NSColor::labelColor().colorWithAlphaComponent(0.04),
    );
    if values.len() < 2 {
        return;
    }
    let step = rect.size.width / (cap - 1) as f64;
    let x0 = rect.origin.x + rect.size.width - step * (values.len() - 1) as f64;
    let y_of = |v: f32| {
        rect.origin.y + rect.size.height
            - (v.clamp(0.0, 1.0) as f64) * (rect.size.height - 2.0)
            - 1.0
    };

    let area = NSBezierPath::bezierPath();
    area.moveToPoint(NSPoint::new(x0, rect.origin.y + rect.size.height));
    for (i, v) in values.iter().enumerate() {
        area.lineToPoint(NSPoint::new(x0 + step * i as f64, y_of(*v)));
    }
    area.lineToPoint(NSPoint::new(
        x0 + step * (values.len() - 1) as f64,
        rect.origin.y + rect.size.height,
    ));
    area.closePath();
    color.colorWithAlphaComponent(0.18).setFill();
    area.fill();

    let line = NSBezierPath::bezierPath();
    for (i, v) in values.iter().enumerate() {
        let p = NSPoint::new(x0 + step * i as f64, y_of(*v));
        if i == 0 {
            line.moveToPoint(p)
        } else {
            line.lineToPoint(p)
        }
    }
    line.setLineWidth(1.5);
    line.setLineJoinStyle(objc2_app_kit::NSLineJoinStyle::Round);
    color.setStroke();
    line.stroke();
}

fn core_bars(rect: NSRect, snap: &Snapshot, cores: &[CoreKind]) {
    let n = cores.len().max(1);
    let groups = cores.windows(2).filter(|w| w[0] != w[1]).count() + 1;
    let gap = 6.0;
    let bar_gap = 2.5;
    let slot = (rect.size.width - gap * (groups as f64 - 1.0)) / n as f64;
    let mut x = rect.origin.x;
    for (i, kind) in cores.iter().enumerate() {
        if i > 0 && cores[i - 1] != *kind {
            x += gap;
        }
        let bar = NSRect::new(
            NSPoint::new(x, rect.origin.y),
            NSSize::new(slot - bar_gap, rect.size.height),
        );
        fill_round(bar, 2.0, &track_color());
        let load = snap
            .cpu
            .per_core
            .get(i)
            .copied()
            .unwrap_or(0.0)
            .clamp(0.0, 1.0) as f64;
        if load > 0.005 {
            let h = (load * rect.size.height).max(2.0);
            let fill = NSRect::new(
                NSPoint::new(bar.origin.x, rect.origin.y + rect.size.height - h),
                NSSize::new(bar.size.width, h),
            );
            let color = match kind {
                CoreKind::Efficiency => NSColor::systemTealColor(),
                _ => NSColor::systemBlueColor(),
            };
            let color = if load > 0.9 {
                NSColor::systemOrangeColor()
            } else {
                color
            };
            fill_round(fill, 2.0, &color);
        }
        x += slot;
    }
}

/// Render the stats view to PNG bytes at 2x (development aid).
pub fn render_stats_png(mtm: MainThreadMarker, shared: Arc<Shared>, dark: bool) -> Option<Vec<u8>> {
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace,
    };
    let view = StatsView::new(mtm, shared);
    let name = unsafe {
        if dark {
            NSAppearanceNameDarkAqua
        } else {
            NSAppearanceNameAqua
        }
    };
    let appearance = NSAppearance::appearanceNamed(name)?;
    view.setAppearance(Some(&appearance));
    let bounds = view.bounds();
    let (w, h) = (
        (bounds.size.width * 2.0) as isize,
        (bounds.size.height * 2.0) as isize,
    );
    unsafe {
        let rep = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(), std::ptr::null_mut(), w, h, 8, 4, true, false, NSDeviceRGBColorSpace, 0, 0,
        )?;
        rep.setSize(bounds.size);
        let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&ctx));
        let bg = if dark {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.17, 0.17, 0.18, 1.0)
        } else {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.95, 0.95, 0.95, 1.0)
        };
        bg.setFill();
        NSBezierPath::fillRect(bounds);
        NSGraphicsContext::restoreGraphicsState_class();
        view.displayRectIgnoringOpacity_inContext(bounds, &ctx);
        ctx.flushGraphics();
        let data = rep
            .representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())?;
        Some(data.to_vec())
    }
}
