//! 星遇同频运行时：把应用层服务挂到桌面运行时上。

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use vrcx_0_application::vrcc::{StoredCredential, VrccService, VrccTokenStore};
use vrcx_0_contracts::vrcc::{
    VrccBoard, VrccConnectionSnapshot, VrccRecentNotifications, VrccStatus, VrccToday,
    VrccUsageSnapshot, VrccWristSummary,
};
use vrcx_0_integrations::vrcc::resolve_client_id;
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

/// 星遇同频提醒发生变化时发给前端的事件名。
///
/// 只作为「该刷新了」的信号，不带正文——玩家内容没必要经手额外的事件层。
pub const VRCC_NOTICES_CHANGED_EVENT: &str = "vrccNoticesChanged";

/// 桌面侧的星遇同频运行时。
pub struct DesktopVrccRuntime {
    service: Arc<VrccService>,
}

impl DesktopVrccRuntime {
    /// 后台轮询的基准间隔。
    ///
    /// 官方建议 60 秒，并明确 `/me/notifications` 有 25 秒的最小间隔。
    /// 取 60 是照官方建议，不是我们保守。
    const BACKGROUND_POLL_BASE: Duration = Duration::from_secs(60);
    /// 抖动上限，避免所有客户端撞在同一秒（官方脚本也这样做）。
    const BACKGROUND_POLL_JITTER: Duration = Duration::from_secs(20);

    /// 星遇企划图标在站点上的路径。
    ///
    /// 512×512：Windows toast 会把它缩到 48×48（随 DPI 放大），源图更大
    /// 降采样后更锐利。
    pub const NOTICE_ICON_PATH: &str = "/assets/match-icon-512.png";
    /// 图标缓存文件名，落在应用数据目录。
    const NOTICE_ICON_FILE: &str = "vrcc-notice-icon.png";

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
    pub async fn boards(&self, limit: u32) -> Result<VrccBoard, String> {
        self.service
            .boards(limit)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn status(&self) -> Result<VrccStatus, String> {
        self.service
            .status()
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
    /// `event_sink` 用来告诉前端「有新数据了」。前端不再自己轮询——双轨会让
    /// 两边各拉一次、`mark` 语义互相打架，而且隐藏窗口时前端定时器会被节流。
    pub fn spawn_background_poll(
        &self,
        supervisor: &vrcx_0_application_core::TaskSupervisor,
        notifier: Arc<dyn crate::notification::DesktopNotifier>,
        event_sink: Arc<dyn vrcx_0_application_core::RuntimeEventSink>,
        owner: Arc<dyn Fn() -> Option<vrcx_0_core::OwnerId> + Send + Sync>,
    ) -> vrcx_0_application_core::TaskSpawnOutcome {
        let service = Arc::clone(&self.service);
        let base = Self::BACKGROUND_POLL_BASE;
        let jitter = Self::BACKGROUND_POLL_JITTER;

        supervisor.spawn_cancellable(move |stop_token| {
            let service = Arc::clone(&service);
            let notifier = Arc::clone(&notifier);
            let event_sink = Arc::clone(&event_sink);
            let owner = Arc::clone(&owner);
            async move {
                // 去重状态在**所有轮次之间共享**，这是关键：服务端游标长期不动，
                // 只靠它判断会让同一条提醒每轮重弹一次（实测几十次）。
                let notified = NotifiedIds::default();
                // 通知图标也在轮次之间共享：第一次成功下载后就只用本地文件。
                let mut notice_icon = NoticeIcon::default();
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
                    // 未连接时不必轮询：`recent_notifications` 会直接报错。
                    if service.snapshot().connected {
                        match poll_once(
                            &service,
                            notifier.as_ref(),
                            &notified,
                            &mut notice_icon,
                            owner.as_ref(),
                        )
                        .await
                        {
                            Ok(outcome) => {
                                // 每一轮都记录结果，但用 debug：正常运行时不该刷屏，
                                // 排查时打开 RUST_LOG=debug 就有完整时间线。
                                tracing::debug!(
                                    round,
                                    unread = outcome.unread,
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
                    let delay = base + Duration::from_millis(jitter_millis(&jitter));
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
/// 拆成结构体而不是裸 `bool`：排查时最常问的三个问题分别是「拉到几条」、
/// 「有几条未读」、「弹出去几次」，只回一个布尔值等于什么都没说。
#[derive(Clone, Copy, Debug, Default)]
struct PollOutcome {
    /// 判定为未读（`id > read`）的条数。
    unread: usize,
    /// 实际尝试弹系统通知的次数（受 `MAX_NOTIFIED_PER_POLL` 限制）。
    toast_attempted: usize,
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

/// 一次轮询：拉最近提醒，把「还没弹过」的发成系统通知。
///
/// **未读的判定**：`recent` 里的 `read` 是玩家最后已读的提醒 id（顶层游标），
/// 不是每条条目上的标记。所以 `item.id > read` 才算未读——和服务端语义一致，
/// 也解释了为什么 `read` 为 `None` 时所有条目都算未读。
async fn poll_once(
    service: &VrccService,
    notifier: &dyn crate::notification::DesktopNotifier,
    notified: &NotifiedIds,
    icon: &mut NoticeIcon,
    owner: &(dyn Fn() -> Option<vrcx_0_core::OwnerId> + Send + Sync),
) -> Result<PollOutcome, String> {
    let recent = service
        .recent_notifications(false)
        .await
        .map_err(|error| error.to_string())?;

    // 只有真的要弹通知时才去解析图标：没提醒就不该产生任何额外流量。
    let has_unread = recent
        .items
        .iter()
        .any(|item| item.id > recent.read.unwrap_or(0));
    let icon_path = if has_unread {
        icon.resolve(service).await
    } else {
        None
    };

    // 拉取与处理分开：`handle_recent_notices` 只接收数据、不看网络，
    // 于是「拿到什么样的数据 → 弹哪些通知」可以脱离服务端直接测。
    Ok(handle_recent_notices(
        &recent,
        notifier,
        notified,
        icon_path,
        owner(),
    ))
}

/// 已弹过通知的提醒 id 集合。
///
/// **为什么需要它**：服务端的 `read` 游标只在玩家主动标记已读时前进，而后台
/// 轮询用的是 `mark=false`（不该替他标已读，那是对的）。于是游标长期不动，
/// 同一条提醒在每一轮里都会被判成"未读"——实测每 60 秒重弹一次，同一条提醒
/// 弹了几十次。
///
/// 所以「未读」和「还没弹过」是两件事：
/// - 未读 = `id > read`，决定界面角标；
/// - 还没弹过 = 不在这个集合里，决定要不要发系统通知。
///
/// 只在内存里：进程重启后集合清空，此时服务端游标通常已经因为玩家打开页面
/// 而前进，最多重弹最近一条。为跨重启保存它需要落库，代价与收益不成比例。
#[derive(Default)]
struct NotifiedIds {
    seen: std::sync::Mutex<std::collections::HashSet<i64>>,
}

impl NotifiedIds {
    /// 记下这批 id，返回其中**之前没见过**的那些。
    ///
    /// 判定与写入在同一次加锁内完成，避免两轮轮询交错时重复弹。
    fn take_new(&self, ids: impl IntoIterator<Item = i64>) -> Vec<i64> {
        let mut seen = match self.seen.lock() {
            Ok(seen) => seen,
            Err(error) => error.into_inner(),
        };
        let mut fresh = Vec::new();
        for id in ids {
            if seen.insert(id) {
                fresh.push(id);
            }
        }
        fresh
    }
}

/// 处理一批刚拉到的提醒：算未读、挑出要弹的、交给 notifier。
///
/// 这个函数不发请求也不读时钟，是排查「为什么没弹」时最该被测试覆盖的一段。
///
/// `icon` 是通知图标的本地路径（Windows toast 只接受本地文件，不接受 URL）。
/// 传 `None` 也能正常发通知，只是没有图。
fn handle_recent_notices(
    recent: &vrcx_0_contracts::vrcc::VrccRecentNotifications,
    notifier: &dyn crate::notification::DesktopNotifier,
    notified: &NotifiedIds,
    icon: Option<&std::path::Path>,
    owner: Option<vrcx_0_core::OwnerId>,
) -> PollOutcome {
    // 没有游标 = 全都没读过。
    let read_cursor = recent.read.unwrap_or(0);
    let total = recent.items.len();
    let unread = recent
        .items
        .iter()
        .filter(|item| item.id > read_cursor)
        .count();

    tracing::debug!(total, unread, read_cursor, "vrcc: fetched recent notices");

    // 决策与投递分开：`select_notices_to_notify` 是纯函数，可以脱离网络和
    // 通知桥单独测。这里只负责把它选出来的结果交给 notifier。
    let selection = select_notices_to_notify(&recent.items, read_cursor, MAX_NOTIFIED_PER_POLL);

    // 真正要弹的是「选中 且 之前没弹过」的那些。这一层不能省：游标不前进时
    // `selection` 每轮都一样，只靠它会导致重复打扰。
    let candidates = selection.notices;
    let fresh_ids = notified.take_new(candidates.iter().map(|notice| notice.id));
    let notices: Vec<_> = candidates
        .into_iter()
        .filter(|notice| fresh_ids.contains(&notice.id))
        .collect();
    // 本轮被本地去重挡下的条数：非零说明「服务端还认为未读，但已经弹过了」，
    // 这正是游标不前进时的常态，值得在 debug 里看见。
    let already_notified = fresh_ids.len().saturating_sub(notices.len());

    if selection.deferred > 0 {
        // 有未读但超出本轮上限：这是「玩家可能没被及时通知」的唯一线索。
        tracing::warn!(
            unread,
            limit = MAX_NOTIFIED_PER_POLL,
            deferred = selection.deferred,
            "vrcc: unread notices exceed per-poll toast limit; remaining deferred"
        );
    }
    if already_notified > 0 {
        tracing::debug!(
            already_notified,
            "vrcc: skipped notices already notified in a previous round"
        );
    }

    let mut toast_attempted = 0usize;
    // `DesktopNotifier` 的 image 参数是字符串（Windows 侧最终当成文件路径用），
    // 这里把 Path 转一次，转换失败（非 UTF-8 路径）就当作没有图标。
    let icon_str = icon.and_then(|path| path.to_str());
    for notice in &notices {
        // **带 action 是为了让图标生效**，不只是为了点击跳转。
        //
        // `TauriDesktopNotifier::show` 有两条分支：
        // - action 为 Some → winrt（`Toast::icon(path, …)`，真能带图）
        // - action 为 None → tauri-plugin-notification，它调的是
        //   `notify_rust::Notification::icon()`，而那个方法在 Windows 上是
        //   空操作（Windows 需要的是 `image_path()`）。于是图标会被静默吞掉。
        //
        // 实测证据：不带 action 时，系统通知库里的 toast XML 里没有
        // `<image>` 元素，尽管我们确实传了标的文件。
        //
        // 拿不到 owner（未登录）时退回 None：那时没有可跳转的目标，
        // 图标也就跟着没有——比弹出一个点了没反应的通知要好。
        let action = owner.as_ref().and_then(|owner| {
            crate::notification::DesktopNotificationAction::open_vrcc(
                owner,
                Some(&notice.id.to_string()),
            )
        });
        match notifier.show(
            &notice.title,
            notice.body.as_deref(),
            icon_str,
            true,
            action.as_ref(),
        ) {
            Ok(()) => {
                // 只记 id 与长度，不记正文：提醒内容属于玩家数据。
                tracing::info!(
                    notice_id = notice.id,
                    body_len = notice.body.as_deref().map(str::len).unwrap_or(0),
                    icon = icon_str.is_some(),
                    actionable = action.is_some(),
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
        unread,
        toast_attempted,
        changed: unread > 0,
    }
}

/// 每次轮询最多弹几条。
///
/// 首次连接时可能一次性拉到很多历史提醒，全弹出来会刷屏。
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

/// 通知图标的本地缓存状态。
///
/// **为什么下载而不是打包进仓库**：官方《使用要求》第 17 条把「星遇同频的
/// 图标」列为不可作为我方品牌使用，它是对方的美术资产。运行时取用并在本地
/// 缓存不构成再分发；顺带官方换图标时我们也能跟着变。
///
/// 失败**不阻断通知**：图标是锦上添花，不是发通知的前置条件。失败后下一轮
/// 会再试（而不是本会话放弃），因为「启动时正好没网」不该永久失去图标。
#[derive(Default)]
struct NoticeIcon {
    path: Option<std::path::PathBuf>,
}

impl NoticeIcon {
    /// 拿到图标的本地路径；已缓存则直接返回，不再联网。
    async fn resolve(&mut self, service: &VrccService) -> Option<&std::path::Path> {
        if self.path.is_some() {
            return self.path.as_deref();
        }
        match ensure_notice_icon(service).await {
            Some(path) => {
                self.path = Some(path);
                self.path.as_deref()
            }
            None => None,
        }
    }
}

/// 确保通知图标已缓存在本地，返回它的路径。
async fn ensure_notice_icon(service: &VrccService) -> Option<std::path::PathBuf> {
    let dir = vrcx_0_platform::app_paths::default_app_data_dir().ok()?;
    let target = dir.join(DesktopVrccRuntime::NOTICE_ICON_FILE);
    if target.is_file() {
        return Some(target);
    }

    let bytes = match service
        .fetch_site_asset(DesktopVrccRuntime::NOTICE_ICON_PATH)
        .await
    {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::debug!(%error, "vrcc: notice icon fetch failed; notifications will have no icon");
            return None;
        }
    };
    if bytes.is_empty() {
        tracing::debug!("vrcc: notice icon response was empty");
        return None;
    }

    // 先写临时文件再改名：避免下载中断留下半个 PNG —— 那会让后续每一轮
    // 都认为「已缓存」，却渲染不出图标。
    let temp = dir.join(format!("{}.tmp", DesktopVrccRuntime::NOTICE_ICON_FILE));
    if let Err(error) = std::fs::write(&temp, &bytes) {
        tracing::debug!(%error, "vrcc: notice icon write failed");
        return None;
    }
    if let Err(error) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        tracing::debug!(%error, "vrcc: notice icon rename failed");
        return None;
    }
    tracing::info!(
        bytes = bytes.len(),
        path = %target.display(),
        "vrcc: notice icon cached"
    );
    Some(target)
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
    /// 本轮要弹的通知，已按 `limit` 截断。
    notices: Vec<NoticeToNotify>,
    /// 因超出 `limit` 而推迟到下一轮的条数。
    ///
    /// 单独暴露而不是让调用方再数一遍：它是「玩家可能没被及时通知」的唯一
    /// 指标，日志里要靠它说话。
    deferred: usize,
}

/// 决定这一轮该弹哪些通知。
///
/// 纯函数：不碰网络、不碰通知桥、不读时钟。这样「未读怎么判、上限怎么截、
/// 迟到的那批怎么算」都能脱离环境直接测——而这正是最容易写错、也最难在
/// 真机上复现的部分。
///
/// 规则：
/// - `id > read_cursor` 才算未读（`read_cursor` 是服务端的已读游标）；
/// - 最多返回 `limit` 条，超出的计入 `deferred`；
/// - 标题为空时用「星遇同频」，避免弹出一个没有标题的空通知。
fn select_notices_to_notify(
    items: &[vrcx_0_contracts::vrcc::VrccRecentNotificationItem],
    read_cursor: i64,
    limit: usize,
) -> NoticeSelection {
    let mut notices = Vec::new();
    let mut deferred = 0usize;
    for item in items {
        if item.id <= read_cursor {
            continue;
        }
        if notices.len() >= limit {
            deferred += 1;
            continue;
        }
        notices.push(NoticeToNotify {
            id: item.id,
            title: item
                .title
                .clone()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| "星遇同频".to_string()),
            body: item.body.clone(),
        });
    }
    NoticeSelection { notices, deferred }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use vrcx_0_contracts::vrcc::VrccRecentNotificationItem;

    fn item(id: i64, title: Option<&str>, body: Option<&str>) -> VrccRecentNotificationItem {
        VrccRecentNotificationItem {
            id,
            title: title.map(str::to_string),
            body: body.map(str::to_string),
            at: Some(1_700_000_000_000),
            url: None,
        }
    }

    #[test]
    fn selects_only_notices_after_the_read_cursor() {
        let items = vec![
            item(1, Some("old"), Some("a")),
            item(2, Some("old"), Some("b")),
            item(3, Some("new"), Some("c")),
        ];
        // 游标 2 表示 1、2 已读。
        let selection = select_notices_to_notify(&items, 2, 10);
        assert_eq!(selection.notices.len(), 1);
        assert_eq!(selection.notices[0].id, 3);
        assert_eq!(selection.deferred, 0);
    }

    #[test]
    fn cursor_equal_to_newest_id_means_nothing_is_unread() {
        let items = vec![item(7, Some("n"), None), item(8, Some("n"), None)];
        let selection = select_notices_to_notify(&items, 8, 10);
        assert!(selection.notices.is_empty());
        assert_eq!(selection.deferred, 0);
    }

    #[test]
    fn zero_cursor_treats_everything_as_unread() {
        let items = vec![item(1, None, None), item(2, None, None)];
        // 服务端返回 read 为 None 时调用方传 0，此时全部算未读。
        let selection = select_notices_to_notify(&items, 0, 10);
        assert_eq!(selection.notices.len(), 2);
    }

    #[test]
    fn respects_the_limit_and_counts_what_it_deferred() {
        let items = (1..=6).map(|id| item(id, None, None)).collect::<Vec<_>>();
        let selection = select_notices_to_notify(&items, 0, 3);
        assert_eq!(selection.notices.len(), 3);
        // 6 条未读、上限 3 → 恰好推迟 3 条。
        assert_eq!(selection.deferred, 3);
        // 截断的是后面那批，不是随机挑。
        assert_eq!(
            selection.notices.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn deferred_counts_only_unread_over_the_limit() {
        // 已读的条目不该被算进 deferred——它本来就轮不到弹。
        let items = vec![
            item(1, None, None), // 已读
            item(2, None, None), // 未读，选中
            item(3, None, None), // 未读，超限
        ];
        let selection = select_notices_to_notify(&items, 1, 1);
        assert_eq!(selection.notices.len(), 1);
        assert_eq!(selection.notices[0].id, 2);
        assert_eq!(selection.deferred, 1);
    }

    #[test]
    fn blank_title_falls_back_so_no_notification_is_titleless() {
        let items = vec![item(1, Some("   "), Some("body"))];
        let selection = select_notices_to_notify(&items, 0, 10);
        assert_eq!(selection.notices[0].title, "星遇同频");
    }

    #[test]
    fn keeps_the_body_verbatim() {
        let items = vec![item(1, Some("标题"), Some("正文内容"))];
        let selection = select_notices_to_notify(&items, 0, 10);
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

    // -- handle_recent_notices：拉取与投递之间的那一段 --------------------

    /// 记录调用的假通知桥。
    #[derive(Default)]
    struct RecordingNotifier {
        /// (标题, 正文, 图片路径)。图片要一起记：它是本模块新加的能力，
        /// 不记的话「图标有没有真的传给 notifier」就只能靠肉眼验证。
        shown: Mutex<Vec<(String, Option<String>, Option<String>)>>,
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
            image: Option<&str>,
            _play_sound: bool,
            _action: Option<&crate::notification::DesktopNotificationAction>,
        ) -> Result<(), String> {
            if self.fail {
                return Err("notifier unavailable".to_string());
            }
            self.shown.lock().unwrap().push((
                title.to_string(),
                body.map(str::to_string),
                image.map(str::to_string),
            ));
            Ok(())
        }
    }

    fn recent(
        read: Option<i64>,
        items: Vec<vrcx_0_contracts::vrcc::VrccRecentNotificationItem>,
    ) -> vrcx_0_contracts::vrcc::VrccRecentNotifications {
        vrcx_0_contracts::vrcc::VrccRecentNotifications { items, read }
    }

    #[test]
    fn handle_notifies_unread_and_reports_changed() {
        let notifier = RecordingNotifier::default();
        let outcome = handle_recent_notices(
            &recent(
                Some(1),
                vec![item(1, None, None), item(2, Some("新"), Some("正文"))],
            ),
            &notifier,
            &NotifiedIds::default(),
            None,
            None,
        );
        assert_eq!(outcome.unread, 1);
        assert_eq!(outcome.toast_attempted, 1);
        assert!(outcome.changed);
        let shown = notifier.shown.lock().unwrap().clone();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].0, "新");
        assert_eq!(shown[0].1.as_deref(), Some("正文"));
    }

    #[test]
    fn handle_does_nothing_when_everything_is_read() {
        let notifier = RecordingNotifier::default();
        let outcome = handle_recent_notices(
            &recent(Some(9), vec![item(8, None, None), item(9, None, None)]),
            &notifier,
            &NotifiedIds::default(),
            None,
            None,
        );
        assert_eq!(outcome.unread, 0);
        assert_eq!(outcome.toast_attempted, 0);
        // 「没有变化」很关键：调用方据此决定要不要通知前端刷新。
        assert!(!outcome.changed);
        assert_eq!(notifier.count(), 0);
    }

    #[test]
    fn handle_still_reports_changed_when_the_notifier_fails() {
        // 通知投递失败不影响「有新提醒」这个事实——界面仍然该刷新。
        let notifier = RecordingNotifier::failing();
        let outcome = handle_recent_notices(
            &recent(Some(0), vec![item(5, None, None)]),
            &notifier,
            &NotifiedIds::default(),
            None,
            None,
        );
        assert_eq!(outcome.unread, 1);
        assert!(outcome.changed);
        // 尝试过一次，但没记成「已展示」。
        assert_eq!(outcome.toast_attempted, 1);
    }

    #[test]
    fn handle_treats_missing_cursor_as_all_unread() {
        let notifier = RecordingNotifier::default();
        let outcome = handle_recent_notices(
            &recent(None, vec![item(1, None, None), item(2, None, None)]),
            &notifier,
            &NotifiedIds::default(),
            None,
            None,
        );
        assert_eq!(outcome.unread, 2);
        assert_eq!(outcome.toast_attempted, 2);
    }

    #[test]
    fn handle_caps_toasts_but_still_reports_full_unread_count() {
        // 上限只影响"弹几次"，不改变"有多少未读"——后者决定界面角标。
        let notifier = RecordingNotifier::default();
        let items = (1..=10).map(|id| item(id, None, None)).collect::<Vec<_>>();
        let outcome = handle_recent_notices(
            &recent(Some(0), items),
            &notifier,
            &NotifiedIds::default(),
            None,
            None,
        );
        assert_eq!(outcome.unread, 10);
        assert_eq!(outcome.toast_attempted, MAX_NOTIFIED_PER_POLL);
        assert_eq!(notifier.count(), MAX_NOTIFIED_PER_POLL);
        assert!(outcome.changed);
    }

    // -- 跨轮次行为：轮流跑多遍会发生什么 --------------------------------

    /// 同一个提醒在两轮之间不该被重复弹。
    ///
    /// 两种去重都要成立：游标推进时（玩家在别处读了）不算未读；游标不动时
    /// （玩家没读）也不该重弹——后者靠本地 `NotifiedIds`。
    #[test]
    fn a_notice_is_not_notified_twice_across_rounds() {
        let notifier = RecordingNotifier::default();
        let notified = NotifiedIds::default();
        let items = vec![item(1, Some("提醒"), Some("正文"))];

        // 第一轮：游标 0，全都未读 → 弹一次。
        let first = handle_recent_notices(
            &recent(Some(0), items.clone()),
            &notifier,
            &notified,
            None,
            None,
        );
        assert_eq!(first.toast_attempted, 1);
        assert!(first.changed);

        // 第二轮：游标已推到 1，同一条不再未读 → 不再弹。
        let second =
            handle_recent_notices(&recent(Some(1), items), &notifier, &notified, None, None);
        assert_eq!(second.toast_attempted, 0);
        assert!(!second.changed);
        // 累计仍只有第一次那一条。
        assert_eq!(notifier.count(), 1);
    }

    /// **游标不前进时也不重复弹。**
    ///
    /// 这是实测暴露出来的真实缺陷：后台轮询用 `mark=false`（不替玩家标已读），
    /// 所以服务端 `read` 游标长期不动，同一条提醒每一轮都被判成未读。修复前
    /// 它会每 60 秒重弹一次，同一条提醒弹了几十次。
    ///
    /// 现在靠 `NotifiedIds` 兜住：弹过就不再弹，与服务端游标无关。
    #[test]
    fn same_notice_does_not_repeat_when_the_cursor_is_stuck() {
        let notifier = RecordingNotifier::default();
        let notified = NotifiedIds::default();
        let items = vec![item(7, None, None)];

        // 五轮，游标一直停在 0（模拟玩家从未打开页面标已读）。
        let mut outcomes = Vec::new();
        for _ in 0..5 {
            outcomes.push(handle_recent_notices(
                &recent(Some(0), items.clone()),
                &notifier,
                &notified,
                None,
                None,
            ));
        }

        // 只有第一轮弹了，其余四轮都被本地去重挡下。
        assert_eq!(outcomes[0].toast_attempted, 1);
        for outcome in &outcomes[1..] {
            assert_eq!(outcome.toast_attempted, 0);
        }
        assert_eq!(notifier.count(), 1);

        // 但「未读」这个事实每轮都还在——界面角标不该因此消失。
        assert!(outcomes.iter().all(|outcome| outcome.unread == 1));
        // 且每轮都仍要通知前端（内容没变，但状态是"有未读"）。
        assert!(outcomes.iter().all(|outcome| outcome.changed));
    }

    /// 新提醒陆续到来：每轮只弹新增的那些。
    #[test]
    fn each_round_notifies_only_the_new_arrivals() {
        let notifier = RecordingNotifier::default();
        let notified = NotifiedIds::default();

        let round1 = handle_recent_notices(
            &recent(Some(0), vec![item(1, Some("A"), None)]),
            &notifier,
            &notified,
            None,
            None,
        );
        assert_eq!(round1.toast_attempted, 1);

        // 第二轮：多了 id=2，游标还停在 1。
        let round2 = handle_recent_notices(
            &recent(
                Some(1),
                vec![item(1, Some("A"), None), item(2, Some("B"), None)],
            ),
            &notifier,
            &notified,
            None,
            None,
        );
        assert_eq!(round2.toast_attempted, 1);
        let shown = notifier.shown.lock().unwrap().clone();
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0].0, "A");
        assert_eq!(shown[1].0, "B");
    }

    // -- 通知图标 ----------------------------------------------------------

    #[test]
    fn passes_the_icon_path_through_to_the_notifier() {
        // Windows toast 只接受本地文件路径，所以这里传的就是本地缓存路径。
        let notifier = RecordingNotifier::default();
        let icon =
            std::path::Path::new(r"C:\Users\someone\AppData\Roaming\VRCX-0\vrcc-notice-icon.png");

        let outcome = handle_recent_notices(
            &recent(Some(0), vec![item(1, Some("提醒"), None)]),
            &notifier,
            &NotifiedIds::default(),
            Some(icon),
            None,
        );

        assert_eq!(outcome.toast_attempted, 1);
        let shown = notifier.shown.lock().unwrap().clone();
        assert_eq!(shown[0].2.as_deref(), icon.to_str());
    }

    #[test]
    fn still_notifies_when_there_is_no_icon() {
        // 图标是锦上添花：下载失败时通知照发，不能因为没图就不发。
        let notifier = RecordingNotifier::default();
        let outcome = handle_recent_notices(
            &recent(Some(0), vec![item(1, Some("提醒"), None)]),
            &notifier,
            &NotifiedIds::default(),
            None,
            None,
        );

        assert_eq!(outcome.toast_attempted, 1);
        let shown = notifier.shown.lock().unwrap().clone();
        assert_eq!(shown[0].2, None);
    }

    // -- event_to_emit：要不要通知前端 ------------------------------------

    #[test]
    fn changed_outcome_emits_the_notices_event() {
        let outcome = PollOutcome {
            unread: 1,
            toast_attempted: 1,
            changed: true,
        };
        assert_eq!(outcome.event_to_emit(), Some(VRCC_NOTICES_CHANGED_EVENT));
    }

    #[test]
    fn unchanged_outcome_emits_nothing() {
        // 没有变化时绝不能发事件：否则前端每轮都白刷新一次。
        let outcome = PollOutcome {
            unread: 0,
            toast_attempted: 0,
            changed: false,
        };
        assert_eq!(outcome.event_to_emit(), None);
    }

    /// 事件名必须和前端订阅的一致，否则前端永远收不到。
    ///
    /// 这条钉的是跨语言契约：Rust 侧常量改名而前端没跟上时，编译和类型检查
    /// 都不会报错，只会表现为「界面不刷新」，极难定位。
    #[test]
    fn notices_event_name_matches_the_frontend_contract() {
        assert_eq!(VRCC_NOTICES_CHANGED_EVENT, "vrccNoticesChanged");
    }
}
