//! The page, its icons, the manifest and the service worker, compiled into the binary.
//!
//! `include_bytes!` and a fixed table: the set is small (16 files, about 170 KB) and never changes at run time, so there is
//! no directory walk, no extra dependency, no build script and no difference between Windows and Unix paths (`rust-embed` and
//! `include_dir` would add a proc-macro crate for the same result). The page and the manifest use only relative URLs, so the
//! same bytes work at `/` and under any mount prefix: nothing is rewritten at run time.

/// One file the router serves: the name it is asked for (relative to where the page is mounted), its content type and bytes.
pub struct Asset {
    pub name: &'static str,
    pub content_type: &'static str,
    pub bytes: &'static [u8],
}

macro_rules! asset {
    ($name:literal, $ctype:literal) => {
        Asset {
            name: $name,
            content_type: $ctype,
            bytes: include_bytes!(concat!("../assets/", $name)),
        }
    };
}

/// The page itself (served at the mount's root).
pub const INDEX: &[u8] = include_bytes!("../assets/index.html");

/// Everything else. `favicon.ico` is answered with `icon.svg`, as Python does (see [`find`]).
pub static ASSETS: [Asset; 15] = [
    asset!("icon.svg", "image/svg+xml"),
    asset!("icon-180.png", "image/png"),
    asset!("icon-192.png", "image/png"),
    asset!("icon-512.png", "image/png"),
    asset!("icon-maskable-512.png", "image/png"),
    asset!("manifest.webmanifest", "application/manifest+json"),
    asset!("sw.js", "text/javascript"),
    asset!("ico-library.svg", "image/svg+xml"),
    asset!("ico-playlist.svg", "image/svg+xml"),
    asset!("ico-presentation.svg", "image/svg+xml"),
    asset!("ico-new.svg", "image/svg+xml"),
    asset!("ico-replace.svg", "image/svg+xml"),
    asset!("ico-append.svg", "image/svg+xml"),
    asset!("ico-history.svg", "image/svg+xml"),
    asset!("ico-config.svg", "image/svg+xml"),
];

/// The asset served under `name` (a path relative to the mount, no leading slash).
pub fn find(name: &str) -> Option<&'static Asset> {
    let name = if name == "favicon.ico" {
        "icon.svg"
    } else {
        name
    };
    ASSETS.iter().find(|a| a.name == name)
}
