//! 星遇同频运行时：把应用层服务挂到桌面运行时上。

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use vrcx_0_application::vrcc::{StoredCredential, VrccService, VrccTokenStore};
use vrcx_0_contracts::vrcc::{
    VrccAnnouncements, VrccBoard, VrccBoardSort, VrccConnectionSnapshot, VrccNotes,
    VrccRecentNotifications, VrccStatus, VrccToday, VrccUnread, VrccUsageSnapshot,
    VrccWristSummary,
};
use vrcx_0_integrations::vrcc::{resolve_client_id, VrccError};
use vrcx_0_outbound_adapters::LocalVrccTokenStore;

/// 把本地加密仓库适配成应用层端口。
///
/// 适配层只做类型转换：契约形状在 `application`，密文读写在 outbound-adapters。
struct TokenStoreAdapter {
    inner: LocalVrccTokenStore,
}

impl VrccTokenStore for TokenStoreAdapter {
    fn load(&self) -> vrcx_0_application_core::Result<Option<StoredCredential>> {
        let stored = self.inner.load()?;
        Ok(stored.map(|credential| StoredCredential {
            token: credential.token,
            star: credential.star,
            vrc_name: credential.vrc_name,
            scope: credential.scope,
        }))
    }

    fn save(&self, credential: &StoredCredential) -> vrcx_0_application_core::Result<()> {
        self.inner
            .save(&vrcx_0_outbound_adapters::StoredVrccCredential {
                token: credential.token.clone(),
                star: credential.star,
                vrc_name: credential.vrc_name.clone(),
                scope: credential.scope.clone(),
            })
    }

    fn clear(&self) -> vrcx_0_application_core::Result<()> {
        self.inner.clear()
    }
}

/// 星遇同频提醒或未读数发生变化时发给前端的事件名。
///
/// 只作为「该刷新了」的信号，不带正文——玩家内容没必要经手额外的事件层。
pub const VRCC_NOTICES_CHANGED_EVENT: &str = "vrccNoticesChanged";

/// 令牌被服务器判了失效（`404 gone`）、本地已经清掉时发给前端的事件名。
///
/// 前端收到后重读连接状态，显示「连接已失效，请重新连接」。
pub const VRCC_CONNECTION_CHANGED_EVENT: &str = "vrccConnectionChanged";

/// 桌面侧的星遇同频运行时。
pub struct DesktopVrccRuntime {
    service: Arc<VrccService>,
}

impl DesktopVrccRuntime {
    /// 后台轮询的基准间隔。
    ///
    /// 官方建议按 `me/notifications` 返回的 `every`（通常 60 秒）问，最少隔
    /// 25 秒。这里取 60 秒和 `every` 里大的那个：`me/unread` 每天 1500 次，
    /// 一分钟问一次正好够一整天。
    const BACKGROUND_POLL_BASE: Duration = Duration::from_secs(60);
    /// 抖动上限，避免所有客户端撞在同一秒（官方脚本也这样做）。
    const BACKGROUND_POLL_JITTER: Duration = Duration::from_secs(20);

    pub fn new(store: LocalVrccTokenStore) -> Self {
        Self::with_client_override(store, None, None)
    }

    /// 用指定的 `X-Client` 标识与代理构造。
    ///
    /// `client_override` 来自配置：正式态把平台发的 `app_…` 填进去即可，
    /// 不用重新编译。`None` 时按 `resolve_client_id` 的优先级回落到常量。
    ///
    /// `proxy_url` 必须传宿主应用的统一代理（`runtime_host().proxy_url()`）：
    /// 本机实测 `vrcc.cc` 直连 DNS 超时，不接代理时所有接口都会「暂时不可用」。
    pub fn with_client_override(
        store: LocalVrccTokenStore,
        client_override: Option<&str>,
        proxy_url: Option<&str>,
    ) -> Self {
        let usage_store = Arc::new(vrcx_0_outbound_adapters::LocalVrccUsageStore::new(
            Arc::clone(store.db()),
        ));
        let port = Arc::new(TokenStoreAdapter { inner: store });
        let client_id = resolve_client_id(client_override);
        Self {
            service: Arc::new(VrccService::with_usage_store(
                port,
                client_id,
                proxy_url.map(str::to_string),
                Some(usage_store),
            )),
        }
    }

    /// 腕表摘要。纯内存读取，不发请求。
    pub fn wrist_summary(&self, world_id: Option<&str>) -> Option<VrccWristSummary> {
        self.service.wrist_summary(world_id)
    }

    pub fn connection(&self) -> VrccConnectionSnapshot {
        self.service.snapshot()
    }

    pub fn usage(&self) -> VrccUsageSnapshot {
        self.service.usage()
    }

    pub fn client_id(&self) -> &str {
        self.service.client_id()
    }

    /// 地图榜：开放数据，不需要连接。
    pub async fn boards(&self, limit: u32, sort: VrccBoardSort) -> Result<VrccBoard, String> {
        self.service
            .boards(limit, sort)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn status(&self) -> Result<VrccStatus, String> {
        self.service
            .status()
            .await
            .map_err(|error| error.to_string())
    }

    /// 公告和停站。服务器还没有这个接口时为 `None`。
    pub async fn announcements(&self) -> Result<Option<VrccAnnouncements>, String> {
        self.service
            .announcements()
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn begin_device_auth(
        &self,
        scope: Option<String>,
    ) -> Result<vrcx_0_application::vrcc::VrccDeviceAuthPrompt, String> {
        self.service
            .begin_device_auth(scope.as_deref())
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn poll_device_auth(
        &self,
        prompt: &vrcx_0_application::vrcc::VrccDeviceAuthPrompt,
    ) -> Result<vrcx_0_application::vrcc::VrccDeviceAuthStep, String> {
        self.service
            .poll_device_auth(prompt)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn disconnect(&self) -> Result<(), String> {
        self.service
            .disconnect()
            .await
            .map_err(|error| error.to_string())
    }

    /// 最近的提醒。`mark` 为 `true` 时同时把已读状态同步到账号（跨设备）。
    pub async fn recent_notifications(
        &self,
        mark: bool,
    ) -> Result<VrccRecentNotifications, String> {
        self.service
            .recent_notifications(mark)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn poll_notifications(
        &self,
    ) -> Result<vrcx_0_contracts::vrcc::VrccNotifications, String> {
        self.service
            .poll_notifications()
            .await
            .map_err(|error| error.to_string())
    }

    /// 几处未读数。
    ///
    /// `refresh` 为 `false` 时只读后台轮询留下的缓存，不发请求——前端收到
    /// 「有变化」的事件时用这个，后台刚问过，不必再花一次。服务器还没有
    /// `me/unread` 时为 `None`。
    pub async fn unread(&self, refresh: bool) -> Result<Option<VrccUnread>, String> {
        if !refresh {
            return Ok(self.service.cached_unread());
        }
        self.service
            .refresh_unread()
            .await
            .map(|result| result.map(|(counts, _)| counts))
            .map_err(|error| error.to_string())
    }

    /// 通知中心。服务器还没有这个接口时为 `None`。
    pub async fn notes(&self) -> Result<Option<VrccNotes>, String> {
        self.service
            .notes()
            .await
            .map_err(|error| error.to_string())
    }

    /// 当前所在地图在地图榜上的状态。`off` 的地图返回 `None`（不给看）。
    pub async fn my_map(
        &self,
        world: &str,
    ) -> Result<Option<vrcx_0_contracts::vrcc::VrccMyMap>, String> {
        self.service
            .my_map(world)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn today(&self) -> Result<VrccToday, String> {
        self.service
            .today()
            .await
            .map_err(|error| error.to_string())
    }

    /// 「互相连上」的星友列表。需要 `mutuals` 敏感权限。
    pub async fn mutuals(&self) -> Result<vrcx_0_contracts::vrcc::VrccMutuals, String> {
        self.service
            .mutuals()
            .await
            .map_err(|error| error.to_string())
    }

    /// 起一个后台轮询任务，把新提醒推成系统通知。
    ///
    /// **为什么要放到 Rust 侧**：原来轮询在前端（`useVrccController` 的
    /// `setTimeout`）。窗口隐藏到托盘后 WebView 仍存活（`window.hide()` 不销毁
    /// WebView），但 Chromium 会节流隐藏页面的定时器，60 秒的间隔可能被拉到
    /// 几分钟——而"后台也能收到提醒"恰恰是这个功能最重要的场景。
    ///
    /// 这里用 `TaskSupervisor::spawn_cancellable`，让任务随应用退出一起停，
    /// 不用自己管生命周期。
    ///
    /// 通知走宿主注入的 `DesktopNotifier`：Headless 下没人挂载它，
    /// `show()` 返回 `Err`，这里只记日志，不会 panic。
    ///
    /// `event_sink` 用来告诉前端「有新数据了」或「连接失效了」。前端不再自己
    /// 轮询——双轨会让两边各拉一次、`mark` 语义互相打架，而且隐藏窗口时前端
    /// 定时器会被节流。
    pub fn spawn_background_poll(
        &self,
        supervisor: &vrcx_0_application_core::TaskSupervisor,
        notifier: Arc<dyn crate::notification::DesktopNotifier>,
        event_sink: Arc<dyn vrcx_0_application_core::RuntimeEventSink>,
    ) -> vrcx_0_application_core::TaskSpawnOutcome {
        let service = Arc::clone(&self.service);
        let base = Self::BACKGROUND_POLL_BASE;
        let jitter = Self::BACKGROUND_POLL_JITTER;

        supervisor.spawn_cancellable(move |stop_token| {
            let service = Arc::clone(&service);
            let notifier = Arc::clone(&notifier);
            let event_sink = Arc::clone(&event_sink);
            async move {
                // 启动探针：没有它，分不清「轮询没起来」和「起来了但没数据」。
                tracing::info!(
                    interval_secs = base.as_secs(),
                    jitter_secs = jitter.as_secs(),
                    "vrcc: background poll task started"
                );
                let mut round: u64 = 0;
                loop {
                    if stop_token.is_stop_requested() {
                        tracing::info!(round, "vrcc: background poll stop requested");
                        return;
                    }
                    round = round.wrapping_add(1);
                    // 未连接时不必轮询：个人接口会直接报错。
                    if service.snapshot().connected {
                        match poll_once(&service, notifier.as_ref()).await {
                            Ok(outcome) => {
                                // 每一轮都记录结果，但用 debug：正常运行时不该刷屏，
                                // 排查时打开 RUST_LOG=debug 就有完整时间线。
                                tracing::debug!(
                                    round,
                                    fresh = outcome.fresh,
                                    unread = ?outcome.unread,
                                    toast_attempted = outcome.toast_attempted,
                                    changed = outcome.changed,
                                    "vrcc: background poll round finished"
                                );
                                // 只发一个「变了」的信号，具体数据前端自己拉。
                                // 不在事件里带提醒正文：那会把玩家内容写进
                                // 事件经手的所有层，多一条泄露面。
                                if let Some(event) = outcome.event_to_emit() {
                                    event_sink.emit(event, json!({}));
                                }
                            }
                            Err(error) if error.is_gone() => {
                                // 令牌失效，应用层已经把它清掉了：之后几轮会因为
                                // 未连接而跳过，等玩家重新连接。告诉前端改显示。
                                tracing::info!(
                                    round,
                                    "vrcc: token gone, background poll idle until reconnect"
                                );
                                event_sink.emit(VRCC_CONNECTION_CHANGED_EVENT, json!({}));
                            }
                            Err(error) => {
                                // 不把错误抛出去：一次失败不该杀掉整个轮询。
                                // 用 warn 而不是 debug：轮询连续失败是需要被看到的，
                                // 但它不是致命错误，所以不升级成 error 去污染错误日志。
                                tracing::warn!(round, %error, "vrcc: background poll failed");
                            }
                        }
                    } else {
                        tracing::debug!(round, "vrcc: background poll skipped (not connected)");
                    }
                    // 用带抖动的固定间隔。这里刻意用 `sleep` 而不是绝对时刻对齐：
                    // 唤醒后最多补一次，不会像"补 N 个周期"那样突发请求。
                    let every = Duration::from_secs(service.poll_every_secs().max(0) as u64);
                    let delay = base.max(every) + Duration::from_millis(jitter_millis(&jitter));
                    tracing::debug!(
                        round,
                        delay_ms = delay.as_millis() as u64,
                        "vrcc: background poll sleeping"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        })
    }
}

/// 一次轮询的结果，用于日志与事件判定。
///
/// 拆成结构体而不是裸 `bool`：排查时最常问的问题分别是「来了几条新的」、
/// 「现在几处未读」、「弹出去几次」，只回一个布尔值等于什么都没说。
#[derive(Clone, Copy, Debug, Default)]
struct PollOutcome {
    /// 这一轮 `me/notifications` 给的新提醒条数。
    fresh: usize,
    /// 实际尝试弹系统通知的次数（受 `MAX_NOTIFIED_PER_POLL` 限制）。
    toast_attempted: usize,
    /// `me/unread` 的合计；服务器还没有这个接口时为 `None`。
    unread: Option<i64>,
    /// 是否需要让前端刷新。
    changed: bool,
}

impl PollOutcome {
    /// 该给前端发哪个事件；`None` 表示这一轮没有变化，不必打扰前端。
    ///
    /// 抽成方法而不是在循环里写 `if outcome.changed`：这是「要不要通知前端」
    /// 的唯一判断点，值得单独钉住——写反了会导致界面永远不刷新（静默），
    /// 或者每轮都刷新（浪费）。
    fn event_to_emit(&self) -> Option<&'static str> {
        if self.changed {
            Some(VRCC_NOTICES_CHANGED_EVENT)
        } else {
            None
        }
    }
}

/// 一次轮询。
///
/// 1. `me/notifications?since=`：官方给桌面客户端设计的轮询，**没有每天上限**，
///    每次只给游标之后的新提醒，所以每条只弹一次。第一次不带游标，服务器只
///    告诉「现在到哪了」，不给旧提醒——启动时不会把一天的旧提醒全弹一遍。
/// 2. `me/unread`（1.13.0 起）：几处红点的数字，每天 1500 次单独计数，给腕表和
///    页面角标。
/// 3. 旧服务器没有 `me/unread` 时，只在刚连上或来了新提醒时拉一次「最近的提醒」
///    （每天 1000 次的第一份读），腕表的未读数靠它。
///
/// 原来每轮都拉「最近的提醒」：一分钟一次一整天要一千多次，超过第一份读的
/// 每天上限；而且只要玩家没在网页上看过，同一条提醒每轮都会再弹一次。
async fn poll_once(
    service: &VrccService,
    notifier: &dyn crate::notification::DesktopNotifier,
) -> Result<PollOutcome, VrccError> {
    let first_poll = service.notifications_cursor().is_none();
    let fresh = match service.poll_notifications().await {
        Ok(result) => result.items,
        // 离上一次不到 25 秒（比如刚重新连上）：这一轮没有新的，下一轮补上。
        Err(VrccError::TooSoon { .. }) => Vec::new(),
        Err(error) => return Err(error),
    };

    // 拉取与处理分开：`handle_new_notices` 只接收数据、不看网络，
    // 于是「拿到什么样的数据 → 弹哪些通知」可以脱离服务端直接测。
    let mut outcome = handle_new_notices(&fresh, notifier);

    match service.refresh_unread().await {
        Ok(Some((counts, changed))) => {
            outcome.unread = Some(counts.total());
            outcome.changed |= changed;
        }
        Ok(None) => {
            if first_poll || !fresh.is_empty() {
                match service.recent_notifications(false).await {
                    Ok(_) => outcome.changed = true,
                    Err(error) if error.is_gone() => return Err(error),
                    Err(error) => tracing::warn!(%error, "vrcc: recent notices refresh failed"),
                }
            }
        }
        Err(error) if error.is_gone() => return Err(error),
        // 未读数拿不到不影响提醒：记一笔，下一轮再试。
        Err(error) => tracing::warn!(%error, "vrcc: unread count failed"),
    }
    Ok(outcome)
}

/// 处理一批刚拉到的新提醒：挑出要弹的、交给 notifier。
///
/// 这个函数不发请求也不读时钟，是排查「为什么没弹」时最该被测试覆盖的一段。
fn handle_new_notices(
    items: &[vrcx_0_contracts::vrcc::VrccNotificationItem],
    notifier: &dyn crate::notification::DesktopNotifier,
) -> PollOutcome {
    tracing::debug!(fresh = items.len(), "vrcc: fetched new notices");

    // 决策与投递分开：`select_notices_to_notify` 是纯函数，可以脱离网络和
    // 通知桥单独测。这里只负责把它选出来的结果交给 notifier。
    let selection = select_notices_to_notify(items, MAX_NOTIFIED_PER_POLL);
    if selection.skipped > 0 {
        // 一轮来了太多：老的那几条不弹，在「提醒」页里看得到。
        tracing::warn!(
            fresh = items.len(),
            limit = MAX_NOTIFIED_PER_POLL,
            skipped = selection.skipped,
            "vrcc: new notices exceed per-poll toast limit; older ones not toasted"
        );
    }

    let mut toast_attempted = 0usize;
    for notice in &selection.notices {
        match notifier.show(&notice.title, notice.body.as_deref(), None, true, None) {
            Ok(()) => {
                // 只记 id 与长度，不记正文：提醒内容属于玩家数据。
                tracing::info!(
                    notice_id = notice.id,
                    body_len = notice.body.as_deref().map(str::len).unwrap_or(0),
                    "vrcc: desktop notification dispatched"
                );
            }
            Err(error) => {
                // Headless 或未挂载通知桥时走到这里。
                tracing::warn!(notice_id = notice.id, %error, "vrcc: desktop notification not delivered");
            }
        }
        toast_attempted += 1;
    }

    PollOutcome {
        fresh: items.len(),
        toast_attempted,
        unread: None,
        changed: !items.is_empty(),
    }
}

/// 每次轮询最多弹几条。
///
/// `me/notifications` 一次最多给 5 条，全弹出来会刷屏。
const MAX_NOTIFIED_PER_POLL: usize = 3;

/// 在 `0..jitter` 里取一个伪随机毫秒数。
///
/// 不用 `rand` crate：只要一个够散的抖动，混入当前时间纳秒即可，
/// 避免为这点需求引入依赖。
fn jitter_millis(jitter: &Duration) -> u64 {
    let span = jitter.as_millis().max(1) as u64;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos() as u64)
        .unwrap_or(0);
    now % span
}

/// 一条准备好投递的通知（已经从契约类型里摘出来，便于测试构造）。
#[derive(Clone, Debug, PartialEq, Eq)]
struct NoticeToNotify {
    id: i64,
    title: String,
    body: Option<String>,
}

/// `select_notices_to_notify` 的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct NoticeSelection {
    /// 本轮要弹的通知，已按 `limit` 截断，旧的在前。
    notices: Vec<NoticeToNotify>,
    /// 因超出 `limit` 而没弹的条数。
    ///
    /// 游标已经过去了，它们不会在下一轮再来，只能在「提醒」页里看到。
    /// 单独暴露而不是让调用方再数一遍：日志里要靠它说话。
    skipped: usize,
}

/// 决定这一轮该弹哪些通知。
///
/// 纯函数：不碰网络、不碰通知桥、不读时钟。
///
/// 规则：
/// - `items` 都是游标之后的新提醒（旧的在前），不用再判已读；
/// - 最多弹 `limit` 条，挑最新的那几条，按时间顺序弹，最新的落在最上面；
/// - 标题为空时用「星遇同频」，避免弹出一个没有标题的空通知。
fn select_notices_to_notify(
    items: &[vrcx_0_contracts::vrcc::VrccNotificationItem],
    limit: usize,
) -> NoticeSelection {
    let skipped = items.len().saturating_sub(limit);
    let notices = items[skipped..]
        .iter()
        .map(|item| NoticeToNotify {
            id: item.id,
            title: item
                .title
                .clone()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| "星遇同频".to_string()),
            body: item.body.clone(),
        })
        .collect();
    NoticeSelection { notices, skipped }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use vrcx_0_contracts::vrcc::VrccNotificationItem;

    fn item(id: i64, title: Option<&str>, body: Option<&str>) -> VrccNotificationItem {
        VrccNotificationItem {
            id,
            title: title.map(str::to_string),
            body: body.map(str::to_string),
            at: Some(1_700_000_000_000),
        }
    }

    #[test]
    fn every_new_notice_is_selected_in_order() {
        let items = vec![item(41, Some("a"), None), item(42, Some("b"), None)];
        let selection = select_notices_to_notify(&items, 10);
        assert_eq!(
            selection.notices.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![41, 42]
        );
        assert_eq!(selection.skipped, 0);
    }

    #[test]
    fn nothing_new_selects_nothing() {
        let selection = select_notices_to_notify(&[], 3);
        assert!(selection.notices.is_empty());
        assert_eq!(selection.skipped, 0);
    }

    #[test]
    fn over_the_limit_keeps_the_newest_and_counts_the_rest() {
        let items = (1..=5).map(|id| item(id, None, None)).collect::<Vec<_>>();
        let selection = select_notices_to_notify(&items, 3);
        // 弹最新的三条，按时间顺序。
        assert_eq!(
            selection.notices.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
        assert_eq!(selection.skipped, 2);
    }

    #[test]
    fn blank_title_falls_back_so_no_notification_is_titleless() {
        let items = vec![item(1, Some("   "), Some("body"))];
        let selection = select_notices_to_notify(&items, 10);
        assert_eq!(selection.notices[0].title, "星遇同频");
    }

    #[test]
    fn keeps_the_body_verbatim() {
        let items = vec![item(1, Some("标题"), Some("正文内容"))];
        let selection = select_notices_to_notify(&items, 10);
        assert_eq!(selection.notices[0].body.as_deref(), Some("正文内容"));
    }

    #[test]
    fn jitter_stays_below_the_span() {
        // 抖动必须是 0..span，否则会在基准间隔之外多等一截。
        let span = Duration::from_secs(20);
        for _ in 0..200 {
            assert!(jitter_millis(&span) < 20_000);
        }
        // 极小 span 不能返回 0 以外的值（除零保护）。
        assert_eq!(jitter_millis(&Duration::from_millis(0)), 0);
    }

    // -- handle_new_notices：拉取与投递之间的那一段 ----------------------

    /// 记录调用的假通知桥。
    #[derive(Default)]
    struct RecordingNotifier {
        shown: Mutex<Vec<(String, Option<String>)>>,
        fail: bool,
    }

    impl RecordingNotifier {
        fn failing() -> Self {
            Self {
                shown: Mutex::new(Vec::new()),
                fail: true,
            }
        }

        fn count(&self) -> usize {
            self.shown.lock().unwrap().len()
        }
    }

    impl crate::notification::DesktopNotifier for RecordingNotifier {
        fn show(
            &self,
            title: &str,
            body: Option<&str>,
            _image: Option<&str>,
            _play_sound: bool,
            _action: Option<&crate::notification::DesktopNotificationAction>,
        ) -> Result<(), String> {
            if self.fail {
                return Err("notifier unavailable".to_string());
            }
            self.shown
                .lock()
                .unwrap()
                .push((title.to_string(), body.map(str::to_string)));
            Ok(())
        }
    }

    #[test]
    fn handle_notifies_new_notices_and_reports_changed() {
        let notifier = RecordingNotifier::default();
        let outcome = handle_new_notices(&[item(2, Some("新"), Some("正文"))], &notifier);
        assert_eq!(outcome.fresh, 1);
        assert_eq!(outcome.toast_attempted, 1);
        assert!(outcome.changed);
        let shown = notifier.shown.lock().unwrap().clone();
        assert_eq!(shown, vec![("新".to_string(), Some("正文".to_string()))]);
    }

    #[test]
    fn handle_does_nothing_when_nothing_is_new() {
        let notifier = RecordingNotifier::default();
        let outcome = handle_new_notices(&[], &notifier);
        assert_eq!(outcome.fresh, 0);
        assert_eq!(outcome.toast_attempted, 0);
        // 「没有变化」很关键：调用方据此决定要不要通知前端刷新。
        assert!(!outcome.changed);
        assert_eq!(notifier.count(), 0);
    }

    #[test]
    fn handle_still_reports_changed_when_the_notifier_fails() {
        // 通知投递失败不影响「有新提醒」这个事实——界面仍然该刷新。
        let notifier = RecordingNotifier::failing();
        let outcome = handle_new_notices(&[item(5, None, None)], &notifier);
        assert_eq!(outcome.fresh, 1);
        assert!(outcome.changed);
        // 尝试过一次，但没记成「已展示」。
        assert_eq!(outcome.toast_attempted, 1);
        assert_eq!(notifier.count(), 0);
    }

    #[test]
    fn handle_caps_toasts_but_still_reports_every_fresh_notice() {
        // 上限只影响"弹几次"，不改变"来了几条"。
        let notifier = RecordingNotifier::default();
        let items = (1..=5).map(|id| item(id, None, None)).collect::<Vec<_>>();
        let outcome = handle_new_notices(&items, &notifier);
        assert_eq!(outcome.fresh, 5);
        assert_eq!(outcome.toast_attempted, MAX_NOTIFIED_PER_POLL);
        assert_eq!(notifier.count(), MAX_NOTIFIED_PER_POLL);
        assert!(outcome.changed);
    }

    // -- 跨轮次行为：轮流跑多遍会发生什么 --------------------------------

    /// 同一条提醒只弹一次。
    ///
    /// `me/notifications?since=` 每次只给游标之后的，下一轮游标已经过去，
    /// 它就不会再来。原来按「最近的提醒」的 `read` 游标判，玩家没在网页上
    /// 看过之前，同一条每轮都会再弹一次。
    #[test]
    fn a_notice_is_notified_once_across_rounds() {
        let notifier = RecordingNotifier::default();
        let first = handle_new_notices(&[item(1, Some("提醒"), Some("正文"))], &notifier);
        assert_eq!(first.toast_attempted, 1);
        // 下一轮服务器只给游标之后的：没有新的。
        let second = handle_new_notices(&[], &notifier);
        assert_eq!(second.toast_attempted, 0);
        assert!(!second.changed);
        assert_eq!(notifier.count(), 1);
    }

    /// 新提醒陆续到来：每轮只弹新增的那些。
    #[test]
    fn each_round_notifies_only_the_new_arrivals() {
        let notifier = RecordingNotifier::default();
        handle_new_notices(&[item(1, Some("A"), None)], &notifier);
        handle_new_notices(&[item(2, Some("B"), None)], &notifier);
        let shown = notifier.shown.lock().unwrap().clone();
        assert_eq!(
            shown
                .iter()
                .map(|(title, _)| title.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "B"]
        );
    }

    // -- event_to_emit：要不要通知前端 ------------------------------------

    #[test]
    fn changed_outcome_emits_the_notices_event() {
        let outcome = PollOutcome {
            fresh: 1,
            toast_attempted: 1,
            unread: Some(3),
            changed: true,
        };
        assert_eq!(outcome.event_to_emit(), Some(VRCC_NOTICES_CHANGED_EVENT));
    }

    #[test]
    fn unchanged_outcome_emits_nothing() {
        // 没有变化时绝不能发事件：否则前端每轮都白刷新一次。
        let outcome = PollOutcome::default();
        assert_eq!(outcome.event_to_emit(), None);
    }

    /// 事件名必须和前端订阅的一致，否则前端永远收不到。
    ///
    /// 这条钉的是跨语言契约：Rust 侧常量改名而前端没跟上时，编译和类型检查
    /// 都不会报错，只会表现为「界面不刷新」，极难定位。
    #[test]
    fn event_names_match_the_frontend_contract() {
        assert_eq!(VRCC_NOTICES_CHANGED_EVENT, "vrccNoticesChanged");
        assert_eq!(VRCC_CONNECTION_CHANGED_EVENT, "vrccConnectionChanged");
    }
}
