//! Live system metrics: CPU (overall and per core), memory and memory
//! pressure, swap, disk space and I/O, network throughput, GPU load.
//!
//! Everything is read straight from the kernel (Mach host calls, sysctl,
//! IOKit), so a full sample costs well under a millisecond.

use std::ffi::c_int;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::ffi::{self, *};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoreKind {
    Efficiency,
    Performance,
    /// Intel Macs (no core clusters).
    Standard,
}

impl CoreKind {
    pub fn short(self) -> &'static str {
        match self {
            CoreKind::Efficiency => "E",
            CoreKind::Performance => "P",
            CoreKind::Standard => "C",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Pressure {
    #[default]
    Normal,
    Warning,
    Critical,
}

impl Pressure {
    pub fn label(self) -> &'static str {
        match self {
            Pressure::Normal => "Normal",
            Pressure::Warning => "Elevated",
            Pressure::Critical => "Critical",
        }
    }
}

/// Static facts about the machine.
#[derive(Clone, Debug, Default)]
pub struct SystemInfo {
    /// Marketing name, e.g. "MacBook Pro (14-inch, Nov 2023)".
    pub model_name: String,
    /// Model identifier, e.g. "Mac15,7".
    pub model_id: String,
    /// e.g. "Apple M3 Pro".
    pub chip: String,
    pub os_version: String,
    pub host_name: String,
    pub memory_total: u64,
    /// Kind of each logical CPU, indexed by CPU number.
    pub cores: Vec<CoreKind>,
}

impl SystemInfo {
    pub fn read() -> Self {
        let model_id = sysctl_string("hw.model").unwrap_or_default();
        let model_name = registry_property_bytes("IODeviceTree:/product", "product-name")
            .map(|b| {
                String::from_utf8_lossy(&b)
                    .trim_end_matches('\0')
                    .trim()
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| model_id.clone());
        let chip = sysctl_string("machdep.cpu.brand_string").unwrap_or_default();
        let os_version = sysctl_string("kern.osproductversion").unwrap_or_default();
        let memory_total = sysctl_u64("hw.memsize").unwrap_or(0);
        let host_name = host_name();
        let ncpu = sysctl_i32("hw.ncpu").unwrap_or(1).max(1) as usize;
        let cores = core_kinds(ncpu);
        Self {
            model_name,
            model_id,
            chip,
            os_version,
            host_name,
            memory_total,
            cores,
        }
    }

    pub fn efficiency_cores(&self) -> usize {
        self.cores
            .iter()
            .filter(|k| **k == CoreKind::Efficiency)
            .count()
    }

    pub fn performance_cores(&self) -> usize {
        self.cores
            .iter()
            .filter(|k| **k == CoreKind::Performance)
            .count()
    }
}

fn host_name() -> String {
    let mut buf = [0u8; 256];
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).into_owned();
    name.trim_end_matches(".local").replace('-', " ")
}

/// Work out which logical CPUs are efficiency vs performance cores. The
/// device tree knows for sure; fall back to "efficiency cores come first",
/// which holds on every Apple Silicon chip so far.
fn core_kinds(ncpu: usize) -> Vec<CoreKind> {
    let mut kinds = vec![CoreKind::Standard; ncpu];
    let mut found = 0;
    ffi::for_each_registry_child("IODeviceTree:/cpus", |props| unsafe {
        let id = cf_number_i64(dict_get(props, "logical-cpu-id"));
        let cluster = cf_data_bytes(dict_get(props, "cluster-type"));
        if let (Some(id), Some(cluster)) = (id, cluster) {
            let id = id as usize;
            if id < ncpu {
                kinds[id] = match cluster.first() {
                    Some(b'E') => CoreKind::Efficiency,
                    Some(b'P') => CoreKind::Performance,
                    _ => CoreKind::Standard,
                };
                found += 1;
            }
        }
    });
    if found == ncpu {
        return kinds;
    }
    let levels = sysctl_i32("hw.nperflevels").unwrap_or(1);
    if levels >= 2 {
        let p = sysctl_i32("hw.perflevel0.logicalcpu").unwrap_or(0).max(0) as usize;
        let e = sysctl_i32("hw.perflevel1.logicalcpu").unwrap_or(0).max(0) as usize;
        if p + e == ncpu {
            return (0..ncpu)
                .map(|i| {
                    if i < e {
                        CoreKind::Efficiency
                    } else {
                        CoreKind::Performance
                    }
                })
                .collect();
        }
    }
    vec![CoreKind::Standard; ncpu]
}

// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CpuLoad {
    /// 0..=1 across all cores.
    pub total: f32,
    pub user: f32,
    pub system: f32,
    /// 0..=1 per logical CPU.
    pub per_core: Vec<f32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Memory {
    pub total: u64,
    /// App + wired + compressed, like Activity Monitor's "Memory Used".
    pub used: u64,
    pub app: u64,
    pub wired: u64,
    pub compressed: u64,
    /// File cache + purgeable; reclaimable on demand.
    pub cached: u64,
    pub free: u64,
    pub pressure: Pressure,
    /// 0..=100, the inverse of the kernel's "memory free" percentage.
    pub pressure_pct: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Swap {
    pub total: u64,
    pub used: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Volume {
    pub name: String,
    pub total: u64,
    /// What Finder reports as available (includes purgeable space).
    pub available: u64,
    /// Strictly free blocks, without purgeable space.
    pub free: u64,
}

impl Volume {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Throughput {
    pub read_per_sec: f64,
    pub write_per_sec: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub taken: SystemTime,
    pub cpu: CpuLoad,
    pub memory: Memory,
    pub swap: Swap,
    pub volume: Volume,
    /// Network: read = received, write = sent.
    pub network: Throughput,
    pub disk_io: Throughput,
    pub gpu: Option<f32>,
    pub load: [f64; 3],
    pub uptime: Duration,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            taken: SystemTime::now(),
            cpu: CpuLoad::default(),
            memory: Memory::default(),
            swap: Swap::default(),
            volume: Volume::default(),
            network: Throughput::default(),
            disk_io: Throughput::default(),
            gpu: None,
            load: [0.0; 3],
            uptime: Duration::ZERO,
        }
    }
}

/// Stateful sampler: CPU usage and throughput are deltas between calls.
pub struct Sampler {
    prev_ticks: Option<Vec<[u32; CPU_STATE_MAX]>>,
    prev_net: Option<(Instant, u64, u64)>,
    prev_disk: Option<(Instant, u64, u64)>,
    page_size: u64,
    memory_total: u64,
    volume: Volume,
    volume_checked: Option<Instant>,
    volume_probe: Option<Box<dyn Fn() -> Option<Volume> + Send>>,
    probed_at: Option<Instant>,
    purgeable: u64,
    io: IoServices,
}

/// IOKit services we read every second, looked up once (and refreshed now
/// and then, as disks come and go) instead of on every sample.
#[derive(Default)]
struct IoServices {
    gpu: Vec<io_object_t>,
    disks: Vec<io_object_t>,
    refreshed: Option<Instant>,
}

impl IoServices {
    fn refresh_if_stale(&mut self, now: Instant) {
        if self
            .refreshed
            .is_some_and(|t| now.duration_since(t) < Duration::from_secs(30))
        {
            return;
        }
        self.release();
        self.gpu = ffi::matching_services("IOAccelerator");
        self.disks = ffi::matching_services("IOBlockStorageDriver");
        self.refreshed = Some(now);
    }

    fn release(&mut self) {
        for s in self.gpu.drain(..).chain(self.disks.drain(..)) {
            unsafe { IOObjectRelease(s) };
        }
    }
}

impl Drop for IoServices {
    fn drop(&mut self) {
        self.release();
    }
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        let mut page: vm_size_t = 0;
        unsafe { host_page_size(host(), &mut page) };
        let mut s = Self {
            prev_ticks: None,
            prev_net: None,
            prev_disk: None,
            page_size: if page == 0 { 16384 } else { page as u64 },
            memory_total: sysctl_u64("hw.memsize").unwrap_or(0),
            volume: Volume::default(),
            volume_checked: None,
            volume_probe: None,
            probed_at: None,
            purgeable: 0,
            io: IoServices::default(),
        };
        s.io.refresh_if_stale(Instant::now());
        // Prime the delta-based readings so the first real sample is meaningful.
        s.prev_ticks = read_cpu_ticks();
        s.prev_net = read_network_bytes().map(|(r, w)| (Instant::now(), r, w));
        s.prev_disk = read_disk_bytes(&s.io.disks).map(|(r, w)| (Instant::now(), r, w));
        s
    }

    /// Install a better volume probe (the app uses NSURL to include purgeable
    /// space, which plain `statfs` cannot see).
    pub fn set_volume_probe(&mut self, probe: impl Fn() -> Option<Volume> + Send + 'static) {
        self.volume_probe = Some(Box::new(probe));
        self.volume_checked = None;
        self.probed_at = None;
    }

    pub fn sample(&mut self) -> Snapshot {
        let now = Instant::now();
        let cpu = self.sample_cpu();
        let memory = self.sample_memory();
        let swap = sysctl_raw::<xsw_usage>("vm.swapusage")
            .map(|x| Swap {
                total: x.xsu_total,
                used: x.xsu_used,
            })
            .unwrap_or_default();

        // Free space: cheap statfs every 10 s. The purgeable part (which Finder
        // counts as available) needs a slow CacheDelete query, so refresh that
        // only every 5 minutes and reuse it in between.
        if self
            .volume_checked
            .is_none_or(|t| now.duration_since(t) > Duration::from_secs(10))
        {
            if self
                .probed_at
                .is_none_or(|t| now.duration_since(t) > Duration::from_secs(300))
                && let Some(v) = self.volume_probe.as_ref().and_then(|p| p())
            {
                self.purgeable = v.available.saturating_sub(v.free);
                self.volume.name = v.name;
                self.probed_at = Some(now);
            }
            if let Some(fs) = statfs_volume("/") {
                let name = if self.volume.name.is_empty() {
                    fs.name.clone()
                } else {
                    self.volume.name.clone()
                };
                self.volume = Volume {
                    name,
                    total: fs.total,
                    free: fs.free,
                    available: (fs.free + self.purgeable).min(fs.total),
                };
            }
            self.volume_checked = Some(now);
        }

        self.io.refresh_if_stale(now);
        let network = rate(&mut self.prev_net, read_network_bytes(), now);
        let disk_io = rate(&mut self.prev_disk, read_disk_bytes(&self.io.disks), now);

        let mut load = [0.0f64; 3];
        unsafe { libc::getloadavg(load.as_mut_ptr(), 3) };

        Snapshot {
            taken: SystemTime::now(),
            cpu,
            memory,
            swap,
            volume: self.volume.clone(),
            network,
            disk_io,
            gpu: read_gpu_utilization(&self.io.gpu),
            load,
            uptime: uptime(),
        }
    }

    fn sample_cpu(&mut self) -> CpuLoad {
        let Some(ticks) = read_cpu_ticks() else {
            return CpuLoad::default();
        };
        let mut out = CpuLoad {
            per_core: Vec::with_capacity(ticks.len()),
            ..Default::default()
        };
        let (mut sum_user, mut sum_sys, mut sum_busy, mut sum_all) = (0u64, 0u64, 0u64, 0u64);
        if let Some(prev) = &self.prev_ticks
            && prev.len() == ticks.len()
        {
            for (now, before) in ticks.iter().zip(prev) {
                let d = |i: usize| now[i].wrapping_sub(before[i]) as u64;
                let user = d(CPU_STATE_USER) + d(CPU_STATE_NICE);
                let sys = d(CPU_STATE_SYSTEM);
                let idle = d(CPU_STATE_IDLE);
                let all = user + sys + idle;
                out.per_core.push(if all == 0 {
                    0.0
                } else {
                    (user + sys) as f32 / all as f32
                });
                sum_user += user;
                sum_sys += sys;
                sum_busy += user + sys;
                sum_all += all;
            }
        } else {
            out.per_core = vec![0.0; ticks.len()];
        }
        if sum_all > 0 {
            out.total = sum_busy as f32 / sum_all as f32;
            out.user = sum_user as f32 / sum_all as f32;
            out.system = sum_sys as f32 / sum_all as f32;
        }
        self.prev_ticks = Some(ticks);
        out
    }

    fn sample_memory(&self) -> Memory {
        let mut stats = vm_statistics64::default();
        let mut count = HOST_VM_INFO64_COUNT;
        let rc = unsafe {
            host_statistics64(
                host(),
                HOST_VM_INFO64,
                (&mut stats as *mut vm_statistics64).cast(),
                &mut count,
            )
        };
        let total = self.memory_total;
        if rc != KERN_SUCCESS {
            return Memory {
                total,
                ..Default::default()
            };
        }
        let page = self.page_size;
        let pages = |n: u32| n as u64 * page;
        let app = pages(
            stats
                .internal_page_count
                .saturating_sub(stats.purgeable_count),
        );
        let wired = pages(stats.wire_count);
        let compressed = pages(stats.compressor_page_count);
        let cached = pages(stats.external_page_count) + pages(stats.purgeable_count);
        let used = app + wired + compressed;
        let free = total.saturating_sub(used + cached);

        let pressure = match sysctl_i32("kern.memorystatus_vm_pressure_level").unwrap_or(1) {
            4 => Pressure::Critical,
            2 => Pressure::Warning,
            _ => Pressure::Normal,
        };
        let pressure_pct = sysctl_i32("kern.memorystatus_level")
            .map(|free_pct| (100 - free_pct.clamp(0, 100)) as f32)
            .unwrap_or_else(|| {
                if total > 0 {
                    used as f32 / total as f32 * 100.0
                } else {
                    0.0
                }
            });

        Memory {
            total,
            used,
            app,
            wired,
            compressed,
            cached,
            free,
            pressure,
            pressure_pct,
        }
    }
}

fn rate(
    prev: &mut Option<(Instant, u64, u64)>,
    current: Option<(u64, u64)>,
    now: Instant,
) -> Throughput {
    let Some((r, w)) = current else {
        return Throughput::default();
    };
    let out = match *prev {
        Some((t, pr, pw)) => {
            let dt = now.duration_since(t).as_secs_f64();
            if dt > 0.0 {
                Throughput {
                    read_per_sec: r.saturating_sub(pr) as f64 / dt,
                    write_per_sec: w.saturating_sub(pw) as f64 / dt,
                }
            } else {
                Throughput::default()
            }
        }
        None => Throughput::default(),
    };
    *prev = Some((now, r, w));
    out
}

fn read_cpu_ticks() -> Option<Vec<[u32; CPU_STATE_MAX]>> {
    let mut count: natural_t = 0;
    let mut info: *mut integer_t = std::ptr::null_mut();
    let mut info_count: mach_msg_type_number_t = 0;
    unsafe {
        let rc = host_processor_info(
            host(),
            PROCESSOR_CPU_LOAD_INFO,
            &mut count,
            &mut info,
            &mut info_count,
        );
        if rc != KERN_SUCCESS || info.is_null() {
            return None;
        }
        let raw = std::slice::from_raw_parts(info, info_count as usize);
        let mut out = Vec::with_capacity(count as usize);
        for cpu in 0..count as usize {
            let base = cpu * CPU_STATE_MAX;
            if base + CPU_STATE_MAX > raw.len() {
                break;
            }
            let mut t = [0u32; CPU_STATE_MAX];
            for (i, slot) in t.iter_mut().enumerate() {
                *slot = raw[base + i] as u32;
            }
            out.push(t);
        }
        vm_deallocate(
            task_self(),
            info as vm_address_t,
            info_count as usize * size_of::<integer_t>(),
        );
        Some(out)
    }
}

pub fn statfs_volume(path: &str) -> Option<Volume> {
    let cpath = std::ffi::CString::new(path).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(cpath.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let bsize = st.f_bsize as u64;
    let total = st.f_blocks * bsize;
    let free = st.f_bavail * bsize;
    Some(Volume {
        name: "Macintosh HD".into(),
        total,
        available: free,
        free,
    })
}

/// Received / sent bytes summed over physical interfaces (Wi-Fi, Ethernet,
/// cellular), read with 64-bit counters via `NET_RT_IFLIST2`.
fn read_network_bytes() -> Option<(u64, u64)> {
    const NET_RT_IFLIST2: c_int = 6;
    const RTM_IFINFO2: u8 = 0x12;
    const IFF_LOOPBACK: i32 = 0x8;
    let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, NET_RT_IFLIST2, 0];
    let mut len = 0usize;
    unsafe {
        if libc::sysctl(
            mib.as_mut_ptr(),
            6,
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        let mut buf = vec![0u8; len];
        if libc::sysctl(
            mib.as_mut_ptr(),
            6,
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        buf.truncate(len);
        let (mut rx, mut tx) = (0u64, 0u64);
        let mut off = 0usize;
        while off + 4 <= buf.len() {
            let msglen = u16::from_ne_bytes([buf[off], buf[off + 1]]) as usize;
            if msglen == 0 || off + msglen > buf.len() {
                break;
            }
            let msg = &buf[off..off + msglen];
            // struct if_msghdr2: type @3, flags @8, index @12, if_data64 @32
            if msg[3] == RTM_IFINFO2 && msg.len() >= 32 + 80 {
                let flags = i32::from_ne_bytes(msg[8..12].try_into().unwrap());
                let index = u16::from_ne_bytes(msg[12..14].try_into().unwrap());
                if flags & IFF_LOOPBACK == 0 && is_physical_interface(index) {
                    let data = &msg[32..];
                    rx += u64::from_ne_bytes(data[64..72].try_into().unwrap());
                    tx += u64::from_ne_bytes(data[72..80].try_into().unwrap());
                }
            }
            off += msglen;
        }
        Some((rx, tx))
    }
}

fn is_physical_interface(index: u16) -> bool {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: Mutex<Option<HashMap<u16, bool>>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    let map = cache.get_or_insert_with(HashMap::new);
    if let Some(v) = map.get(&index) {
        return *v;
    }
    let v = interface_is_physical(index);
    map.insert(index, v);
    v
}

fn interface_is_physical(index: u16) -> bool {
    let mut name = [0 as libc::c_char; libc::IF_NAMESIZE];
    let p = unsafe { libc::if_indextoname(index as u32, name.as_mut_ptr()) };
    if p.is_null() {
        return false;
    }
    let name = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }.to_bytes();
    name.starts_with(b"en") || name.starts_with(b"pdp_ip")
}

/// Bytes read / written by all block storage drivers since boot.
fn read_disk_bytes(services: &[io_object_t]) -> Option<(u64, u64)> {
    let (mut r, mut w, mut seen) = (0u64, 0u64, false);
    for &service in services {
        let stats = ffi::entry_property(service, "Statistics");
        unsafe {
            if cf_is_dict(stats) {
                r += cf_number_i64(dict_get(stats, "Bytes (Read)"))
                    .unwrap_or(0)
                    .max(0) as u64;
                w += cf_number_i64(dict_get(stats, "Bytes (Write)"))
                    .unwrap_or(0)
                    .max(0) as u64;
                seen = true;
            }
            if !stats.is_null() {
                CFRelease(stats);
            }
        }
    }
    seen.then_some((r, w))
}

/// GPU utilisation 0..=1 as reported by the accelerator driver.
fn read_gpu_utilization(services: &[io_object_t]) -> Option<f32> {
    let mut best: Option<i64> = None;
    for &service in services {
        let perf = ffi::entry_property(service, "PerformanceStatistics");
        unsafe {
            if cf_is_dict(perf) {
                let v = cf_number_i64(dict_get(perf, "Device Utilization %"))
                    .or_else(|| cf_number_i64(dict_get(perf, "GPU Activity(%)")));
                if let Some(v) = v {
                    best = Some(best.map_or(v, |b| b.max(v)));
                }
            }
            if !perf.is_null() {
                CFRelease(perf);
            }
        }
    }
    best.map(|v| (v.clamp(0, 100) as f32) / 100.0)
}

fn uptime() -> Duration {
    let Some(boot) = sysctl_raw::<libc::timeval>("kern.boottime") else {
        return Duration::ZERO;
    };
    let boot =
        SystemTime::UNIX_EPOCH + Duration::new(boot.tv_sec as u64, (boot.tv_usec as u32) * 1000);
    SystemTime::now().duration_since(boot).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Processes, grouped by the app bundle they belong to.

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppUsage {
    pub name: String,
    /// Path to the outermost `.app` bundle, when the process lives in one.
    pub bundle: Option<String>,
    /// Percent of one core (can exceed 100 for multi-threaded apps).
    pub cpu: f32,
    /// Physical footprint, like Activity Monitor's "Memory" column.
    pub memory: u64,
    pub processes: u32,
}

#[derive(Clone)]
struct Identity {
    key: String,
    name: String,
    bundle: Option<String>,
}

#[derive(Default)]
pub struct ProcessSampler {
    prev: std::collections::HashMap<i32, (u64, u64)>,
    prev_at: Option<Instant>,
    identities: std::collections::HashMap<(i32, u64), Identity>,
}

impl ProcessSampler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns per-app usage sorted by CPU, busiest first.
    pub fn sample(&mut self) -> Vec<AppUsage> {
        let pids = list_pids();
        let now = Instant::now();
        let dt_ns = self
            .prev_at
            .map(|t| now.duration_since(t).as_nanos() as f64)
            .unwrap_or(0.0);
        let mut next = std::collections::HashMap::with_capacity(pids.len());
        let mut apps: std::collections::HashMap<String, AppUsage> =
            std::collections::HashMap::new();

        for pid in pids {
            let mut ru = rusage_info_v2::default();
            let rc = unsafe {
                proc_pid_rusage(pid, RUSAGE_INFO_V2, (&mut ru as *mut rusage_info_v2).cast())
            };
            if rc != 0 {
                continue;
            }
            let cpu_ns = mach_to_ns(ru.ri_user_time + ru.ri_system_time);
            let start = ru.ri_proc_start_abstime;
            let cpu_pct = match self.prev.get(&pid) {
                Some(&(prev_start, prev_ns)) if prev_start == start && dt_ns > 0.0 => {
                    (cpu_ns.saturating_sub(prev_ns) as f64 / dt_ns * 100.0) as f32
                }
                _ => 0.0,
            };
            next.insert(pid, (start, cpu_ns));

            let id = self
                .identities
                .entry((pid, start))
                .or_insert_with(|| identify(pid))
                .clone();
            let entry = apps.entry(id.key.clone()).or_insert_with(|| AppUsage {
                name: id.name.clone(),
                bundle: id.bundle.clone(),
                ..Default::default()
            });
            entry.cpu += cpu_pct;
            entry.memory += ru.ri_phys_footprint;
            entry.processes += 1;
        }

        // Forget identities of processes that are gone.
        self.identities
            .retain(|(pid, start), _| next.get(pid).is_some_and(|(s, _)| s == start));
        self.prev = next;
        self.prev_at = Some(now);

        let mut list: Vec<AppUsage> = apps.into_values().collect();
        list.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.memory.cmp(&a.memory)));
        list
    }
}

fn list_pids() -> Vec<i32> {
    unsafe {
        let n = proc_listallpids(std::ptr::null_mut(), 0);
        if n <= 0 {
            return Vec::new();
        }
        let cap = n as usize + 64;
        let mut pids = vec![0i32; cap];
        let got = proc_listallpids(pids.as_mut_ptr().cast(), (cap * size_of::<i32>()) as c_int);
        if got <= 0 {
            return Vec::new();
        }
        pids.truncate(got as usize);
        pids.retain(|p| *p > 0);
        pids
    }
}

fn identify(pid: i32) -> Identity {
    let mut buf = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n > 0 {
        buf.truncate(n as usize);
        let path = String::from_utf8_lossy(&buf).into_owned();
        if let Some(idx) = path.find(".app/") {
            let bundle = &path[..idx + 4];
            let name = bundle
                .rsplit('/')
                .next()
                .unwrap_or(bundle)
                .trim_end_matches(".app")
                .to_string();
            return Identity {
                key: bundle.to_string(),
                name,
                bundle: Some(bundle.to_string()),
            };
        }
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        return Identity {
            key: format!("bin:{name}"),
            name,
            bundle: None,
        };
    }
    let mut name = [0u8; 64];
    let n = unsafe { proc_name(pid, name.as_mut_ptr().cast(), name.len() as u32) };
    let name = if n > 0 {
        String::from_utf8_lossy(&name[..n as usize]).into_owned()
    } else {
        format!("pid {pid}")
    };
    Identity {
        key: format!("bin:{name}"),
        name,
        bundle: None,
    }
}

/// Development aid: print how long each reading takes.
#[doc(hidden)]
pub fn bench_parts(n: u32) {
    let time = |name: &str, f: &mut dyn FnMut()| {
        let t = Instant::now();
        for _ in 0..n {
            f();
        }
        println!("{name:>10}: {:?}", t.elapsed() / n);
    };
    let mut io = IoServices::default();
    io.refresh_if_stale(Instant::now());
    time("cpu", &mut || drop(read_cpu_ticks()));
    time("network", &mut || {
        let _ = read_network_bytes();
    });
    time("disk io", &mut || {
        let _ = read_disk_bytes(&io.disks);
    });
    time("gpu", &mut || {
        let _ = read_gpu_utilization(&io.gpu);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_sane() {
        let info = SystemInfo::read();
        assert!(info.memory_total > 0);
        assert!(!info.cores.is_empty());
        let mut s = Sampler::new();
        std::thread::sleep(Duration::from_millis(200));
        let snap = s.sample();
        assert_eq!(snap.cpu.per_core.len(), info.cores.len());
        assert!((0.0..=1.0).contains(&snap.cpu.total));
        assert!(snap.memory.used > 0 && snap.memory.used <= snap.memory.total);
        assert!(snap.volume.total > 0);
        println!("{info:#?}\n{snap:#?}");
    }

    #[test]
    fn processes_are_grouped() {
        let mut p = ProcessSampler::new();
        p.sample();
        std::thread::sleep(Duration::from_millis(300));
        let apps = p.sample();
        assert!(!apps.is_empty());
        for a in apps.iter().take(8) {
            println!(
                "{:>6.1}% {:>8} MB x{:<3} {}",
                a.cpu,
                a.memory / 1_000_000,
                a.processes,
                a.name
            );
        }
    }
}
