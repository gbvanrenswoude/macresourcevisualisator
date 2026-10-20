//! The eframe application: owns the menu bar item and the dashboard window,
//! which starts hidden and is shown on demand.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use egui::{Key, Modifiers, ViewportCommand};
use objc2::MainThreadMarker;
use tessera_core::disk::ScanPriority;

use crate::state::Shared;
use crate::statusbar::{self, StatusBar};
use crate::ui::{Dashboard, theme};
use crate::{debug, macos};

pub struct TesseraApp {
    shared: Arc<Shared>,
    _status: StatusBar,
    dashboard: Dashboard,
    visible: bool,
    /// Whether `ui` draws the dashboard this frame.
    draw: bool,
    first_frame: bool,
    quitting: bool,
    last_scan_check: Option<Instant>,
    screenshot: Option<debug::Screenshot>,
}

impl TesseraApp {
    pub fn new(cc: &eframe::CreationContext<'_>, shared: Arc<Shared>, show_window: bool) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        match std::env::var("TESSERA_THEME").as_deref() {
            Ok("dark") => cc.egui_ctx.set_theme(egui::Theme::Dark),
            Ok("light") => cc.egui_ctx.set_theme(egui::Theme::Light),
            _ => {}
        }
        macos::apply_forced_appearance();
        let _ = shared.egui.set(cc.egui_ctx.clone());
        let mtm = MainThreadMarker::new().expect("eframe runs the app on the main thread");
        let status = StatusBar::new(mtm, shared.clone());

        let screenshot = debug::Screenshot::from_env();
        let deferred = debug::show_later_from_env(&shared);
        if !deferred && (screenshot.is_some() || (show_window && !macos::launched_as_login_item()))
        {
            shared.show_dashboard.store(true, Ordering::SeqCst);
        }
        debug::menu_snapshot_from_env(mtm, &shared);
        debug::capture_windows_from_env(&shared);
        cc.egui_ctx.request_repaint();

        Self {
            shared,
            _status: status,
            dashboard: Dashboard::new(),
            visible: false,
            draw: false,
            first_frame: true,
            quitting: false,
            last_scan_check: None,
            screenshot,
        }
    }

    fn show(&mut self, ctx: &egui::Context) {
        macos::set_dock_visible(true);
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        macos::activate();
        self.visible = true;
        self.shared.dashboard_visible.store(true, Ordering::SeqCst);
        ctx.request_repaint();
    }

    fn hide(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        macos::set_dock_visible(false);
        self.visible = false;
        self.shared.dashboard_visible.store(false, Ordering::SeqCst);
    }

    /// Scan when there is no index yet, or the index has gone stale.
    fn maybe_scan(&mut self) {
        let due = self
            .last_scan_check
            .is_none_or(|t| t.elapsed() > Duration::from_secs(600));
        if !due {
            return;
        }
        self.last_scan_check = Some(Instant::now());
        let hours = self
            .shared
            .settings
            .lock()
            .map(|s| s.rescan_hours)
            .unwrap_or(6)
            .max(1);
        match self.shared.disk.index_age() {
            None => statusbar::start_scan(&self.shared, ScanPriority::Interactive),
            Some(age) if age > Duration::from_secs(hours * 3600) => {
                statusbar::start_scan(&self.shared, ScanPriority::Background)
            }
            _ => {}
        }
    }
}

impl eframe::App for TesseraApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.shared.quit.load(Ordering::SeqCst) && !self.quitting {
            self.quitting = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
            macos::quit();
            return;
        }
        let first_frame = std::mem::take(&mut self.first_frame);
        let was_visible = self.visible;
        if self.shared.show_dashboard.swap(false, Ordering::SeqCst) {
            self.show(ctx);
        } else if first_frame {
            // eframe shows the root window once it has painted the first
            // frame, whatever the viewport builder said. Starting in the menu
            // bar only, put it straight back (this command is applied right
            // after that, in the same pass).
            self.hide(ctx);
        }
        // Not only while `visible`: closing the window must never quit.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.hide(ctx);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::W)) {
            self.hide(ctx);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Q)) {
            self.shared.request_quit();
        }
        // Keep the last frame intact while the window is going away.
        self.draw = self.visible || was_visible;
        self.maybe_scan();
        // Check scan freshness again later even when nothing else happens.
        ctx.request_repaint_after(Duration::from_secs(60));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // eframe only stops calling this once macOS reports the window
        // occluded, which never happens when it was hidden in its first
        // frame. Drawing nothing also stops the dashboard's animations from
        // asking for more frames.
        if !self.draw {
            return;
        }
        self.dashboard.ui(ui, &self.shared);
        if let Some(shot) = &mut self.screenshot {
            shot.tick(ui.ctx());
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}
