//! Owns the disk index: loads the cached one at launch, runs scans on a
//! background thread, and derives the semantic overview.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tessera_core::disk::{self, DiskIndex, Overview, ScanConfig, ScanPriority, ScanProgress};
use tessera_core::metrics::Volume;
use tessera_core::paths;

pub struct ActiveScan {
    pub progress: Arc<ScanProgress>,
    pub started: Instant,
}

#[derive(Default)]
struct Inner {
    index: Option<Arc<DiskIndex>>,
    overview: Option<Arc<Overview>>,
    scan: Option<Arc<ActiveScan>>,
    error: Option<String>,
    generation: u64,
    volume: Volume,
}

/// A cheap, cloneable view of the current disk state for the UI.
#[derive(Clone, Default)]
pub struct DiskView {
    pub index: Option<Arc<DiskIndex>>,
    pub overview: Option<Arc<Overview>>,
    pub scan: Option<Arc<ActiveScan>>,
    pub error: Option<String>,
    pub generation: u64,
}

pub struct DiskService {
    inner: Mutex<Inner>,
}

impl DiskService {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
        })
    }

    pub fn view(&self) -> DiskView {
        let i = self.inner.lock().unwrap();
        DiskView {
            index: i.index.clone(),
            overview: i.overview.clone(),
            scan: i.scan.clone(),
            error: i.error.clone(),
            generation: i.generation,
        }
    }

    /// Load the index saved by the previous run, if any.
    pub fn load_cached(&self) -> bool {
        let Ok(index) = DiskIndex::load(&paths::index_file()) else {
            return false;
        };
        let mut i = self.inner.lock().unwrap();
        let volume = i.volume.clone();
        i.overview = Some(Arc::new(build_overview(&index, &volume)));
        i.index = Some(Arc::new(index));
        i.generation += 1;
        true
    }

    #[cfg(test)]
    pub fn set_index(&self, index: DiskIndex) {
        let mut i = self.inner.lock().unwrap();
        let volume = i.volume.clone();
        i.overview = Some(Arc::new(build_overview(&index, &volume)));
        i.index = Some(Arc::new(index));
        i.generation += 1;
    }

    /// Keep the volume numbers fresh so the "hidden space" block is right.
    pub fn set_volume(&self, volume: &Volume) {
        let mut i = self.inner.lock().unwrap();
        let changed = i.volume.total != volume.total
            || i.volume.available.abs_diff(volume.available) > 512 << 20;
        i.volume = volume.clone();
        if changed && let Some(index) = i.index.clone() {
            i.overview = Some(Arc::new(build_overview(&index, volume)));
            i.generation += 1;
        }
    }

    pub fn index_age(&self) -> Option<Duration> {
        self.inner
            .lock()
            .unwrap()
            .index
            .as_ref()
            .map(|i| disk::scan::age(i.scanned_at))
    }

    /// Start a scan unless one is already running. `on_done` runs on the
    /// scanner thread once the new index is in place.
    pub fn start_scan(
        self: &Arc<Self>,
        priority: ScanPriority,
        exclude: Vec<PathBuf>,
        on_done: impl FnOnce() + Send + 'static,
    ) -> bool {
        let progress = Arc::new(ScanProgress::default());
        {
            let mut i = self.inner.lock().unwrap();
            if i.scan.is_some() {
                return false;
            }
            i.scan = Some(Arc::new(ActiveScan {
                progress: progress.clone(),
                started: Instant::now(),
            }));
            i.error = None;
        }
        let this = self.clone();
        std::thread::Builder::new()
            .name("tessera-scan".into())
            .spawn(move || {
                let mut cfg = scan_config();
                cfg.priority = priority;
                cfg.exclude.extend(exclude);
                let result = disk::scan(&cfg, &progress);
                {
                    let mut i = this.inner.lock().unwrap();
                    i.scan = None;
                    match result {
                        Ok(index) => {
                            if let Err(e) = index.save(&paths::index_file()) {
                                eprintln!("tessera: could not save disk index: {e}");
                            }
                            let overview = build_overview(&index, &i.volume);
                            i.overview = Some(Arc::new(overview));
                            i.index = Some(Arc::new(index));
                            i.generation += 1;
                        }
                        Err(e) => i.error = Some(e.to_string()),
                    }
                }
                on_done();
            })
            .expect("spawn scan thread");
        true
    }
}

fn scan_config() -> ScanConfig {
    // TESSERA_SCAN_ROOT lets development builds scan a small folder instead
    // of the whole volume.
    let mut cfg = match std::env::var_os("TESSERA_SCAN_ROOT") {
        Some(root) => ScanConfig::for_path(root),
        None => ScanConfig::data_volume(),
    };
    if let Some(ex) = std::env::var_os("TESSERA_EXCLUDE") {
        cfg.exclude.extend(std::env::split_paths(&ex));
    }
    cfg
}

fn build_overview(index: &DiskIndex, volume: &Volume) -> Overview {
    let used = if volume.total > 0 {
        volume.used()
    } else {
        index.total()
    };
    disk::overview::build(index, used, &paths::home_dir())
}
