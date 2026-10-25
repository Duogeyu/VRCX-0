//! Offscreen web page renderer for VR overlays.
//!
//! The module opens a hidden host window, hosts a WebView2 browser inside it and
//! periodically captures the rendered page into an RGBA frame. The frames are
//! consumed by the VR overlay runtime; user input (pointer / wheel / keyboard)
//! is posted back into the hosted browser so the page stays interactive.
//!
//! Everything runs on one dedicated thread (WebView2 requires an STA thread with
//! a message pump). On non-Windows targets the module degrades to a stub that
//! reports an error instead of panicking.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

mod manifest;
pub use manifest::{
    default_plugin, ensure_builtin_plugin, load_plugins, resolve_plugin_url, OverlayWebPlugin,
};

mod static_server;
pub use static_server::StaticServer;

#[cfg(windows)]
mod engine;

/// Configuration for one offscreen web surface.
#[derive(Clone, Debug)]
pub struct WebSurfaceConfig {
    /// Page to load.
    pub url: String,
    /// Capture width in pixels.
    pub width: u32,
    /// Capture height in pixels.
    pub height: u32,
    /// Page zoom factor (1.0 = 100%).
    pub zoom: f64,
    /// Minimum interval between two captured frames.
    pub refresh_ms: u64,
    /// Optional custom user agent.
    pub user_agent: Option<String>,
    /// Optional CSS injected after the document is created.
    pub inject_css: Option<String>,
    /// Optional JS injected after the document is created.
    pub inject_js: Option<String>,
    /// Persistent profile directory (cookies / logins survive restarts).
    pub profile_dir: Option<PathBuf>,
}

impl WebSurfaceConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            width: 640,
            height: 480,
            zoom: 1.0,
            refresh_ms: 120,
            user_agent: None,
            inject_css: None,
            inject_js: None,
            profile_dir: None,
        }
    }

    pub fn from_plugin(plugin: &OverlayWebPlugin, profile_dir: Option<PathBuf>) -> Self {
        Self {
            url: plugin.url.clone(),
            width: plugin.width.unwrap_or(640).clamp(160, 2048),
            height: plugin.height.unwrap_or(480).clamp(120, 2048),
            zoom: plugin.zoom.unwrap_or(1.0).clamp(0.25, 4.0),
            refresh_ms: plugin.refresh_ms.unwrap_or(120).clamp(33, 5000),
            user_agent: plugin.user_agent.clone(),
            inject_css: plugin.inject_css.clone(),
            inject_js: plugin.inject_js.clone(),
            profile_dir,
        }
    }
}

/// One captured frame.
#[derive(Clone, Debug)]
pub struct WebSurfaceFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
    pub seq: u64,
}

impl WebSurfaceFrame {
    pub fn is_empty(&self) -> bool {
        self.rgba.is_empty()
    }
}

/// Input forwarded from the VR overlay into the hosted page.
#[derive(Clone, Debug)]
pub enum WebInputEvent {
    /// Pointer moved to `x`,`y` (page pixels).
    PointerMove { x: i32, y: i32 },
    /// Primary button pressed at `x`,`y`.
    PointerDown { x: i32, y: i32 },
    /// Primary button released at `x`,`y`.
    PointerUp { x: i32, y: i32 },
    /// Wheel scrolled at `x`,`y` by `delta` (120 = one notch down).
    Wheel { x: i32, y: i32, delta: i32 },
    /// Virtual key pressed or released.
    Key { vk: u16, down: bool },
    /// Printable character typed.
    Char { ch: char },
    /// Give the page keyboard focus.
    Focus,
    /// Navigate to another URL.
    Navigate(String),
    /// Reload the current page.
    Reload,
    /// Tear the surface down.
    Shutdown,
}

pub fn format_age(seq: u64, latest: u64) -> u64 {
    latest.saturating_sub(seq)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WebSurfaceStatus {
    Starting,
    Ready,
    Failed { message: String },
    Stopped,
}

impl Default for WebSurfaceStatus {
    fn default() -> Self {
        Self::Starting
    }
}

pub(crate) struct Shared {
    pub frame: Mutex<Option<WebSurfaceFrame>>,
    pub status: Mutex<WebSurfaceStatus>,
    pub seq: Mutex<u64>,
}

impl Shared {
    pub fn publish_frame(&self, width: u32, height: u32, rgba: Vec<u8>) {
        let mut seq = self.seq.lock().unwrap_or_else(|e| e.into_inner());
        *seq = seq.wrapping_add(1);
        let frame = WebSurfaceFrame {
            width,
            height,
            rgba: rgba.into(),
            seq: *seq,
        };
        if let Ok(mut slot) = self.frame.lock() {
            *slot = Some(frame);
        }
    }

    pub fn set_status(&self, status: WebSurfaceStatus) {
        if let Ok(mut slot) = self.status.lock() {
            *slot = status;
        }
    }
}

/// Handle to a running offscreen web surface.
pub struct WebSurface {
    shared: Arc<Shared>,
    sender: std::sync::mpsc::Sender<WebInputEvent>,
    thread: Option<std::thread::JoinHandle<()>>,
    config: WebSurfaceConfig,
}

impl WebSurface {
    /// Spawn the renderer. The page starts loading asynchronously; poll
    /// [`WebSurface::latest_frame`] until a frame arrives.
    pub fn spawn(config: WebSurfaceConfig) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            frame: Mutex::new(None),
            status: Mutex::new(WebSurfaceStatus::Starting),
            seq: Mutex::new(0),
        });
        let (sender, receiver) = std::sync::mpsc::channel();

        #[cfg(windows)]
        let thread = {
            let shared = Arc::clone(&shared);
            let cfg = config.clone();
            std::thread::Builder::new()
                .name("vrcx-web-surface".to_string())
                .spawn(move || engine::run_engine(cfg, shared, receiver))
                .map_err(|error| format!("spawn web surface thread failed: {error}"))?
        };

        #[cfg(not(windows))]
        let thread = {
            shared.set_status(WebSurfaceStatus::Failed {
                message: "web overlay is only supported on Windows".to_string(),
            });
            let _ = receiver;
            std::thread::Builder::new()
                .name("vrcx-web-surface-stub".to_string())
                .spawn(|| {})
                .map_err(|error| format!("spawn web surface thread failed: {error}"))?
        };

        Ok(Self {
            shared,
            sender,
            thread: Some(thread),
            config,
        })
    }

    pub fn config(&self) -> &WebSurfaceConfig {
        &self.config
    }

    /// Latest captured frame, if any.
    pub fn latest_frame(&self) -> Option<WebSurfaceFrame> {
        self.shared
            .frame
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or(None)
    }

    pub fn status(&self) -> WebSurfaceStatus {
        self.shared
            .status
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or(WebSurfaceStatus::Stopped)
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.status(), WebSurfaceStatus::Ready)
    }

    /// Forward an input event to the page. Returns `false` when the surface is gone.
    pub fn send(&self, event: WebInputEvent) -> bool {
        self.sender.send(event).is_ok()
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(160);
        self.config.height = height.max(120);
    }

    pub fn shutdown(&mut self) {
        let _ = self.sender.send(WebInputEvent::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for WebSurface {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Default profile directory for the offscreen browser (cookies, cache).
pub fn default_profile_dir() -> PathBuf {
    let base = dirs_local_app_data().unwrap_or_else(|| PathBuf::from("."));
    base.join("vrcx-0").join("web-overlay-profile")
}

/// Directory where users drop overlay plugin folders / manifests.
pub fn plugins_dir() -> PathBuf {
    let base = dirs_local_app_data().unwrap_or_else(|| PathBuf::from("."));
    base.join("vrcx-0").join("overlay-plugins")
}

#[cfg(windows)]
fn dirs_local_app_data() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

#[cfg(not(windows))]
fn dirs_local_app_data() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Make sure `dir` exists; returns the directory.
pub fn ensure_dir(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("create directory {} failed: {error}", dir.display()))?;
    Ok(dir.to_path_buf())
}
