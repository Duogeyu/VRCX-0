// SPDX-License-Identifier: MPL-2.0

//! 星遇同频集成的应用服务。
//!
//! 这一层负责编排：拿令牌 → 调接口 → 投影成前端/腕表能用的形状。
//! 网络细节通过 [`VrccRemote`] 端口注入，持久化通过 [`VrccTokenStore`] 注入，
//! 两者的实现都在 outbound-adapters。本层不认识任何具体协议或存储。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use vrcx_0_application_core::Result;
use vrcx_0_contracts::vrcc::{
    device_auth_scope, is_dev_client, missing_scopes, ErrorDetail, ErrorLayer, TransportKind,
    VrccAnnouncements, VrccBoard, VrccBoardSort, VrccClubResponse, VrccClubs,
    VrccConnectionSnapshot, VrccCredentialState, VrccCrossingCards, VrccCrossings, VrccDeviceCode,
    VrccError, VrccGuidanceState, VrccMapPage, VrccModelBoard, VrccModelDetail, VrccMutuals,
    VrccMyBoard, VrccMyBoardSort, VrccMyClubResponse, VrccMyClubs, VrccMyMap, VrccMyMapPage,
    VrccNotes, VrccNotifications, VrccRatedMaps, VrccRatingText, VrccRecentNotifications,
    VrccStatus, VrccStickers, VrccToday, VrccTokenResponse, VrccUnread, VrccUsageSnapshot,
    VrccVote, VrccVoteKind, VrccVoteResult, VrccWristSummary,
};

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

    /// 读「上一个令牌的结局」。
    ///
    /// 没有记录（新账号、或从没连过的账号）时返回 [`VrccCredentialState::Never`]。
    /// 令牌还在时以 [`Self::load`] 为准，这里不会给出 `Active`。
    fn load_state(&self) -> Result<VrccCredentialState>;

    /// 记下「上一个令牌的结局」。
    ///
    /// **必须和 [`Self::clear`] 分开**：`clear` 只清令牌，这一项要留着，否则
    /// 「连过又断开」的账号重启后会退化成「从没连过」。`Active` / `Never`
    /// 落盘时等于清掉记录。
    fn save_state(&self, state: VrccCredentialState) -> Result<()>;

    /// 读「首连引导讲到哪一步」。
    ///
    /// 和凭据状态同一个账号命名空间，但**互不影响**：断开连接不该让玩家
    /// 重新看一遍引导，换账号也不该继承别人的「已看过」。
    fn load_guidance(&self) -> Result<VrccGuidanceState>;

    /// 记下「首连引导讲到哪一步」。
    fn save_guidance(&self, state: VrccGuidanceState) -> Result<()>;
}

/// 一次异步调用的结果。前端/上层都按 [`VrccError`] 分类，所以这里固定用它的
/// 错误类型，而不是 `vrcx_0_application_core::Error`——两者语义不同：
/// `VrccError` 要能区分「重新授权 / 缺权限 / 限流 / 网络」，那是业务判定。
pub type VrccFuture<'a, T> = futures_util::future::BoxFuture<'a, std::result::Result<T, VrccError>>;

/// 会话状态快照（应用层视角）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VrccConnection {
    pub connected: bool,
    /// 令牌被服务器判过失效（`404 gone`），界面要说清是「连接已失效」。
    pub expired: bool,
    pub star: Option<i64>,
    pub vrc_name: Option<String>,
    pub scope: Option<String>,
    /// 当前使用的 `X-Client` 标识。
    pub client: String,
    /// `me/notifications` 的游标；连上以后还没问过时为 `None`。
    pub notifications_cursor: Option<i64>,
    /// 上次成功轮询提醒的时间（毫秒）。
    pub last_poll_at: Option<i64>,
    /// 服务器建议的轮询间隔（秒，不低于 25）。
    pub poll_every_secs: i64,
}

/// 本客户端的用量快照（应用层视角）。
///
/// 全部来自服务端的 `RateLimit-*`。没拿到过就是 `None`——界面显示「未知」，
/// 不要拿本地日期推算一个数出来（官方按北京时间零点重置，本地算不准）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VrccLocalUsage {
    pub reads: Option<(i64, i64)>,
    pub unread: Option<(i64, i64)>,
    pub window_used: usize,
    pub backoff_remaining_ms: i64,
    pub reset_secs: Option<i64>,
}

/// 星遇同频网络端口。
///
/// **为什么需要这一层**：应用层要编排（先鉴权、再按语义算未读、决定要不要
/// 让玩家重连），但具体协议在 `integrations`。`backend_architecture` 规定
/// 应用层不得依赖出向实现，所以这里只声明能力，实现放在 `outbound-adapters`。
///
/// 所有方法都返回 [`VrccError`]，因为分类是应用层的事：同一个 404，
/// `gone` 要让玩家重连、`not_found` 只是接口还没上线。
pub trait VrccRemote: Send + Sync {
    // -- 开放数据（不需要连接） -------------------------------------------
    fn boards<'a>(&'a self, limit: u32, sort: VrccBoardSort) -> VrccFuture<'a, VrccBoard>;
    /// 模型榜（1.17.0 起）。和 [`Self::boards`] 同一套 `sort` 语义。
    fn models_board<'a>(
        &'a self,
        limit: u32,
        sort: VrccBoardSort,
    ) -> VrccFuture<'a, VrccModelBoard>;
    /// 单个模型。不在榜上时服务器回 `404 not_found`，这里转成 `None`。
    fn model<'a>(&'a self, id: i64) -> VrccFuture<'a, Option<VrccModelDetail>>;
    fn maps<'a>(&'a self, worlds: &'a [String]) -> VrccFuture<'a, VrccBoard>;
    fn status<'a>(&'a self) -> VrccFuture<'a, VrccStatus>;
    fn fetch_site_asset<'a>(&'a self, path: &'a str) -> VrccFuture<'a, Vec<u8>>;
    fn announcements<'a>(&'a self) -> VrccFuture<'a, VrccAnnouncements>;
    fn stickers<'a>(&'a self) -> VrccFuture<'a, VrccStickers>;
    fn map_page<'a>(&'a self, world: &'a str) -> VrccFuture<'a, VrccMapPage>;
    /// 社团公开页（`GET /clubs/{code}`，1.18.0 起）。**不需要连接。**
    fn club<'a>(&'a self, code: &'a str) -> VrccFuture<'a, VrccClubResponse>;
    /// 所有公开社团（`GET /clubs`，1.19.2 起）。**不需要连接。**
    fn clubs<'a>(&'a self, limit: u32, offset: u32) -> VrccFuture<'a, VrccClubs>;
    fn my_map_page<'a>(
        &'a self,
        world: &'a str,
        sort: Option<&'a str>,
    ) -> VrccFuture<'a, VrccMyMapPage>;
    // -- 写操作（需要 maps.rate） ------------------------------------------
    fn rate_map<'a>(
        &'a self,
        world: &'a str,
        score: i64,
        text: &'a VrccRatingText,
    ) -> VrccFuture<'a, VrccMyMap>;
    fn delete_map_rating<'a>(&'a self, world: &'a str) -> VrccFuture<'a, ()>;
    // -- 写操作（需要 maps.write） -----------------------------------------
    fn vote_map_content<'a>(
        &'a self,
        kind: VrccVoteKind,
        id: i64,
        vote: VrccVote,
    ) -> VrccFuture<'a, VrccVoteResult>;

    // -- 设备授权 ----------------------------------------------------------
    fn device_code<'a>(&'a self, scope: Option<&'a str>) -> VrccFuture<'a, VrccDeviceCode>;
    fn device_token<'a>(&'a self, device_code: &'a str) -> VrccFuture<'a, VrccTokenResponse>;
    fn revoke<'a>(&'a self) -> VrccFuture<'a, ()>;

    // -- 个人接口（需要连接） ----------------------------------------------
    fn recent_notifications<'a>(&'a self, mark: bool) -> VrccFuture<'a, VrccRecentNotifications>;
    fn notifications<'a>(&'a self, since: Option<i64>) -> VrccFuture<'a, VrccNotifications>;
    fn my_map<'a>(&'a self, world: &'a str) -> VrccFuture<'a, VrccMyMap>;
    fn my_maps<'a>(&'a self) -> VrccFuture<'a, VrccRatedMaps>;
    fn my_board<'a>(
        &'a self,
        sort: VrccMyBoardSort,
        q: Option<&'a str>,
    ) -> VrccFuture<'a, VrccMyBoard>;
    fn unread<'a>(&'a self) -> VrccFuture<'a, VrccUnread>;
    fn notes<'a>(&'a self) -> VrccFuture<'a, VrccNotes>;
    fn today<'a>(&'a self) -> VrccFuture<'a, VrccToday>;
    fn mutuals<'a>(&'a self) -> VrccFuture<'a, VrccMutuals>;
    /// 玩家能看到的社团（`GET /me/clubs`，1.18.0 起）。需要 `clubs` 敏感权限。
    fn my_clubs<'a>(&'a self) -> VrccFuture<'a, VrccMyClubs>;
    /// 一个社团的社团页（`GET /me/clubs/{code}`）。需要 `clubs`。
    fn my_club<'a>(&'a self, code: &'a str) -> VrccFuture<'a, VrccMyClubResponse>;
    /// 星轨相交列表。`mark` 只在玩家真正打开该页时为 true。
    fn crossings<'a>(&'a self, mark: bool) -> VrccFuture<'a, VrccCrossings>;
    /// 只看「星轨相交对这个玩家可见吗」，不标已读。
    ///
    /// 和 `crossings(false)` 请求的是同一份数据，存在的理由是**本地限流槽
    /// 不一样**：默认实现退化成普通读取，只有真的做限流的实现（`VrccClient`）
    /// 才覆盖它去用独立的一个槽。
    ///
    /// 为什么需要它：可见性探测发生在打开页面时，紧接着玩家就可能切到这个
    /// 页签，那一次要带 `mark=1` 清红点。两者共用一个 30 秒的槽时，探测会把
    /// 玩家那次挡在本地——请求发不出去，红点永远消不掉。
    fn crossings_probe<'a>(&'a self) -> VrccFuture<'a, VrccCrossings> {
        self.crossings(false)
    }
    /// 星轨相交的完整星卡（1.13.0 起）。算第二份读的额度。
    fn crossing_cards<'a>(&'a self, mark: bool) -> VrccFuture<'a, VrccCrossingCards>;

    // -- 本地状态（不联网） -------------------------------------------------
    /// 把令牌交给实现方持有。之后所有个人请求自动带上它。
    fn connect(
        &self,
        token: String,
        star: Option<i64>,
        vrc_name: Option<String>,
        scope: Option<String>,
    );
    /// 玩家主动断开：只清本地。
    fn disconnect(&self);
    /// 令牌被判失效（`404 gone`）：清本地并记下原因。
    ///
    /// **只在会话代次还是 `generation` 时清**，返回清了没有。失败是拿那一代的
    /// 令牌换来的；会话之后如果已经换过（重新授权、换号），说的就是旧令牌。
    /// 判断和清理必须在实现方的同一把锁里，中间插不进别的连接。
    fn expire(&self, generation: u64) -> bool;
    /// 当前会话代次：连上、断开、被判失效时各推进一次。
    ///
    /// 请求发出前记下，响应回来时拿它判断「这个响应还属不属于现在的会话」。
    fn generation(&self) -> u64;
    fn connection(&self) -> VrccConnection;
    fn client_id(&self) -> String;
    fn local_usage(&self) -> VrccLocalUsage;
    /// 记一次轮询结果（游标与建议间隔）。
    ///
    /// **只在会话代次还是 `generation` 时记**，返回记了没有：换号或重新授权之后
    /// 才回来的结果，不能把旧会话的游标写进新会话。
    fn note_poll(
        &self,
        generation: u64,
        at_ms: i64,
        cursor: Option<i64>,
        every_secs: Option<i64>,
    ) -> bool;
    /// 限流窗口容量（每分钟）。
    fn window_limit(&self) -> i64;
    /// 当前毫秒时间戳，由实现方提供以保持「时间来自一处」。
    fn now_millis(&self) -> i64;
}

/// 吊销失败是否属于「请求没到服务器、值得再试一次」。
///
/// 判据是**层级**：本机限流（`Client`）和传输失败（`Transport`）都意味着
/// 请求没有送到服务器，重试有意义。服务器答复里的错误（`Server`）**不算**——
/// 那说明请求已经到了，服务器自己决定拒绝，重试也不会变。
///
/// 早先这里逐个匹配 `RateLimited` / `TooSoon` / `Transport` 三个变体，
/// 每加一种错误就要回来补一次；现在只问一次层级。
fn retriable_revoke_failure(error: &VrccError) -> bool {
    matches!(error.layer, ErrorLayer::Client | ErrorLayer::Transport)
}

/// 前端能区分的行为分类。
///
/// 这一层是给界面的**动作提示**，不是错误的结构。结构见 [`VrccError::layer`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VrccErrorKind {
    /// 需要玩家重新授权。
    NeedsReauth,
    /// 缺某个权限，`need` 里说明是哪项。
    MissingScope,
    /// 限流或额度用尽，等等再来。
    Throttled,
    /// 服务端还没准备好，稍后再试（`409 busy`：这一轮的星卡还在生成）。
    ///
    /// 和 [`Self::Throttled`] 分开：那个是「你问太多了」，这个是「那边还没好」，
    /// 玩家能做的动作不同（前者该等，后者该过一会儿重试，都不用重连）。
    NotReady,
    /// 网络或服务端问题。
    Transport,
    /// 其它。
    Other,
}

/// 跨层传递的错误：**分类结论 + 层级 + 缺的权限 + 原文**。
///
/// 为什么不是 `String`：错误分类必须在产生错误的地方算（那里知道
/// `VrccError` 的确切结构），而消费它的地方（命令层、前端）不该靠解析
/// 中文文案反推。早先 runtime 层把错误折成 `to_string()`，前端再对文案做
/// 子串匹配，结果 `401 token` 落进 `Other`、`need` 在转字符串时直接丢失、
/// `missing_scope` 提示永远不显示——都是这一个设计问题带出来的。
///
/// `message` 保留原文只为日志与兜底显示；判断分支一律看 `kind` / `layer` /
/// `need`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VrccFailure {
    pub kind: VrccErrorKind,
    /// 失败发生在哪一层。前端据此区分「网络问题」和「业务拒绝」。
    pub layer: ErrorLayer,
    /// 缺哪个权限（`kind == MissingScope` 时通常有值）。
    pub need: Option<String>,
    /// 传输层失败卡在哪一步；不是传输层错误时为 `None`。
    pub transport_kind: Option<TransportKind>,
    /// 发起调用的接口标识，用于区分「同一个码在不同接口下的含义」。
    pub op: Option<&'static str>,
    /// 人可读的错误原文，用于日志与兜底显示。
    pub message: String,
}

impl VrccFailure {
    /// 从应用层错误造一个。分类在这里一次算清，后面不再重算。
    pub fn from_error(error: &VrccError) -> Self {
        let (kind, need) = classify_vrcc_error(error);
        let transport_kind = match &error.detail {
            ErrorDetail::Transport { kind, .. } => Some(*kind),
            _ => None,
        };
        Self {
            kind,
            layer: error.layer,
            need,
            transport_kind,
            op: error.http().and_then(|http| http.op),
            message: error.to_string(),
        }
    }
}

impl std::fmt::Display for VrccFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// 判定一个错误属于哪一类，并把缺的权限一并带回。
///
/// 先看层级（[`ErrorLayer`]），再在 `Server` 层里看具体错误码。
///
/// **`Throttled` 只表示「你问太多了」**，不表示「额度用完了」：本机限流
/// 和 `429 too_soon` / `429 too_many` / `429 quota` 都归这里，因为玩家该做的
/// 动作都是「等」。
///
/// **`notReady` 表示「那边还没好」**，和 `Throttled` 相反。`409 busy`（星卡还在
/// 生成）和 `429 busy`（全站连接功能暂停到次日）都归这里——后者本机一次都没
/// 用过，说成「额度用完了」是错的。
pub fn classify_vrcc_error(error: &VrccError) -> (VrccErrorKind, Option<String>) {
    let need = error.needed_scope().map(str::to_string);

    // 本机拦下的请求都没发出去。玩家该做的就是等，或者改用缓存。
    if error.layer == ErrorLayer::Client {
        return (VrccErrorKind::Throttled, need);
    }
    // 网络层：传输失败、非 JSON 响应。停站维护（`503 stop`）也在这里——
    // 令牌没事，等一会儿就好，排查方向和网络问题一致。
    if error.layer == ErrorLayer::Transport {
        return (VrccErrorKind::Transport, need);
    }

    let kind = match &error.detail {
        ErrorDetail::TokenMissing => VrccErrorKind::NeedsReauth,
        // 我们自己的 bug，界面上只配显示「操作失败」。它本不该发生，
        // 落在这里说明代码有问题，会由日志暴露出来。
        ErrorDetail::AbsolutePath(_)
        | ErrorDetail::BodyTooLarge
        | ErrorDetail::InvalidArgument(_)
        | ErrorDetail::MissingArgument(_) => VrccErrorKind::Other,
        ErrorDetail::Transport { .. } | ErrorDetail::Decode(_) => VrccErrorKind::Transport,
        ErrorDetail::ThrottledLocally { .. } => VrccErrorKind::Throttled,
        ErrorDetail::Http(http) => {
            // 同一个 `busy` 串在 409 和 429 下含义相反，只有按状态码才能分开
            // （官方文档：「同一个码有两种状态的（busy、xd_world），按状态码区分」）。
            if http.error == "busy" {
                VrccErrorKind::NotReady
            } else if http.status == 429 {
                VrccErrorKind::Throttled
            } else if http.status == 503 && http.error == "stop" {
                VrccErrorKind::Transport
            } else if error.needs_reauth()
                && matches!(http.error.as_str(), "scope" | "scope_unreviewed")
            {
                // 缺权限：令牌仍有效，只能提示重新授权去多要一项。
                // 判据按错误码而不是 `need` 是否为空——服务器漏带 `need`
                // 时它也归这里，否则会落到 NeedsReauth，被 settle() 当成
                // 令牌作废清掉，白扔一份还能收提醒、看榜单的授权。
                VrccErrorKind::MissingScope
            } else if error.needs_reauth() {
                VrccErrorKind::NeedsReauth
            } else {
                VrccErrorKind::Other
            }
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
    /// 签发这个令牌时用的 `X-Client`；旧存档没有这一项。
    ///
    /// 令牌只能配签发它的那个 client 使用，换了标识会 `403 client_mismatch`。
    /// 加载时和当前构建期标识比对，不一致就丢弃——
    /// 否则换标识后旧令牌会在启动时被恢复成「已连接」，之后每次请求都 403。
    pub client_id: Option<String>,
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

/// 令牌被判失效之后的回调，见 [`VrccService::on_session_expired`]。
type SessionExpiredHook = Arc<dyn Fn() + Send + Sync>;

/// 星遇同频服务。
pub struct VrccService {
    store: Arc<dyn VrccTokenStore>,
    /// 网络端口。实现是 `outbound-adapters` 里的适配器，本层不知道协议细节。
    remote: Arc<dyn VrccRemote>,
    /// 令牌被判失效、本地已经清掉之后要做的事，见 [`Self::on_session_expired`]。
    session_expired: Arc<RwLock<Option<SessionExpiredHook>>>,
    /// 凭据状态（跨重启）。
    ///
    /// 不从 `remote.connection()` 取：那是网络侧会话的即时状态，重启就没了。
    /// 这一项在构造时从仓库读一次，之后由「连上 / 断开 / 被判失效」三个动作更新。
    credential: Arc<RwLock<VrccCredentialState>>,
    /// 首连引导讲到哪一步（跨重启，按账号）。
    ///
    /// 和 `credential` 分开：一个说「连没连上」，一个说「讲过了没有」。
    /// 连上过的人可能从没看过引导（引导是后加的功能），反过来也一样。
    guidance: Arc<RwLock<VrccGuidanceState>>,
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
    /// 按 BOOTH 商品编号缓存的单个模型详情（含热评）。
    ///
    /// 不拿榜单里那一行顶上：行里没有热评，而热评正是对话框要显示的东西。
    /// 官方说信息最多 5 分钟更新一次，所以同一张卡在
    /// 几分钟内反复点开不该重新请求——玩家常常点开、关掉、再点开同一条。
    ///
    /// 值里带时间戳：热评会变，缓存不能永久有效。
    ///
    /// ## 键里为什么要带账号
    ///
    /// **不能假设「这个接口永远不带令牌」。** 今天 `GET /models/{id}` 确实是
    /// 公开数据（`authed: false`），但权限是可以再申请的，接口也可能开始返回
    /// 跟登录态绑定的东西（「你投过票没有」「你的收藏」）。那一天到来时，
    /// 一个不含账号维度的缓存会**直接把 A 的详情给 B 看**。
    ///
    /// 所以这里不靠注释约定，而是让类型强制：键是 [`OwnedModelKey`]，
    /// 构造它**必须**带上 [`VrccCacheScope`]——当前令牌的 `star`，未登录时是
    /// `None`。谁以后把接口改成带令牌，只要还是走这个键，就不会串号。
    model_details: Arc<RwLock<HashMap<OwnedModelKey, (Instant, VrccModelDetail)>>>,
    /// 最近确认「不在榜上」的模型，以及确认的时刻。键同 [`Self::model_details`]。
    ///
    /// 只记 [`MODEL_MISSING_TTL`]（30 秒），不是 5 分钟。理由是它挡的根本
    /// 不是「服务器状态可能变了」，而是**本地限流**：`models/{id}` 的
    /// `min_gap` 是 30 秒，同一个 id 在这段时间里再问一次会被
    /// [`MAX_ACQUIRE_WAIT`] 直接拒掉。既然这 30 秒里本来就不允许再问，
    /// 那「刚刚问过、服务器说没有」就是这段时间里最新的答案。
    ///
    /// 不记 5 分钟：下架和重新上架都会发生，把「没有」记久了会让玩家以为
    /// 它消失了。
    model_missing: Arc<RwLock<HashMap<OwnedModelKey, Instant>>>,
}

/// 缓存的分区：这条数据是在**哪个登录态**下拿到的。
///
/// `None` 是「未登录 / 没有令牌」，它自己独占一个分区——匿名拿到的公开数据
/// 和登录后拿到的数据不能互相顶替。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct VrccCacheScope(Option<i64>);

/// 单模型缓存的键：**账号分区 + BOOTH 编号**。
///
/// 单独一个类型而不是 `(i64, i64)`，就是为了让「忘了加账号维度」编译不过：
/// 所有取键的地方都得先拿到 [`VrccCacheScope`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct OwnedModelKey {
    scope: VrccCacheScope,
    id: i64,
}

impl OwnedModelKey {
    fn new(scope: VrccCacheScope, id: i64) -> Self {
        Self { scope, id }
    }
}

/// 单个模型详情的本地缓存时长。
///
/// 官方说明信息最多 5 分钟更新一次，取 5 分钟正好卡在「服务器可能已经变了」
/// 和「玩家反复点开同一条不必重新请求」之间。
const MODEL_DETAIL_TTL: Duration = Duration::from_secs(300);

/// 单模型缓存最多留几份，防止长时间浏览把内存堆起来。
///
/// 容量按「点开过多少个详情」算，不是按榜单页数算——这里装的是每次点开卡片
/// 才会取的那份详情，跟一页显示几行没有关系。超量时淘汰最旧的四分之一，
/// 不是整表清空（见 [`VrccService::remember_model_detail`]）：整表清空会让
/// 「刚看过的一屏模型」一起失效，下次点开又是满屏转圈。
const MODEL_DETAIL_CACHE_MAX: usize = 200;

/// 「不在榜上」这份短记要留多久。
///
/// 取 30 秒，和 `models/{id}` 的本地限流间隔一样长：超过这个窗口就可以
/// 重新问服务器了，所以没有理由把负结果留得比窗口更久。
const MODEL_MISSING_TTL: Duration = Duration::from_secs(30);

/// 从仓库恢复会话，并给出「这个账号处在哪一档」。
///
/// 只有这里会做「令牌和当前 `X-Client` 是否匹配」的判断：令牌只能配签发它的那个
/// `X-Client` 使用，换了标识会 `403 client_mismatch`。与其恢复成「已连接」再让每次
/// 请求都失败，不如当作没连，让玩家走一次新的设备授权。
///
/// 读仓库失败不往外抛：构造发生在装配阶段，这里没有能把错误交给玩家的地方。
/// 退回 [`VrccCredentialState::Never`] 是安全的——最坏结果是把老玩家当新人多讲
/// 一遍入门，而不是把一个坏令牌当成「已连接」。
fn restore_credential(
    store: &Arc<dyn VrccTokenStore>,
    remote: &Arc<dyn VrccRemote>,
) -> VrccCredentialState {
    match store.load() {
        Ok(Some(credential)) => {
            let signed_by = credential.client_id.as_deref();
            let current = remote.client_id();
            let usable = match signed_by {
                // 旧存档没记标识：当时只有 `dev-` 一种，按兼容处理继续用。
                None => true,
                Some(id) => id == current,
            };
            if usable {
                // 权限也是签发时定死的：这个构建要的权限如果多于令牌拿到的，
                // 那些接口会一直 `403 scope`。先算出来，连上之后再决定报哪一档。
                let missing =
                    missing_scopes(credential.scope.as_deref(), device_auth_scope(&current));
                remote.connect(
                    credential.token,
                    credential.star,
                    credential.vrc_name,
                    credential.scope,
                );
                if missing.is_empty() {
                    return VrccCredentialState::Active;
                }
                // **不清令牌**：它还能收提醒、看榜单，清掉等于白扔一份授权。
                // 界面该说的是「重新连一次可以多要一项权限」，不是「连接已失效」。
                tracing::info!(
                    ?missing,
                    "vrcc: the stored token grants fewer scopes than this build asks for"
                );
                return VrccCredentialState::ScopeMismatch;
            }
            tracing::info!(
                stored = signed_by.unwrap_or(""),
                current = %current,
                "vrcc: stored token was issued by a different X-Client; asking for a new authorization"
            );
            // 顺手清掉，免得每次启动都重新判断一遍；但**要把结局记下来**，
            // 否则这个账号重启后会变成「从没连过」。
            let _ = store.clear();
            if let Err(error) = store.save_state(VrccCredentialState::GrantMismatch) {
                tracing::warn!(%error, "vrcc: failed to record the credential outcome");
            }
            VrccCredentialState::GrantMismatch
        }
        // 没有令牌：结局由上次离开时记下的那一档决定。
        Ok(None) => match store.load_state() {
            // `Active` 却没有令牌说明记录和实际不符（例如令牌被手动删了），
            // 当「从没连过」处理，不要显示一个不存在的连接。
            Ok(state) if state != VrccCredentialState::Active => state,
            Ok(_) => VrccCredentialState::Never,
            Err(error) => {
                tracing::warn!(%error, "vrcc: failed to read the credential outcome");
                VrccCredentialState::Never
            }
        },
        Err(error) => {
            tracing::warn!(%error, "vrcc: failed to load the stored credential");
            VrccCredentialState::Never
        }
    }
}

impl VrccService {
    /// 从仓库恢复会话：有令牌就直接进入已连接状态。
    pub fn new(store: Arc<dyn VrccTokenStore>, remote: Arc<dyn VrccRemote>) -> Self {
        let credential_state = restore_credential(&store, &remote);
        // 读不到就当「没弹过」：最坏是给老玩家多讲一遍入门，而不会把该讲的
        // 人永久跳过。
        let guidance_state = store.load_guidance().unwrap_or_default();
        Self {
            store,
            remote,
            session_expired: Arc::new(RwLock::new(None)),
            credential: Arc::new(RwLock::new(credential_state)),
            guidance: Arc::new(RwLock::new(guidance_state)),
            wrist: Arc::new(RwLock::new(None)),
            boards: Arc::new(RwLock::new(HashMap::new())),
            unread: Arc::new(RwLock::new(UnreadCache::default())),
            model_details: Arc::new(RwLock::new(HashMap::new())),
            model_missing: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 登录账号变了（登录、登出、换号）：按新账号重新恢复会话。
    ///
    /// 令牌、凭据结局和引导表态都按账号存，而 [`Self::new`] 发生在装配阶段，
    /// 那时还没登录——仓库对「没有账号」一律答「没有」，所以那一次恢复对连过
    /// 的玩家永远是「从没连过」：每次重启都要重新授权，首连引导也会再弹一遍。
    /// 真正的恢复要等登录作用域就位之后再做；换号、登出时也一样，上一个账号的
    /// 令牌、游标和未读数不能留给下一个账号。
    ///
    /// **只清本地，不吊销**：换号不等于玩家要在服务器上断开那个账号的授权，
    /// 换回来时还要接着用。
    pub fn reload_account(&self) {
        self.remote.disconnect();
        self.forget_unread();
        let credential_state = restore_credential(&self.store, &self.remote);
        let guidance_state = self.store.load_guidance().unwrap_or_default();
        if let Ok(mut credential) = self.credential.write() {
            *credential = credential_state;
        }
        if let Ok(mut guidance) = self.guidance.write() {
            *guidance = guidance_state;
        }
    }

    /// 腕表摘要。`world_id` 由调用方给（腕表的渲染侧知道玩家在哪张图）。
    ///
    /// 纯内存读取，不发请求。返回 `None` 的三种情况：还没连上、还没拉到
    /// 任何数据、缓存锁坏了。腕表在 `None` 时整块不显示，底栏继续显示
    /// 原本的玩家数。
    ///
    /// **必须检查连接状态**：地图榜是开放数据，没连接也能拉到，而
    /// `note_boards` 一拉到就会把缓存从 `None` 变成 `Some`。若不在这里拦，
    /// 一个「从没连过星遇同频、只打开过一次页面」的玩家进入上榜地图后，
    /// 腕表底栏会从「12 名玩家」变成「#3 8.8」，而且没有任何设置能恢复。
    pub fn wrist_summary(&self, world_id: Option<&str>) -> Option<VrccWristSummary> {
        if !self.remote.connection().connected {
            return None;
        }
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
    ///
    /// `generation` 是请求发出时的会话代次：会话换过了就不写，见 [`Self::note_unread`]。
    fn note_recent(&self, generation: u64, unread: u32) {
        if self.cached_unread().is_some() {
            return;
        }
        let Ok(mut wrist) = self.wrist.write() else {
            return;
        };
        if self.remote.generation() != generation {
            return;
        }
        let entry = wrist.get_or_insert_with(VrccWristSummary::default);
        entry.unread = unread;
    }

    /// 拿到 `me/unread` 后更新缓存和腕表。返回和上一次比有没有变。
    ///
    /// `generation` 是请求发出时的会话代次。换号、重新授权之后才回来的结果
    /// 不能写进来，否则新账号会看到旧账号的未读数。
    ///
    /// 代次在**持着锁的时候**判断：会话变化时总是先推进代次、再
    /// [`Self::forget_unread`]（它要拿同样两把锁）。所以要么这里先看到新代次、
    /// 不写；要么这里先写完，随后被 `forget_unread` 清掉。不会留下旧数。
    fn note_unread(&self, generation: u64, counts: &VrccUnread) -> bool {
        let changed = {
            let Ok(mut cache) = self.unread.write() else {
                return false;
            };
            if self.remote.generation() != generation {
                return false;
            }
            let changed = cache.counts.as_ref() != Some(counts);
            cache.counts = Some(counts.clone());
            cache.skip_until = None;
            changed
        };
        if let Ok(mut wrist) = self.wrist.write() {
            if self.remote.generation() != generation {
                return false;
            }
            let entry = wrist.get_or_insert_with(VrccWristSummary::default);
            entry.unread = u32::try_from(counts.total()).unwrap_or(u32::MAX);
        }
        changed
    }

    /// 断开、令牌失效或换账号后，这些数字不再属于任何人：整块清掉。
    ///
    /// 只把 `unread` 归零是不够的——腕表里还留着上一个账号的 `●N` 和地图
    /// 名次缓存；重新连上时 `note_unread` 会 `get_or_insert_with` 复用它，
    /// 于是新账号先看到旧账号的摘要。直接置 `None`，让 `wrist_summary`
    /// 按「没连过」处理，底栏回落显示玩家数。
    fn forget_unread(&self) {
        if let Ok(mut cache) = self.unread.write() {
            cache.counts = None;
        }
        if let Ok(mut wrist) = self.wrist.write() {
            *wrist = None;
        }
    }

    /// 最近一次 `me/unread` 的结果，纯内存读取。
    pub fn cached_unread(&self) -> Option<VrccUnread> {
        self.unread.read().ok()?.counts.clone()
    }

    /// 收拾一个错误：令牌已被服务器判作废时清掉它。
    ///
    /// 判据复用 [`classify_vrcc_error`] 的结论，而不是自己再解析一次 HTTP
    /// 细节：分类只在那一处算（错误结构最清楚的地方），这里只负责副作用。
    /// `NeedsReauth` 涵盖 `404 gone`、`403 client_mismatch` / `client_unknown` /
    /// `client_off` / `client_missing`、`401 token`；`MissingScope`（缺权限）
    /// 不在此列——那种令牌仍有效，清掉等于白扔一份还能收提醒、看榜单的授权。
    ///
    /// 不清的话，后台轮询会拿着死令牌每分钟问一次，界面还一直显示「已连接」。
    /// 1.10.0 起「90 天没调用过任何接口」也会这样，所以这不是罕见情况。
    ///
    /// `generation` 是请求发出时的会话代次。两种情况不收拾，只把错误原样交回：
    ///
    /// - `TokenMissing`：本地本来就没有令牌，没有东西可清。不排除的话，玩家点
    ///   「断开」时正在路上的请求回来 `TokenMissing`，会把刚记下的
    ///   `Disconnected` 改写成 `Expired`，界面说「连接已失效」。
    /// - 会话已经换过（重新授权、换号）：失败说的是旧令牌。例如重新授权期间，
    ///   旧令牌的请求回来 `404 gone`（同一客户端再次授权会让旧令牌失效），
    ///   不能拿它把刚存下的新令牌删掉。
    ///
    /// 清掉之后调用 [`Self::on_session_expired`] 挂上的回调。
    fn settle(&self, generation: u64, error: VrccError) -> VrccError {
        if matches!(error.detail, ErrorDetail::TokenMissing) {
            return error;
        }
        if classify_vrcc_error(&error).0 != VrccErrorKind::NeedsReauth {
            return error;
        }
        // 先在会话里按代次清：判断和清理在同一把锁里。会话已经换过就到此为止。
        if !self.remote.expire(generation) {
            tracing::debug!("vrcc: a token failure from an earlier session came back; ignoring it");
            return error;
        }
        tracing::info!(
            "vrcc: token is invalid (disconnected on the site, re-authorized elsewhere, client mismatch, or unused for 90 days); dropping it"
        );
        if let Err(clear_error) = self.store.clear() {
            tracing::warn!(%clear_error, "vrcc: failed to clear the dead token");
        }
        self.forget_unread();
        self.set_credential_state(VrccCredentialState::Expired);
        self.notify_session_expired();
        error
    }

    /// 令牌被服务器判失效、本地已经清掉之后要做什么。桌面侧在这里告诉前端
    /// 「连接变了」。
    ///
    /// 前台命令和后台轮询撞上的失效都经过 [`Self::settle`]，挂在这一处，两条路
    /// 都会通知到。早先只有后台轮询发事件，前台撞上的（例如提交评分收到
    /// `404 gone`）不发，已经打开的面板一直拿着旧账号的数据。
    ///
    /// 再挂一次会替换上一个。
    pub fn on_session_expired(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.session_expired.write() {
            *slot = Some(hook);
        }
    }

    fn notify_session_expired(&self) {
        // 先取出来再调：回调里如果又碰到服务，不能还持着这把锁。
        let hook = self
            .session_expired
            .read()
            .ok()
            .and_then(|slot| slot.clone());
        if let Some(hook) = hook {
            hook();
        }
    }

    /// 拿到地图榜后更新名次缓存。
    ///
    /// 榜单是开放数据，不带令牌也能拉。但这里**不把腕表缓存设成 `Some`**：
    /// 那会让「从没连过、只打开过一次页面」的玩家也在腕表底栏看到名次，
    /// 把原本的玩家数顶掉。名次只在已经连上、且腕表本来就有数据时才显示，
    /// 由 `wrist_summary` 现查缓存补上。
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
    }

    /// 当前登录态对应的缓存分区。
    ///
    /// 取的是令牌里的 `star`：它标识「这条数据是在谁的登录态下拿到的」。
    /// 未连接时是 `None`，那是一个独自分区——匿名拿到的公开数据不能顶替
    /// 登录后拿到的数据，反之亦然。
    ///
    /// 每个碰缓存的地方都先取它，这样「忘了加账号维度」在编译期就过不去。
    fn cache_scope(&self) -> VrccCacheScope {
        VrccCacheScope(self.remote.connection().star)
    }

    /// 取缓存里的单模型详情；过期就当成没有。
    fn cached_model_detail(&self, key: OwnedModelKey) -> Option<VrccModelDetail> {
        let cache = self.model_details.read().ok()?;
        let (at, detail) = cache.get(&key)?;
        (at.elapsed() < MODEL_DETAIL_TTL).then(|| detail.clone())
    }

    /// 记下一份单模型详情；超量时淘汰最旧的一批，而不是整表清空。
    fn remember_model_detail(&self, key: OwnedModelKey, detail: &VrccModelDetail) {
        let Ok(mut cache) = self.model_details.write() else {
            return;
        };
        if cache.len() >= MODEL_DETAIL_CACHE_MAX {
            // 整表清空会让「刚看过的一屏模型」全部失效，下次点开又是满屏
            // 转圈。按时间戳淘汰最旧的 1/4：两百条排序一次的开销可以忽略，
            // 而且发生频率远低于每次写入。
            let drop = MODEL_DETAIL_CACHE_MAX.div_ceil(4);
            let mut by_age: Vec<(Instant, OwnedModelKey)> =
                cache.iter().map(|(key, (at, _))| (*at, *key)).collect();
            by_age.sort_unstable_by_key(|(at, _)| *at);
            for (_, key) in by_age.into_iter().take(drop) {
                cache.remove(&key);
            }
        }
        cache.insert(key, (Instant::now(), detail.clone()));
    }

    /// 这个 id 在**当前登录态**下，最近是不是刚被确认过「不在榜上」。
    fn recently_missing_model(&self, key: OwnedModelKey) -> bool {
        let Ok(cache) = self.model_missing.read() else {
            return false;
        };
        cache
            .get(&key)
            .is_some_and(|at| at.elapsed() < MODEL_MISSING_TTL)
    }

    /// 记下「这个 id 不在榜上」，只留 [`MODEL_MISSING_TTL`]。
    ///
    /// 顺手把过期的条目清掉：正常浏览时这个表最多几十条，不清也不会长，
    /// 但长时间挂着应用又反复点开不同模型时会慢慢攒。
    fn remember_missing_model(&self, key: OwnedModelKey) {
        let Ok(mut cache) = self.model_missing.write() else {
            return;
        };
        cache.retain(|_, at| at.elapsed() < MODEL_MISSING_TTL);
        cache.insert(key, Instant::now());
    }

    pub fn client_id(&self) -> String {
        self.remote.client_id()
    }

    /// 凭据状态（跨重启）。界面据此决定要不要把玩家当新人讲一遍。
    pub fn credential_state(&self) -> VrccCredentialState {
        self.credential
            .read()
            .map(|state| *state)
            .unwrap_or_default()
    }

    /// 更新凭据状态并落盘。
    ///
    /// 落盘失败只记日志、不回滚：内存里已经是新状态，界面按它显示是对的；
    /// 最坏情况是下次启动读到旧结局，不会因此把一个坏令牌显示成「已连接」。
    fn set_credential_state(&self, state: VrccCredentialState) {
        if let Ok(mut guard) = self.credential.write() {
            *guard = state;
        }
        if let Err(error) = self.store.save_state(state) {
            tracing::warn!(%error, ?state, "vrcc: failed to record the credential outcome");
        }
    }

    /// 首连引导讲到哪一步。
    ///
    /// 纯内存读取；初值来自构造时那一次仓库读取（[`Self::new`]）。
    pub fn guidance_state(&self) -> VrccGuidanceState {
        self.guidance.read().map(|state| *state).unwrap_or_default()
    }

    /// 记下玩家对引导的表态。
    ///
    /// 落盘失败只记日志：内存里的结论仍然成立，最坏情况是下次启动再问一遍，
    /// 比整块功能因为一次写失败而不可用要好。
    pub fn set_guidance_state(&self, state: VrccGuidanceState) -> Result<()> {
        {
            let Ok(mut guard) = self.guidance.write() else {
                return Err(vrcx_0_application_core::Error::Custom(
                    "vrcc: guidance state lock poisoned".into(),
                ));
            };
            *guard = state;
        }
        self.store.save_guidance(state)
    }

    pub fn snapshot(&self) -> VrccConnectionSnapshot {
        let connection = self.remote.connection();
        let client_is_dev = is_dev_client(&connection.client);
        VrccConnectionSnapshot {
            connected: connection.connected,
            expired: connection.expired,
            credential: self.credential_state(),
            star: connection.star,
            vrc_name: connection.vrc_name,
            scope: connection.scope,
            client: connection.client,
            client_is_dev,
            last_poll_at: connection.last_poll_at,
            next_poll_at: connection
                .last_poll_at
                .map(|last| last + connection.poll_every_secs * 1000),
        }
    }

    /// 服务器建议的提醒轮询间隔（秒，不低于 25），后台轮询用。
    pub fn poll_every_secs(&self) -> i64 {
        self.remote.connection().poll_every_secs
    }

    /// `me/notifications` 的游标；连上以后还没问过时为 `None`。
    pub fn notifications_cursor(&self) -> Option<i64> {
        self.remote.connection().notifications_cursor
    }

    /// 用量快照。
    ///
    /// 口径：说「今日已用」多少，**全部来自服务端的 `RateLimit-*`**。还没拿到
    /// 过那组头时字段是 `None`，界面显示「未知」——不要拿本地日期推算，
    /// 官方按北京时间零点重置，本地算出来的是错的。
    pub fn usage(&self) -> VrccUsageSnapshot {
        let usage = self.remote.local_usage();
        VrccUsageSnapshot {
            personal_used_today: usage.reads.map(|(used, _)| used),
            personal_daily_limit: usage.reads.map(|(_, limit)| limit),
            unread_used_today: usage.unread.map(|(used, _)| used),
            unread_daily_limit: usage.unread.map(|(_, limit)| limit),
            window_used: usage.window_used as i64,
            window_limit: self.remote.window_limit(),
            backoff_remaining_ms: usage.backoff_remaining_ms,
            reset_secs: usage.reset_secs,
        }
    }

    // -- 开放数据 ----------------------------------------------------------

    /// 地图榜。不需要连接，也不计个人额度。
    ///
    /// `sort` 是本周榜或新上榜、而回答里没有 `sort` 时，说明服务器还不认
    /// 这个参数、回的是总榜：照样返回，由界面判断「还不支持」。
    ///
    /// **限流窗口内点刷新不报错**，但接住它的不在这里：客户端在间隔内直接复用
    /// 上一份回答，请求根本发不出去，也就走不到这一层（见 `vrcc-client` 的
    /// `lookup_reply`）。应用层原来自己存了一份榜单来兜底，客户端那层做完之后
    /// 它就再也走不到了。
    pub async fn boards(
        &self,
        limit: u32,
        sort: VrccBoardSort,
    ) -> std::result::Result<VrccBoard, VrccError> {
        let board = self.remote.boards(limit, sort).await?;
        // 顺手填充腕表的排名缓存：这次请求本来就要发，不额外花额度。
        // 只用总榜：名次指的是总榜名次，本周榜和新上榜只列一部分地图。
        if sort == VrccBoardSort::Top || board.sort.is_none() {
            self.note_boards(&board);
        }
        Ok(board)
    }

    /// 服务状态。
    pub async fn status(&self) -> std::result::Result<VrccStatus, VrccError> {
        self.remote.status().await
    }

    /// 模型榜（`GET /boards/models`，1.17.0 起）。不需要连接。
    ///
    /// **旧服务器没有这个接口**（`404 not_found`）时返回 `None`，和
    /// [`Self::announcements`] / [`Self::stickers`] 一样：这是「这台服务器还
    /// 没有模型榜」，不是故障，界面据此显示成空的即可。
    ///
    /// **不碰腕表缓存**：腕表显示的是「当前所在地图的名次」，和 BOOTH 模型
    /// 无关，混进来只会把地图名次覆盖掉。
    ///
    /// 限流窗口内点刷新不报错，接住它的地方和 [`Self::boards`] 一样在客户端：
    /// 间隔内的同一个读直接复用上一份回答，请求根本发不出去。
    pub async fn models_board(
        &self,
        limit: u32,
        sort: VrccBoardSort,
    ) -> std::result::Result<Option<VrccModelBoard>, VrccError> {
        match self.remote.models_board(limit, sort).await {
            Ok(board) => Ok(Some(board)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// 单个模型（`GET /models/{id}`）。不需要连接。
    ///
    /// 不在榜上时 `None`——那是正常状态，不是故障。
    ///
    /// **带本地缓存**：官方说信息最多 5 分钟更新一次，而玩家常常点开、关掉、
    /// 再点开同一条。每次都发请求既慢又白占额度。缓存按 BOOTH 编号存，
    /// 过期就重新问。
    ///
    /// `None`（不在榜上）只记 [`MODEL_MISSING_TTL`] 那么短：下架和重新上架都是
    /// 会发生的事，把「没有」记 5 分钟会让玩家以为它消失了；但完全不记又会
    /// 让「关掉再点开同一个已下架模型」撞上本地限流的 30 秒间隔，表现成
    /// 一个假的网络故障。
    ///
    /// ## 没有做 single-flight
    ///
    /// 同一个 id 的两个并发调用会各自 miss、各发一次请求。这里**有意不做**：
    ///
    /// - 唯一的调用入口是玩家点开一张卡，前端同一时刻只允许一个弹窗，实际上
    ///   并发窗口只有「连点两下」那么多；
    /// - 要做就得在 `model()` 里持一把跨 `await` 的锁或记「正在请求」，而
    ///   这个类型的其余部分刻意避免持锁跨 await（见 `cached_model_detail`）。
    ///   为一条几乎不存在的路径引入这种结构，是拿可见的正确性去换不可见的
    ///   一次请求。
    ///
    /// 真出现并发压力时，正确的做法是在调用侧合并（比如前端按 id 去重），
    /// 而不是在这一层加锁。
    pub async fn model(&self, id: i64) -> std::result::Result<Option<VrccModelDetail>, VrccError> {
        if id <= 0 {
            return Ok(None);
        }

        // 缓存按「登录态 + BOOTH 编号」分区。取分区放在这里，后面每个碰缓存的
        // 地方都用同一个 key——谁想漏掉账号维度都过不了类型检查。
        let key = OwnedModelKey::new(self.cache_scope(), id);

        // 先看缓存。读锁用完立刻释放，不要在持锁期间 await。
        if let Some(cached) = self.cached_model_detail(key) {
            return Ok(Some(cached));
        }

        // 刚确认过「不在榜上」就不要再问：同一个 id 的本地限流间隔是 30 秒，
        // 这段时间里再问会被 `MAX_ACQUIRE_WAIT` 直接拒掉，病根却只是玩家
        // 关掉又点开了同一个已下架的模型——那不该表现为「评论加载失败」。
        if self.recently_missing_model(key) {
            return Ok(None);
        }

        match self.remote.model(id).await {
            Ok(Some(detail)) => {
                self.remember_model_detail(key, &detail);
                Ok(Some(detail))
            }
            // 不在榜上：不长期缓存，只记 [`MODEL_MISSING_TTL`]。
            Ok(None) => {
                self.remember_missing_model(key);
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// 批量查地图（`GET /maps?ids=`）。
    ///
    /// 只返回**在榜上**的那些，所以结果条数可能少于传入的数量；调用方要按
    /// `world` 去配，不能按下标对应。
    ///
    /// 这里不接「本地限流就把旧结果还给他」那套：批量查询是调用方自己
    /// 攒出来的一次性请求，没有可回退的缓存，而拿别的批次的答案冒充这一批
    /// 会直接给出错的地图数据——那比报一句「太频繁」危险得多。
    pub async fn maps(&self, worlds: &[String]) -> std::result::Result<VrccBoard, VrccError> {
        self.remote.maps(worlds).await
    }

    /// 取站点上的静态资源（通知图标之类）。
    ///
    /// 不需要连接：那是站点资源，和玩家令牌无关。仍然走端口，于是复用
    /// 同一份代理配置与「只和 vrcc.cc 说话」的硬拦。
    pub async fn fetch_site_asset(&self, path: &str) -> std::result::Result<Vec<u8>, VrccError> {
        self.remote.fetch_site_asset(path).await
    }

    /// 公告和停站（1.12.0 起）。不需要连接。
    ///
    /// 旧服务器没有这个接口（`404 not_found`）时返回 `None`，界面就不显示。
    pub async fn announcements(&self) -> std::result::Result<Option<VrccAnnouncements>, VrccError> {
        match self.remote.announcements().await {
            Ok(announcements) => Ok(Some(announcements)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    // -- 贴纸素材 ----------------------------------------------------------

    /// 所有贴纸和材质名。
    ///
    /// 旧服务器没有这个接口（`404 not_found`），那时返回 `None`——贴纸是
    /// 装饰，缺了它不该让整个「我的」页签报错。
    pub async fn stickers(&self) -> std::result::Result<Option<VrccStickers>, VrccError> {
        match self.remote.stickers().await {
            Ok(stickers) => Ok(Some(stickers)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    // -- 设备授权 ----------------------------------------------------------

    /// 第一步：申请连接码。
    ///
    /// `scope` 传 `None` 时按当前标识选（见 [`device_auth_scope`]）。
    /// 不能交给服务器默认：自助登记的 `app_…` 不写只有 `profile`，
    /// 提醒、所在地图、「连上」全都会 `403 scope`。
    pub async fn begin_device_auth(
        &self,
        scope: Option<&str>,
    ) -> std::result::Result<VrccDeviceAuthPrompt, VrccError> {
        let client_id = self.remote.client_id();
        let scope = scope
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .unwrap_or_else(|| device_auth_scope(&client_id));
        let code = self.remote.device_code(Some(scope)).await?;
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
        match self.remote.device_token(&prompt.device_code).await {
            Ok(token) => {
                let credential = StoredCredential {
                    token: token.access_token.clone(),
                    star: token.me.as_ref().and_then(|me| me.star),
                    vrc_name: token.me.as_ref().and_then(|me| me.vrc.clone()),
                    scope: token.scope.clone(),
                    // 记下是哪个 `X-Client` 换来的：将来换标识时才能认出这个
                    // 令牌已经不可用，而不是继续拿它请求、收一堆 403。
                    client_id: Some(self.remote.client_id()),
                };
                self.store.save(&credential).map_err(|error| {
                    // 写本地令牌失败与网络无关：是本机存储出了问题。
                    // 归 `Input` 而不是 `Transport`，否则界面会引导玩家去查网络。
                    VrccError::new(
                        ErrorLayer::Input,
                        ErrorDetail::AbsolutePath(format!("保存令牌失败：{error}")),
                    )
                })?;
                self.remote.connect(
                    credential.token,
                    credential.star,
                    credential.vrc_name,
                    credential.scope,
                );
                // 新账号（或重新授权）不该先看到上一个账号的未读数和名次。
                self.forget_unread();
                self.set_credential_state(VrccCredentialState::Active);
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
    /// **可重试的失败不清本地**。这条很重要：如果吊销请求根本没发出去
    /// （网络不通、或恰好在限流退避里），令牌却已经删了，那服务器上那份授权
    /// 就**永远**吊销不了了——玩家在网页「第三方客户端」里会一直看到一个
    /// 早就该消失的客户端。所以这种情况保留令牌、把错误交出去，让玩家再点一次。
    ///
    /// 其余失败照旧清本地：玩家点了断开，界面上就该断开。`needs_reauth`
    /// （`gone` / `401` / `client_mismatch`）说明服务器那边早已不认这个令牌，
    /// 等于本来就断着。
    pub async fn disconnect(&self) -> std::result::Result<(), VrccError> {
        let revoke = self.remote.revoke().await;
        // 服务器没收到吊销请求：什么都别动，等玩家重试。
        if let Err(error) = &revoke {
            if retriable_revoke_failure(error) {
                tracing::info!(
                    %error,
                    "vrcc: revoke did not reach the server; keeping the token so it can be retried"
                );
                return Err(error.clone());
            }
        }
        self.store.clear().map_err(|error| {
            VrccError::new(
                ErrorLayer::Input,
                ErrorDetail::AbsolutePath(format!("清除令牌失败：{error}")),
            )
        })?;
        self.remote.disconnect();
        self.forget_unread();
        // 玩家自己断的：重启之后仍然算这一档，不要退回「从没连过」。
        self.set_credential_state(VrccCredentialState::Disconnected);
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
        let generation = self.remote.generation();
        let result = self
            .remote
            .recent_notifications(mark)
            .await
            .map_err(|error| self.settle(generation, error))?;
        // 未读数按官方语义算：id 大于 read 的算未读。`read` 缺省时（账号上
        // 还没记过已读），把返回的条目都当作未读——这是唯一诚实的解读。
        //
        // 刚标记过 `mark=1` 时服务器会把 read 推到最新，未读随之变 0，这是对的：
        // 玩家确实看到了。
        let unread = match result.read {
            Some(read) => result.items.iter().filter(|item| item.id > read).count() as u32,
            None => result.items.len() as u32,
        };
        self.note_recent(generation, unread);
        self.remote
            .note_poll(generation, self.remote.now_millis(), None, None);
        Ok(result)
    }

    /// 当前所在地图在地图榜上的状态与我自己的评分。
    ///
    /// 需要 `maps`（普通权限）。
    ///
    /// ## `off` 不再折成 `None`
    ///
    /// 官方的硬性要求是 `off` 为 `true` 时客户端不要显示这张图的卡片。早先的
    /// 做法是直接返回 `None`——调用方拿不到对象就不可能误显示，看起来更可靠。
    ///
    /// 但那样会把**三种完全不同的情况压成同一个 `None`**：被合规隐藏、
    /// 玩家没连接、以及「查不到」。界面上只能一律显示「不在榜上」，其中
    /// 「被隐藏」那种说法虽然不算错，却让玩家不知道真正的原因。
    ///
    /// 现在把 `off` 原样交给界面，由它**显示一句「为遵守相关条款不得展示」**。
    /// 这仍然满足「不显示这张图的卡片」——不给任何数据，只说明原因。代价是
    /// **每个调用点都必须自己判断 `off`**，所以两处调用点（所在地图卡、世界
    /// 详情页）都做了显式判断，不要依赖「反正拿不到」。
    pub async fn my_map(&self, world: &str) -> std::result::Result<Option<VrccMyMap>, VrccError> {
        // 只认 wrld_…：带房间实例号的 location 会让接口查错图。
        let world = vrcx_0_core::location::world_id_from_location(world);
        if world.is_empty() {
            return Ok(None);
        }
        self.ensure_connected()?;
        let generation = self.remote.generation();
        let map = self
            .remote
            .my_map(&world)
            .await
            .map_err(|error| self.settle(generation, error))?;
        if !map.is_visible() {
            // 只是记一条日志，仍然把对象交出去——界面要用 `off` 说明原因。
            tracing::debug!(
                world = %world,
                "vrcc: this map is not shown to this player, the UI must hide its data"
            );
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
        let generation = self.remote.generation();
        let skip = self
            .unread
            .read()
            .ok()
            .and_then(|cache| cache.skip_until)
            .is_some_and(|until| Instant::now() < until);
        if skip {
            return Ok(None);
        }
        match self.remote.unread().await {
            Ok(counts) => {
                let changed = self.note_unread(generation, &counts);
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
            Err(error) => Err(self.settle(generation, error)),
        }
    }

    /// 通知中心（`me/notes`，1.13.0 起，第二份读）。读了不标已读。
    ///
    /// 旧服务器没有这个接口时返回 `None`，界面就不显示这一块。
    pub async fn notes(&self) -> std::result::Result<Option<VrccNotes>, VrccError> {
        self.ensure_connected()?;
        let generation = self.remote.generation();
        match self.remote.notes().await {
            Ok(notes) => Ok(Some(notes)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(self.settle(generation, error)),
        }
    }

    /// 增量轮询。游标与建议间隔都自动维护。
    ///
    /// 这是官方给桌面客户端设计的轮询方式：两次之间至少 25 秒，**没有每天
    /// 上限**，每次只给 `since` 之后的新提醒。后台轮询就用它来弹系统通知。
    ///
    /// **不要给前端补一条调用它的 IPC 命令**：`since` 游标由后台轮询独占，
    /// 前端再调一次会推进同一个游标，那一批提醒就不会再被后台拿到，系统
    /// 通知会静默漏掉。前端的提醒列表走 `recent_notifications` + 后端推来的
    /// `vrccNoticesChanged` 事件，不走这里。
    ///
    /// 响应里的 `dot` 是「星轨相交尚未查看的新人数」，**不是未读提醒数**，
    /// 所以这里不拿它更新腕表的 `unread`——两者混用会显示错误的数字。
    pub async fn poll_notifications(
        &self,
    ) -> std::result::Result<vrcx_0_contracts::vrcc::VrccNotifications, VrccError> {
        self.ensure_connected()?;
        let generation = self.remote.generation();
        let cursor = self.remote.connection().notifications_cursor;
        let polled = self
            .remote
            .notifications(cursor)
            .await
            .map_err(|error| self.settle(generation, error))?;
        // 换号或重新授权之后才回来的结果不写：新会话的游标要从头问。
        self.remote.note_poll(
            generation,
            self.remote.now_millis(),
            polled.last,
            polled.every,
        );
        // 有新提醒到达时，未读数会变；下次打开页面拉 recent 时会修正。
        // 这里若把 items 长度加上去，会和 `read` 的语义打架。
        Ok(polled)
    }

    /// 公开地图页（1.13.0 起）。**不需要令牌。**
    ///
    /// 不在榜上或没过审时是 `404 not_found`——那是这张图还没上榜，返回
    /// `None` 而不是错误。把它报成故障会让一张新图看起来像服务坏了。
    pub async fn map_page(
        &self,
        world: &str,
    ) -> std::result::Result<Option<VrccMapPage>, VrccError> {
        let generation = self.remote.generation();
        let world = vrcx_0_core::location::world_id_from_location(world);
        if world.is_empty() {
            return Ok(None);
        }
        match self.remote.map_page(&world).await {
            Ok(page) => Ok(Some(page)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(self.settle(generation, error)),
        }
    }

    /// 带令牌的地图页：完整短评列表 + 我的评分（需要 `maps` 普通权限）。
    ///
    /// 与公开版 [`Self::map_page`] 的区别是**能看到别人的评价**。公开版按
    /// 官方说明「短评和介绍不给」，只有一条热评。
    ///
    /// 不在榜上时服务器回 `404 not_found`——那是正常状态（这张图还没上榜），
    /// 转成 `None`，让界面说「不在榜上」而不是报错。
    ///
    /// 未连接时先拒绝，不把一个必然 `TokenMissing` 的请求发出去。
    pub async fn my_map_page(
        &self,
        world: &str,
        sort: Option<&str>,
    ) -> std::result::Result<Option<VrccMyMapPage>, VrccError> {
        let world = vrcx_0_core::location::world_id_from_location(world);
        if world.is_empty() {
            return Ok(None);
        }
        self.ensure_connected()?;
        let generation = self.remote.generation();
        match self.remote.my_map_page(&world, sort).await {
            Ok(page) => Ok(Some(page)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(self.settle(generation, error)),
        }
    }

    /// 给地图打分 / 写短评（需要 `maps.rate` 敏感权限）。
    ///
    /// ## 这是写操作，只允许由玩家每次主动点击触发
    ///
    /// 官方《使用要求》把打分列为敏感写操作，明确禁止自动与批量。所以这里
    /// **不做任何重试或自动续发**：失败就把错误原样交给界面，由玩家决定要不要
    /// 再点一次。这与读接口的策略相反（那些失败会退回缓存），因为「重试一次
    /// 写」和「重试一次读」的后果完全不同。
    ///
    /// ## 短评没通过审核要原样报出来
    ///
    /// `yq_words` / `yq_empty` 不是网络故障，是内容被拒。界面需要拿 `why`
    /// 告诉玩家具体原因（例如「请不要在短评里留联系方式」）。所以这里不做
    /// 任何降级，让错误一路带着 `why` 上到界面。
    pub async fn rate_map(
        &self,
        world: &str,
        score: i64,
        text: &VrccRatingText,
    ) -> std::result::Result<VrccMyMap, VrccError> {
        let world = vrcx_0_core::location::world_id_from_location(world);
        if world.is_empty() {
            // 读接口遇到空 ID 会安静地返回 `None`，写接口不能：一个写操作
            // 「什么都没发生」是最坏的结局——玩家会以为提交成功了。
            return Err(VrccError::new(
                vrcx_0_contracts::vrcc::ErrorLayer::Input,
                vrcx_0_contracts::vrcc::ErrorDetail::MissingArgument("world"),
            ));
        }
        self.ensure_connected()?;
        let generation = self.remote.generation();
        self.remote
            .rate_map(&world, score, text)
            .await
            .map_err(|error| self.settle(generation, error))
    }

    /// 删掉自己给这张图的评分（需要 `maps.rate`）。
    ///
    /// 同样是写操作，不做重试。`404` 表示本来就没打过分——对它而言目标已经
    /// 达成，所以按成功处理，免得玩家看到一句莫名其妙的错误。
    pub async fn delete_map_rating(&self, world: &str) -> std::result::Result<(), VrccError> {
        let world = vrcx_0_core::location::world_id_from_location(world);
        if world.is_empty() {
            return Err(VrccError::new(
                vrcx_0_contracts::vrcc::ErrorLayer::Input,
                vrcx_0_contracts::vrcc::ErrorDetail::MissingArgument("world"),
            ));
        }
        self.ensure_connected()?;
        let generation = self.remote.generation();
        match self.remote.delete_map_rating(&world).await {
            Ok(()) => Ok(()),
            Err(error) if error.is_not_found() => Ok(()),
            Err(error) => Err(self.settle(generation, error)),
        }
    }

    /// 给短评投票（需要 `maps.write` 敏感权限）。
    ///
    /// ## 不能给自己的内容投票
    ///
    /// 这条规则在服务端，但**界面和这里都要提前拦**：让玩家点下去再收到一个
    /// 403 是最差的体验。所以这里除了转发，还在本地挡掉「投自己的票」——
    /// 我们拿不到「这条是不是我的」以外的信息，判据由调用方传进来。
    ///
    /// 实际上调用方（界面）已经按 `VrccMapRate::mine` 禁用了按钮，这里不再
    /// 重复判断：多一处判断就多一处会和服务端分叉的地方。
    ///
    /// 写操作：不重试（重试等于投两次）。
    pub async fn vote_map_content(
        &self,
        kind: VrccVoteKind,
        id: i64,
        vote: VrccVote,
    ) -> std::result::Result<VrccVoteResult, VrccError> {
        if id <= 0 {
            return Err(VrccError::new(
                vrcx_0_contracts::vrcc::ErrorLayer::Input,
                vrcx_0_contracts::vrcc::ErrorDetail::MissingArgument("id"),
            ));
        }
        self.ensure_connected()?;
        let generation = self.remote.generation();
        self.remote
            .vote_map_content(kind, id, vote)
            .await
            .map_err(|error| self.settle(generation, error))
    }

    /// 玩家视角地图榜（需要 `maps` 普通权限）。
    ///
    /// `q` 非空时走搜索。搜索时服务器把回答里的 `sort` 改写成 `"find"`，
    /// 所以**不要拿回答去回填界面上的排序状态**——用请求时自己那份。
    pub async fn my_board(
        &self,
        sort: VrccMyBoardSort,
        q: Option<&str>,
    ) -> std::result::Result<VrccMyBoard, VrccError> {
        self.ensure_connected()?;
        let generation = self.remote.generation();
        self.remote
            .my_board(sort, q)
            .await
            .map_err(|error| self.settle(generation, error))
    }

    /// 我打过分的地图（需要 `maps` 普通权限）。最近的在前。
    ///
    /// 未连接时先拒绝，不把一个必然 `TokenMissing` 的请求发出去——那会被
    /// 判成「要重新授权」，在还没连过的玩家那里就是一句莫名其妙的红字。
    pub async fn my_maps(&self) -> std::result::Result<VrccRatedMaps, VrccError> {
        self.ensure_connected()?;
        let generation = self.remote.generation();
        self.remote
            .my_maps()
            .await
            .map_err(|error| self.settle(generation, error))
    }

    /// 今天这一轮 + 轮次进度（需要 `today` 敏感权限）。
    pub async fn today(&self) -> std::result::Result<VrccToday, VrccError> {
        self.ensure_connected()?;
        let generation = self.remote.generation();
        self.remote
            .today()
            .await
            .map_err(|error| self.settle(generation, error))
    }

    /// 「互相连上」的星友列表（需要 `mutuals` 敏感权限）。
    ///
    /// 条目带 `vrcUser` 时，前端可以打开 VRCX-0 自己的玩家详情页。按官方合规
    /// 要求，这里**不提供**代玩家发送好友请求的通道——加好友必须由玩家在
    /// 原生界面里自己点。
    pub async fn mutuals(&self) -> std::result::Result<VrccMutuals, VrccError> {
        self.ensure_connected()?;
        let generation = self.remote.generation();
        self.remote
            .mutuals()
            .await
            .map_err(|error| self.settle(generation, error))
    }

    /// 一个社团的公开页（`GET /clubs/{code}`，1.18.0 起）。**不需要连接。**
    ///
    /// # 404 是一条正常结论，不是错误
    ///
    /// 没入驻、审核中、下架、只给成员看、不展示的，官方一律回同样的
    /// `404 not_found`——**看不出是哪一种**。所以这里转成 `Ok(None)`，
    /// 界面上只能说「这个群组没有星遇同频社团」，不能猜原因，更不能报故障。
    ///
    /// 这和 [`Self::map_page`] 是同一个套路：把「没有这一份数据」和「请求
    /// 失败」分开，否则一次 404 会让群组详情整块变红。
    pub async fn club(
        &self,
        code: &str,
    ) -> std::result::Result<Option<VrccClubResponse>, VrccError> {
        let generation = self.remote.generation();
        match self.remote.club(code).await {
            Ok(club) => Ok(Some(club)),
            Err(error) if error.is_absent() => Ok(None),
            Err(error) => Err(self.settle(generation, error)),
        }
    }

    /// 所有公开社团（`GET /clubs`，1.19.2 起）。**不需要连接，也不吃个人额度。**
    ///
    /// 顺序由服务器定死（点赞多 → 群组人数多 → 短码），接口没有 `sort`：这一层
    /// 不做任何重排，`offset` 原样往下传。
    ///
    /// 社团功能关着时服务器回空列表（不是错误），所以这里没有「不存在」这一档。
    pub async fn clubs(
        &self,
        limit: u32,
        offset: u32,
    ) -> std::result::Result<VrccClubs, VrccError> {
        let generation = self.remote.generation();
        match self.remote.clubs(limit, offset).await {
            Ok(clubs) => Ok(clubs),
            Err(error) => Err(self.settle(generation, error)),
        }
    }

    /// 玩家能看到的社团（`GET /me/clubs`）。需要 `clubs` 敏感权限。
    ///
    /// **`404 club_off` 也转成 `Ok(None)`**：16–17 岁星池和「社团功能关着」
    /// 都走这一条，两种对玩家都是「没有这一块」，不该弹错误。
    ///
    /// 缺权限（`403 scope`）**不吞**：那要引导玩家重新授权，是个需要说出来
    /// 的状态，和「这个星池没有社团」是两回事。
    pub async fn my_clubs(&self) -> std::result::Result<Option<VrccMyClubs>, VrccError> {
        self.ensure_connected()?;
        let generation = self.remote.generation();
        match self.remote.my_clubs().await {
            Ok(clubs) => Ok(Some(clubs)),
            Err(error) if error.is_absent() => Ok(None),
            Err(error) => Err(self.settle(generation, error)),
        }
    }

    /// 一个社团的社团页（`GET /me/clubs/{code}`）。需要 `clubs`。
    ///
    /// `404 club_gone` / `club_off` 同样转成 `Ok(None)`——玩家看不到这个社团
    /// 是**正常结论**，界面上那块不显示就是了。
    pub async fn my_club(
        &self,
        code: &str,
    ) -> std::result::Result<Option<VrccMyClubResponse>, VrccError> {
        self.ensure_connected()?;
        let generation = self.remote.generation();
        match self.remote.my_club(code).await {
            Ok(club) => Ok(Some(club)),
            Err(error) if error.is_absent() => Ok(None),
            Err(error) => Err(self.settle(generation, error)),
        }
    }

    /// 星轨相交：在同房间偶遇过的玩家列表（需要 `crossings` 敏感权限）。
    ///
    /// `mark` 只在玩家真正打开「星轨相交」页时为 true —— 它会把已读同步到
    /// 账号，网页和手机上的红点跟着消。后台只为算未读数时**必须**传 false，
    /// 否则等于替玩家把提醒标成看过了。
    ///
    /// 与 `mutuals` 的区别：互相连上是**双向已连线**的星友，星轨相交只是
    /// **同房间擦肩**，大多数还没连线。所以界面要按 `linked` / `mutual`
    /// 两个状态区分展示与可执行动作。
    ///
    /// 返回里的 `why` 必须由调用方处理：`hide` 时**不应显示**该模块
    /// （16–17 岁星池，或玩家在隐私设置里关了星轨相交）。
    pub async fn crossings(&self, mark: bool) -> std::result::Result<VrccCrossings, VrccError> {
        self.ensure_connected()?;
        let generation = self.remote.generation();
        let mut crossings = self
            .remote
            .crossings(mark)
            .await
            .map_err(|error| self.settle(generation, error))?;
        // `why == "hide"` 的判定只在这里做一次，消费方只看 `hidden`。
        crossings.settle_visibility();
        Ok(crossings)
    }

    /// 只看「星轨相交对这个玩家可见吗」，不标已读。
    ///
    /// 官方要求 `hide` 时**客户端不显示此模块**，而 `hide` 只有问过才知道。
    /// 界面在打开星遇同频页面时先调这一条，拿到 `hidden` 再决定入口是否
    /// 存在——不探测就只能「先显示、点进去再消失」，那等于已经确认了功能
    /// 存在，正是 `hide` 要避免的。
    ///
    /// 走 [`VrccRemote::crossings_probe`] 的独立限流槽：探测不该占住玩家随后
    /// 点开页签那一次（那一次要带 `mark=1` 清红点）。
    pub async fn crossings_visibility(&self) -> std::result::Result<VrccCrossings, VrccError> {
        self.ensure_connected()?;
        let generation = self.remote.generation();
        let mut crossings = self
            .remote
            .crossings_probe()
            .await
            .map_err(|error| self.settle(generation, error))?;
        crossings.settle_visibility();
        Ok(crossings)
    }

    /// 星轨相交的完整星卡（`/me/crossings/cards`，1.13.0 起）。
    ///
    /// 比列表重，且算**第二份读**的额度——是和 `/me/today`、`/me/notes`、
    /// 地图榜**共享**同一份每天 1000 次，不是独立额度。所以只应在玩家打开
    /// 星轨相交页时按需拉取，不要进后台轮询。
    ///
    /// 界面其实只用到 `left` / `max` 两个整数（显示「今天还能连几颗」），
    /// 但服务器只给整份 `cards[]`，没有更轻的端点。这个交换是有意的：可见性
    /// 和额度都在这一次响应里，拆成两个请求反而更贵。
    ///
    /// 旧服务器没有这个接口，回 `404 not_found` 时返回 `None`：它只影响
    /// 「今天还能连几颗」那一小块，不该让整个列表跟着失败。
    pub async fn crossing_cards(
        &self,
        mark: bool,
    ) -> std::result::Result<Option<VrccCrossingCards>, VrccError> {
        self.ensure_connected()?;
        let generation = self.remote.generation();
        match self.remote.crossing_cards(mark).await {
            Ok(cards) => Ok(Some(cards)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(self.settle(generation, error)),
        }
    }

    fn ensure_connected(&self) -> std::result::Result<(), VrccError> {
        if self.remote.connection().connected {
            Ok(())
        } else {
            Err(VrccError::new(
                ErrorLayer::Session,
                ErrorDetail::TokenMissing,
            ))
        }
    }
}

#[cfg(test)]
mod tests;
