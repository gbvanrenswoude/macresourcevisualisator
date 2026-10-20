//! Time each part of a metrics sample.
use std::time::Instant;
use tessera_core::metrics::{ProcessSampler, Sampler};

fn main() {
    let mut s = Sampler::new();
    let n = 50;
    let t = Instant::now();
    for _ in 0..n {
        let _ = s.sample();
    }
    println!("full sample: {:?}", t.elapsed() / n);
    let mut p = ProcessSampler::new();
    p.sample();
    let t = Instant::now();
    for _ in 0..10 {
        let _ = p.sample();
    }
    println!("process sample: {:?}", t.elapsed() / 10);
    tessera_core::metrics::bench_parts(n);
}
