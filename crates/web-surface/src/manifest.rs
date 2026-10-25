//! Overlay plugin manifests.
//!
//! A plugin is a small directory (or a single JSON file) describing what the
//! wrist overlay should show. Because the overlay renders a real browser page,
//! plugins are just "a URL + presentation hints": users can drop a new plugin
//! into the plugins folder, tweak its JSON, and the overlay picks it up on the
//! next (re)load — no rebuild required.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One overlay plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverlayWebPlugin {
    /// Stable id, used in the config and the plugins folder.
    pub id: String,
    /// Display name.
    pub name: String,
    /// URL (https://… or file:///…).
    pub url: String,
    /// Capture width in pixels (default 640).
    #[serde(default)]
    pub width: Option<u32>,
    /// Capture height in pixels (default 480).
    #[serde(default)]
    pub height: Option<u32>,
    /// Page zoom (default 1.0).
    #[serde(default)]
    pub zoom: Option<f64>,
    /// Capture interval in ms (default 120).
    #[serde(default)]
    pub refresh_ms: Option<u64>,
    /// Optional user agent.
    #[serde(default)]
    pub user_agent: Option<String>,
    /// CSS injected into the page.
    #[serde(default)]
    pub inject_css: Option<String>,
    /// JS injected into the page.
    #[serde(default)]
    pub inject_js: Option<String>,
    /// Short description shown in the settings UI.
    #[serde(default)]
    pub description: Option<String>,
    /// Author / source line.
    #[serde(default)]
    pub author: Option<String>,
    /// Declared VRCC API scopes this plugin wants (documentation only).
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Directory the manifest was read from (not serialised). Relative `url`
    /// values are resolved against it, so a plugin directory can keep its page
    /// next to its manifest.
    #[serde(skip)]
    pub base_dir: Option<std::path::PathBuf>,
}

impl OverlayWebPlugin {
    /// Plugin with the built-in VRCC dashboard.
    pub fn vrcc_dashboard(url: impl Into<String>) -> Self {
        Self {
            id: "vrcc".to_string(),
            name: "星遇同频 VRCC".to_string(),
            url: url.into(),
            width: Some(640),
            height: Some(640),
            zoom: None,
            refresh_ms: Some(250),
            user_agent: None,
            inject_css: None,
            inject_js: None,
            description: Some("VRChat 中文社区（星遇同频）腕表面板".to_string()),
            author: Some("VRCC".to_string()),
            scopes: vec![
                "card".into(),
                "today".into(),
                "notifications".into(),
                "crossings".into(),
                "mutuals".into(),
                "maps".into(),
            ],
            base_dir: None,
        }
    }
}

/// Built-in default plugin: the local VRCC dashboard page.
pub fn default_plugin() -> OverlayWebPlugin {
    OverlayWebPlugin::vrcc_dashboard("file:///vrcc/index.html")
}

/// Built-in plugin page, embedded at compile time so a fresh install always has
/// a working overlay without shipping extra resource files.
const BUILTIN_VRCC_INDEX: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/vrcc/index.html"
));

const BUILTIN_VRCC_MANIFEST: &str = concat!(
    "{\n",
    "  \"id\": \"vrcc\",\n",
    "  \"name\": \"星遇同频 VRCC\",\n",
    "  \"description\": \"VRChat 中文社区（星遇同频）腕表面板：地图榜、提醒、我的星卡。\",\n",
    "  \"url\": \"index.html\",\n",
    "  \"width\": 640,\n",
    "  \"height\": 640,\n",
    "  \"zoom\": 1.0,\n",
    "  \"refresh_ms\": 250,\n",
    "  \"scopes\": [\"profile\", \"notifications\", \"maps\"]\n",
    "}\n"
);

/// Write the built-in VRCC plugin into `dir` when it is missing.
///
/// Existing files are never overwritten, so user edits survive upgrades.
pub fn ensure_builtin_plugin(dir: &Path) -> std::io::Result<()> {
    let plugin_dir = dir.join("vrcc");
    std::fs::create_dir_all(&plugin_dir)?;
    let index = plugin_dir.join("index.html");
    if !index.is_file() {
        std::fs::write(&index, BUILTIN_VRCC_INDEX)?;
    }
    let manifest = plugin_dir.join("manifest.json");
    if !manifest.is_file() {
        std::fs::write(&manifest, BUILTIN_VRCC_MANIFEST)?;
    }
    Ok(())
}

/// Load plugins from `dir`.
///
/// Both layouts are supported, matching what `ensure_builtin_plugin` writes:
///
/// ```text
/// plugins/<id>/manifest.json   one manifest per plugin directory
/// plugins/<name>.json          loose manifest files
/// ```
///
/// Every `*.json` file may contain one plugin object or an array of them. Files
/// that fail to parse are skipped (a warning is emitted through `tracing`).
pub fn load_plugins(dir: &Path) -> Vec<OverlayWebPlugin> {
    let mut plugins = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return plugins;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Plugin directory: read its manifest, if any.
            let manifest = path.join("manifest.json");
            if manifest.is_file() {
                collect_from_file(&manifest, &mut plugins);
            }
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        collect_from_file(&path, &mut plugins);
    }
    plugins.sort_by(|a, b| a.id.cmp(&b.id));
    plugins
}

fn collect_from_file(path: &Path, plugins: &mut Vec<OverlayWebPlugin>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&text);
    let Ok(value) = parsed else {
        tracing::warn!(path = %path.display(), "overlay plugin manifest is not valid JSON");
        return;
    };
    let base_dir = path
        .parent()
        .and_then(|parent| std::fs::canonicalize(parent).ok());
    let mut push = |plugin: OverlayWebPlugin| {
        plugins.push(OverlayWebPlugin {
            base_dir: base_dir.clone(),
            ..plugin
        });
    };
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                if let Ok(plugin) = serde_json::from_value::<OverlayWebPlugin>(item) {
                    push(plugin);
                }
            }
        }
        other => {
            if let Ok(plugin) = serde_json::from_value::<OverlayWebPlugin>(other) {
                push(plugin);
            }
        }
    }
}

/// Resolve a plugin URL against the plugin base directory, so manifests can use
/// relative paths (`index.html`, `./panel/index.html`, …).
pub fn resolve_plugin_url(plugin: &OverlayWebPlugin, base: &Path) -> String {
    let url = plugin.url.trim();
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://") {
        return url.to_string();
    }
    let path = base.join(url);
    let absolute = std::fs::canonicalize(&path).unwrap_or(path);
    let mut text = absolute.to_string_lossy().replace('\\', "/");
    if !text.starts_with('/') {
        text.insert(0, '/');
    }
    format!("file://{text}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_urls_become_file_urls() {
        let plugin = OverlayWebPlugin {
            id: "x".into(),
            name: "x".into(),
            url: "index.html".into(),
            width: None,
            height: None,
            zoom: None,
            refresh_ms: None,
            user_agent: None,
            inject_css: None,
            inject_js: None,
            description: None,
            author: None,
            scopes: Vec::new(),
            base_dir: None,
        };
        let url = resolve_plugin_url(&plugin, Path::new("C:/plugins/x"));
        assert!(url.starts_with("file:///C:/plugins/x/index.html"), "{url}");
    }

    #[test]
    fn absolute_urls_pass_through() {
        let mut plugin = default_plugin();
        plugin.url = "https://vrcc.cc/".into();
        assert_eq!(
            resolve_plugin_url(&plugin, Path::new("C:/anything")),
            "https://vrcc.cc/"
        );
    }

    #[test]
    fn plugin_directories_are_discovered() {
        // Layout: <dir>/alpha/manifest.json + index.html, and a loose JSON file.
        let root = std::env::temp_dir().join("vrcx-web-plugins-discovery-test");
        let _ = std::fs::remove_dir_all(&root);
        let alpha = root.join("alpha");
        std::fs::create_dir_all(&alpha).expect("create plugin dir");
        std::fs::write(
            alpha.join("manifest.json"),
            r#"{"id":"alpha","name":"Alpha","url":"index.html"}"#,
        )
        .expect("write manifest");
        std::fs::write(alpha.join("index.html"), "<!doctype html>").expect("write page");
        std::fs::write(
            root.join("loose.json"),
            r#"{"id":"loose","name":"Loose","url":"https://example.com/"}"#,
        )
        .expect("write loose manifest");

        let plugins = load_plugins(&root);
        let ids = plugins
            .iter()
            .map(|plugin| plugin.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["alpha", "loose"]);
        let alpha = plugins.iter().find(|plugin| plugin.id == "alpha").unwrap();
        // The manifest's own directory travels with the plugin.
        assert_eq!(alpha.base_dir.as_deref(), Some(alpha_path(&root).as_path()));
    }

    fn alpha_path(root: &Path) -> std::path::PathBuf {
        std::fs::canonicalize(root.join("alpha")).expect("canonicalize")
    }
}
