use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use vrcx_0_contracts::{runtime_event_payload, RuntimeEventPayload};

use crate::backend_runtime::{BackendRuntimeTelemetry, RealtimeProjectionSync};
use crate::events::FriendProfileLoadStatusPayload;
use crate::ports::HostSessionProjection;
use crate::{FavoriteChangeScope, FavoriteEntityKind, RuntimeAuthScopeSnapshot};
use vrcx_0_core::json::RawJson;
use vrcx_0_core::OwnerId;

pub trait RuntimeEventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

#[derive(Clone, Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FavoritesChangedPayload {
    pub owner_user_id: OwnerId,
    pub endpoint: String,
    pub kind: FavoriteChangeScope,
    pub local: bool,
    pub remote: bool,
    pub changes: Vec<FavoriteChange>,
    pub requires_refresh: bool,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FavoriteChange {
    LocalAdded {
        kind: FavoriteEntityKind,
        #[serde(rename = "entityId")]
        entity_id: String,
        #[serde(rename = "groupName")]
        group_name: String,
    },
    LocalRemoved {
        kind: FavoriteEntityKind,
        #[serde(rename = "entityId")]
        entity_id: String,
        #[serde(rename = "groupName")]
        group_name: String,
    },
    LocalGroupCreated {
        kind: FavoriteEntityKind,
        #[serde(rename = "groupName")]
        group_name: String,
    },
    LocalGroupRenamed {
        kind: FavoriteEntityKind,
        #[serde(rename = "groupName")]
        group_name: String,
        #[serde(rename = "newGroupName")]
        new_group_name: String,
    },
    LocalGroupDeleted {
        kind: FavoriteEntityKind,
        #[serde(rename = "groupName")]
        group_name: String,
    },
    RemoteAdded {
        favorite: RawJson,
    },
    RemoteRemoved {
        #[serde(rename = "objectId")]
        object_id: String,
    },
}

impl FavoritesChangedPayload {
    pub fn invalidated(
        scope: &RuntimeAuthScopeSnapshot,
        kind: FavoriteChangeScope,
        local: bool,
        remote: bool,
    ) -> Self {
        Self::from_changes(scope, kind, local, remote, Vec::new())
    }

    pub fn from_changes(
        scope: &RuntimeAuthScopeSnapshot,
        kind: FavoriteChangeScope,
        local: bool,
        remote: bool,
        changes: Vec<FavoriteChange>,
    ) -> Self {
        let requires_refresh = changes.is_empty();
        Self {
            owner_user_id: OwnerId::new(scope.current_user_id.clone()),
            endpoint: scope.endpoint.clone(),
            kind,
            local,
            remote,
            changes,
            requires_refresh,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct VrcStatusSnapshot {
    pub status: String,
    pub indicator: String,
    pub summary: String,
    pub updated_at: Option<String>,
    pub last_fetched_at: Option<String>,
    pub polling_interval_ms: u32,
    pub refreshing: bool,
    pub error: String,
}

/// 一次探测拿到的事实。
///
/// 只报告**采集层看到的**，不在这里下"服务健不健康"的结论：四态判定在前端
/// （`src/lib/vrccHealth.ts`），那边有覆盖全部分支的测试，也负责把 `layer` /
/// `needsReauth` 翻成玩家看得懂的话。这和 VRChat 那条的分工一致 ——
/// `VrcStatusSnapshot` 同样只带原始字段。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize, specta::Type)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum VrccHealthProbe {
    /// 还没探过。
    #[default]
    Idle,
    /// 200，且 `state` 是 `ok`。
    Ok {
        /// 服务端给的时间戳（毫秒）。
        at: Option<i64>,
    },
    /// 200，但 `state` 不是 `ok`。契约里这个 enum 只有 `ok`，别的值我们读不懂。
    StateNotOk { raw: String },
    /// 请求失败。字段照 `VrccError` 的分类原样带出。
    Failed {
        /// `transport` / `server` / `client` / `session` / `input`。
        layer: String,
        /// 传输层卡在哪一步：`dns` / `connect` / `timeout` / `body` / `other`。
        #[serde(rename = "transportKind")]
        transport_kind: Option<String>,
        /// `403 client_*` 那一类：是我们自己的标识坏了，不是站方挂了。
        #[serde(rename = "needsReauth")]
        needs_reauth: bool,
        message: String,
    },
}

/// 星遇同频（vrcc.cc）服务可达性的快照。
///
/// ## 采集为什么在 Rust
///
/// 前端自己 `setInterval` 探测有个硬伤：窗口最小化到托盘后 WebView 会节流定时器，
/// 探测停摆。而"切出去很久、回来想知道 vrcc 还能不能用"恰恰是它最该可靠的场景。
/// 同一个理由当初把提醒轮询从 `useVrccController` 搬到了
/// `DesktopVrccRuntime::spawn_background_poll`。
///
/// 采集不看登录态：`GET /api/v1/status` 不要令牌，所以登录页也读这份快照。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct VrccHealthSnapshot {
    pub probe: VrccHealthProbe,
    /// 上次拿到结论的时刻（ISO 8601）。`None` 表示还没有过结论 ——
    /// 和"结论是 unknown"是两件事：探测被本机限流挡下时不产生结论，这里不动。
    pub checked_at: Option<String>,
    pub refreshing: bool,
    /// 下次自动探测的间隔（毫秒）。正常与异常时不同，和 VRChat 那条一致。
    pub polling_interval_ms: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRealtimeTransportEpoch {
    pub client_run_id: u64,
    pub generation: u64,
    pub session_generation: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeVrchatAuthFailurePayload {
    pub owner_user_id: OwnerId,
    pub endpoint: String,
    pub path: String,
    pub reason: String,
    pub status_code: i32,
    pub auth_scope_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realtime_transport: Option<RuntimeRealtimeTransportEpoch>,
}

runtime_event_payload!(FavoritesChangedPayload, "favoritesChanged");
runtime_event_payload!(VrcStatusSnapshot, "vrcStatus");
runtime_event_payload!(VrccHealthSnapshot, "vrccHealth");
runtime_event_payload!(RuntimeVrchatAuthFailurePayload, "runtimeVrchatAuthFailure");
runtime_event_payload!(BackendRuntimeTelemetry, "backendRuntimeTelemetry");
runtime_event_payload!(RealtimeProjectionSync, "realtimeProjectionSync");
runtime_event_payload!(HostSessionProjection, "updateIsGameRunning");
runtime_event_payload!(FriendProfileLoadStatusPayload, "friendProfileLoadStatus");

#[cfg(any(test, feature = "test-utils"))]
#[derive(Clone, Debug)]
pub struct RuntimeEventForTest {
    pub name: String,
    pub payload: RawJson,
}

#[derive(Clone, Default)]
pub struct RuntimeEventBus {
    sink: Arc<Mutex<Option<Arc<dyn RuntimeEventSink>>>>,
    #[cfg(any(test, feature = "test-utils"))]
    events: Arc<Mutex<Vec<RuntimeEventForTest>>>,
}

impl RuntimeEventBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_sink<S>(&self, sink: S)
    where
        S: RuntimeEventSink + 'static,
    {
        *self.sink.lock().unwrap() = Some(Arc::new(sink));
    }

    pub fn emit<T: RuntimeEventPayload>(&self, payload: T) {
        let event = T::EVENT_NAME;
        match serde_json::to_value(&payload) {
            Ok(value) => self.emit_value(event, value),
            Err(error) => {
                tracing::warn!(event, error = %error, "failed to serialize runtime event payload");
            }
        }
    }

    fn emit_value(&self, event: &str, payload: Value) {
        #[cfg(any(test, feature = "test-utils"))]
        {
            self.events.lock().unwrap().push(RuntimeEventForTest {
                name: event.to_string(),
                payload: payload.clone().into(),
            });
        }

        let sink = self.sink.lock().unwrap().clone();
        if let Some(sink) = sink {
            sink.emit(event, payload);
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn take_events_for_test(&self) -> Vec<RuntimeEventForTest> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{FavoriteChange, FavoritesChangedPayload, RuntimeEventBus};
    use crate::{FavoriteChangeScope, FavoriteEntityKind, RuntimeAuthScopeSnapshot};

    #[test]
    fn favorite_delta_preserves_scope_and_camel_case_wire_shape() {
        let bus = RuntimeEventBus::new();
        let scope = RuntimeAuthScopeSnapshot {
            current_user_id: "usr_self".into(),
            endpoint: "https://api.vrchat.cloud/api/1".into(),
            generation: 7,
            active: true,
        };

        bus.emit(FavoritesChangedPayload::from_changes(
            &scope,
            FavoriteChangeScope::Friend,
            true,
            false,
            vec![FavoriteChange::LocalAdded {
                kind: FavoriteEntityKind::Friend,
                entity_id: "usr_friend".into(),
                group_name: "Close".into(),
            }],
        ));

        let events = bus.take_events_for_test();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "favoritesChanged");
        assert_eq!(
            events[0].payload,
            json!({
                "ownerUserId": "usr_self",
                "endpoint": "https://api.vrchat.cloud/api/1",
                "kind": "friend",
                "local": true,
                "remote": false,
                "changes": [{
                    "type": "localAdded",
                    "kind": "friend",
                    "entityId": "usr_friend",
                    "groupName": "Close"
                }],
                "requiresRefresh": false
            })
            .into()
        );
    }
}
