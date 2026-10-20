//! User preferences, stored as JSON next to the disk index.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Show the live CPU percentage next to the menu bar icon.
    pub cpu_in_menu_bar: bool,
    /// Extra folders to leave out of disk scans.
    pub excluded: Vec<String>,
    /// Re-scan automatically when the index is older than this many hours.
    pub rescan_hours: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            cpu_in_menu_bar: false,
            excluded: Vec::new(),
            rescan_hours: 6,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read(tessera_core::paths::settings_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = tessera_core::paths::settings_file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}
