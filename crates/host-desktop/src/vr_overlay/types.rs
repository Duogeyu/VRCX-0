use serde::{Deserialize, Serialize};
use vrcx_0_vr_overlay::{OverlaySize, OverlaySurfaceId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendStartError {
    pub message: String,
    pub reason: BackendStartErrorReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendStartErrorReason {
    Other,
    RuntimeUnavailable,
    Unsupported,
}

impl BackendStartError {
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            reason: BackendStartErrorReason::Unsupported,
        }
    }

    pub fn transient(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            reason: BackendStartErrorReason::Other,
        }
    }

    pub fn runtime_unavailable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            reason: BackendStartErrorReason::RuntimeUnavailable,
        }
    }
}

impl std::fmt::Display for BackendStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct OverlaySurfaceConfig {
    pub surface_id: OverlaySurfaceId,
    pub size: OverlaySize,
    pub physical_width_meters: f32,
    pub placement: OverlayPlacement,
    #[serde(default)]
    pub activation_button: OverlayActivationButton,
    #[serde(default)]
    pub force_visible: bool,
    /// Optional lower bound for frame uploads while the surface is visible.
    /// Web pages need a much faster cadence than the 1 Hz default of static
    /// panels; `None` keeps the built-in interval.
    #[serde(default)]
    pub min_upload_interval_ms: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum OverlayPlacement {
    TrackedDeviceRelative {
        device_hint: String,
    },
    HeadLocked {
        offset_y_meters: f32,
        distance_meters: f32,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum OverlayActivationButton {
    #[default]
    Grip,
    Menu,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct VrDeviceSnapshot {
    pub label: String,
    pub serial: Option<String>,
    pub status: VrDeviceStatus,
    pub battery_percent: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum VrDeviceStatus {
    Normal,
    LowBattery,
    CriticalBattery,
    Charging,
    TrackingWarning,
    Disconnected,
}

/// Pointer input produced by the VR backend for an interactive overlay
/// surface (currently the web wrist panel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayInputEvent {
    PointerMove { x: i32, y: i32 },
    PointerDown { x: i32, y: i32 },
    PointerUp { x: i32, y: i32 },
    Wheel { x: i32, y: i32, delta: i32 },
}
