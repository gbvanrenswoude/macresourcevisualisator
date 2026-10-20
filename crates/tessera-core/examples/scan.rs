//! `cargo run --release -p tessera-core --example scan -- <path>`
//! Scans a folder (or the whole data volume with `--volume`) and prints what
//! the index looks like.

use std::time::Instant;

use tessera_core::disk::{self, ScanConfig, ScanProgress};
use tessera_core::format;

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let mut cfg = if arg == "--volume" {
        ScanConfig::data_volume()
    } else {
        ScanConfig::for_path(&arg)
    };
    if let Some(ex) = std::env::var_os("EXCLUDE") {
        cfg.exclude = std::env::split_paths(&ex).collect();
    }
    if std::env::var_os("INTERACTIVE").is_some() {
        cfg.priority = disk::ScanPriority::Interactive;
    }
    let progress = ScanProgress::default();
    let t = Instant::now();
    let index = disk::scan(&cfg, &progress).expect("scan");
    let elapsed = t.elapsed();
    let out = std::env::temp_dir().join("tessera-example-index.bin");
    index.save(&out).unwrap();
    let file_size = std::fs::metadata(&out).unwrap().len();
    println!(
        "scanned {} in {:.2?}: {} files, {} dirs, {} denied -> {} nodes, index {}",
        format::bytes(index.total()),
        elapsed,
        format::count(index.stats.files),
        format::count(index.stats.dirs),
        index.stats.denied,
        index.len(),
        format::bytes(file_size)
    );
    let t = Instant::now();
    let loaded = disk::DiskIndex::load(&out).unwrap();
    println!("reload in {:.2?}", t.elapsed());
    let home = tessera_core::paths::home_dir();
    let ov = disk::overview::build(&loaded, loaded.total(), &home);
    for g in ov.groups.iter().take(12) {
        let _ = g.category;
        println!("{:>10}  {}", format::bytes(g.size), g.category.label());
        for i in g.items.iter().take(7) {
            println!(
                "{:>22}  {}  {}{}",
                format::bytes(i.size),
                i.label,
                i.detail.as_deref().unwrap_or(""),
                if i.reclaimable > 0 {
                    format!("  [{} reclaimable]", format::bytes(i.reclaimable))
                } else {
                    String::new()
                }
            );
        }
    }
}
