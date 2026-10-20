//! Disk scanning, the persisted index, and the semantic overview on top.

pub mod index;
pub mod kinds;
pub mod overview;
pub mod scan;

pub use index::{DiskIndex, Node, NodeId};
pub use kinds::{Kind, Lang};
pub use overview::{Category, Group, Item, Overview};
pub use scan::{ScanConfig, ScanPriority, ScanProgress, flags, scan};
