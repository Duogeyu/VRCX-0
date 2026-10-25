//! Web overlay settings: where the wrist page comes from and how big it is.

use std::path::PathBuf;

use vrcx_0_persistence::config::ConfigRepository;
use vrcx_0_web_surface::{plugins_dir, OverlayWebPlugin};

pub const WEB_ENABLED_KEY: &str = "wristOverlayWebEnabled";
pub const WEB_PLUGIN_KEY: &str = "wristOverlayWebPlugin";
pub const WEB_URL_KEY: &str = "wristOverlayWebUrl";
pub const WEB_WIDTH_KEY: &str = "wristOverlayWebWidth";
pub const WEB_HEIGHT_KEY: &str = "wristOverlayWebHeight";
pub const WEB_ZOOM_KEY: &str = "wristOverlayWebZoom";
pub const WEB_REFRESH_KEY: &str = "wristOverlayWebRefreshMs";
pub const WEB_PHYSICAL_WIDTH_KEY: &str = "wristOverlayWebPhysicalWidthMm";

const DEFAULT_PLUGIN: &str = "vrcc";
const DEFAULT_WIDTH: u32 = 640;
const DEFAULT_HEIGHT: u32 = 640;
const DEFAULT_ZOOM: f64 = 1.0;
const DEFAULT_REFRESH_MS: u64 = 250;
const DEFAULT_PHYSICAL_WIDTH_MM: u32 = 260;

/// Fully resolved web overlay settings.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WebOverlaySettings {
    /// Explicit URL; when `None` the plugin directory is served over HTTP.
    pub url: Option<String>,
    /// Directory to serve (plugin folder) when `url` is absent.
    pub plugin_root: Option<PathBuf>,
    /// Page path relative to `plugin_root`.
    pub page: String,
    pub plugin_id: String,
    /// Manifest behind the page, when one was found. Carries the presentation
    /// hints (zoom / UA / injected CSS and JS) that the page itself cannot set.
    pub plugin: Option<OverlayWebPlugin>,
    pub width: u32,
    pub height: u32,
    pub zoom_x1000: u32,
    pub refresh_ms: u64,
    pub physical_width_mm: u32,
}

impl WebOverlaySettings {
    pub fn zoom(&self) -> f64 {
        f64::from(self.zoom_x1000) / 1000.0
    }

    /// Fingerprint used to decide whether the surface must be recreated.
    pub fn key(&self) -> String {
        let plugin = self.plugin.as_ref();
        let user_agent = plugin
            .and_then(|plugin| plugin.user_agent.as_deref())
            .unwrap_or("");
        let inject = plugin
            .map(|plugin| {
                u8::from(plugin.inject_css.is_some()) | (u8::from(plugin.inject_js.is_some()) << 1)
            })
            .unwrap_or(0);
        format!(
            "{}|{}|{}|{}x{}|{}|{}|{}|{user_agent}|{inject}",
            self.url.as_deref().unwrap_or(""),
            self.plugin_root
                .as_ref()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default(),
            self.page,
            self.width,
            self.height,
            self.refresh_ms,
            self.zoom_x1000,
            self.physical_width_mm,
        )
    }
}

fn read_u32(config: &ConfigRepository, key: &str, default: u32, min: u32, max: u32) -> u32 {
    config
        .get_raw(key)
        .ok()
        .flatten()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

/// Load the web overlay settings, or `None` when the feature is disabled.
pub(crate) fn load(config: &ConfigRepository) -> Option<WebOverlaySettings> {
    // This build ships the web wrist panel as the default surface.
    let enabled = config.get_bool(WEB_ENABLED_KEY, true).unwrap_or(true);
    if !enabled {
        return None;
    }
    let plugin_id = config
        .get_string(WEB_PLUGIN_KEY, DEFAULT_PLUGIN)
        .unwrap_or_else(|_| DEFAULT_PLUGIN.to_string())
        .trim()
        .to_string();
    let explicit_url = config
        .get_string(WEB_URL_KEY, "")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let dir = plugins_dir();
    // Provision the bundled plugin once per process; `load` runs on every
    // config reload, so avoid touching the filesystem each time.
    static BUILTIN_READY: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    BUILTIN_READY.get_or_init(|| {
        if let Err(error) = vrcx_0_web_surface::ensure_builtin_plugin(&dir) {
            tracing::warn!(%error, "web overlay: cannot provision built-in plugin");
        }
    });
    let mut page = "index.html".to_string();
    let mut plugin_root: Option<PathBuf> = None;
    let mut plugin: Option<OverlayWebPlugin> = None;

    if explicit_url.is_none() {
        plugin = load_plugin(&plugin_id).filter(|plugin| {
            plugin
                .base_dir
                .as_deref()
                .map(|base| base.join(page_of(plugin)).is_file())
                .unwrap_or(false)
        });
        plugin_root = plugin.as_ref().and_then(|plugin| plugin.base_dir.clone());
        if let Some(found) = plugin.as_ref() {
            page = page_of(found);
        } else {
            // No manifest: fall back to the bare directory layout.
            let root = plugins_dir().join(&plugin_id);
            if root.join(&page).is_file() {
                plugin_root = Some(root);
            }
        }
    }

    Some(WebOverlaySettings {
        url: explicit_url,
        plugin_root,
        page,
        plugin_id,
        plugin,
        width: read_u32(config, WEB_WIDTH_KEY, DEFAULT_WIDTH, 160, 2048),
        height: read_u32(config, WEB_HEIGHT_KEY, DEFAULT_HEIGHT, 120, 2048),
        zoom_x1000: read_u32(
            config,
            WEB_ZOOM_KEY,
            (DEFAULT_ZOOM * 1000.0) as u32,
            250,
            4000,
        ),
        refresh_ms: u64::from(read_u32(
            config,
            WEB_REFRESH_KEY,
            DEFAULT_REFRESH_MS as u32,
            33,
            5000,
        )),
        physical_width_mm: read_u32(
            config,
            WEB_PHYSICAL_WIDTH_KEY,
            DEFAULT_PHYSICAL_WIDTH_MM,
            80,
            800,
        ),
    })
}

/// Page path declared by a manifest, normalised for joining onto the server
/// root. Absolute URLs are rejected earlier, so a manifest page is always a
/// path inside the plugin directory.
fn page_of(plugin: &OverlayWebPlugin) -> String {
    let url = plugin.url.trim();
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://") {
        return "index.html".to_string();
    }
    let page = url.trim_start_matches('/');
    if page.is_empty() {
        "index.html".to_string()
    } else {
        page.to_string()
    }
}

fn load_plugin(plugin_id: &str) -> Option<OverlayWebPlugin> {
    let root = plugins_dir();
    vrcx_0_web_surface::load_plugins(&root)
        .into_iter()
        .find(|plugin| plugin.id == plugin_id)
}
