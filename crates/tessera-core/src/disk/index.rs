//! The persisted disk index: a flat, breadth-first array of nodes with
//! contiguous children, plus one shared string arena for names. A typical
//! home folder fits in a few megabytes and loads in milliseconds, so the app
//! can show the last known picture of the disk instantly at launch.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::kinds::{Kind, Lang};
use super::scan::{DATA_VOLUME, Tmp, flags};

pub type NodeId = u32;

const MAGIC: &[u8; 8] = b"TESSIDX\0";
const FORMAT_VERSION: u32 = 3;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Node {
    /// Allocated bytes on disk, for the whole subtree.
    pub size: u64,
    /// Number of files in the subtree.
    pub files: u32,
    /// Newest modification time in the subtree (unix seconds).
    pub mtime: u32,
    pub parent: NodeId,
    pub first_child: NodeId,
    pub child_count: u32,
    name_start: u32,
    name_len: u16,
    pub kind: Kind,
    pub lang: u8,
    pub flags: u8,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        self.flags & flags::DIR != 0
    }

    pub fn lang(&self) -> Lang {
        Lang::from_u8(self.lang)
    }

    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IndexStats {
    pub files: u64,
    pub dirs: u64,
    pub denied: u64,
    pub denied_paths: Vec<String>,
    pub duration: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiskIndex {
    pub root_path: PathBuf,
    pub data_volume: bool,
    pub scanned_at: SystemTime,
    pub stats: IndexStats,
    /// The other APFS volumes sharing the container (system, swap, …) with
    /// the space each uses. They are not scanned, but they take disk space.
    pub other_volumes: Vec<OtherVolume>,
    nodes: Vec<Node>,
    names: String,
}

impl DiskIndex {
    pub(crate) fn from_tree(
        root: Tmp,
        root_path: PathBuf,
        data_volume: bool,
        scanned_at: SystemTime,
        stats: IndexStats,
    ) -> Self {
        let mut index = DiskIndex {
            root_path,
            data_volume,
            scanned_at,
            stats,
            other_volumes: if data_volume {
                other_volumes()
            } else {
                Vec::new()
            },
            nodes: Vec::new(),
            names: String::new(),
        };
        let mut queue: VecDeque<(Tmp, NodeId)> = VecDeque::new();
        let root_id = index.push(&root, NodeId::MAX);
        queue.push_back((root, root_id));
        while let Some((tmp, id)) = queue.pop_front() {
            let first = index.nodes.len() as NodeId;
            let count = tmp.children.len() as u32;
            for child in tmp.children {
                let cid = index.push(&child, id);
                queue.push_back((child, cid));
            }
            let n = &mut index.nodes[id as usize];
            n.first_child = first;
            n.child_count = count;
        }
        index.nodes.shrink_to_fit();
        index.names.shrink_to_fit();
        index
    }

    fn push(&mut self, t: &Tmp, parent: NodeId) -> NodeId {
        let name = String::from_utf8_lossy(&t.name);
        let name = if name.len() > u16::MAX as usize {
            &name[..u16::MAX as usize]
        } else {
            &name
        };
        let name_start = self.names.len() as u32;
        self.names.push_str(name);
        self.nodes.push(Node {
            size: t.size,
            files: t.files.min(u32::MAX as u64) as u32,
            mtime: t.mtime,
            parent,
            first_child: 0,
            child_count: 0,
            name_start,
            name_len: name.len() as u16,
            kind: t.kind,
            lang: t.lang as u8,
            flags: t.flags,
        });
        (self.nodes.len() - 1) as NodeId
    }

    pub fn root(&self) -> NodeId {
        0
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn name(&self, id: NodeId) -> &str {
        let n = &self.nodes[id as usize];
        &self.names[n.name_start as usize..n.name_start as usize + n.name_len as usize]
    }

    /// Children, biggest first.
    pub fn children(&self, id: NodeId) -> impl ExactSizeIterator<Item = NodeId> + use<> {
        let n = &self.nodes[id as usize];
        n.first_child..n.first_child + n.child_count
    }

    pub fn child_by_name(&self, id: NodeId, name: &str) -> Option<NodeId> {
        self.children(id).find(|c| self.name(*c) == name)
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        let p = self.nodes[id as usize].parent;
        (p != NodeId::MAX).then_some(p)
    }

    /// Bytes and file count not covered by the indexed children: loose files
    /// and everything too small to be indexed individually.
    pub fn remainder(&self, id: NodeId) -> (u64, u64) {
        let n = self.node(id);
        let (mut bytes, mut files) = (0u64, 0u64);
        for c in self.children(id) {
            bytes += self.node(c).size;
            files += self.node(c).files as u64;
        }
        (
            n.size.saturating_sub(bytes),
            (n.files as u64).saturating_sub(files),
        )
    }

    /// Real path on disk.
    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut parts = Vec::new();
        let mut cur = id;
        while let Some(p) = self.parent(cur) {
            parts.push(self.name(cur));
            cur = p;
        }
        let mut path = self.root_path.clone();
        for part in parts.iter().rev() {
            path.push(part);
        }
        path
    }

    /// Path as the user knows it (`/Users/…`, not `/System/Volumes/Data/Users/…`).
    pub fn display_path(&self, id: NodeId) -> String {
        let p = self.path(id);
        let s = p.to_string_lossy();
        match s.strip_prefix(DATA_VOLUME) {
            Some("") => "/".to_string(),
            Some(rest) => rest.to_string(),
            None => s.into_owned(),
        }
    }

    /// Find the node for a path (accepts either the firmlinked or the
    /// data-volume spelling).
    pub fn find(&self, path: &Path) -> Option<NodeId> {
        let rel = path
            .strip_prefix(&self.root_path)
            .ok()
            .map(Path::to_path_buf)
            .or_else(|| {
                self.data_volume
                    .then(|| path.strip_prefix("/").ok().map(Path::to_path_buf))
                    .flatten()
            })?;
        let mut cur = self.root();
        for comp in rel.components() {
            let name = comp.as_os_str().to_str()?;
            cur = self.child_by_name(cur, name)?;
        }
        Some(cur)
    }

    pub fn total(&self) -> u64 {
        self.nodes.first().map(|n| n.size).unwrap_or(0)
    }

    // -- persistence --------------------------------------------------------

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = postcard::to_stdvec(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(MAGIC)?;
            f.write_all(&FORMAT_VERSION.to_le_bytes())?;
            f.write_all(&body)?;
            f.sync_all()?;
        }
        std::fs::rename(tmp, path)
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let mut f = std::fs::File::open(path)?;
        let mut header = [0u8; 12];
        f.read_exact(&mut header)?;
        if &header[..8] != MAGIC
            || u32::from_le_bytes(header[8..12].try_into().unwrap()) != FORMAT_VERSION
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "index format changed",
            ));
        }
        let mut body = Vec::new();
        f.read_to_end(&mut body)?;
        postcard::from_bytes(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OtherVolume {
    pub label: String,
    pub detail: String,
    pub used: u64,
}

fn other_volumes() -> Vec<OtherVolume> {
    let candidates = [
        ("/", "macOS", "The sealed, read-only system volume"),
        (
            "/System/Volumes/VM",
            "Swap",
            "Virtual memory swapped out to disk",
        ),
        (
            "/System/Volumes/Preboot",
            "Preboot",
            "Boot files and cryptexes",
        ),
        (
            "/System/Volumes/Update",
            "Update staging",
            "Space reserved for macOS updates",
        ),
    ];
    candidates
        .iter()
        .filter_map(|(path, label, detail)| {
            let used = volume_space_used(path)?;
            (used > 0).then(|| OtherVolume {
                label: (*label).into(),
                detail: (*detail).into(),
                used,
            })
        })
        .collect()
}

/// Bytes used by one APFS volume. `statfs` only knows about the shared
/// container, so ask for `ATTR_VOL_SPACEUSED` like `df` does.
pub fn volume_space_used(path: &str) -> Option<u64> {
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
        fn getattrlist(
            path: *const libc::c_char,
            attr_list: *mut std::ffi::c_void,
            attr_buf: *mut std::ffi::c_void,
            size: usize,
            options: u32,
        ) -> libc::c_int;
    }
    const ATTR_VOL_INFO: u32 = 0x8000_0000;
    const ATTR_VOL_SPACEUSED: u32 = 0x0080_0000;
    let cpath = std::ffi::CString::new(path).ok()?;
    let mut attrs = AttrList {
        bitmapcount: 5,
        reserved: 0,
        commonattr: 0,
        volattr: ATTR_VOL_INFO | ATTR_VOL_SPACEUSED,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut buf = [0u8; 32];
    let rc = unsafe {
        getattrlist(
            cpath.as_ptr(),
            (&mut attrs as *mut AttrList).cast(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    let used = i64::from_ne_bytes(buf[4..12].try_into().ok()?);
    (used >= 0).then_some(used as u64)
}

#[cfg(test)]
mod tests {
    use super::super::scan::{ScanConfig, ScanProgress, scan};
    use super::*;

    #[test]
    fn roundtrip_and_paths() {
        let dir = std::env::temp_dir().join(format!("tessera-idx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proj/src")).unwrap();
        std::fs::create_dir_all(dir.join("proj/target/debug")).unwrap();
        std::fs::write(dir.join("proj/Cargo.toml"), "[package]").unwrap();
        std::fs::write(dir.join("proj/target/debug/big.bin"), vec![0u8; 3 << 20]).unwrap();

        let cfg = ScanConfig {
            min_dir: 0,
            min_file: 1 << 20,
            ..ScanConfig::for_path(&dir)
        };
        let index = scan(&cfg, &ScanProgress::default()).unwrap();
        let proj = index.find(&dir.join("proj")).unwrap();
        assert_eq!(index.node(proj).kind, Kind::Project);
        assert_eq!(index.node(proj).lang(), Lang::Rust);
        let target = index.find(&dir.join("proj/target")).unwrap();
        assert_eq!(index.node(target).kind, Kind::Artifacts);
        let big = index.find(&dir.join("proj/target/debug/big.bin")).unwrap();
        assert!(!index.node(big).is_dir());
        assert_eq!(index.path(big), dir.join("proj/target/debug/big.bin"));

        let file = dir.join("index.bin");
        index.save(&file).unwrap();
        let loaded = DiskIndex::load(&file).unwrap();
        assert_eq!(loaded.len(), index.len());
        assert_eq!(loaded.name(big), "big.bin");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
