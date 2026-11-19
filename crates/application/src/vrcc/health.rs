//! 星遇同频服务可达性的常驻探测。
//!
//! ## 为什么在 Rust 而不是前端
//!
//! 前端自己 `setInterval` 有个硬伤：窗口最小化到托盘后 WebView 会节流定时器，
//! 探测就停摆了。而「切出去很久、回来想知道 vrcc 还能不能用」恰恰是它最该可靠的
//! 场景。同一个理由当初把提醒轮询从 `useVrccController` 搬到了
//! [`super::super`] 之上的 `DesktopVrccRuntime::spawn_background_poll`。
//!
//! ## 结构照抄 VRChat 那条
//!
//! [`crate::profile::VrcStatusService`] 是同一个形状：单飞刷新（并发调用共用一次
//! 请求）、自适应间隔（正常 5 分钟、异常 2 分钟）、每次变化都推事件、循环可以被
//! 任务监管器取消。两边保持同构，排查时不用在脑子里切两套模型。
//!
//! ## 它探的是什么
//!
//! `GET /api/v1/status`：官方探活接口，不要令牌、不计入任何个人额度，服务端自己
//! 缓存一分钟。所以**采集不看登录态** —— 登录页也读这份快照。
//!
//! ## 判定不在这里
//!
//! 本层只记录"拿到了什么"（[`VrccHealthProbe`]），四态判定在前端
//! `src/lib/vrccHealth.ts`：那边有覆盖全部分支的测试，也负责把 `layer` /
//! `needsReauth` 翻成玩家看得懂的话。这和 `VrcStatusSnapshot` 的分工一致。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::{BoxFuture, FutureExt, Shared};
use tokio::sync::Mutex as AsyncMutex;
use vrcx_0_application_core::{
    sleep_until_due_or_stopped, Error, Result, RuntimeEventBus, TaskSupervisor, VrccHealthProbe,
    VrccHealthSnapshot,
};
use vrcx_0_contracts::vrcc::{ErrorDetail, ErrorLayer, TransportKind};
use vrcx_0_core::time::now_iso;

use super::VrccRemote;

/// 一切正常时的轮询间隔。
const OK_POLL_MS: u32 = 5 * 60 * 1_000;
/// 出问题时的轮询间隔：问得更勤，好让恢复尽快被看到。
const ISSUE_POLL_MS: u32 = 2 * 60 * 1_000;

type SharedRefresh = Shared<BoxFuture<'static, std::result::Result<VrccHealthSnapshot, String>>>;

struct VrccHealthServiceInner {
    remote: Arc<dyn VrccRemote>,
    event_bus: RuntimeEventBus,
    snapshot: Mutex<VrccHealthSnapshot>,
    refresh_in_flight: AsyncMutex<Option<(u64, SharedRefresh)>>,
    next_refresh_id: AtomicU64,
    loop_started: AtomicBool,
}

#[derive(Clone)]
pub struct VrccHealthService {
    inner: Arc<VrccHealthServiceInner>,
}

impl VrccHealthService {
    pub fn new(remote: Arc<dyn VrccRemote>, event_bus: RuntimeEventBus) -> Self {
        Self {
            inner: Arc::new(VrccHealthServiceInner {
                remote,
                event_bus,
                snapshot: Mutex::new(VrccHealthSnapshot {
                    polling_interval_ms: OK_POLL_MS,
                    ..VrccHealthSnapshot::default()
                }),
                refresh_in_flight: AsyncMutex::new(None),
                next_refresh_id: AtomicU64::new(1),
                loop_started: AtomicBool::new(false),
            }),
        }
    }

    pub fn snapshot(&self) -> VrccHealthSnapshot {
        self.inner.snapshot.lock().unwrap().clone()
    }

    /// 探一次。并发调用共用同一次请求。
    pub async fn refresh(&self) -> Result<VrccHealthSnapshot> {
        let (refresh_id, refresh) = {
            let mut current = self.inner.refresh_in_flight.lock().await;
            if let Some((refresh_id, refresh)) = current.as_ref() {
                (*refresh_id, refresh.clone())
            } else {
                let refresh_id = self.inner.next_refresh_id.fetch_add(1, Ordering::Relaxed);
                let service = self.clone();
                let refresh = async move {
                    service
                        .perform_refresh()
                        .await
                        .map_err(|error| error.to_string())
                }
                .boxed()
                .shared();
                *current = Some((refresh_id, refresh.clone()));
                (refresh_id, refresh)
            }
        };
        let result = refresh.await.map_err(Error::Custom);
        let mut current = self.inner.refresh_in_flight.lock().await;
        if current
            .as_ref()
            .is_some_and(|(current_id, _)| *current_id == refresh_id)
        {
            *current = None;
        }
        result
    }

    async fn perform_refresh(&self) -> Result<VrccHealthSnapshot> {
        self.update_snapshot(|snapshot| snapshot.refreshing = true);

        match self.inner.remote.status().await {
            Ok(status) => {
                let probe = if status.state.as_deref() == Some("ok") {
                    VrccHealthProbe::Ok { at: status.at }
                } else {
                    // 契约里 `state` 的 enum 只有 `"ok"`。别的值说明服务端加了新
                    // 语义，我们读不懂 —— 读不懂就不能说它正常。
                    VrccHealthProbe::StateNotOk {
                        raw: status.state.clone().unwrap_or_default(),
                    }
                };
                let snapshot = VrccHealthSnapshot {
                    probe,
                    checked_at: Some(now_iso()),
                    refreshing: false,
                    polling_interval_ms: OK_POLL_MS,
                };
                self.replace_snapshot(snapshot.clone());
                Ok(snapshot)
            }
            Err(error) => {
                // 本机限流（`ErrorLayer::Client`）：请求**根本没发出去**，服务状态
                // 没变 —— 拿它覆盖上一条结论，等于让一次无关的限流抹掉一条真实
                // 结论。只把 refreshing 放下。
                if error.layer == ErrorLayer::Client {
                    return Ok(self.update_snapshot(|snapshot| snapshot.refreshing = false));
                }
                let message = error.to_string();
                let probe = VrccHealthProbe::Failed {
                    layer: layer_name(error.layer).to_string(),
                    transport_kind: transport_kind_name(&error.detail).map(str::to_string),
                    needs_reauth: error.needs_reauth(),
                    message: message.clone(),
                };
                self.update_snapshot(|snapshot| {
                    snapshot.probe = probe;
                    snapshot.checked_at = Some(now_iso());
                    snapshot.polling_interval_ms = ISSUE_POLL_MS;
                    snapshot.refreshing = false;
                });
                Err(Error::Custom(message))
            }
        }
    }

    /// 起后台循环。重复调用无副作用（和 [`crate::profile::VrcStatusService`] 一致）。
    pub fn start_loop(&self, tasks: TaskSupervisor) {
        if self.inner.loop_started.swap(true, Ordering::AcqRel) {
            return;
        }

        let service = self.clone();
        tasks.spawn_cancellable(move |stop_token| async move {
            loop {
                let interval_ms = match service.refresh().await {
                    Ok(snapshot) => snapshot.polling_interval_ms,
                    Err(error) => {
                        tracing::warn!(error = %error, "failed to refresh vrcc health");
                        ISSUE_POLL_MS
                    }
                };
                if !sleep_until_due_or_stopped(
                    Duration::from_millis(u64::from(interval_ms)),
                    &stop_token,
                )
                .await
                {
                    service.inner.loop_started.store(false, Ordering::Release);
                    return;
                }
            }
        });
    }

    fn update_snapshot(&self, update: impl FnOnce(&mut VrccHealthSnapshot)) -> VrccHealthSnapshot {
        let snapshot = {
            let mut current = self.inner.snapshot.lock().unwrap();
            update(&mut current);
            current.clone()
        };
        self.inner.event_bus.emit(snapshot.clone());
        snapshot
    }

    fn replace_snapshot(&self, snapshot: VrccHealthSnapshot) {
        *self.inner.snapshot.lock().unwrap() = snapshot.clone();
        self.inner.event_bus.emit(snapshot);
    }
}

/// 和前端 `VrccErrorLayer` 联合类型逐字一致，前端可以直接当判别用。
fn layer_name(layer: ErrorLayer) -> &'static str {
    match layer {
        ErrorLayer::Input => "input",
        ErrorLayer::Client => "client",
        ErrorLayer::Transport => "transport",
        ErrorLayer::Server => "server",
        ErrorLayer::Session => "session",
    }
}

/// 和前端 `VrccTransportKind` 联合类型逐字一致。
fn transport_kind_name(detail: &ErrorDetail) -> Option<&'static str> {
    match detail {
        ErrorDetail::Transport { kind, .. } => Some(match kind {
            TransportKind::Dns => "dns",
            TransportKind::Connect => "connect",
            TransportKind::Timeout => "timeout",
            TransportKind::Body => "body",
            TransportKind::Other => "other",
        }),
        _ => None,
    }
}
