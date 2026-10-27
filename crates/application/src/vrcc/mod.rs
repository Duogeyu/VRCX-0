//! 星遇同频集成的应用服务。
//!
//! 这一层负责编排：拿令牌 → 调接口 → 投影成前端/腕表能用的形状。
//! 网络细节在 `vrcx_0_integrations::vrcc`，持久化细节在 outbound-adapters，
//! 这里只做组合与口径统一。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use vrcx_0_application_core::Result;
use vrcx_0_contracts::vrcc::{
    VrccAnnouncements, VrccBoard, VrccBoardSort, VrccConnectionSnapshot, VrccMutuals, VrccMyMap,
    VrccNotes, VrccRecentNotifications, VrccStatus, VrccToday, VrccUnread, VrccUsageSnapshot,
    VrccWristSummary,
};
use vrcx_0_integrations::vrcc::{VrccClient, VrccError, VrccSession, WINDOW_LIMIT};

/// 服务器回答 `me/unread` 是 `404 not_found`（还没更新到 1.13.0）之后，
/// 多久再试一次。服务器更新了，客户端不用重启也能接上。
const UNREAD_RETRY_AFTER_NOT_FOUND: Duration = Duration::from_secs(3600);

/// 未读数的缓存。
#[derive(Debug, Default)]
struct UnreadCache {
    /// 最近一次 `me/unread` 的结果；还没问过、服务器没有这个接口时为 `None`。
    counts: Option<VrccUnread>,
    /// 服务器说没有这个接口时，到这个时刻之前不再问。
    skip_until: Option<Instant>,
}

/// 令牌仓库端口。实现留在 outbound-adapters。
pub trait VrccTokenStore: Send + Sync {
    fn load(&self) -> Result<Option<StoredCredential>>;
    fn save(&self, credential: &StoredCredential) -> Result<()>;
    fn clear(&self) -> Result<()>;
}

/// 前端能区分的行为分类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VrccErrorKind {
    /// 需要玩家重新授权。
    NeedsReauth,
    /// 缺某个权限，`need` 里说明是哪项。
    MissingScope,
    /// 限流或额度用尽，等等再来。
    Throttled,
    /// 网络或服务端问题。
    Transport,
    /// 其它。
    Other,
}

/// 判定一个错误属于哪一类，并把缺的权限一并带回。
pub fn classify_vrcc_error(error: &VrccError) -> (VrccErrorKind, Option<String>) {
    let need = error.needed_scope().map(str::to_string);
    let kind = match error {
        // `TooSoon` 是「问勤了」，归到 Throttled 只是为了让前端显示
        // 「稍后再试」；它不会触发长时间退避，也不消耗额度。
        VrccError::RateLimited(_) | VrccError::QuotaExhausted(_) | VrccError::TooSoon { .. } => {
            VrccErrorKind::Throttled
        }
        // 停站维护：令牌没事，等一会儿就好，和网络问题一样处理。
        VrccError::Transport(_) | VrccError::Decode(_) | VrccError::Paused { .. } => {
            VrccErrorKind::Transport
        }
        VrccError::TokenMissing => VrccErrorKind::NeedsReauth,
        VrccError::Api { need, .. } if need.is_some() && error.needs_reauth() => {
            VrccErrorKind::MissingScope
        }
        _ if error.needs_reauth() => VrccErrorKind::NeedsReauth,
        VrccError::AbsolutePath(_) | VrccError::BodyTooLarge | VrccError::Api { .. } => {
            VrccErrorKind::Other
        }
    };
    (kind, need)
}

/// 仓库里的凭据形状（与 outbound-adapters 的实现解耦）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoredCredential {
    pub token: String,
    pub star: Option<i64>,
    pub vrc_name: Option<String>,
    pub scope: Option<String>,
}

/// 一次设备授权的中间态。给前端显示用，不落盘。
#[derive(Clone, Debug)]
pub struct VrccDeviceAuthPrompt {
    pub user_code: String,
    pub verification_uri: Option<String>,
    pub verification_uri_complete: Option<String>,
    pub expires_in_secs: i64,
    pub interval_secs: i64,
    /// 内部用：换令牌时要回传，不给玩家看。
    pub device_code: String,
    pub scope: Option<String>,
}

/// 设备授权的下一步。
#[derive(Clone, Debug)]
pub enum VrccDeviceAuthStep {
    /// 玩家还没点同意，按 `interval_secs` 后再问。
    Pending { interval_secs: i64 },
    /// 拿到令牌，已保存。
    Connected(VrccConnectionSnapshot),
    /// 玩家取消或缺码过期，这条流程结束。
    Terminal(String),
}

/// 地图榜缓存：world 映射到（名次，均分 ×10）。
type BoardRanks = HashMap<String, (i64, Option<i64>)>;

/// 星遇同频服务。
pub struct VrccService {
    store: Arc<dyn VrccTokenStore>,
    session: VrccSession,
    client: VrccClient,
    /// 腕表摘要的缓存。
    ///
    /// 腕表每帧都会读它，所以这里必须是纯内存读取——不能在这里发请求。
    /// 内容由本来就会发的请求顺手填充：后台轮询的 `me/unread`（旧服务器上是
    /// `recent_notifications`）提供未读数，`boards` 提供地图名次（开放数据，
    /// 不占个人额度）。
    ///
    /// `None` 表示"还没拿到任何数据"，和"未读 0 条"是两件事：前者不该在腕表
    /// 上显示一个数字，后者才该显示 0。
    wrist: Arc<RwLock<Option<VrccWristSummary>>>,
    /// 最近一次地图榜：world 映射到（名次，均分 ×10）。
    ///
    /// 缓存整个榜单而不是只缓存"当前地图"：玩家随时换图，而榜单是开放数据，
    /// 一次请求就能覆盖上百张图，按图缓存没有意义。
    boards: Arc<RwLock<BoardRanks>>,
    /// 最近一次 `me/unread`。后台轮询填，页面和腕表读。
    unread: Arc<RwLock<UnreadCache>>,
}

impl VrccService {
    /// 从仓库恢复会话：有令牌就直接进入已连接状态。
    pub fn new(store: Arc<dyn VrccTokenStore>, client_id: impl Into<String>) -> Self {
        Self::with_proxy(store, client_id, None)
    }

    /// 带代理设置构造。
    ///
    /// `proxy_url` 用宿主应用的统一代理设置。不传的话在本机这种需要代理才能
    /// 访问外网的环境里，所有请求都会 DNS 超时。
    pub fn with_proxy(
        store: Arc<dyn VrccTokenStore>,
        client_id: impl Into<String>,
        proxy_url: Option<String>,
    ) -> Self {
        Self::with_usage_store(store, client_id, proxy_url, None)
    }

    /// 完整构造：代理 + 日配额落盘。
    ///
    /// `usage_store` 不传时配额只在内存里记，**重启即清零**——本地预扣就失效了。
    /// 生产装配必须传（见 `runtime-host-desktop`）。
    pub fn with_usage_store(
        store: Arc<dyn VrccTokenStore>,
        client_id: impl Into<String>,
        proxy_url: Option<String>,
        usage_store: Option<Arc<dyn vrcx_0_integrations::vrcc::DailyUsageStore>>,
    ) -> Self {
        let mut session = VrccSession::with_proxy(client_id, proxy_url);
        if let Some(usage_store) = usage_store {
            session = session.with_usage_store(usage_store);
        }
        if let Ok(Some(credential)) = store.load() {
            session.connect(
                credential.token,
                credential.star,
                credential.vrc_name,
                credential.scope,
            );
        }
        let client = session.client();
        Self {
            store,
            session,
            client,
            wrist: Arc::new(RwLock::new(None)),
            boards: Arc::new(RwLock::new(HashMap::new())),
            unread: Arc::new(RwLock::new(UnreadCache::default())),
        }
    }

    /// 腕表摘要。`world_id` 由调用方给（腕表的渲染侧知道玩家在哪张图）。
    ///
    /// 纯内存读取，不发请求。返回 `None` 的两种情况：还没拉到任何数据、
    /// 缓存锁坏了。腕表在 `None` 时整块不显示。
    ///
    /// 这里**不判断是否已连接**：地图榜是开放数据，没连接也能拉到，玩家在
    /// 腕表上看到"我在的图排第几"本身就是有用的。未读数在没有连接时保持 0，
    /// 那是事实——没有连接就收不到提醒。
    pub fn wrist_summary(&self, world_id: Option<&str>) -> Option<VrccWristSummary> {
        let mut summary = self.wrist.read().ok()?.clone()?;
        // 地图名次从榜单缓存现查：玩家换图后不必等下一次 boards 请求。
        let rank = world_id.and_then(|world| {
            self.boards
                .read()
                .ok()
                .and_then(|boards| boards.get(world).copied())
        });
        summary.map_rank = rank.map(|(rank, _)| rank);
        summary.map_avg_x10 = rank.and_then(|(_, avg)| avg);
        Some(summary)
    }

    /// 拿到 `recent` 结果后更新未读数。
    ///
    /// 顺手把缓存从 `None` 变成 `Some`：这已经是真实数据了，腕表可以显示。
    ///
    /// 服务器有 `me/unread` 时，腕表显示的是那几处红点的合计，这里就不覆盖它；
    /// 只有旧服务器（没有 `me/unread`）才用「最近的提醒」里 `id > read` 的条数。
    fn note_recent(&self, unread: u32) {
        if self.cached_unread().is_some() {
            return;
        }
        let Ok(mut wrist) = self.wrist.write() else {
            return;
        };
        let entry = wrist.get_or_insert_with(VrccWristSummary::default);
        entry.unread = unread;
    }

    /// 拿到 `me/unread` 后更新缓存和腕表。返回和上一次比有没有变。
    fn note_unread(&self, counts: &VrccUnread) -> bool {
        let changed = {
            let Ok(mut cache) = self.unread.write() else {
                return false;
            };
            let changed = cache.counts.as_ref() != Some(counts);
            cache.counts = Some(counts.clone());
            cache.skip_until = None;
            changed
        };
        if let Ok(mut wrist) = self.wrist.write() {
            let entry = wrist.get_or_insert_with(VrccWristSummary::default);
            entry.unread = u32::try_from(counts.total()).unwrap_or(u32::MAX);
        }
        changed
    }

    /// 断开或令牌失效后，未读数不再属于任何人：清掉，腕表回到 0。
    fn forget_unread(&self) {
        if let Ok(mut cache) = self.unread.write() {
            cache.counts = None;
        }
        if let Ok(mut wrist) = self.wrist.write() {
            if let Some(entry) = wrist.as_mut() {
                entry.unread = 0;
            }
        }
    }

    /// 最近一次 `me/unread` 的结果，纯内存读取。
    pub fn cached_unread(&self) -> Option<VrccUnread> {
        self.unread.read().ok()?.counts.clone()
    }

    /// 收拾一个错误：`404 gone` 表示令牌已经失效，清掉它。
    ///
    /// 官方的要求是「收到 `404 gone` 就停下轮询，让玩家重新授权」。不清的话，
    /// 后台轮询会拿着一个死令牌每分钟问一次，界面上也一直显示「已连接」。
    /// 1.10.0 起「90 天没调用过任何接口」也会这样，所以这不是罕见情况。
    fn settle(&self, error: VrccError) -> VrccError {
        if error.is_gone() {
            tracing::info!(
                "vrcc: token is gone (disconnected on the site, re-authorized elsewhere, or unused for 90 days); dropping it"
            );
            if let Err(clear_error) = self.store.clear() {
                tracing::warn!(%clear_error, "vrcc: failed to clear the dead token");
            }
            self.session.expire();
            self.forget_unread();
        }
        error
    }

    /// 拿到地图榜后更新名次缓存。
    ///
    /// 榜单是开放数据，不带令牌也能拉；只要拉到了就认为腕表有数据可显示，
    /// 这样"没连接但想看榜单"的场景也有内容。
    fn note_boards(&self, board: &VrccBoard) {
        let Ok(mut boards) = self.boards.write() else {
            return;
        };
        boards.clear();
        for map in &board.maps {
            let Some(rank) = map.rank else { continue };
            let avg_x10 = map.avg.map(|avg| (avg * 10.0).round() as i64);
            boards.insert(map.world.clone(), (rank, avg_x10));
        }
        drop(boards);
        if let Ok(mut wrist) = self.wrist.write() {
            wrist.get_or_insert_with(VrccWristSummary::default);
        }
    }

    pub fn client_id(&self) -> &str {
        self.session.client_id()
    }

    pub fn snapshot(&self) -> VrccConnectionSnapshot {
        self.session.snapshot().into()
    }

    /// 服务器建议的提醒轮询间隔（秒，不低于 25），后台轮询用。
    pub fn poll_every_secs(&self) -> i64 {
        self.session.snapshot().poll_every_secs
    }

    /// `me/notifications` 的游标；连上以后还没问过时为 `None`。
    pub fn notifications_cursor(&self) -> Option<i64> {
        self.session.snapshot().notifications_cursor
    }

    /// 用量快照。
    ///
    /// 口径：说「今日已用」多少。服务器回过 `RateLimit-*`（1.11.0 起）就是
    /// 服务器按这个授权算的准数，否则是本客户端自己记的；`personal_from_server`
    /// 告诉界面是哪一种。
    pub fn usage(&self) -> VrccUsageSnapshot {
        let usage = self.client.local_usage();
        VrccUsageSnapshot {
            personal_used_today: usage.personal_used_today,
            personal_daily_limit: usage.personal_daily_limit,
            personal_from_server: usage.personal_from_server,
            unread_used_today: usage.unread.map(|(used, _)| used),
            unread_daily_limit: usage.unread.map(|(_, limit)| limit),
            device_code_used_today: usage.device_code_used_today,
            device_code_daily_limit: vrcx_0_integrations::vrcc::DEVICE_CODE_DAILY_LIMIT,
            window_used: usage.window_used as i64,
            window_limit: WINDOW_LIMIT as i64,
            backoff_remaining_ms: usage.backoff_remaining.as_millis() as i64,
            date: usage.date,
        }
    }

    // -- 开放数据 ----------------------------------------------------------

    /// 地图榜。不需要连接，也不计个人额度。
    ///
    /// `sort` 是本周榜或新上榜、而回答里没有 `sort` 时，说明服务器还不认
    /// 这个参数、回的是总榜：照样返回，由界面判断「还不支持」。
    pub async fn boards(
        &self,
        limit: u32,
        sort: VrccBoardSort,
    ) -> std::result::Result<VrccBoard, VrccError> {
        let board = self.client.boards(limit, sort).await?;
        // 顺手填充腕表的排名缓存：这次请求本来就要发，不额外花额度。
        // 只用总榜：名次指的是总榜名次，本周榜和新上榜只列一部分地图。
        if sort == VrccBoardSort::Top || board.sort.is_none() {
            self.note_boards(&board);
        }
        Ok(board)
    }

    /// 服务状态。
    pub async fn status(&self) -> std::result::Result<VrccStatus, VrccError> {
        self.client.status().await
    }

    /// 公告和停站（1.12.0 起）。不需要连接。
    ///
    /// 旧服务器没有这个接口（`404 not_found`）时返回 `None`，界面就不显示。
    pub async fn announcements(&self) -> std::result::Result<Option<VrccAnnouncements>, VrccError> {
        match self.client.announcements().await {
            Ok(announcements) => Ok(Some(announcements)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// 取站点上的静态资源（通知图标之类）。
    ///
    /// 不需要连接：那是站点资源，和玩家令牌无关。仍然走客户端，于是复用
    /// 同一份代理配置与「只和 vrcc.cc 说话」的硬拦。
    pub async fn fetch_site_asset(&self, path: &str) -> std::result::Result<Vec<u8>, VrccError> {
        self.client.fetch_site_asset(path).await
    }

    // -- 设备授权 ----------------------------------------------------------

    /// 第一步：申请连接码。
    ///
    /// `scope` 传 `None` 表示用官方默认（`dev-` 是
    /// `profile notifications maps`）。
    pub async fn begin_device_auth(
        &self,
        scope: Option<&str>,
    ) -> std::result::Result<VrccDeviceAuthPrompt, VrccError> {
        let code = self.client.device_code(scope).await?;
        Ok(VrccDeviceAuthPrompt {
            user_code: code.user_code,
            verification_uri: code.verification_uri,
            verification_uri_complete: code.verification_uri_complete,
            expires_in_secs: code.expires_in.unwrap_or(600),
            interval_secs: code.interval.unwrap_or(5).max(5),
            device_code: code.device_code,
            scope: code.scope,
        })
    }

    /// 第二步：问一次令牌。按官方三种结果分类。
    pub async fn poll_device_auth(
        &self,
        prompt: &VrccDeviceAuthPrompt,
    ) -> std::result::Result<VrccDeviceAuthStep, VrccError> {
        match self.client.device_token(&prompt.device_code).await {
            Ok(token) => {
                let credential = StoredCredential {
                    token: token.access_token.clone(),
                    star: token.me.as_ref().and_then(|me| me.star),
                    vrc_name: token.me.as_ref().and_then(|me| me.vrc.clone()),
                    scope: token.scope.clone(),
                };
                self.store
                    .save(&credential)
                    .map_err(|error| VrccError::Transport(format!("保存令牌失败：{error}")))?;
                self.session.connect(
                    credential.token,
                    credential.star,
                    credential.vrc_name,
                    credential.scope,
                );
                Ok(VrccDeviceAuthStep::Connected(self.snapshot()))
            }
            Err(error) if error.is_authorization_pending() => Ok(VrccDeviceAuthStep::Pending {
                interval_secs: prompt.interval_secs,
            }),
            Err(error) if error.is_device_auth_terminal() => {
                Ok(VrccDeviceAuthStep::Terminal(error.to_string()))
            }
            Err(error) => Err(error),
        }
    }

    /// 断开：先让服务器吊销，再清本地。
    ///
    /// 吊销失败也照样清本地——玩家点了断开，界面上就该断开。
    pub async fn disconnect(&self) -> std::result::Result<(), VrccError> {
        let revoke = self.client.disconnect().await;
        self.store
            .clear()
            .map_err(|error| VrccError::Transport(format!("清除令牌失败：{error}")))?;
        self.session.disconnect();
        self.forget_unread();
        match revoke {
            Ok(()) => Ok(()),
            // 令牌本来就无效，等于已经断开了。
            Err(error) if error.needs_reauth() => Ok(()),
            Err(error) => Err(error),
        }
    }

    // -- 提醒 --------------------------------------------------------------

    /// 拉最近的提醒。
    ///
    /// `mark` 对应官方的 `?mark=1`：传 `true` 表示**玩家此刻正在看这个列表**，
    /// 服务器会把最新一条 ID 记到账号的 `read` 字段，跨设备同步已读状态。
    ///
    /// 两种调用场景必须区分：玩家打开「提醒」页时传 `true`；后台只为算数字时
    /// 传 `false`——否则等于替玩家把提醒标成看过了，而其实他没看。
    pub async fn recent_notifications(
        &self,
        mark: bool,
    ) -> std::result::Result<VrccRecentNotifications, VrccError> {
        self.ensure_connected()?;
        let result = self
            .client
            .recent_notifications(mark)
            .await
            .map_err(|error| self.settle(error))?;
        // 未读数按官方语义算：id 大于 read 的算未读。`read` 缺省时（账号上
        // 还没记过已读），把返回的条目都当作未读——这是唯一诚实的解读。
        //
        // 刚标记过 `mark=1` 时服务器会把 read 推到最新，未读随之变 0，这是对的：
        // 玩家确实看到了。
        let unread = match result.read {
            Some(read) => result.items.iter().filter(|item| item.id > read).count() as u32,
            None => result.items.len() as u32,
        };
        self.note_recent(unread);
        self.session
            .note_poll(vrcx_0_integrations::vrcc::now_millis(), None, None);
        Ok(result)
    }

    /// 当前所在地图在地图榜上的状态与我自己的评分。
    ///
    /// 需要 `maps`（普通权限）。官方的硬性要求是 `off` 为 `true` 时客户端不要
    /// 显示打分面板；这里在 `off` 时直接返回 `None`，调用方拿不到对象就不可能
    /// 误显示——比在每个 UI 分支里记得判断 `off` 更可靠。
    pub async fn my_map(&self, world: &str) -> std::result::Result<Option<VrccMyMap>, VrccError> {
        // 只认 wrld_…：带房间实例号的 location 会让接口查错图。
        let world = vrcx_0_core::location::world_id_from_location(world);
        if world.is_empty() {
            return Ok(None);
        }
        self.ensure_connected()?;
        let map = self
            .client
            .my_map(&world)
            .await
            .map_err(|error| self.settle(error))?;
        if !map.is_visible() {
            tracing::debug!(
                world = %world,
                "vrcc: this map is not shown to this player, hiding the card"
            );
            return Ok(None);
        }
        Ok(Some(map))
    }

    /// 问一次几处未读数（`me/unread`，1.13.0 起），结果进缓存。
    ///
    /// 返回 `(结果, 和上次比变没变)`。旧服务器没有这个接口（`404 not_found`）
    /// 时返回 `None`，之后一小时内不再问——服务器更新以后不用重启就能接上。
    pub async fn refresh_unread(
        &self,
    ) -> std::result::Result<Option<(VrccUnread, bool)>, VrccError> {
        self.ensure_connected()?;
        let skip = self
            .unread
            .read()
            .ok()
            .and_then(|cache| cache.skip_until)
            .is_some_and(|until| Instant::now() < until);
        if skip {
            return Ok(None);
        }
        match self.client.unread().await {
            Ok(counts) => {
                let changed = self.note_unread(&counts);
                Ok(Some((counts, changed)))
            }
            Err(error) if error.is_not_found() => {
                tracing::debug!("vrcc: me/unread is not on this server yet");
                if let Ok(mut cache) = self.unread.write() {
                    cache.counts = None;
                    cache.skip_until = Some(Instant::now() + UNREAD_RETRY_AFTER_NOT_FOUND);
                }
                Ok(None)
            }
            Err(error) => Err(self.settle(error)),
        }
    }

    /// 通知中心（`me/notes`，1.13.0 起，第二份读）。读了不标已读。
    ///
    /// 旧服务器没有这个接口时返回 `None`，界面就不显示这一块。
    pub async fn notes(&self) -> std::result::Result<Option<VrccNotes>, VrccError> {
        self.ensure_connected()?;
        match self.client.notes().await {
            Ok(notes) => Ok(Some(notes)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(self.settle(error)),
        }
    }

    /// 增量轮询。游标与建议间隔都自动维护。
    ///
    /// 这是官方给桌面客户端设计的轮询方式：两次之间至少 25 秒，**没有每天
    /// 上限**，每次只给 `since` 之后的新提醒。后台轮询就用它来弹系统通知。
    ///
    /// 响应里的 `dot` 是「星轨相交尚未查看的新人数」，**不是未读提醒数**，
    /// 所以这里不拿它更新腕表的 `unread`——两者混用会显示错误的数字。
    pub async fn poll_notifications(
        &self,
    ) -> std::result::Result<vrcx_0_contracts::vrcc::VrccNotifications, VrccError> {
        self.ensure_connected()?;
        let cursor = self.session.snapshot().notifications_cursor;
        let result = self
            .client
            .notifications(cursor)
            .await
            .map_err(|error| self.settle(error))?;
        self.session.note_poll(
            vrcx_0_integrations::vrcc::now_millis(),
            result.last,
            result.every,
        );
        // 有新提醒到达时，未读数会变；下次打开页面拉 recent 时会修正。
        // 这里若把 items 长度加上去，会和 `read` 的语义打架。
        Ok(result)
    }

    /// 今天这一轮 + 轮次进度（需要 `today` 敏感权限）。
    pub async fn today(&self) -> std::result::Result<VrccToday, VrccError> {
        self.ensure_connected()?;
        self.client
            .today()
            .await
            .map_err(|error| self.settle(error))
    }

    /// 「互相连上」的星友列表（需要 `mutuals` 敏感权限）。
    ///
    /// 条目带 `vrcUser` 时，前端可以打开 VRCX-0 自己的玩家详情页。按官方合规
    /// 要求，这里**不提供**代玩家发送好友请求的通道——加好友必须由玩家在
    /// 原生界面里自己点。
    pub async fn mutuals(&self) -> std::result::Result<VrccMutuals, VrccError> {
        self.ensure_connected()?;
        self.client
            .mutuals()
            .await
            .map_err(|error| self.settle(error))
    }

    fn ensure_connected(&self) -> std::result::Result<(), VrccError> {
        if self.session.snapshot().connected {
            Ok(())
        } else {
            Err(VrccError::TokenMissing)
        }
    }
}

#[cfg(test)]
mod tests;
