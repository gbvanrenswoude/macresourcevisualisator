//! Parallel volume scanner.
//!
//! Directories are enumerated with `getattrlistbulk(2)`, which returns name,
//! type, allocated size, link count and flags for a whole batch of entries in
//! one syscall — several times faster than `readdir` + `lstat` per file. Work
//! is spread over a rayon pool running at utility QoS so a scan never fights
//! foreground apps for the CPU.
//!
//! Only entries that matter are kept: directories of at least `min_dir`
//! bytes and files of at least `min_file` bytes (at most `max_children` per
//! directory). Everything smaller is folded into its parent's total. That is
//! what keeps the resulting index tiny while sizes stay exact.

use std::collections::HashSet;
use std::ffi::{CString, c_int, c_void};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use rayon::prelude::*;

use super::index::{DiskIndex, IndexStats};
use super::kinds::{Kind, Lang, Markers, dir_kind, file_kind};

/// Node flags stored in the index.
pub mod flags {
    /// Permission denied while listing this directory.
    pub const DENIED: u8 = 1 << 0;
    /// A project lives somewhere below.
    pub const HAS_PROJECT: u8 = 1 << 1;
    /// An application bundle lives somewhere below.
    pub const HAS_APP: u8 = 1 << 2;
    /// An evicted cloud placeholder (contents live in the cloud, not on disk).
    pub const DATALESS: u8 = 1 << 3;
    /// Directory (as opposed to a file).
    pub const DIR: u8 = 1 << 4;
}

#[derive(Clone, Debug)]
pub struct ScanConfig {
    pub root: PathBuf,
    /// Keep directories at least this big in the index.
    pub min_dir: u64,
    /// Keep individual files at least this big in the index.
    pub min_file: u64,
    /// Keep at most this many children per directory.
    pub max_children: usize,
    pub threads: usize,
    /// Scanning the macOS data volume: apply the volume layout rules (only
    /// look for projects under /Users, not in system folders).
    pub data_volume: bool,
    pub priority: ScanPriority,
    /// Folders to leave out entirely.
    pub exclude: Vec<PathBuf>,
}

/// Background scans yield to everything else; a scan the user asked for and
/// is waiting on runs at user-initiated priority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScanPriority {
    #[default]
    Background,
    Interactive,
}

pub const DATA_VOLUME: &str = "/System/Volumes/Data";

impl ScanConfig {
    /// Scan the whole data volume (everything that is not the sealed system).
    pub fn data_volume() -> Self {
        let root = if Path::new(DATA_VOLUME).join("Users").is_dir() {
            PathBuf::from(DATA_VOLUME)
        } else {
            "/".into()
        };
        Self {
            data_volume: true,
            ..Self::for_path(root)
        }
    }

    pub fn for_path(root: impl Into<PathBuf>) -> Self {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self {
            root: root.into(),
            min_dir: 1 << 20,
            min_file: 8 << 20,
            max_children: 400,
            threads: cpus.clamp(2, 8),
            data_volume: false,
            priority: ScanPriority::Background,
            exclude: Vec::new(),
        }
    }
}

/// Live counters, readable from any thread while a scan runs.
#[derive(Default)]
pub struct ScanProgress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub denied: AtomicU64,
    cancel: AtomicBool,
}

impl ScanProgress {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// A scanned node before it is flattened into the index.
pub(crate) struct Tmp {
    pub name: Box<[u8]>,
    pub kind: Kind,
    pub lang: Lang,
    pub flags: u8,
    pub size: u64,
    pub files: u64,
    pub mtime: u32,
    pub children: Vec<Tmp>,
}

#[derive(Clone, Copy)]
struct Ctx {
    parent_markers: Markers,
    allow_projects: bool,
    in_artifacts: bool,
    depth: u16,
}

struct Walker<'a> {
    cfg: &'a ScanConfig,
    exclude: HashSet<PathBuf>,
    progress: &'a ScanProgress,
    root_dev: u64,
    hard_links: Mutex<HashSet<u64>>,
    denied_paths: Mutex<Vec<String>>,
}

pub fn scan(cfg: &ScanConfig, progress: &ScanProgress) -> io::Result<DiskIndex> {
    let started = Instant::now();
    let root_meta = std::fs::metadata(&cfg.root)?;
    let root_dev = std::os::unix::fs::MetadataExt::dev(&root_meta);
    raise_fd_limit();

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(cfg.threads)
        .stack_size(8 << 20)
        .thread_name(|i| format!("tessera-scan-{i}"))
        .start_handler({
            let priority = cfg.priority;
            move |_| set_thread_qos(priority)
        })
        .build()
        .map_err(io::Error::other)?;

    let exclude = cfg
        .exclude
        .iter()
        .map(|p| {
            // Accept both `/Users/…` and `/System/Volumes/Data/Users/…`.
            if cfg.data_volume && !p.starts_with(&cfg.root) {
                cfg.root.join(p.strip_prefix("/").unwrap_or(p))
            } else {
                p.clone()
            }
        })
        .collect();
    let walker = Walker {
        cfg,
        exclude,
        progress,
        root_dev: root_dev as u64,
        hard_links: Mutex::new(HashSet::new()),
        denied_paths: Mutex::new(Vec::new()),
    };
    let ctx = Ctx {
        parent_markers: Markers::empty(),
        allow_projects: !cfg.data_volume,
        in_artifacts: false,
        depth: 0,
    };
    let mut root = pool.install(|| walker.walk(&cfg.root, b"".to_vec().into_boxed_slice(), ctx));
    if progress.is_cancelled() {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "scan cancelled"));
    }
    root.name = root_display_name(&cfg.root).into_bytes().into_boxed_slice();
    root.kind = Kind::Folder;

    let stats = IndexStats {
        files: progress.files.load(Ordering::Relaxed),
        dirs: progress.dirs.load(Ordering::Relaxed),
        denied: progress.denied.load(Ordering::Relaxed),
        denied_paths: walker.denied_paths.into_inner().unwrap_or_default(),
        duration: started.elapsed(),
    };
    let scanned_at = SystemTime::now();
    Ok(DiskIndex::from_tree(
        root,
        cfg.root.clone(),
        cfg.data_volume,
        scanned_at,
        stats,
    ))
}

fn root_display_name(root: &Path) -> String {
    if root == Path::new(DATA_VOLUME) || root == Path::new("/") {
        "Macintosh HD".into()
    } else {
        root.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string())
    }
}

impl Walker<'_> {
    fn walk(&self, path: &Path, name: Box<[u8]>, ctx: Ctx) -> Tmp {
        let mut node = Tmp {
            name,
            kind: Kind::Folder,
            lang: Lang::Unknown,
            flags: flags::DIR,
            size: 0,
            files: 0,
            mtime: 0,
            children: Vec::new(),
        };
        if self.progress.is_cancelled() {
            return node;
        }

        let mut entries = Vec::new();
        match read_dir_bulk(path, self.root_dev, &mut entries) {
            Ok(()) => {}
            Err(DirError::OtherDevice) => return node,
            Err(DirError::Io(e)) => {
                if matches!(e.raw_os_error(), Some(libc::EACCES) | Some(libc::EPERM)) {
                    node.flags |= flags::DENIED;
                    self.progress.denied.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut d) = self.denied_paths.lock()
                        && d.len() < 64
                    {
                        d.push(path.display().to_string());
                    }
                }
                return node;
            }
        }

        // Pass 1: markers, files, and the list of subdirectories.
        let mut markers = Markers::empty();
        for e in &entries {
            markers.observe(&e.name, e.objtype == VDIR);
        }
        if ctx.in_artifacts {
            node.kind = Kind::Folder;
        } else {
            let allow = ctx.allow_projects && !(self.cfg.data_volume && ctx.depth <= 2);
            let (kind, lang) = dir_kind(&node.name, markers, ctx.parent_markers, allow);
            node.kind = kind;
            node.lang = lang;
        }
        let in_artifacts = ctx.in_artifacts || node.kind == Kind::Artifacts;
        let child_allow_base = ctx.allow_projects
            && !matches!(
                node.kind,
                Kind::App
                    | Kind::Package
                    | Kind::Artifacts
                    | Kind::Git
                    | Kind::Cache
                    | Kind::VirtualMachine
                    | Kind::MediaLibrary
            );

        let mut files_bytes = 0u64;
        let mut files = 0u64;
        let mut mtime = 0u32;
        let mut kept: Vec<Tmp> = Vec::new();
        let mut subdirs: Vec<(Box<[u8]>, bool)> = Vec::new();
        for e in entries {
            if e.objtype == VDIR {
                let dataless = e.flags & SF_DATALESS != 0;
                subdirs.push((e.name, dataless));
                continue;
            }
            if e.objtype == VREG && e.nlink > 1 {
                let mut seen = self.hard_links.lock().unwrap();
                if !seen.insert(e.fileid) {
                    continue; // another hard link to this file was already counted
                }
            }
            files += 1;
            files_bytes += e.alloc;
            mtime = mtime.max(e.mtime);
            if e.alloc >= self.cfg.min_file {
                let kind = if in_artifacts {
                    Kind::File
                } else {
                    file_kind(&e.name)
                };
                kept.push(Tmp {
                    name: e.name,
                    kind,
                    lang: Lang::Unknown,
                    flags: 0,
                    size: e.alloc,
                    files: 1,
                    mtime: e.mtime,
                    children: Vec::new(),
                });
            }
        }
        self.progress.files.fetch_add(files, Ordering::Relaxed);
        self.progress
            .bytes
            .fetch_add(files_bytes, Ordering::Relaxed);
        self.progress.dirs.fetch_add(1, Ordering::Relaxed);

        // Pass 2: recurse into subdirectories in parallel.
        let child_ctx = |cname: &[u8]| Ctx {
            parent_markers: markers,
            allow_projects: if self.cfg.data_volume && ctx.depth == 0 {
                cname == b"Users"
            } else {
                child_allow_base && !cname.starts_with(b".") && cname != b"Library"
            },
            in_artifacts,
            depth: ctx.depth.saturating_add(1),
        };
        let dirs: Vec<Tmp> = subdirs
            .into_par_iter()
            .map(|(cname, dataless)| {
                if dataless {
                    return Tmp {
                        name: cname,
                        kind: Kind::Folder,
                        lang: Lang::Unknown,
                        flags: flags::DIR | flags::DATALESS,
                        size: 0,
                        files: 0,
                        mtime: 0,
                        children: Vec::new(),
                    };
                }
                let child_path = path.join(std::ffi::OsStr::from_bytes(&cname));
                if !self.exclude.is_empty() && self.exclude.contains(&child_path) {
                    return Tmp {
                        name: cname,
                        kind: Kind::Folder,
                        lang: Lang::Unknown,
                        flags: flags::DIR,
                        size: 0,
                        files: 0,
                        mtime: 0,
                        children: Vec::new(),
                    };
                }
                let c = child_ctx(&cname);
                self.walk(&child_path, cname, c)
            })
            .collect();

        let mut size = files_bytes;
        for d in dirs {
            size += d.size;
            files += d.files;
            mtime = mtime.max(d.mtime);
            if d.kind == Kind::Project || d.flags & flags::HAS_PROJECT != 0 {
                node.flags |= flags::HAS_PROJECT;
            }
            if d.kind == Kind::App || d.flags & flags::HAS_APP != 0 {
                node.flags |= flags::HAS_APP;
            }
            if d.size >= self.cfg.min_dir || d.flags & flags::DENIED != 0 {
                kept.push(d);
            }
        }
        kept.sort_unstable_by_key(|t| std::cmp::Reverse(t.size));
        kept.truncate(self.cfg.max_children);
        kept.shrink_to_fit();

        node.size = size;
        node.files = files;
        node.mtime = mtime;
        node.children = kept;
        node
    }
}

// ---------------------------------------------------------------------------
// getattrlistbulk

const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;
const SF_DATALESS: u32 = 0x4000_0000;

const ATTR_BIT_MAP_COUNT: u16 = 5;
const ATTR_CMN_NAME: u32 = 0x0000_0001;
const ATTR_CMN_DEVID: u32 = 0x0000_0002;
const ATTR_CMN_OBJTYPE: u32 = 0x0000_0008;
const ATTR_CMN_MODTIME: u32 = 0x0000_0400;
const ATTR_CMN_FLAGS: u32 = 0x0004_0000;
const ATTR_CMN_FILEID: u32 = 0x0200_0000;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
const ATTR_FILE_LINKCOUNT: u32 = 0x0000_0001;
const ATTR_FILE_ALLOCSIZE: u32 = 0x0000_0004;

#[repr(C)]
struct AttrList {
    bitmapcount: u16,
    reserved: u16,
    commonattr: u32,
    volattr: u32,
    dirattr: u32,
    fileattr: u32,
    forkattr: u32,
}

unsafe extern "C" {
    fn getattrlistbulk(
        dirfd: c_int,
        attr_list: *mut c_void,
        attr_buf: *mut c_void,
        size: usize,
        options: u64,
    ) -> c_int;
}

pub(crate) struct Entry {
    pub name: Box<[u8]>,
    pub objtype: u32,
    pub mtime: u32,
    pub flags: u32,
    pub fileid: u64,
    pub nlink: u32,
    pub alloc: u64,
}

enum DirError {
    Io(io::Error),
    OtherDevice,
}

thread_local! {
    static BUF: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(vec![0u8; 128 * 1024]);
}

fn read_dir_bulk(path: &Path, root_dev: u64, out: &mut Vec<Entry>) -> Result<(), DirError> {
    let cpath =
        CString::new(path.as_os_str().as_bytes()).map_err(|e| DirError::Io(io::Error::other(e)))?;
    let fd = unsafe {
        libc::open(
            cpath.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(DirError::Io(io::Error::last_os_error()));
    }
    struct Fd(c_int);
    impl Drop for Fd {
        fn drop(&mut self) {
            unsafe { libc::close(self.0) };
        }
    }
    let fd = Fd(fd);

    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.0, &mut st) } != 0 {
        return Err(DirError::Io(io::Error::last_os_error()));
    }
    if st.st_dev as u64 != root_dev {
        return Err(DirError::OtherDevice); // a mount point: another volume
    }

    let mut attrs = AttrList {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: ATTR_CMN_RETURNED_ATTRS
            | ATTR_CMN_NAME
            | ATTR_CMN_DEVID
            | ATTR_CMN_OBJTYPE
            | ATTR_CMN_MODTIME
            | ATTR_CMN_FLAGS
            | ATTR_CMN_FILEID,
        volattr: 0,
        dirattr: 0,
        fileattr: ATTR_FILE_LINKCOUNT | ATTR_FILE_ALLOCSIZE,
        forkattr: 0,
    };

    BUF.with(|buf| {
        let mut buf = buf.borrow_mut();
        loop {
            let n = unsafe {
                getattrlistbulk(
                    fd.0,
                    (&mut attrs as *mut AttrList).cast(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    0,
                )
            };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                if err.raw_os_error() == Some(libc::ENOTSUP) {
                    return read_dir_fallback(path, out).map_err(DirError::Io);
                }
                return Err(DirError::Io(err));
            }
            if n == 0 {
                return Ok(());
            }
            let mut off = 0usize;
            for _ in 0..n {
                let Some(entry_len) = read::<u32>(&buf, off) else {
                    break;
                };
                let entry_len = entry_len as usize;
                if entry_len == 0 || off + entry_len > buf.len() {
                    break;
                }
                if let Some(e) = parse_entry(&buf[off..off + entry_len]) {
                    out.push(e);
                }
                off += entry_len;
            }
        }
    })
}

fn read<T: Copy>(buf: &[u8], off: usize) -> Option<T> {
    if off + size_of::<T>() > buf.len() {
        return None;
    }
    Some(unsafe { std::ptr::read_unaligned(buf.as_ptr().add(off).cast::<T>()) })
}

/// Parse one packed entry. Attributes appear in bit order, and only those
/// flagged in the entry's returned-attributes set are present (directories,
/// for instance, carry no file attributes), so walk the set to find offsets.
fn parse_entry(e: &[u8]) -> Option<Entry> {
    // 0: u32 length, 4: attribute_set_t (common, vol, dir, file, fork)
    let common: u32 = read(e, 4)?;
    let file: u32 = read(e, 4 + 12)?;
    let mut p = 24usize;
    let mut name: &[u8] = &[];
    let (mut objtype, mut mtime, mut flags, mut fileid, mut nlink, mut alloc) =
        (0u32, 0i64, 0u32, 0u64, 1u32, 0i64);

    if common & ATTR_CMN_NAME != 0 {
        let off: i32 = read(e, p)?;
        let len: u32 = read(e, p + 4)?;
        let start = (p as i64 + off as i64) as usize;
        let end = start + (len as usize).saturating_sub(1); // drop the NUL
        if len == 0 || end > e.len() {
            return None;
        }
        name = &e[start..end];
        p += 8;
    }
    if common & ATTR_CMN_DEVID != 0 {
        p += 4;
    }
    if common & ATTR_CMN_OBJTYPE != 0 {
        objtype = read(e, p)?;
        p += 4;
    }
    if common & ATTR_CMN_MODTIME != 0 {
        mtime = read(e, p)?;
        p += 16;
    }
    if common & ATTR_CMN_FLAGS != 0 {
        flags = read(e, p)?;
        p += 4;
    }
    if common & ATTR_CMN_FILEID != 0 {
        fileid = read(e, p)?;
        p += 8;
    }
    if file & ATTR_FILE_LINKCOUNT != 0 {
        nlink = read(e, p)?;
        p += 4;
    }
    if file & ATTR_FILE_ALLOCSIZE != 0 {
        alloc = read(e, p)?;
    }

    if name.is_empty() || name == b"." || name == b".." {
        return None;
    }
    Some(Entry {
        name: name.into(),
        objtype,
        mtime: mtime.clamp(0, u32::MAX as i64) as u32,
        flags,
        fileid,
        nlink,
        alloc: if objtype == VDIR {
            0
        } else {
            alloc.max(0) as u64
        },
    })
}

/// Portable fallback for file systems without `getattrlistbulk`.
fn read_dir_fallback(path: &Path, out: &mut Vec<Entry>) -> io::Result<()> {
    use std::os::macos::fs::MetadataExt;
    for entry in std::fs::read_dir(path)? {
        let Ok(entry) = entry else { continue };
        let Ok(md) = entry.metadata() else { continue };
        let ft = md.file_type();
        out.push(Entry {
            name: entry.file_name().as_bytes().into(),
            objtype: if ft.is_dir() {
                VDIR
            } else if ft.is_file() {
                VREG
            } else if ft.is_symlink() {
                VLNK
            } else {
                0
            },
            mtime: md.st_mtime().clamp(0, u32::MAX as i64) as u32,
            flags: md.st_flags(),
            fileid: md.st_ino(),
            nlink: md.st_nlink() as u32,
            alloc: if ft.is_dir() { 0 } else { md.st_blocks() * 512 },
        });
    }
    Ok(())
}

fn set_thread_qos(priority: ScanPriority) {
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos: u32, relpri: c_int) -> c_int;
    }
    const QOS_CLASS_UTILITY: u32 = 0x11;
    const QOS_CLASS_USER_INITIATED: u32 = 0x19;
    let qos = match priority {
        ScanPriority::Background => QOS_CLASS_UTILITY,
        ScanPriority::Interactive => QOS_CLASS_USER_INITIATED,
    };
    unsafe { pthread_set_qos_class_self_np(qos, 0) };
}

fn raise_fd_limit() {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) == 0 && lim.rlim_cur < 4096 {
            lim.rlim_cur = lim.rlim_max.min(4096);
            libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
        }
    }
}

/// How long ago the index was built, for "scanned 2 h ago".
pub fn age(scanned_at: SystemTime) -> Duration {
    SystemTime::now()
        .duration_since(scanned_at)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn bulk_matches_std() {
        let dir = std::env::temp_dir().join(format!("tessera-bulk-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub/deeper")).unwrap();
        fs::write(dir.join("a.txt"), vec![1u8; 10_000]).unwrap();
        fs::write(dir.join("sub/b.bin"), vec![2u8; 300_000]).unwrap();
        fs::hard_link(dir.join("sub/b.bin"), dir.join("sub/b-link.bin")).unwrap();
        std::os::unix::fs::symlink("a.txt", dir.join("link")).unwrap();

        let dev = std::os::unix::fs::MetadataExt::dev(&fs::metadata(&dir).unwrap());
        let mut bulk = Vec::new();
        assert!(read_dir_bulk(&dir, dev as u64, &mut bulk).is_ok());
        let mut std_entries = Vec::new();
        read_dir_fallback(&dir, &mut std_entries).unwrap();
        let key = |v: &mut Vec<Entry>| {
            v.sort_by(|a, b| a.name.cmp(&b.name));
            v.iter()
                .map(|e| (e.name.clone(), e.objtype, e.alloc, e.fileid))
                .collect::<Vec<_>>()
        };
        assert_eq!(key(&mut bulk), key(&mut std_entries));

        let cfg = ScanConfig {
            min_dir: 0,
            min_file: 0,
            ..ScanConfig::for_path(&dir)
        };
        let progress = ScanProgress::default();
        let index = scan(&cfg, &progress).unwrap();
        let expected = 10_000u64.div_ceil(4096) * 4096 + 300_000u64.div_ceil(4096) * 4096;
        // The hard link must be counted once; the symlink is tiny.
        let root = index.node(index.root());
        assert!(
            root.size >= expected && root.size < expected + 64 * 1024,
            "{} vs {}",
            root.size,
            expected
        );
        assert_eq!(progress.files.load(Ordering::Relaxed), 3);
        fs::remove_dir_all(&dir).unwrap();
    }
}
