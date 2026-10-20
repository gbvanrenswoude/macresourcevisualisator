//! The semantic view of the disk: instead of raw folders, group storage into
//! things people recognise — apps, projects, developer tooling, media, app
//! data, caches — with every byte counted exactly once.
//!
//! The walk is driven by well-known macOS locations. Inside generic folders
//! (Documents, ~/code, …) it splits off projects and apps wherever they are
//! found and leaves the rest with the folder's own category.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::index::{DiskIndex, NodeId};
use super::kinds::Kind;
use super::scan::flags;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Category {
    Apps,
    Projects,
    Developer,
    AiModels,
    Media,
    Documents,
    AppData,
    Caches,
    Trash,
    System,
    Other,
}

impl Category {
    pub const ALL: [Category; 11] = [
        Category::Apps,
        Category::Projects,
        Category::Developer,
        Category::AiModels,
        Category::Media,
        Category::Documents,
        Category::AppData,
        Category::Caches,
        Category::Trash,
        Category::System,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Apps => "Applications",
            Category::Projects => "Projects",
            Category::Developer => "Developer",
            Category::AiModels => "AI",
            Category::Media => "Media",
            Category::Documents => "Documents",
            Category::AppData => "App Data",
            Category::Caches => "Caches & Logs",
            Category::Trash => "Trash",
            Category::System => "System",
            Category::Other => "Other",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Category::Apps => "apps",
            Category::Projects => "projects",
            Category::Developer => "developer",
            Category::AiModels => "ai",
            Category::Media => "media",
            Category::Documents => "documents",
            Category::AppData => "appdata",
            Category::Caches => "caches",
            Category::Trash => "trash",
            Category::System => "system",
            Category::Other => "other",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Category::Apps => "Application bundles in /Applications and ~/Applications",
            Category::Projects => {
                "Source trees, recognised by Cargo.toml, package.json, .git and friends"
            }
            Category::Developer => "Toolchains, SDKs, simulators, package caches and containers",
            Category::AiModels => {
                "Local models and AI coding tools (Ollama, Hugging Face, Claude, Codex)"
            }
            Category::Media => "Pictures, Movies and Music",
            Category::Documents => "Desktop, Documents, Downloads and cloud drives",
            Category::AppData => "Data apps keep in ~/Library",
            Category::Caches => "Caches, logs and temporary files: safe to clear",
            Category::Trash => "Deleted files waiting to be emptied",
            Category::System => "macOS itself, swap, and space macOS keeps out of sight",
            Category::Other => "Everything else",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub label: String,
    pub size: u64,
    pub files: u64,
    pub mtime: u32,
    /// The index node this item stands for (`None` for synthetic items such
    /// as "macOS" or loose files).
    pub node: Option<NodeId>,
    pub category: Category,
    pub detail: Option<String>,
    /// Bytes of build artifacts / caches inside, which could be reclaimed.
    pub reclaimable: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Group {
    pub category: Category,
    pub size: u64,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Overview {
    pub groups: Vec<Group>,
    pub total: u64,
}

impl Overview {
    pub fn group(&self, c: Category) -> Option<&Group> {
        self.groups.iter().find(|g| g.category == c)
    }

    /// Biggest items across all groups.
    pub fn top_items(&self, n: usize) -> Vec<&Item> {
        let mut all: Vec<&Item> = self.groups.iter().flat_map(|g| g.items.iter()).collect();
        all.sort_by_key(|i| std::cmp::Reverse(i.size));
        all.truncate(n);
        all
    }

    pub fn reclaimable(&self) -> u64 {
        self.groups
            .iter()
            .flat_map(|g| &g.items)
            .map(|i| i.reclaimable)
            .sum()
    }
}

/// Build the overview. `volume_used` is what the volume reports as used, so
/// the space nobody can see (snapshots, other APFS volumes, unreadable
/// folders) shows up as its own block instead of silently missing.
pub fn build(index: &DiskIndex, volume_used: u64, home: &Path) -> Overview {
    let mut b = Builder {
        index,
        items: HashMap::new(),
    };
    let root = index.root();
    if index.data_volume {
        b.data_volume(root, home);
    } else {
        b.container(root, Category::Other);
    }

    let scanned = index.total();
    if index.data_volume {
        let mut volumes = 0;
        for v in &index.other_volumes {
            b.synthetic(Category::System, &v.label, v.used, Some(&v.detail));
            volumes += v.used;
        }
        let hidden = volume_used.saturating_sub(scanned + volumes);
        if hidden > 256 << 20 {
            b.synthetic(
                Category::System,
                "Hidden space",
                hidden,
                Some("Snapshots, other APFS volumes and folders Tessera may not read"),
            );
        }
    }

    let mut groups: Vec<Group> = b
        .items
        .into_iter()
        .filter_map(|(category, mut items)| {
            items.retain(|i| i.size > 0);
            if items.is_empty() {
                return None;
            }
            items.sort_by_key(|i| std::cmp::Reverse(i.size));
            let size = items.iter().map(|i| i.size).sum();
            Some(Group {
                category,
                size,
                items,
            })
        })
        .collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.size));
    let total = groups.iter().map(|g| g.size).sum();
    Overview { groups, total }
}

struct Builder<'a> {
    index: &'a DiskIndex,
    items: HashMap<Category, Vec<Item>>,
}

impl Builder<'_> {
    fn name(&self, id: NodeId) -> &str {
        self.index.name(id)
    }

    fn add(
        &mut self,
        category: Category,
        id: NodeId,
        label: Option<String>,
        detail: Option<String>,
    ) {
        let n = *self.index.node(id);
        if n.size == 0 {
            return;
        }
        let label = label.unwrap_or_else(|| pretty_name(self.name(id), n.kind));
        let detail = detail.or_else(|| default_detail(self.index, id));
        let reclaimable = if n.kind.is_reclaimable() {
            n.size
        } else {
            reclaimable_inside(self.index, id)
        };
        self.items.entry(category).or_default().push(Item {
            label,
            size: n.size,
            files: n.files as u64,
            mtime: n.mtime,
            node: Some(id),
            category,
            detail,
            reclaimable,
        });
    }

    fn synthetic(&mut self, category: Category, label: &str, size: u64, detail: Option<&str>) {
        if size == 0 {
            return;
        }
        self.items.entry(category).or_default().push(Item {
            label: label.to_string(),
            size,
            files: 0,
            mtime: 0,
            node: None,
            category,
            detail: detail.map(str::to_string),
            reclaimable: if category == Category::Caches || category == Category::Trash {
                size
            } else {
                0
            },
        });
    }

    fn loose_files(&mut self, category: Category, id: NodeId, label: &str) {
        let (bytes, files) = self.index.remainder(id);
        if bytes == 0 {
            return;
        }
        let detail = if files > 0 {
            Some(format!("{} smaller items", group_digits(files)))
        } else {
            None
        };
        self.items.entry(category).or_default().push(Item {
            label: label.to_string(),
            size: bytes,
            files,
            mtime: 0,
            node: None,
            category,
            detail,
            reclaimable: 0,
        });
    }

    /// Split a generic folder: projects and apps go to their own groups
    /// wherever they are; everything else stays in `category`.
    fn container(&mut self, id: NodeId, category: Category) {
        for c in self.index.children(id) {
            let n = *self.index.node(c);
            match n.kind {
                Kind::Project => self.add(Category::Projects, c, None, None),
                Kind::App => self.add(Category::Apps, c, None, None),
                _ if n.is_dir() && (n.has(flags::HAS_PROJECT) || n.has(flags::HAS_APP)) => {
                    self.container(c, category)
                }
                _ if self.name(c).starts_with('.') => {
                    let label = format!("{}/{}", self.name(id), self.name(c));
                    self.add(category, c, Some(label), None)
                }
                _ => self.add(category, c, None, None),
            }
        }
        let label = format!("Other files in {}", self.name(id));
        self.loose_files(category, id, &label);
    }

    /// Each child becomes an item of `category`.
    fn each_child(&mut self, id: NodeId, category: Category, rest_label: &str) {
        for c in self.index.children(id) {
            self.add(category, c, None, None);
        }
        self.loose_files(category, id, rest_label);
    }

    fn data_volume(&mut self, root: NodeId, home: &Path) {
        let home_name = home.file_name().and_then(|n| n.to_str()).unwrap_or("");
        for c in self.index.children(root) {
            match self.name(c) {
                "Users" => {
                    for u in self.index.children(c) {
                        let uname = self.name(u).to_string();
                        if uname == home_name {
                            self.home(u);
                        } else if uname == "Shared" {
                            self.add(
                                Category::Other,
                                u,
                                Some("Shared".into()),
                                Some("/Users/Shared".into()),
                            );
                        } else {
                            self.add(
                                Category::Other,
                                u,
                                Some(format!("{uname}'s home")),
                                Some("Another user".into()),
                            );
                        }
                    }
                    self.loose_files(Category::Other, c, "Other files in Users");
                }
                "Applications" => self.applications(c),
                "Library" => self.system_library(c),
                "opt" => {
                    for o in self.index.children(c) {
                        let label = match self.name(o) {
                            "homebrew" => "Homebrew".to_string(),
                            other => format!("/opt/{other}"),
                        };
                        self.add(Category::Developer, o, Some(label), None);
                    }
                    self.loose_files(Category::Developer, c, "Other files in /opt");
                }
                "usr" => {
                    for u in self.index.children(c) {
                        if self.name(u) == "local" {
                            self.add(
                                Category::Developer,
                                u,
                                Some("/usr/local".into()),
                                Some("Homebrew (Intel) and manual installs".into()),
                            );
                        } else {
                            let label = format!("/usr/{}", self.name(u));
                            self.add(Category::System, u, Some(label), None);
                        }
                    }
                    self.loose_files(Category::System, c, "Other files in /usr");
                }
                "private" => self.private(c),
                "System" => self.add(
                    Category::System,
                    c,
                    Some("System data".into()),
                    Some("Data macOS keeps on the data volume".into()),
                ),
                "cores" => self.add(
                    Category::Caches,
                    c,
                    Some("Crash dumps".into()),
                    Some("Core dumps from crashed processes".into()),
                ),
                other => {
                    let label = format!("/{other}");
                    self.add(Category::System, c, Some(label), None);
                }
            }
        }
        self.loose_files(Category::System, root, "Other system files");
    }

    fn private(&mut self, id: NodeId) {
        for c in self.index.children(id) {
            match self.name(c) {
                "var" => {
                    for v in self.index.children(c) {
                        match self.name(v) {
                            "vm" => self.add(
                                Category::System,
                                v,
                                Some("Swap & sleep image".into()),
                                Some("Virtual memory on disk".into()),
                            ),
                            "folders" => self.add(
                                Category::Caches,
                                v,
                                Some("Temporary items".into()),
                                Some("Per-user temporary files and caches".into()),
                            ),
                            "log" | "logs" => {
                                self.add(Category::Caches, v, Some("System logs".into()), None)
                            }
                            "db" => {
                                self.add(Category::System, v, Some("System databases".into()), None)
                            }
                            other => {
                                let label = format!("/private/var/{other}");
                                self.add(Category::System, v, Some(label), None);
                            }
                        }
                    }
                    self.loose_files(Category::System, c, "Other files in /private/var");
                }
                "tmp" => self.add(
                    Category::Caches,
                    c,
                    Some("Temporary files".into()),
                    Some("/private/tmp".into()),
                ),
                other => {
                    let label = format!("/private/{other}");
                    self.add(Category::System, c, Some(label), None);
                }
            }
        }
        self.loose_files(Category::System, id, "Other files in /private");
    }

    fn applications(&mut self, id: NodeId) {
        for c in self.index.children(id) {
            let n = *self.index.node(c);
            if n.kind != Kind::App && n.is_dir() && n.has(flags::HAS_APP) {
                self.applications(c);
            } else {
                self.add(Category::Apps, c, None, None);
            }
        }
        let label = format!("Other files in {}", self.name(id));
        self.loose_files(Category::Apps, id, &label);
    }

    fn system_library(&mut self, id: NodeId) {
        for c in self.index.children(id) {
            match self.name(c) {
                "Caches" => self.add(
                    Category::Caches,
                    c,
                    Some("System caches".into()),
                    Some("/Library/Caches".into()),
                ),
                "Logs" => self.add(
                    Category::Caches,
                    c,
                    Some("System logs".into()),
                    Some("/Library/Logs".into()),
                ),
                "Developer" => {
                    for d in self.index.children(c) {
                        let label = match self.name(d) {
                            "CommandLineTools" => "Command Line Tools".to_string(),
                            "CoreSimulator" => "Simulator runtimes".to_string(),
                            other => other.to_string(),
                        };
                        self.add(
                            Category::Developer,
                            d,
                            Some(label),
                            Some("/Library/Developer".into()),
                        );
                    }
                    self.loose_files(Category::Developer, c, "Other developer files");
                }
                "Application Support" => {
                    for a in self.index.children(c) {
                        self.add(
                            Category::AppData,
                            a,
                            None,
                            Some("/Library/Application Support".into()),
                        );
                    }
                    self.loose_files(Category::AppData, c, "Other shared app data");
                }
                "Updates" => self.add(
                    Category::System,
                    c,
                    Some("Software updates".into()),
                    Some("Downloaded macOS updates".into()),
                ),
                other => {
                    let label = format!("/Library/{other}");
                    self.add(Category::System, c, Some(label), None);
                }
            }
        }
        self.loose_files(Category::System, id, "Other files in /Library");
    }

    fn home(&mut self, id: NodeId) {
        for c in self.index.children(id) {
            let name = self.name(c).to_string();
            match name.as_str() {
                "Library" => self.user_library(c),
                "Applications" => self.applications(c),
                "Pictures" | "Movies" | "Music" => self.container(c, Category::Media),
                "Desktop" | "Documents" | "Downloads" | "Public" => {
                    self.container(c, Category::Documents)
                }
                ".Trash" => self.add(
                    Category::Trash,
                    c,
                    Some("Trash".into()),
                    Some("Empty the Trash to reclaim this".into()),
                ),
                ".cache" => {
                    for k in self.index.children(c) {
                        match self.name(k) {
                            "huggingface" => self.add(
                                Category::AiModels,
                                k,
                                Some("Hugging Face".into()),
                                Some("~/.cache/huggingface".into()),
                            ),
                            "lm-studio" => self.add(
                                Category::AiModels,
                                k,
                                Some("LM Studio".into()),
                                Some("~/.cache/lm-studio".into()),
                            ),
                            other => {
                                let label = format!("~/.cache/{other}");
                                self.add(Category::Caches, k, Some(label), None);
                            }
                        }
                    }
                    self.loose_files(Category::Caches, c, "Other files in ~/.cache");
                }
                "go" => self.add(
                    Category::Developer,
                    c,
                    Some("Go workspace".into()),
                    Some("~/go".into()),
                ),
                "miniconda3" | "anaconda3" | "miniforge3" | "mambaforge" => self.add(
                    Category::Developer,
                    c,
                    Some("Conda".into()),
                    Some(format!("~/{name}")),
                ),
                "OrbStack" => self.add(
                    Category::Developer,
                    c,
                    Some("OrbStack machines".into()),
                    None,
                ),
                _ if name.starts_with('.') => {
                    let (category, label) = dotfolder(&name);
                    self.add(category, c, Some(label), Some(format!("~/{name}")));
                }
                _ => self.container(c, Category::Other),
            }
        }
        self.loose_files(Category::Other, id, "Files in your home folder");
    }

    fn user_library(&mut self, id: NodeId) {
        for c in self.index.children(id) {
            let name = self.name(c).to_string();
            match name.as_str() {
                "Caches" => self.each_child(c, Category::Caches, "Other caches"),
                "Logs" => self.add(
                    Category::Caches,
                    c,
                    Some("Logs".into()),
                    Some("~/Library/Logs".into()),
                ),
                "Application Support" => {
                    for a in self.index.children(c) {
                        let aname = self.name(a).to_string();
                        let (category, label, detail) = match aname.as_str() {
                            "Code" => (Category::Developer, "VS Code".to_string(), None),
                            "Cursor" | "JetBrains" | "Zed" | "Windsurf" => {
                                (Category::Developer, aname.clone(), None)
                            }
                            "MobileSync" => (
                                Category::AppData,
                                "iPhone & iPad backups".to_string(),
                                Some("Finder device backups".to_string()),
                            ),
                            "Steam" => (Category::AppData, "Steam games".to_string(), None),
                            "Ollama" | "LM Studio" | "nomic.ai" | "DiffusionBee" => {
                                (Category::AiModels, aname.clone(), None)
                            }
                            _ => (Category::AppData, aname.clone(), None),
                        };
                        self.add(
                            category,
                            a,
                            Some(label),
                            detail.or_else(|| Some("Application Support".into())),
                        );
                    }
                    self.loose_files(Category::AppData, c, "Other files in Application Support");
                }
                "Containers" | "Group Containers" | "Daemon Containers" => {
                    let place = name.clone();
                    for a in self.index.children(c) {
                        let aname = self.name(a).to_string();
                        if aname == "com.docker.docker" {
                            self.add(
                                Category::Developer,
                                a,
                                Some("Docker Desktop".into()),
                                Some("Docker disk image and data".into()),
                            );
                        } else {
                            self.add(
                                Category::AppData,
                                a,
                                Some(container_label(&aname)),
                                Some(place.clone()),
                            );
                        }
                    }
                    let label = format!("Other files in {place}");
                    self.loose_files(Category::AppData, c, &label);
                }
                "Developer" => self.user_developer(c),
                "Mobile Documents" => self.add(
                    Category::Documents,
                    c,
                    Some("iCloud Drive".into()),
                    Some("Files stored locally from iCloud".into()),
                ),
                "CloudStorage" => self.each_child(c, Category::Documents, "Other cloud files"),
                "Android" => self.add(
                    Category::Developer,
                    c,
                    Some("Android SDK".into()),
                    Some("~/Library/Android".into()),
                ),
                "pnpm" => self.add(
                    Category::Developer,
                    c,
                    Some("pnpm store".into()),
                    Some("~/Library/pnpm".into()),
                ),
                "Mail" | "Messages" | "Safari" | "Photos" | "Calendars" => {
                    self.add(Category::AppData, c, None, Some("~/Library".into()))
                }
                _ => {
                    let label = format!("Library/{name}");
                    self.add(Category::AppData, c, Some(label), None);
                }
            }
        }
        self.loose_files(Category::AppData, id, "Other Library files");
    }

    fn user_developer(&mut self, id: NodeId) {
        for c in self.index.children(id) {
            match self.name(c) {
                "Xcode" => {
                    for x in self.index.children(c) {
                        let (label, detail) = match self.name(x) {
                            "DerivedData" => {
                                ("Xcode DerivedData", Some("Build products: safe to delete"))
                            }
                            "Archives" => ("Xcode Archives", Some("App builds you archived")),
                            "iOS DeviceSupport" => (
                                "iOS Device Support",
                                Some("Symbols for connected devices: safe to delete"),
                            ),
                            "watchOS DeviceSupport" => {
                                ("watchOS Device Support", Some("Safe to delete"))
                            }
                            "Products" => ("Xcode Products", None),
                            other => {
                                let label = format!("Xcode/{other}");
                                self.add(Category::Developer, x, Some(label), None);
                                continue;
                            }
                        };
                        self.add(
                            Category::Developer,
                            x,
                            Some(label.into()),
                            detail.map(str::to_string),
                        );
                    }
                    self.loose_files(Category::Developer, c, "Other Xcode files");
                }
                "CoreSimulator" => self.add(
                    Category::Developer,
                    c,
                    Some("Simulators".into()),
                    Some("Simulator devices and runtimes".into()),
                ),
                other => {
                    let label = other.to_string();
                    self.add(
                        Category::Developer,
                        c,
                        Some(label),
                        Some("~/Library/Developer".into()),
                    );
                }
            }
        }
        self.loose_files(Category::Developer, id, "Other developer files");
    }
}

fn dotfolder(name: &str) -> (Category, String) {
    let dev = |l: &str| (Category::Developer, l.to_string());
    match name {
        ".rustup" => dev("Rust toolchains"),
        ".cargo" => dev("Cargo registry & tools"),
        ".npm" => dev("npm cache"),
        ".nvm" => dev("Node versions (nvm)"),
        ".yarn" => dev("Yarn"),
        ".bun" => dev("Bun"),
        ".deno" => dev("Deno"),
        ".gradle" => dev("Gradle"),
        ".m2" => dev("Maven repository"),
        ".pyenv" => dev("Python versions (pyenv)"),
        ".conda" => dev("Conda"),
        ".docker" => dev("Docker"),
        ".orbstack" => dev("OrbStack"),
        ".colima" => dev("Colima"),
        ".lima" => dev("Lima"),
        ".vagrant.d" => dev("Vagrant boxes"),
        ".minikube" => dev("Minikube"),
        ".android" => dev("Android"),
        ".gem" => dev("Ruby gems"),
        ".rbenv" => dev("Ruby versions (rbenv)"),
        ".swiftpm" => dev("SwiftPM"),
        ".vscode" => dev("VS Code extensions"),
        ".cursor" => dev("Cursor extensions"),
        ".local" => dev("~/.local"),
        ".pnpm-store" => dev("pnpm store"),
        ".ollama" => (Category::AiModels, "Ollama models".into()),
        ".lmstudio" => (Category::AiModels, "LM Studio models".into()),
        ".claude" => (Category::AiModels, "Claude Code".into()),
        ".codex" => (Category::AiModels, "Codex".into()),
        ".gemini" => (Category::AiModels, "Gemini CLI".into()),
        ".antigravity" => (Category::AiModels, "Antigravity".into()),
        ".continue" => (Category::AiModels, "Continue".into()),
        ".aider" => (Category::AiModels, "Aider".into()),
        ".cache" => (Category::Caches, "~/.cache".into()),
        other => (Category::Other, format!("~/{other}")),
    }
}

/// `com.apple.mail` → `Mail`, `group.com.docker` → `docker`, otherwise as-is.
fn container_label(id: &str) -> String {
    let known: &[(&str, &str)] = &[
        ("com.apple.mail", "Mail"),
        ("com.apple.Safari", "Safari"),
        ("com.apple.photoanalysisd", "Photos analysis"),
        ("com.apple.MobileSMS", "Messages"),
        ("com.apple.Music", "Music"),
        ("com.apple.TV", "TV"),
        ("com.apple.Notes", "Notes"),
        ("com.apple.podcasts", "Podcasts"),
        ("com.apple.iWork.Keynote", "Keynote"),
        ("com.apple.iWork.Pages", "Pages"),
        ("com.apple.iWork.Numbers", "Numbers"),
        ("com.microsoft.teams2", "Microsoft Teams"),
        ("com.microsoft.Outlook", "Outlook"),
        ("com.tinyspeck.slackmacgap", "Slack"),
        ("net.whatsapp.WhatsApp", "WhatsApp"),
    ];
    if let Some((_, label)) = known.iter().find(|(k, _)| *k == id) {
        return (*label).to_string();
    }
    id.to_string()
}

fn pretty_name(name: &str, kind: Kind) -> String {
    match kind {
        Kind::App => name.trim_end_matches(".app").to_string(),
        Kind::MediaLibrary | Kind::VirtualMachine => name
            .rsplit_once('.')
            .map(|(stem, _)| stem.to_string())
            .unwrap_or_else(|| name.to_string()),
        _ => name.to_string(),
    }
}

fn default_detail(index: &DiskIndex, id: NodeId) -> Option<String> {
    let n = index.node(id);
    match n.kind {
        Kind::Project => Some(format!("{} project", n.lang().label())),
        Kind::Folder => None,
        Kind::Artifacts => Some("Build artifacts: safe to delete".into()),
        Kind::Cache => Some("Cache: safe to delete".into()),
        k => Some(k.label().to_string()),
    }
}

/// Bytes in the top-most artifact / cache folders below `id`.
pub fn reclaimable_inside(index: &DiskIndex, id: NodeId) -> u64 {
    let mut total = 0;
    let mut stack: Vec<NodeId> = index.children(id).collect();
    while let Some(c) = stack.pop() {
        let n = index.node(c);
        if n.kind.is_reclaimable() {
            total += n.size;
        } else if n.is_dir() {
            stack.extend(index.children(c));
        }
    }
    total
}

fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::scan::{ScanConfig, ScanProgress, scan};

    fn write(path: &Path, mb: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![7u8; mb << 20]).unwrap();
    }

    #[test]
    fn groups_a_volume_and_counts_every_byte_once() {
        let root = std::env::temp_dir().join(format!("tessera-vol-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("Users/me");
        write(&home.join("code/web/package.json"), 0);
        write(&home.join("code/web/src/app.js"), 2);
        write(&home.join("code/web/node_modules/dep/index.js"), 3);
        write(&home.join("code/notes/todo.txt"), 1);
        write(&home.join("Documents/rust-thing/Cargo.toml"), 0);
        write(&home.join("Documents/rust-thing/target/debug/bin"), 4);
        write(&home.join("Documents/taxes/2025.pdf"), 2);
        write(&home.join("Library/Caches/com.example/blob"), 2);
        write(&home.join("Library/Application Support/Slack/data"), 2);
        write(&home.join(".rustup/toolchains/x"), 2);
        write(&home.join(".ollama/models/llama.gguf"), 3);
        write(&home.join(".Trash/old.mov"), 2);
        write(&root.join("Applications/Big.app/Contents/MacOS/Big"), 5);
        write(
            &root.join("Applications/Utilities/Tool.app/Contents/MacOS/Tool"),
            2,
        );
        write(&root.join("private/var/vm/sleepimage"), 1);

        let cfg = ScanConfig {
            min_dir: 0,
            min_file: 1 << 20,
            data_volume: true,
            ..ScanConfig::for_path(&root)
        };
        let index = scan(&cfg, &ScanProgress::default()).unwrap();
        let ov = build(&index, 0, &home);

        let labels = |c: Category| -> Vec<String> {
            ov.group(c)
                .map(|g| g.items.iter().map(|i| i.label.clone()).collect())
                .unwrap_or_default()
        };
        let projects = labels(Category::Projects);
        assert!(projects.contains(&"web".to_string()), "{projects:?}");
        assert!(projects.contains(&"rust-thing".to_string()), "{projects:?}");
        let apps = labels(Category::Apps);
        assert!(
            apps.contains(&"Big".to_string()) && apps.contains(&"Tool".to_string()),
            "{apps:?}"
        );
        assert!(labels(Category::Developer).contains(&"Rust toolchains".to_string()));
        assert!(labels(Category::AiModels).contains(&"Ollama models".to_string()));
        assert!(labels(Category::Caches).contains(&"com.example".to_string()));
        assert!(labels(Category::AppData).contains(&"Slack".to_string()));
        assert!(labels(Category::Trash).contains(&"Trash".to_string()));
        assert!(labels(Category::Documents).contains(&"taxes".to_string()));
        assert!(
            labels(Category::Other).contains(&"notes".to_string()),
            "{:?}",
            labels(Category::Other)
        );

        // Every scanned byte lands in exactly one item.
        let synthetic: u64 = index.other_volumes.iter().map(|v| v.used).sum();
        assert_eq!(ov.total, index.total() + synthetic);

        // Build output inside projects is reported as reclaimable.
        let web = ov
            .group(Category::Projects)
            .unwrap()
            .items
            .iter()
            .find(|i| i.label == "web")
            .unwrap();
        assert!(web.reclaimable >= 3 << 20, "{}", web.reclaimable);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
