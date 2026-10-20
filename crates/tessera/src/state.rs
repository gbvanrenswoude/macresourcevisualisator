//! State shared between the sampler thread, the disk scanner, the menu bar
//! item and the dashboard window.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tessera_core::metrics::{AppUsage, Pressure, Snapshot, SystemInfo};

use crate::disk::DiskService;
use crate::settings::Settings;

/// Seconds of history kept for the charts.
pub const HISTORY: usize = 120;

#[derive(Clone, Debug)]
pub struct Ring {
    buf: VecDeque<f32>,
    cap: usize,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap),
            cap,
        }
    }

    pub fn push(&mut self, v: f32) {
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(v);
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = f32> + ExactSizeIterator + '_ {
        self.buf.iter().copied()
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn max(&self) -> f32 {
        self.buf.iter().copied().fold(0.0, f32::max)
    }
}

#[derive(Clone, Debug)]
pub struct History {
    pub cpu: Ring,
    pub cpu_user: Ring,
    pub cores: Vec<Ring>,
    pub memory: Ring,
    pub pressure: Ring,
    pub swap: Ring,
    pub net_rx: Ring,
    pub net_tx: Ring,
    pub disk_read: Ring,
    pub disk_write: Ring,
    pub gpu: Ring,
}

impl History {
    fn new(cores: usize) -> Self {
        let r = || Ring::new(HISTORY);
        Self {
            cpu: r(),
            cpu_user: r(),
            cores: (0..cores).map(|_| r()).collect(),
            memory: r(),
            pressure: r(),
            swap: r(),
            net_rx: r(),
            net_tx: r(),
            disk_read: r(),
            disk_write: r(),
            gpu: r(),
        }
    }

    fn push(&mut self, s: &Snapshot) {
        self.cpu.push(s.cpu.total);
        self.cpu_user.push(s.cpu.user);
        if self.cores.len() != s.cpu.per_core.len() {
            self.cores = (0..s.cpu.per_core.len())
                .map(|_| Ring::new(HISTORY))
                .collect();
        }
        for (ring, v) in self.cores.iter_mut().zip(&s.cpu.per_core) {
            ring.push(*v);
        }
        let total = s.memory.total.max(1) as f32;
        self.memory.push(s.memory.used as f32 / total);
        self.pressure.push(s.memory.pressure_pct / 100.0);
        self.swap.push(s.swap.used as f32);
        self.net_rx.push(s.network.read_per_sec as f32);
        self.net_tx.push(s.network.write_per_sec as f32);
        self.disk_read.push(s.disk_io.read_per_sec as f32);
        self.disk_write.push(s.disk_io.write_per_sec as f32);
        self.gpu.push(s.gpu.unwrap_or(0.0));
    }
}

pub struct Metrics {
    pub latest: Snapshot,
    pub history: History,
    pub apps: Vec<AppUsage>,
    pub samples: u64,
}

impl Metrics {
    pub fn push(&mut self, s: Snapshot) {
        // The very first sample has no delta yet, so it reads as idle; skip it.
        if self.samples > 0 {
            self.history.push(&s);
        }
        self.latest = s;
        self.samples += 1;
    }
}

pub struct Shared {
    pub info: SystemInfo,
    pub metrics: Mutex<Metrics>,
    pub disk: Arc<DiskService>,
    pub settings: Mutex<Settings>,
    pub dashboard_visible: AtomicBool,
    pub menu_open: AtomicBool,
    /// Set by the menu, consumed by the dashboard's logic tick.
    pub show_dashboard: AtomicBool,
    pub quit: AtomicBool,
    pub egui: OnceLock<egui::Context>,
}

impl Shared {
    pub fn new(settings: Settings) -> Arc<Self> {
        let info = SystemInfo::read();
        let cores = info.cores.len();
        Arc::new(Self {
            info,
            metrics: Mutex::new(Metrics {
                latest: Snapshot::default(),
                history: History::new(cores),
                apps: Vec::new(),
                samples: 0,
            }),
            disk: DiskService::new(),
            settings: Mutex::new(settings),
            dashboard_visible: AtomicBool::new(false),
            menu_open: AtomicBool::new(false),
            show_dashboard: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            egui: OnceLock::new(),
        })
    }

    /// Wake the dashboard so it repaints (or, while hidden, runs its logic).
    pub fn wake(&self) {
        if let Some(ctx) = self.egui.get() {
            ctx.request_repaint();
        }
    }

    pub fn ui_active(&self) -> bool {
        self.dashboard_visible.load(Ordering::Relaxed) || self.menu_open.load(Ordering::Relaxed)
    }

    pub fn request_dashboard(&self) {
        self.show_dashboard.store(true, Ordering::SeqCst);
        self.wake();
    }

    pub fn request_quit(&self) {
        self.quit.store(true, Ordering::SeqCst);
        self.wake();
    }
}

/// One-word answer to "how busy is my Mac right now?".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Busyness {
    Idle,
    Light,
    Busy,
    VeryBusy,
    MemoryBound,
}

impl Busyness {
    pub fn of(s: &Snapshot) -> Self {
        let cpu = s.cpu.total;
        match s.memory.pressure {
            Pressure::Critical => Busyness::MemoryBound,
            _ if cpu >= 0.75 => Busyness::VeryBusy,
            Pressure::Warning => Busyness::Busy,
            _ if cpu >= 0.40 => Busyness::Busy,
            _ if cpu >= 0.12 => Busyness::Light,
            _ => Busyness::Idle,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Busyness::Idle => "Idle",
            Busyness::Light => "Light load",
            Busyness::Busy => "Busy",
            Busyness::VeryBusy => "Very busy",
            Busyness::MemoryBound => "Memory critical",
        }
    }

    /// 0 = calm … 3 = alarming, used to pick a colour.
    pub fn severity(self) -> u8 {
        match self {
            Busyness::Idle | Busyness::Light => 0,
            Busyness::Busy => 1,
            Busyness::VeryBusy => 2,
            Busyness::MemoryBound => 3,
        }
    }

    pub fn summary(s: &Snapshot) -> String {
        let mut parts = vec![format!("CPU {:.0}%", s.cpu.total * 100.0)];
        match s.memory.pressure {
            Pressure::Normal => parts.push("memory OK".into()),
            Pressure::Warning => parts.push("pressure elevated".into()),
            Pressure::Critical => parts.push("pressure critical".into()),
        }
        if s.swap.used > 1 << 30 {
            parts.push(format!(
                "{} in swap",
                tessera_core::format::bytes_short(s.swap.used)
            ));
        }
        parts.join(" · ")
    }
}
