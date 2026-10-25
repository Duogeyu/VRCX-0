use crate::VrOverlayRuntimeServices;
use vrcx_0_host_desktop::vr_overlay::OverlayActivationButton;

use super::eligibility::WristOverlayStartMode;
use super::localization::{OverlayLocale, OverlayLocalizer};
use super::runtime::{
    HmdNotificationConfig, HmdNotificationPosition, HmdNotificationStyle, VrOverlayRuntimeConfig,
    WebSurfaceDims, WristOverlayHand,
};
use super::service::OverlayBackendPreference;
use super::web_overlay;
use super::{WristOverlayRenderOptions, WristOverlaySizePreset};
use vrcx_0_persistence::config::ConfigRepository;

pub const VR_OVERLAY_ENABLED_CONFIG_KEY: &str = "wristOverlayEnabled";
pub const VR_OVERLAY_BACKEND_CONFIG_KEY: &str = "wristOverlayBackend";
pub const VR_OVERLAY_START_MODE_CONFIG_KEY: &str = "wristOverlayStartMode";
pub const VR_OVERLAY_BUTTON_CONFIG_KEY: &str = "wristOverlayButton";
pub const VR_OVERLAY_HAND_CONFIG_KEY: &str = "wristOverlayHand";
pub const VR_OVERLAY_SIZE_CONFIG_KEY: &str = "wristOverlaySize";
pub const VR_OVERLAY_HIDE_PRIVATE_WORLDS_CONFIG_KEY: &str = "wristOverlayHidePrivateWorlds";
pub const VR_OVERLAY_DARK_BACKGROUND_CONFIG_KEY: &str = "wristOverlayDarkBackground";
pub const VR_OVERLAY_SHOW_DEVICES_CONFIG_KEY: &str = "wristOverlayShowDevices";
pub const VR_OVERLAY_SHOW_BATTERY_PERCENT_CONFIG_KEY: &str = "wristOverlayShowBatteryPercent";
pub const HMD_NOTIFICATIONS_ENABLED_CONFIG_KEY: &str = "hmdNotificationsEnabled";
pub const HMD_NOTIFICATION_START_MODE_CONFIG_KEY: &str = "hmdNotificationStartMode";
pub const HMD_NOTIFICATION_TIMEOUT_CONFIG_KEY: &str = "hmdNotificationTimeout";
pub const HMD_NOTIFICATION_OPACITY_CONFIG_KEY: &str = "hmdNotificationOpacity";
pub const HMD_NOTIFICATION_POSITION_CONFIG_KEY: &str = "hmdNotificationPosition";
pub const HMD_NOTIFICATION_STYLE_CONFIG_KEY: &str = "hmdNotificationStyle";
pub const HMD_NOTIFICATION_AVATARS_CONFIG_KEY: &str = "hmdNotificationAvatars";
pub const HMD_NOTIFICATION_TRACKER_DISCONNECT_CONFIG_KEY: &str = "hmdNotificationTrackerDisconnect";
const APP_LANGUAGE_CONFIG_KEY: &str = "appLanguage";
const DATE_TIME_HOUR12_CONFIG_KEY: &str = "dtHour12";
const SHOW_INSTANCE_ID_IN_LOCATION_CONFIG_KEY: &str = "VRCX_showInstanceIdInLocation";

#[cfg(windows)]
pub(crate) fn load_app_language(config: &ConfigRepository) -> String {
    config
        .get_string(APP_LANGUAGE_CONFIG_KEY, "en")
        .unwrap_or_else(|_| "en".to_string())
}

fn load_locale_settings(config: &ConfigRepository) -> (OverlayLocale, bool) {
    let locale = config
        .get_string(APP_LANGUAGE_CONFIG_KEY, "en")
        .map(|value| OverlayLocale::from_config(&value))
        .unwrap_or_default();
    let show_instance_id_in_location = config
        .get_bool(SHOW_INSTANCE_ID_IN_LOCATION_CONFIG_KEY, false)
        .unwrap_or(false);
    (locale, show_instance_id_in_location)
}

pub(crate) fn is_localizer_config_key(key: &str) -> bool {
    let key = vrcx_0_contracts::resolve_config_key(key);
    [
        APP_LANGUAGE_CONFIG_KEY,
        SHOW_INSTANCE_ID_IN_LOCATION_CONFIG_KEY,
    ]
    .into_iter()
    .any(|localizer_key| vrcx_0_contracts::resolve_config_key(localizer_key) == key)
}

pub(crate) fn load_localizer(config: &ConfigRepository) -> OverlayLocalizer {
    let (locale, show_instance_id) = load_locale_settings(config);
    OverlayLocalizer::with_instance_id(locale, show_instance_id)
}

pub(super) fn load_runtime_config(
    services: &dyn VrOverlayRuntimeServices,
) -> VrOverlayRuntimeConfig {
    let config = services.config();
    let start_mode = config
        .get_string(VR_OVERLAY_START_MODE_CONFIG_KEY, "vrchatVrMode")
        .map(|value| WristOverlayStartMode::from_config(&value))
        .unwrap_or_default();
    let backend = config
        .get_string(VR_OVERLAY_BACKEND_CONFIG_KEY, "auto")
        .map(|value| OverlayBackendPreference::from_config(&value))
        .unwrap_or_default();
    let button = config
        .get_string(VR_OVERLAY_BUTTON_CONFIG_KEY, "grip")
        .map(|value| match value.trim() {
            "menu" => OverlayActivationButton::Menu,
            _ => OverlayActivationButton::Grip,
        })
        .unwrap_or_default();
    let hand = config
        .get_string(VR_OVERLAY_HAND_CONFIG_KEY, "left")
        .map(|value| WristOverlayHand::from_config(&value))
        .unwrap_or_default();
    let size = config
        .get_string(
            VR_OVERLAY_SIZE_CONFIG_KEY,
            WristOverlaySizePreset::Normal.as_config(),
        )
        .map(|value| WristOverlaySizePreset::from_config(&value))
        .unwrap_or_default();
    let hide_private_worlds = config
        .get_bool(VR_OVERLAY_HIDE_PRIVATE_WORLDS_CONFIG_KEY, false)
        .unwrap_or(false);
    let dark_background = config
        .get_bool(VR_OVERLAY_DARK_BACKGROUND_CONFIG_KEY, true)
        .unwrap_or(true);
    let show_devices = config
        .get_bool(VR_OVERLAY_SHOW_DEVICES_CONFIG_KEY, true)
        .unwrap_or(true);
    let show_battery_percent = config
        .get_bool(VR_OVERLAY_SHOW_BATTERY_PERCENT_CONFIG_KEY, false)
        .unwrap_or(false);
    let hmd_enabled = config
        .get_bool(HMD_NOTIFICATIONS_ENABLED_CONFIG_KEY, false)
        .unwrap_or(false);
    let hmd_start_mode = config
        .get_string(HMD_NOTIFICATION_START_MODE_CONFIG_KEY, "vrchatVrMode")
        .map(|value| WristOverlayStartMode::from_config(&value))
        .unwrap_or_default();
    let hmd_timeout_ms = config
        .get_raw(HMD_NOTIFICATION_TIMEOUT_CONFIG_KEY)
        .ok()
        .flatten()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(5_000)
        .clamp(1_000, 30_000);
    let hmd_opacity_percent = config
        .get_raw(HMD_NOTIFICATION_OPACITY_CONFIG_KEY)
        .ok()
        .flatten()
        .and_then(|value| value.trim().parse::<u8>().ok())
        .unwrap_or(90)
        .min(100);
    let hmd_position = config
        .get_string(HMD_NOTIFICATION_POSITION_CONFIG_KEY, "bottom")
        .map(|value| HmdNotificationPosition::from_config(&value))
        .unwrap_or_default();
    let hmd_style = config
        .get_string(HMD_NOTIFICATION_STYLE_CONFIG_KEY, "standard")
        .map(|value| HmdNotificationStyle::from_config(&value))
        .unwrap_or_default();
    let (locale, show_instance_id_in_location) = load_locale_settings(config);
    let dt_hour12 = config
        .get_bool(DATE_TIME_HOUR12_CONFIG_KEY, false)
        .unwrap_or(false);

    VrOverlayRuntimeConfig {
        start_mode,
        backend,
        button,
        hand,
        hmd: HmdNotificationConfig {
            enabled: hmd_enabled,
            start_mode: hmd_start_mode,
            timeout_ms: hmd_timeout_ms,
            opacity_percent: hmd_opacity_percent,
            position: hmd_position,
            style: hmd_style,
            avatars: config
                .get_bool(HMD_NOTIFICATION_AVATARS_CONFIG_KEY, true)
                .unwrap_or(true),
            tracker_disconnect: config
                .get_bool(HMD_NOTIFICATION_TRACKER_DISCONNECT_CONFIG_KEY, true)
                .unwrap_or(true),
        },
        render: WristOverlayRenderOptions {
            size,
            hide_private_worlds,
            dark_background,
            show_devices,
            show_battery_percent,
        },
        locale,
        dt_hour12,
        show_instance_id_in_location,
        web: web_overlay::load(config).map(|settings| WebSurfaceDims {
            width: settings.width,
            height: settings.height,
            physical_width_mm: settings.physical_width_mm,
            refresh_ms: settings.refresh_ms.min(u32::MAX as u64) as u32,
        }),
    }
}
