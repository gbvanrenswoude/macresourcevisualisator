//! Background thread that samples system metrics once a second.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tessera_core::metrics::{ProcessSampler, Sampler};

use crate::state::Shared;

pub fn spawn(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("tessera-sampler".into())
        .spawn(move || run(shared))
        .expect("spawn sampler thread");
}

fn run(shared: Arc<Shared>) {
    crate::macos::set_thread_qos_utility();
    let mut sampler = Sampler::new();
    sampler.set_volume_probe(crate::macos::volume_info);
    let mut processes = ProcessSampler::new();
    let mut last_processes = Instant::now() - Duration::from_secs(60);
    let mut last_volume: Option<Instant> = None;
    processes.sample(); // prime CPU deltas
    std::thread::sleep(Duration::from_millis(250));

    loop {
        let started = Instant::now();
        let snapshot = sampler.sample();
        // Per-app usage costs a couple of syscalls per process; only sample
        // it often while somebody is looking.
        let every = if shared.ui_active() {
            Duration::from_secs(2)
        } else {
            Duration::from_secs(8)
        };
        let apps = (started.duration_since(last_processes) >= every).then(|| {
            last_processes = started;
            processes.sample()
        });
        if last_volume.is_none_or(|t| started.duration_since(t) > Duration::from_secs(30)) {
            last_volume = Some(started);
            shared.disk.set_volume(&snapshot.volume);
        }
        if let Ok(mut m) = shared.metrics.lock() {
            m.push(snapshot);
            if let Some(apps) = apps {
                m.apps = apps;
            }
        }
        if shared.dashboard_visible.load(Ordering::Relaxed) {
            shared.wake();
        }
        let elapsed = started.elapsed();
        std::thread::sleep(Duration::from_secs(1).saturating_sub(elapsed));
    }
}
