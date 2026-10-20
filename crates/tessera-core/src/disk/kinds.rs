//! What a directory or file *is*: an app, a project, build output, media…
//! Detected from names alone while scanning, so it costs nothing extra.

use serde::{Deserialize, Serialize};

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Kind {
    #[default]
    Folder = 0,
    /// A `.app` bundle.
    App,
    /// A source tree with project markers (`Cargo.toml`, `package.json`, `.git`…).
    Project,
    /// Reproducible build output or downloaded dependencies (`node_modules`,
    /// `target`, `.venv`…): safe to delete, it comes back on the next build.
    Artifacts,
    /// A `.git` directory.
    Git,
    /// Other bundle-like directories (frameworks, libraries, archives).
    Package,
    /// A cache directory.
    Cache,
    /// A virtual machine bundle (`.utm`, `.pvm`, `.vmwarevm`).
    VirtualMachine,
    /// A photo / music / video library package.
    MediaLibrary,

    File = 32,
    Video,
    Image,
    Audio,
    Archive,
    DiskImage,
    VmDisk,
    Model,
    Document,
    Database,
}

impl Kind {
    pub fn is_file(self) -> bool {
        (self as u8) >= Kind::File as u8
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Folder => "Folder",
            Kind::App => "Application",
            Kind::Project => "Project",
            Kind::Artifacts => "Build artifacts",
            Kind::Git => "Git history",
            Kind::Package => "Package",
            Kind::Cache => "Cache",
            Kind::VirtualMachine => "Virtual machine",
            Kind::MediaLibrary => "Media library",
            Kind::File => "File",
            Kind::Video => "Video",
            Kind::Image => "Image",
            Kind::Audio => "Audio",
            Kind::Archive => "Archive",
            Kind::DiskImage => "Disk image",
            Kind::VmDisk => "Virtual disk",
            Kind::Model => "ML model",
            Kind::Document => "Document",
            Kind::Database => "Database",
        }
    }

    /// Can the user delete this without losing anything irreplaceable?
    pub fn is_reclaimable(self) -> bool {
        matches!(self, Kind::Artifacts | Kind::Cache)
    }
}

/// Project language / ecosystem, stored in a node's `tag` byte.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Lang {
    #[default]
    Unknown = 0,
    Rust,
    Node,
    Python,
    Go,
    Swift,
    Jvm,
    Godot,
    Ruby,
    Cpp,
    Php,
    Dart,
    Elixir,
    Unreal,
    Unity,
    DotNet,
    Deno,
    Zig,
    Git,
}

impl Lang {
    pub fn from_u8(v: u8) -> Self {
        use Lang::*;
        const ALL: [Lang; 19] = [
            Unknown, Rust, Node, Python, Go, Swift, Jvm, Godot, Ruby, Cpp, Php, Dart, Elixir,
            Unreal, Unity, DotNet, Deno, Zig, Git,
        ];
        ALL.get(v as usize).copied().unwrap_or(Unknown)
    }

    pub fn label(self) -> &'static str {
        match self {
            Lang::Unknown => "Project",
            Lang::Rust => "Rust",
            Lang::Node => "JavaScript",
            Lang::Python => "Python",
            Lang::Go => "Go",
            Lang::Swift => "Swift / Xcode",
            Lang::Jvm => "JVM",
            Lang::Godot => "Godot",
            Lang::Ruby => "Ruby",
            Lang::Cpp => "C / C++",
            Lang::Php => "PHP",
            Lang::Dart => "Dart / Flutter",
            Lang::Elixir => "Elixir",
            Lang::Unreal => "Unreal",
            Lang::Unity => "Unity",
            Lang::DotNet => ".NET",
            Lang::Deno => "Deno",
            Lang::Zig => "Zig",
            Lang::Git => "Git repository",
        }
    }
}

/// Tiny stand-in for the `bitflags` crate (we only need a handful of ops).
macro_rules! bitflags_lite {
    ($(#[$m:meta])* pub struct $name:ident: $t:ty { $(const $f:ident = $v:expr;)* }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub struct $name(pub $t);
        #[allow(dead_code)]
        impl $name {
            $(pub const $f: $name = $name($v);)*
            pub fn empty() -> Self { $name(0) }
            pub fn contains(self, other: $name) -> bool { self.0 & other.0 == other.0 }
            pub fn intersects(self, other: $name) -> bool { self.0 & other.0 != 0 }
            pub fn is_empty(self) -> bool { self.0 == 0 }
            pub fn insert(&mut self, other: $name) { self.0 |= other.0 }
        }
        impl std::ops::BitOr for $name {
            type Output = $name;
            fn bitor(self, rhs: $name) -> $name { $name(self.0 | rhs.0) }
        }
    };
}

bitflags_lite! {
    /// Project markers seen among a directory's entries.
    pub struct Markers: u32 {
        const GIT = 1 << 0;
        const CARGO = 1 << 1;
        const PACKAGE_JSON = 1 << 2;
        const GO_MOD = 1 << 3;
        const PYTHON = 1 << 4;
        const SWIFT = 1 << 5;
        const GRADLE = 1 << 6;
        const MAVEN = 1 << 7;
        const GODOT = 1 << 8;
        const RUBY = 1 << 9;
        const CMAKE = 1 << 10;
        const PHP = 1 << 11;
        const DART = 1 << 12;
        const ELIXIR = 1 << 13;
        const UNREAL = 1 << 14;
        const DOTNET = 1 << 15;
        const DENO = 1 << 16;
        const ZIG = 1 << 17;
        const UNITY_ASSETS = 1 << 18;
        const UNITY_SETTINGS = 1 << 19;
        const PODFILE = 1 << 20;
    }
}

impl Markers {
    pub fn observe(&mut self, name: &[u8], is_dir: bool) {
        let m = match name {
            b".git" => Markers::GIT,
            b"Cargo.toml" => Markers::CARGO,
            b"package.json" => Markers::PACKAGE_JSON,
            b"go.mod" => Markers::GO_MOD,
            b"pyproject.toml" | b"setup.py" | b"Pipfile" | b"requirements.txt" | b"uv.lock" => {
                Markers::PYTHON
            }
            b"Package.swift" => Markers::SWIFT,
            b"build.gradle" | b"build.gradle.kts" | b"settings.gradle" | b"settings.gradle.kts" => {
                Markers::GRADLE
            }
            b"pom.xml" => Markers::MAVEN,
            b"project.godot" => Markers::GODOT,
            b"Gemfile" => Markers::RUBY,
            b"CMakeLists.txt" => Markers::CMAKE,
            b"composer.json" => Markers::PHP,
            b"pubspec.yaml" => Markers::DART,
            b"mix.exs" => Markers::ELIXIR,
            b"deno.json" | b"deno.jsonc" => Markers::DENO,
            b"build.zig" => Markers::ZIG,
            b"Podfile" => Markers::PODFILE,
            b"Assets" if is_dir => Markers::UNITY_ASSETS,
            b"ProjectSettings" if is_dir => Markers::UNITY_SETTINGS,
            _ => {
                if ends_with(name, b".xcodeproj") || ends_with(name, b".xcworkspace") {
                    Markers::SWIFT
                } else if ends_with(name, b".uproject") {
                    Markers::UNREAL
                } else if ends_with(name, b".csproj")
                    || ends_with(name, b".sln")
                    || ends_with(name, b".fsproj")
                {
                    Markers::DOTNET
                } else {
                    return;
                }
            }
        };
        self.insert(m);
    }

    fn is_unity(self) -> bool {
        self.contains(Markers::UNITY_ASSETS | Markers::UNITY_SETTINGS)
    }

    /// Does this directory look like the root of a project?
    pub fn project_lang(self) -> Option<Lang> {
        let order: [(Markers, Lang); 19] = [
            (Markers::GODOT, Lang::Godot),
            (Markers::UNREAL, Lang::Unreal),
            (Markers::CARGO, Lang::Rust),
            (Markers::GO_MOD, Lang::Go),
            (Markers::SWIFT, Lang::Swift),
            (Markers::DART, Lang::Dart),
            (Markers::DENO, Lang::Deno),
            (Markers::PACKAGE_JSON, Lang::Node),
            (Markers::PYTHON, Lang::Python),
            (Markers::GRADLE, Lang::Jvm),
            (Markers::MAVEN, Lang::Jvm),
            (Markers::RUBY, Lang::Ruby),
            (Markers::PHP, Lang::Php),
            (Markers::ELIXIR, Lang::Elixir),
            (Markers::DOTNET, Lang::DotNet),
            (Markers::ZIG, Lang::Zig),
            (Markers::CMAKE, Lang::Cpp),
            (Markers::PODFILE, Lang::Swift),
            (Markers::GIT, Lang::Git),
        ];
        if self.is_unity() {
            return Some(Lang::Unity);
        }
        order
            .iter()
            .find(|(m, _)| self.contains(*m))
            .map(|(_, l)| *l)
    }
}

fn ends_with(name: &[u8], suffix: &[u8]) -> bool {
    name.len() > suffix.len() && name.ends_with(suffix)
}

fn ext_of(name: &[u8]) -> Option<String> {
    let dot = name.iter().rposition(|b| *b == b'.')?;
    if dot == 0 || dot + 1 >= name.len() {
        return None;
    }
    Some(String::from_utf8_lossy(&name[dot + 1..]).to_ascii_lowercase())
}

/// Classify a directory from its name, its entries' markers and the markers
/// of its parent.
pub fn dir_kind(
    name: &[u8],
    markers: Markers,
    parent: Markers,
    allow_projects: bool,
) -> (Kind, Lang) {
    if let Some(ext) = ext_of(name) {
        match ext.as_str() {
            "app" => return (Kind::App, Lang::Unknown),
            "utm" | "pvm" | "vmwarevm" | "vbox" => return (Kind::VirtualMachine, Lang::Unknown),
            "photoslibrary"
            | "musiclibrary"
            | "tvlibrary"
            | "fcpbundle"
            | "imovielibrary"
            | "logicx"
            | "aplibrary"
            | "migratedphotolibrary"
            | "photolibrary" => return (Kind::MediaLibrary, Lang::Unknown),
            "framework" | "bundle" | "plugin" | "kext" | "xcarchive" | "appex" | "sparsebundle"
            | "pkg" | "docset" | "xcframework" | "lproj" => {
                let k = if ext == "sparsebundle" {
                    Kind::DiskImage
                } else {
                    Kind::Package
                };
                return (k, Lang::Unknown);
            }
            _ => {}
        }
    }
    if let Some(kind) = artifact_kind(name, parent) {
        return (kind, Lang::Unknown);
    }
    if name == b".git" {
        return (Kind::Git, Lang::Unknown);
    }
    if allow_projects && let Some(lang) = markers.project_lang() {
        return (Kind::Project, lang);
    }
    match name {
        b"Caches" | b"Cache" | b"cache" | b".cache" | b"CachedData" | b"Code Cache"
        | b"GPUCache" => (Kind::Cache, Lang::Unknown),
        _ => (Kind::Folder, Lang::Unknown),
    }
}

/// Build output and dependency folders, recognised by name (and, for generic
/// names such as `target` or `build`, by what the parent folder contains).
fn artifact_kind(name: &[u8], parent: Markers) -> Option<Kind> {
    let generic_build = parent.intersects(
        Markers::CARGO
            | Markers::MAVEN
            | Markers::GRADLE
            | Markers::CMAKE
            | Markers::PACKAGE_JSON
            | Markers::SWIFT,
    );
    let hit = match name {
        b"node_modules" | b"bower_components" | b".pnpm-store" | b"__pycache__" | b".venv"
        | b".tox" | b".mypy_cache" | b".pytest_cache" | b".ruff_cache" | b".gradle" | b".next"
        | b".nuxt" | b".svelte-kit" | b".turbo" | b".parcel-cache" | b".angular" | b".expo"
        | b"DerivedData" | b".zig-cache" | b"zig-cache" | b"zig-out" | b".dart_tool"
        | b".build" | b".swiftpm" | b".godot" | b".import" | b".terraform" | b".serverless"
        | b".docusaurus" | b".cxx" => true,
        b"venv" | b"env" => parent.contains(Markers::PYTHON),
        b"deps" | b"_build" => parent.contains(Markers::ELIXIR),
        b"Pods" => parent.contains(Markers::PODFILE) || parent.contains(Markers::SWIFT),
        b"target" | b"build" | b"out" | b"dist" | b"cmake-build-debug" | b"cmake-build-release" => {
            generic_build
        }
        b"bin" | b"obj" => parent.contains(Markers::DOTNET),
        b"vendor" => parent.intersects(Markers::GO_MOD | Markers::PHP | Markers::RUBY),
        b"Library" | b"Temp" | b"Logs" => {
            parent.contains(Markers::UNITY_ASSETS | Markers::UNITY_SETTINGS)
        }
        b"Intermediate" | b"Binaries" | b"DerivedDataCache" | b"Saved" => {
            parent.contains(Markers::UNREAL)
        }
        _ => false,
    };
    hit.then_some(Kind::Artifacts)
}

/// Classify a file by extension (and a few well-known names).
pub fn file_kind(name: &[u8]) -> Kind {
    if name == b"Docker.raw" || name == b"data.img" {
        return Kind::VmDisk;
    }
    let Some(ext) = ext_of(name) else {
        return Kind::File;
    };
    match ext.as_str() {
        "mp4" | "mov" | "m4v" | "mkv" | "avi" | "webm" | "mts" | "braw" | "r3d" | "prores"
        | "mxf" => Kind::Video,
        "jpg" | "jpeg" | "png" | "heic" | "heif" | "tif" | "tiff" | "raw" | "cr2" | "cr3"
        | "nef" | "arw" | "dng" | "psd" | "gif" | "webp" | "exr" => Kind::Image,
        "mp3" | "m4a" | "aac" | "wav" | "aif" | "aiff" | "flac" | "ogg" | "caf" | "alac" => {
            Kind::Audio
        }
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "xip" | "lz4" => {
            Kind::Archive
        }
        "dmg" | "iso" | "sparseimage" | "img" | "ipsw" => Kind::DiskImage,
        "qcow2" | "vmdk" | "vdi" | "vhd" | "vhdx" | "hdd" | "raw-disk" => Kind::VmDisk,
        "gguf" | "ggml" | "safetensors" | "ckpt" | "pt" | "pth" | "onnx" | "mlmodel"
        | "mlpackage" | "tflite" | "h5" | "npz" | "bin-model" => Kind::Model,
        "pdf" | "doc" | "docx" | "key" | "pages" | "numbers" | "xls" | "xlsx" | "ppt" | "pptx"
        | "epub" => Kind::Document,
        "sqlite" | "sqlite3" | "db" | "realm" | "ldb" | "sst" | "wal" => Kind::Database,
        _ => Kind::File,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_projects_and_artifacts() {
        let mut m = Markers::empty();
        m.observe(b"Cargo.toml", false);
        m.observe(b".git", true);
        assert_eq!(
            dir_kind(b"tessera", m, Markers::empty(), true),
            (Kind::Project, Lang::Rust)
        );
        assert_eq!(
            dir_kind(b"tessera", m, Markers::empty(), false).0,
            Kind::Folder
        );
        assert_eq!(
            dir_kind(b"target", Markers::empty(), m, true).0,
            Kind::Artifacts
        );
        assert_eq!(
            dir_kind(b"target", Markers::empty(), Markers::empty(), true).0,
            Kind::Folder
        );
        assert_eq!(
            dir_kind(b"node_modules", Markers::empty(), Markers::empty(), true).0,
            Kind::Artifacts
        );
        assert_eq!(
            dir_kind(b"Safari.app", Markers::empty(), Markers::empty(), true).0,
            Kind::App
        );
        assert_eq!(file_kind(b"movie.MOV"), Kind::Video);
        assert_eq!(file_kind(b"Docker.raw"), Kind::VmDisk);
        assert_eq!(file_kind(b"llama-3-8b.Q4_K_M.gguf"), Kind::Model);
        assert_eq!(file_kind(b".zshrc"), Kind::File);
    }
}
