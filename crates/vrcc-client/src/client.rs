// SPDX-License-Identifier: MPL-2.0

//! 星遇同频（vrcc.cc）API 客户端。
//!
//! 这是本集成**唯一的网络出口**。合规约束照官方《使用要求》与参考实现
//! （`xingshi-app` 的 `VrccClient`）逐条落地：
//!
//! - 每个请求都带 `X-Client`，标识客户端身份；
//! - 只允许 `/api/v1/` 下的**相对路径**，绝对地址在这里直接拒绝，
//!   保证令牌不可能被发到第三方地址；
//! - 写操作（打分、连线、私聊）只由玩家点击触发，本层不做队列、不重试、不批处理；
//! - 令牌只发给 `vrcc.cc`。
//!
//! 限流照开放接口 1.14.0：
//! - 所有 `429` 都带 `Retry-After`（1.9.0 起），按它等；旧服务器没给时退回指数退避；
//! - 带每天次数的接口回答 `RateLimit-*`（1.11.0 起），用它代替本地猜的用量；
//! - 某一份额度当天用完，只停那一份，不让别的接口跟着退避。

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use reqwest::header::HeaderMap;
use serde::de::DeserializeOwned;
use serde_json::json;
use vrcx_0_vrcc_contracts::resolve_client_id;
use vrcx_0_vrcc_contracts::{
    ErrorDetail, ErrorLayer, HttpDetail, LocalThrottle, TransportKind, VrccAnnouncements,
    VrccBoard, VrccBoardSort, VrccClubResponse, VrccClubs, VrccCrossingCards, VrccCrossings,
    VrccDeviceCode, VrccErrorBody, VrccMapDetail, VrccMapPage, VrccModelBoard, VrccModelDetail,
    VrccMutuals, VrccMyBoard, VrccMyBoardSort, VrccMyClubResponse, VrccMyClubs, VrccMyMap,
    VrccMyMapPage, VrccNotes, VrccNotifications, VrccRatedMaps, VrccRatingInput, VrccRatingText,
    VrccRecentNotifications, VrccStatus, VrccStickers, VrccToday, VrccTokenResponse, VrccUnread,
    VrccVote, VrccVoteKind, VrccVoteResult, VRCC_API_BASE, VRCC_SITE_ORIGIN,
};

use super::throttle::{backoff_for, RateLimiter, MAX_BACKOFF, NOTIFICATIONS_MIN_GAP};

/// 让 `socks5://` 走代理端 DNS（`socks5h://`），其余代理协议原样返回。
///
/// **这一份是从仓库的 `vrcx-0-core::proxy` 内联过来的 9 行。** 那个 crate 是
/// GPL-3.0-only，为本文件的一条 `use` 拖进整棵 GPL 依赖树，会让这个 MPL 包
/// 对外部消费者彻底失去意义。语义与原实现逐字一致，原位置的单测仍在。
fn rewrite_remote_dns(proxy_url: &str) -> Cow<'_, str> {
    match proxy_url.strip_prefix("socks5://") {
        Some(authority) => Cow::Owned(format!("socks5h://{authority}")),
        None => Cow::Borrowed(proxy_url),
    }
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(25);

/// 一次请求最多愿意为限流等待多久。
///
/// 超过就返回 `ThrottledLocally { reason: Gap }`，交给调用方决定（通常是拿
/// 缓存顶上），而不是让一次用户操作僵在那里。典型场景：`boards` 的 `min_gap` 是 60 秒，
/// 用户点一下刷新不能干等一分钟——官方自己说开放数据最多 5 分钟才更新一次，
/// 那份旧数据完全够用。
///
/// 取 10 秒是因为其余接口的 `min_gap` 都在 5–30 秒之间，正常页面加载
/// （间隔远大于 30 秒）不会碰到；只有连点两次之类的场景才会命中。
const MAX_ACQUIRE_WAIT: Duration = Duration::from_secs(10);

/// 过了有效期、但带着 `ETag` / `Last-Modified` 的回答再留多久，供下一次发条件
/// 请求（`If-None-Match` / `If-Modified-Since`）用，见 [`VrccClient::lookup_reply`]。
const REVALIDATE_WINDOW: Duration = Duration::from_secs(600);

/// 最多留多少份「间隔内复用」的回答，见 [`VrccClient::lookup_reply`]。
///
/// 每份只活一个最小间隔（最长一小时，多数 3–60 秒），正常浏览远到不了这个数；
/// 上限只是防止长时间挂着、反复点开不同世界时慢慢涨。
const REPLY_CACHE_MAX: usize = 256;

/// 客户端错误。定义在契约层（应用层要按它分类，不能反过来依赖网络层），
/// 这里只做转出，保持既有调用点不变。
pub use vrcx_0_vrcc_contracts::VrccError;

/// 官方限制 POST 正文最大 2 KB。
const MAX_BODY_BYTES: usize = vrcx_0_vrcc_contracts::VRCC_MAX_BODY_BYTES;

/// `GET /maps?ids=` 一次最多查几张。超过官方就回 `400 bad_request`。
const MAX_BATCH_WORLDS: usize = 20;

/// `/device/code` 的本地限流键。
///
/// 单独拎成常量是因为 `429` 的文案要按它分流：这个额度的维度是**网络出口
/// 地址**（每天 30 个，同一路由下的所有人共用），和个人授权维度的额度不是
/// 一回事，说错了会让玩家以为是自己账号的问题。
const DEVICE_CODE_KEY: &str = "device_code";

/// 发送前拦下超限的正文。
///
/// 抽成独立函数是为了**能真正测到**：`request()` 要先过 `acquire()` 再
/// `send()`，在单测里跑它要么真发网络、要么卡在限流窗口上。这里是拦截的
/// 全部逻辑，单独测它就等于测到了那条路径。
///
/// 官方限制 POST 正文 2 KB。正文超限属于我们自己构造错了请求，
/// 不该发出去让它变成 400。
/// 把 `reqwest` 的失败归成传输层错误，并判定卡在哪个环节。
///
/// 判据用 `reqwest::Error` 自带的方法（`is_dns` / `is_connect` / `is_timeout`
/// / `is_body`），它们沿 source 链做类型 downcast，**不是**对错误文案做字符串
/// 匹配，所以不受本地化或版本措辞变化影响。
///
/// 判定顺序有讲究：
/// 1. **超时优先**。连接超时的 source 链里也可能带 connect 的错误，
///    但玩家该做的动作是「稍后重试」而不是「检查网络」，说错了会误导。
/// 2. DNS 先于 Connect。本机解析失败时两者都可能为真，而 DNS 更具体。
/// 3. 其余归 `Other`，不去猜。
fn transport_error(error: reqwest::Error) -> VrccError {
    let kind = if error.is_timeout() {
        TransportKind::Timeout
    } else if error.is_dns() {
        TransportKind::Dns
    } else if error.is_connect() {
        TransportKind::Connect
    } else if error.is_body() {
        TransportKind::Body
    } else {
        TransportKind::Other
    };
    VrccError::new(
        ErrorLayer::Transport,
        ErrorDetail::Transport {
            kind,
            message: error.to_string(),
        },
    )
}

/// 发送前拦下超限的正文。
///
/// 抽成独立函数是为了**能真正测到**：`request()` 要先过 `acquire()` 再
/// `send()`，在单测里跑它要么真发网络、要么卡在限流窗口上。这里是拦截的
/// 全部逻辑，单独测它就等于测到了那条路径。
///
/// 官方限制 POST 正文 2 KB。正文超限属于我们自己构造错了请求，
/// 不该发出去让它变成 400。
fn check_body_size(len: usize) -> Result<(), VrccError> {
    if len > MAX_BODY_BYTES {
        return Err(VrccError::new(ErrorLayer::Input, ErrorDetail::BodyTooLarge));
    }
    Ok(())
}

/// 带每天次数的接口各自算在哪一份额度里（官方《频率限制》）。
///
/// **写操作不在这里面**：打分、赞踩、删除各有自己的一份（官方那张表把它们列在
/// 「写，每个授权每天」），传 `bucket: None`。把它们记在读上不只是数字不准——
/// `record_too_many` 会按收到的 429 把**那一整份**标记为用完直到北京时间零点，
/// 于是「今天的地图赞踩用完了」会连带把提醒、连线、所在地图全部停掉。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuotaBucket {
    /// 第一份读：最近的提醒、星轨相交、互相连上、所在的地图、我打过分的地图。
    Reads,
    /// 第二份读：今天的星、**地图榜和地图页**、全部介绍、通知中心等，
    /// 和第一份分开算。
    MoreReads,
    /// `me/unread` 自己的一份。
    Unread,
}

/// 服务器在 `RateLimit-*` 里说的一份额度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerQuota {
    /// `RateLimit-Limit`：每天上限。
    pub limit: i64,
    /// `RateLimit-Remaining`：今天还剩几次（算上这一次之后）。
    pub remaining: i64,
    /// `RateLimit-Reset`：几秒后重置（北京时间零点）；没给时为 0。
    pub reset_secs: u64,
}

impl ServerQuota {
    pub fn used(&self) -> i64 {
        (self.limit - self.remaining).max(0)
    }
}

/// 一次回答里和限流有关的响应头。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RateHeaders {
    /// `Retry-After`（秒）。1.9.0 起所有 `429` 都带。
    pub retry_after_secs: Option<u64>,
    /// `RateLimit-Limit` / `-Remaining` / `-Reset`。1.11.0 起带每天次数的接口给。
    pub quota: Option<ServerQuota>,
}

impl RateHeaders {
    /// 从响应头里读。缺的、写坏的一律当没有，不让一个头把请求弄失败。
    ///
    /// `Retry-After` 按 RFC 9110 也可以是日期，但官方只给秒数，这里只认秒数。
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let number = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<i64>().ok())
        };
        let retry_after_secs = number("retry-after")
            .filter(|secs| *secs >= 0)
            .map(|secs| secs as u64);
        let quota = match (number("ratelimit-limit"), number("ratelimit-remaining")) {
            (Some(limit), Some(remaining)) if limit > 0 => Some(ServerQuota {
                limit,
                remaining: remaining.clamp(0, limit),
                reset_secs: number("ratelimit-reset")
                    .filter(|secs| *secs > 0)
                    .map(|secs| secs as u64)
                    .unwrap_or(0),
            }),
            _ => None,
        };
        Self {
            retry_after_secs,
            quota,
        }
    }
}

/// 服务器最近一次说的某份额度，带作废时间。
#[derive(Clone, Copy, Debug)]
struct SeenQuota {
    limit: i64,
    remaining: i64,
    /// 到这个时刻（服务器的重置时间）这份数就作废了。
    until: Instant,
}

/// 「间隔内复用」那份回答的键。
///
/// 按**完整路径**而不是限流槽分：槽名会去掉 `mark`（见 [`limiter_key`]），而
/// `limit=20` 和 `limit=50` 的回答不能互相顶替。`limit` 因此也留在槽名里：
/// 两边对「是不是同一份资源」必须给同一个答案，否则间隔内会挡下一个手上没有
/// 回答可以顶上的请求。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ReplyKey {
    /// 带令牌的回答按令牌指纹分开（见 [`token_scope`]），开放数据是 `0`。
    /// 换了账号就读不到上一个账号的回答。
    scope: u64,
    key: &'static str,
    path: String,
}

/// 一份可以在间隔内复用的成功回答（原样正文，取用时再解析）。
#[derive(Debug)]
struct CachedReply {
    at: Instant,
    /// 这份回答能用多久，见 [`reply_ttl`]。
    ttl: Duration,
    text: String,
    /// 服务器给的校验值。过期以后靠它发条件请求，服务器回 `304` 就接着用 `text`。
    validators: Validators,
    /// 服务器说过 `no-cache`：可以存，但每次复用前都要先问一次（条件请求）。
    ///
    /// 记在条目上，是因为服务器的 `304` 常常不重复 `Cache-Control`：只看 `304`
    /// 自己的头，这份回答就会重新变成「间隔内新鲜」，下一次不验证就直接给出去。
    must_revalidate: bool,
}

impl CachedReply {
    fn is_live(&self, now: Instant) -> bool {
        !self.must_revalidate && now.saturating_duration_since(self.at) < self.ttl
    }

    /// 过期了还值不值得留着：带校验值的再留一阵，下次可以发条件请求。
    fn worth_keeping(&self, now: Instant) -> bool {
        self.is_live(now)
            || (!self.validators.is_empty()
                && now.saturating_duration_since(self.at) < self.ttl + REVALIDATE_WINDOW)
    }
}

/// 条件请求要用的两个校验值。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Validators {
    etag: Option<String>,
    last_modified: Option<String>,
}

impl Validators {
    fn is_empty(&self) -> bool {
        self.etag.is_none() && self.last_modified.is_none()
    }
}

/// 回答里和缓存有关的头。
///
/// 实测（1.19.2，开放数据，不带令牌）：地图榜、模型榜、社团、公告、状态回答
/// `Cache-Control: public, max-age=60`，没有 `ETag` / `Last-Modified`，条件请求
/// 照样回 `200`。所以眼下起作用的只有开放数据的 `max-age`；校验值和 `304` 这一路
/// 是给服务器以后加上时准备的，不加也不影响。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CacheHeaders {
    /// `max-age` 减去 `Age`：服务器说这份回答还能新鲜多久。没给时是 `None`。
    fresh_for: Option<Duration>,
    /// `no-store`：不能存（RFC 9111 §5.2.2.5）。
    no_store: bool,
    /// `no-cache`：可以存，但复用前必须先向服务器验证（RFC 9111 §5.2.2.4）。
    no_cache: bool,
    /// 回答里到底有没有 `Cache-Control`。`304` 不带时，沿用上一份的 `no-cache`。
    has_control: bool,
    validators: Validators,
}

impl CacheHeaders {
    fn from_headers(headers: &HeaderMap) -> Self {
        let text = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let mut max_age = None;
        let mut no_store = false;
        let mut no_cache = false;
        let control = text("cache-control");
        if let Some(control) = &control {
            for directive in control.split(',') {
                let directive = directive.trim().to_ascii_lowercase();
                if directive == "no-store" {
                    no_store = true;
                } else if directive == "no-cache" {
                    no_cache = true;
                } else if let Some(secs) = directive.strip_prefix("max-age=") {
                    max_age = secs.trim_matches('"').parse::<u64>().ok();
                }
            }
        }
        let age = text("age")
            .and_then(|age| age.parse::<u64>().ok())
            .unwrap_or(0);
        Self {
            fresh_for: max_age.map(|secs| Duration::from_secs(secs.saturating_sub(age))),
            no_store,
            no_cache,
            has_control: control.is_some(),
            validators: Validators {
                etag: text("etag"),
                last_modified: text("last-modified"),
            },
        }
    }
}

/// 一份回答能复用多久。
///
/// 本地的最小间隔是下限：间隔内不复用的话，换来的不是一份更新的回答，而是本机
/// 的「太频繁」。开放数据的 `max-age` 比它长时就照服务器的：服务器自己说了这段
/// 时间里回答不会变（实测 60 秒）。`no-store` 不延长，但也不缩短到间隔以下，
/// 理由同上。
///
/// **带令牌的回答不延长**：只实测过开放数据的头，个人接口给不给 `max-age`、给多长
/// 都没核对过（文档里签名图片是 `private, max-age=600`）。照搬的话，所在地图、
/// 未读数这类本来 3–25 秒就复用到期的回答，可能悄悄变成几分钟。等有人带令牌实测
/// 过再放开。
fn reply_ttl(min_gap: Duration, authed: bool, cache: &CacheHeaders) -> Duration {
    match cache.fresh_for {
        Some(fresh) if !cache.no_store && !cache.no_cache && !authed => fresh.max(min_gap),
        _ => min_gap,
    }
}

/// 一份回答按什么条件存，见 [`VrccClient::keep_reply`]。
#[derive(Clone, Debug)]
struct ReplyPolicy {
    ttl: Duration,
    validators: Validators,
    /// 存下来，但每次复用前都要先验证（`no-cache`）。
    must_revalidate: bool,
    /// 不存（`no-store`）。
    no_store: bool,
}

/// 在缓存里找到的情况，见 [`VrccClient::lookup_reply`]。
enum Cached {
    /// 还新鲜：直接用。
    Fresh(String),
    /// 过期了（或者必须先验证），但带着校验值：发条件请求，`304` 时接着用这份正文。
    Stale {
        text: String,
        validators: Validators,
        must_revalidate: bool,
    },
    /// 没有可用的。
    Missing,
}

/// 客户端共享状态：限流器与服务器下发的配额。
#[derive(Debug, Default)]
pub struct VrccClientState {
    limiter: RateLimiter,
    /// 间隔内的读请求直接复用的上一份成功回答，见 [`VrccClient::lookup_reply`]。
    replies: HashMap<ReplyKey, CachedReply>,
    /// 429 退避截止时间。
    backoff_until: Option<Instant>,
    /// 连续 429 次数，用于指数退避。
    backoff_attempt: u32,
    /// 服务器在 `RateLimit-*` 里说的各份额度。
    ///
    /// **纯内存**：重启即丢，这是有意的。官方没有「查询额度」的接口，这组头
    /// 只在你发请求的那一刻回一次；把它落盘只会把一个必然过期的快照当成事实，
    /// 而官方的时间基准是北京时间零点，本地推算的日期跟它对不上。
    quotas: HashMap<QuotaBucket, SeenQuota>,
}

impl VrccClientState {
    pub fn window_used(&self, now: Instant) -> usize {
        self.limiter.window_used(now)
    }

    pub fn backoff_remaining(&self, now: Instant) -> Duration {
        self.backoff_until
            .map(|until| until.saturating_duration_since(now))
            .unwrap_or_default()
    }

    /// 还没到重置时间的那份服务器额度；没拿到过或已经过了零点时为 `None`。
    fn live_quota(&self, bucket: QuotaBucket, now: Instant) -> Option<SeenQuota> {
        self.quotas
            .get(&bucket)
            .copied()
            .filter(|quota| quota.until > now)
    }
}

/// 客户端用量快照。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VrccLocalUsage {
    /// 第一份读今天已用多少、上限多少。**只来自服务端的 `RateLimit-*`**；
    /// 还没拿到过时为 `None`——界面应显示「未知」，不要填一个自己编的数。
    pub reads: Option<(i64, i64)>,
    /// `me/unread` 的（已用，上限），只有服务器给过响应头时才有。
    pub unread: Option<(i64, i64)>,
    pub window_used: usize,
    pub backoff_remaining: Duration,
    /// 服务端下一次重置还有多少秒；没拿到过响应头时为 `None`。
    pub reset_secs: Option<i64>,
}

/// 一次请求的全部参数。
struct Call<'a> {
    method: &'static str,
    path: &'a str,
    body: Option<serde_json::Value>,
    /// 是否带 `Bearer`。
    authed: bool,
    /// 本地限流用的接口标识。全部是字面量，所以是 `'static`——
    /// 它会被原样带进 [`HttpDetail::op`]，消费方靠它区分「同一个码在不同接口
    /// 下的不同含义」（例如 `429 too_many` 在 `device_code` 上是按网络出口 IP
    /// 算的额度，在别处是玩家自己的每日上限）。
    key: &'static str,
    /// 同一个接口两次之间至少隔多久。
    min_gap: Duration,
    /// 算在哪一份每天的额度里；`None` 是没有每天上限的接口。
    bucket: Option<QuotaBucket>,
    /// 本地限流槽，覆盖默认的「`key` + 路径」。
    ///
    /// 默认的槽是按**资源**算的：同一个世界、同一个排序才算同一个资源。
    /// 但有两种情况「同一份资源」和「同一个操作意图」不是一回事：
    ///
    /// - 星轨相交的可见性探测：它问的是同一份列表，可它是**为了决定要不要
    ///   显示入口**才问的，不该把玩家随后点开页签那一次挡掉（那一次要带
    ///   `mark=1` 去清红点）。给它单独一个槽，探测就不会占住玩家的位置。
    ///
    /// 有值时就原样当槽名，不再拼路径。
    limiter: Option<&'static str>,
    /// 优先级请求：**不受退避和最小间隔限制，立刻发**。
    ///
    /// 只给 `disconnect` 用。理由：吊销是玩家明确要求的动作，而退避是全局的
    /// （别的接口吃到一次 429 就会进入退避）。如果吊销被退避挡住，本地令牌
    /// 已经被清掉，这场授权就**再也没有机会**在服务器上吊销了——服务器上
    /// 那份授权会一直有效，玩家在网页「第三方客户端」里也看不到它该消失。
    ///
    /// 这类请求都是玩家手动触发的一次性动作，不可能刷屏，所以不设限没有风险。
    bypass_gate: bool,
}

impl Call<'_> {
    /// 这次请求的回答能不能在最小间隔内复用。
    ///
    /// 只有读才行。提醒轮询不行：它带游标，复用会让后台把同一批提醒再弹一遍；
    /// 优先级请求（吊销）本来就是写。
    fn reusable(&self) -> bool {
        self.method == "GET" && !self.bypass_gate && self.key != "notifications"
    }

    /// 这次读会顺手把服务器上的已读往前推（`?mark=1`）。
    ///
    /// 这种读其实带着写：优先真的发出去；被本地间隔或退避挡住时才退回上一份回答，
    /// 成功后清掉未读数和通知中心的回答，见 [`VrccClient::request`]。
    fn marks_read(&self) -> bool {
        self.path.contains("mark=1")
    }
}

/// 同一个读请求的「排队号」：同时发起的同一个读，后来的等前面那个的回答。
///
/// 锁里放的是前面那个分给排队者的回答（见 [`VrccClient::share_with_queued`]）：
/// 拿到号以后看到有，就直接用它，不再发第二次。
///
/// 排队没有自己的超时：排在后面的要等前面那个走完。前面那个最多为限流等
/// [`MAX_ACQUIRE_WAIT`]（10 秒），连接最多 [`CONNECT_TIMEOUT`]（15 秒），之后每次读
/// 最多空等 [`READ_TIMEOUT`]（25 秒，按每次读算，不是整个回答的总时长）。排队者
/// 自己被取消时直接离队，不影响前面那个。
type Flight = Arc<FlightTurn>;

/// 排队号里那把锁：锁着前面那个分给排队者的回答。
type FlightTurn = tokio::sync::Mutex<Option<String>>;

/// 一个可克隆的客户端句柄。
#[derive(Clone)]
pub struct VrccClient {
    http: reqwest::Client,
    base: String,
    client_id: String,
    token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    state: Arc<std::sync::Mutex<VrccClientState>>,
    /// 正在路上的读请求，按 [`ReplyKey`] 排队，见 [`Self::flight`]。
    flights: Arc<std::sync::Mutex<HashMap<ReplyKey, Weak<FlightTurn>>>>,
}

impl VrccClient {
    /// 新建客户端。`token` 是个取令牌的回调，这样令牌续期不用重建客户端。
    ///
    /// `proxy_url` 走宿主应用统一的代理设置（`WebClient::proxy_url`）。
    /// **必须传**：本机实测 `vrcc.cc` 直连会 DNS 超时，走代理才通。原来这里
    /// 自己 `build()` 一个不带代理的客户端，结果请求发不出去，界面上表现为
    /// 「接口暂时不可用」——看起来像服务挂了，其实是本地网络。
    ///
    /// 代理的构造方式照宿主 `llm.rs` 的既有做法：`Proxy::all(rewrite_remote_dns(url))`，
    /// `socks5://` 会被改写成 `socks5h://` 让 DNS 也走代理。
    pub fn new(
        client_id: impl Into<String>,
        token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        proxy_url: Option<&str>,
    ) -> Self {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT);
        match proxy_url.map(str::trim).filter(|url| !url.is_empty()) {
            Some(proxy_url) => match reqwest::Proxy::all(rewrite_remote_dns(proxy_url).as_ref()) {
                Ok(proxy) => {
                    builder = builder.proxy(proxy);
                }
                Err(error) => {
                    // 代理串写错不该让整个客户端起不来：退回直连，
                    // 并把原因记下来，否则症状会变成"接口不可用"。
                    tracing::warn!(%error, proxy_url, "vrcc: invalid proxy url, using direct");
                }
            },
            None => {
                tracing::debug!("vrcc: no proxy configured, connecting directly");
            }
        }
        let http = builder.build().unwrap_or_default();
        Self {
            http,
            base: VRCC_API_BASE.to_string(),
            client_id: client_id.into(),
            token,
            state: Arc::new(std::sync::Mutex::new(VrccClientState::default())),
            flights: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// 用构建期确定的标识新建。
    pub fn with_default_client(
        token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        proxy_url: Option<&str>,
    ) -> Self {
        Self::new(resolve_client_id(), token, proxy_url)
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// 读用量。
    ///
    /// 全部取自服务端的 `RateLimit-*`，不落盘、不推算：官方没有查询额度的
    /// 接口，这组头只在你发请求的那一刻回一次。还没拿到过时返回 `None`，
    /// 让界面显示「未知」，而不是编一个本机日期算出来的数。
    pub fn local_usage(&self) -> VrccLocalUsage {
        let now = Instant::now();
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let used = |quota: SeenQuota| ((quota.limit - quota.remaining).max(0), quota.limit);
        let reads = state.live_quota(QuotaBucket::Reads, now);
        let unread = state.live_quota(QuotaBucket::Unread, now);
        VrccLocalUsage {
            reads: reads.map(used),
            unread: unread.map(used),
            window_used: state.window_used(now),
            backoff_remaining: state.backoff_remaining(now),
            reset_secs: reads.map(|quota| {
                quota
                    .until
                    .saturating_duration_since(now)
                    .as_secs()
                    .try_into()
                    .unwrap_or(i64::MAX)
            }),
        }
    }

    /// 只允许 `/api/v1/` 下的相对路径。这是硬约束，不提供绕过开关。
    ///
    /// 除了绝对地址，还要挡住**路径穿越**：`me/maps/{world}/page` 里的 `{world}`
    /// 来自外部数据（VRChat 世界 ID），`wrld_x/../../me/notifications` 这种值在
    /// 规范化之后会落到另一个接口上。世界 ID 在各自入口已经有严格校验（见
    /// [`Self::validate_world`]），这里是第二道——写给「以后照着加接口、忘了校验
    /// 参数」的那一天。
    fn validate_path(path: &str) -> Result<(), VrccError> {
        let lowered = path.to_ascii_lowercase();
        if lowered.starts_with("http") || path.starts_with("//") {
            return Err(VrccError::new(
                ErrorLayer::Input,
                ErrorDetail::AbsolutePath(path.to_string()),
            ));
        }
        let suspicious = path.contains("..")
            || path.contains('\\')
            || path.contains('#')
            || path.chars().any(char::is_control);
        if suspicious {
            return Err(VrccError::new(
                ErrorLayer::Input,
                ErrorDetail::InvalidArgument("path"),
            ));
        }
        Ok(())
    }

    /// 世界 ID 是不是官方认的形状：`wrld_` 加 36 位十六进制（含连字符）。
    ///
    /// 官方 `World` 的格式是 `^wrld_[0-9a-f-]{36}$`。这个值会被拼进 URL 路径，
    /// 所以**发请求之前**就必须挡住：形状不对的 ID，我们没有任何理由替它发一次。
    fn is_valid_world(world: &str) -> bool {
        const TAIL_LEN: usize = 36;
        let Some(tail) = world.strip_prefix("wrld_") else {
            return false;
        };
        tail.len() == TAIL_LEN && tail.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-')
    }

    /// 形状不对就直接失败，不带着它去拼路径。
    ///
    /// 用 [`ErrorDetail::InvalidArgument`] 而不是 `AbsolutePath`：那个说的是「我们
    /// 的路径拼错了」，这里说的是「调用方给的值不能用」，两者排查方向不同。
    fn validate_world(world: &str) -> Result<(), VrccError> {
        if Self::is_valid_world(world) {
            return Ok(());
        }
        Err(VrccError::new(
            ErrorLayer::Input,
            ErrorDetail::InvalidArgument("world"),
        ))
    }

    /// 群组短码的形状检查：`^[A-Za-z0-9]{3,6}\.\d{4}$`。
    ///
    /// 官方 openapi 里那个 `pattern` 就是这么写的，`KIPFEL.2104` 是典型值。
    /// **为什么要在本地先挡一遍**：短码会被直接拼进路径，而
    /// [`Self::validate_path`] 只拒绝绝对地址。不检查形状的话，一个形如
    /// `../me/card` 的输入能拼出 `clubs/../me/card` 这种越权路径——服务器
    /// 也许会规范化掉，但**我们不该把这个判断交给服务器**。
    ///
    /// 只做形状判断，**不做规范化**：大小写原样发出去（实测服务器两种都收），
    /// 由服务器决定认不认。
    fn validate_club_code(code: &str) -> Result<(), VrccError> {
        let code = code.trim();
        let Some((head, tail)) = code.split_once('.') else {
            return Err(VrccError::new(
                ErrorLayer::Input,
                ErrorDetail::InvalidArgument("code"),
            ));
        };
        let head_ok =
            (3..=6).contains(&head.len()) && head.chars().all(|ch| ch.is_ascii_alphanumeric());
        let tail_ok = tail.len() == 4 && tail.chars().all(|ch| ch.is_ascii_digit());
        if head_ok && tail_ok {
            return Ok(());
        }
        Err(VrccError::new(
            ErrorLayer::Input,
            ErrorDetail::InvalidArgument("code"),
        ))
    }

    /// 拿到一次发送许可：检查退避、限流，必要时等待。
    ///
    /// 三个要点：
    /// 1. **预占而不是先算后记**。原来先 `check` 拿 `wait`、放锁睡、再加锁
    ///    `record`，并发调用会拿到同一个 `wait`，一起醒来一起发出去。
    /// 2. **等待有上限**。`boards` 的 gap 是 60 秒，用户点刷新时不能让他干等：
    ///    超过 [`MAX_ACQUIRE_WAIT`] 就返回 `ThrottledLocally { reason: Gap }`，
    ///    让调用方改用缓存（见 `VrccService::boards` 对 `is_local_gap` 的接住）。
    /// 3. **醒来后再看一次退避**。等待期间可能有别的请求吃到了 429。
    /// 4. **占到槽就交出守卫**。从预占那一刻起，这次预占由返回的 [`SlotRelease`]
    ///    管：在这里等着时被取消（future 被丢掉）、睡醒发现开始退避了，守卫都会把
    ///    预占撤回去，不留一个谁都没用上的槽。
    async fn acquire(
        &self,
        key: &str,
        min_gap: Duration,
        budget: Duration,
    ) -> Result<SlotRelease<'_>, VrccError> {
        let now = Instant::now();
        let (at, previous) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let backoff = state.backoff_remaining(now);
            if !backoff.is_zero() {
                return Err(VrccError::new(
                    ErrorLayer::Client,
                    ErrorDetail::ThrottledLocally {
                        retry_after_secs: backoff.as_secs().max(1),
                        reason: LocalThrottle::Backoff,
                    },
                ));
            }
            // 预占前槽上记的时间，和预占在同一把锁里取：撤回时只撤自己那一截。
            let previous = state.limiter.last(key);
            match state.limiter.reserve(key, min_gap, now, budget) {
                Ok(at) => (at, previous),
                Err(wait) => {
                    // 没占槽：调用方可以拿缓存顶上，而不是干等。
                    return Err(VrccError::new(
                        ErrorLayer::Client,
                        ErrorDetail::ThrottledLocally {
                            retry_after_secs: wait.as_secs().max(1),
                            reason: LocalThrottle::Gap,
                        },
                    ));
                }
            }
        };
        let release = SlotRelease {
            client: self,
            held: Some(Reserved {
                slot: key.to_string(),
                at,
                previous,
            }),
        };
        let wait = at.saturating_duration_since(Instant::now());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        // 睡醒再看一眼：等待期间可能已经有别的请求吃到 429，全局退避开始了。
        let backoff = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.backoff_remaining(Instant::now())
        };
        if !backoff.is_zero() {
            return Err(VrccError::new(
                ErrorLayer::Client,
                ErrorDetail::ThrottledLocally {
                    retry_after_secs: backoff.as_secs().max(1),
                    reason: LocalThrottle::Backoff,
                },
            ));
        }
        Ok(release)
    }

    /// 现在是不是有东西挡着重试：全局退避还没结束，或者这一份额度已经用完。
    fn retries_held_back(&self, bucket: Option<QuotaBucket>) -> bool {
        let now = Instant::now();
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        !state.backoff_remaining(now).is_zero()
            || bucket
                .and_then(|bucket| state.live_quota(bucket, now))
                .is_some_and(|quota| quota.remaining <= 0)
    }

    /// 旧服务器的 429 没有 `Retry-After`：按指数退避猜。
    fn note_rate_limited(&self) -> Duration {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let wait = backoff_for(state.backoff_attempt);
        state.backoff_attempt = state.backoff_attempt.saturating_add(1);
        state.backoff_until = Some(Instant::now() + wait);
        wait
    }

    /// 服务器给了 `Retry-After`：照它等，不自己加码。
    fn note_retry_after(&self, wait: Duration) -> Duration {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.backoff_attempt = state.backoff_attempt.saturating_add(1);
        state.backoff_until = Some(Instant::now() + wait);
        wait
    }

    fn note_success(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.backoff_attempt = 0;
        state.backoff_until = None;
    }

    /// 这次请求在缓存里的键；不能复用的请求是 `None`。
    ///
    /// `token` 是这次请求会带上的令牌（[`Self::request`] 开头只读一次）：缓存按它
    /// 分账号，请求头也用它，两边不会因为中途换号而对不上。
    fn reply_key(&self, call: &Call<'_>, token: Option<&str>) -> Option<ReplyKey> {
        if !call.reusable() {
            return None;
        }
        let scope = if call.authed { token_scope(token?) } else { 0 };
        Some(ReplyKey {
            scope,
            key: call.key,
            path: call.path.to_string(),
        })
    }

    /// 同一个读请求在最小间隔内再来一次：直接给上一次的成功回答。
    ///
    /// 客户端在会话里共享以后，每个接口的最小间隔才真正生效（早先每个请求都
    /// 新建客户端，间隔从来没拦过谁）。生效之后，间隔内的第二次读如果还像
    /// 原来那样「最多等 10 秒，否则报太频繁」，打开过的页面切回来就会报错。
    ///
    /// 官方文档里硬性的只有每分钟 120 次、提醒轮询 25 秒和每天的次数，其余各
    /// 接口的最小间隔都是本地定的礼貌值：间隔内服务器那边本来也不会有什么新的。
    /// 所以间隔内直接复用上一份回答——不发请求、不占额度、不等，也不报错。
    /// 没有可复用的（上一次失败了，或者是另一份 `limit`）才照旧走限流。
    ///
    /// 写操作成功后会清掉相关的回答，见 [`Self::forget_replies`]。
    ///
    /// 能用多久见 [`reply_ttl`]：服务器的 `max-age` 更长时照服务器的。过期了但带
    /// 校验值的，返回 [`Cached::Stale`]，由调用方发条件请求。
    fn lookup_reply(&self, key: &ReplyKey) -> Cached {
        let now = Instant::now();
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(reply) = state.replies.get(key) else {
            return Cached::Missing;
        };
        if reply.is_live(now) {
            Cached::Fresh(reply.text.clone())
        } else if reply.worth_keeping(now) {
            Cached::Stale {
                text: reply.text.clone(),
                validators: reply.validators.clone(),
                must_revalidate: reply.must_revalidate,
            }
        } else {
            Cached::Missing
        }
    }

    /// 按服务器的缓存头存一份成功回答（或者不存）。返回**间隔内能不能直接给出**：
    /// 存了、而且不需要先验证才算。
    ///
    /// - `no-store`：不存，并删掉这个键下的旧回答；
    /// - `no-cache`：只有带校验值才存，而且每次复用前都要先验证；没有校验值就和
    ///   `no-store` 一样处理（存了也没法验证）。
    ///
    /// 返回 `false` 时，限流槽由 [`SlotRelease`] 还回去，见那里的说明。
    fn keep_reply(
        &self,
        call: &Call<'_>,
        key: ReplyKey,
        text: String,
        policy: ReplyPolicy,
    ) -> bool {
        let unusable = policy.no_store || (policy.must_revalidate && policy.validators.is_empty());
        if unusable {
            tracing::debug!(
                key = call.key,
                no_store = policy.no_store,
                "vrcc: the server does not let this reply be reused; not keeping it"
            );
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.replies.remove(&key);
            return false;
        }
        let reusable_now = !policy.must_revalidate;
        self.remember_reply(
            key,
            policy.ttl,
            text,
            policy.validators,
            policy.must_revalidate,
        );
        reusable_now
    }

    /// 记下一份成功回答，供之后复用或发条件请求。
    fn remember_reply(
        &self,
        key: ReplyKey,
        ttl: Duration,
        text: String,
        validators: Validators,
        must_revalidate: bool,
    ) {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.replies.retain(|_, reply| reply.worth_keeping(now));
        if state.replies.len() >= REPLY_CACHE_MAX {
            // 还活着却已经满了：丢最旧的那份。
            if let Some(oldest) = state
                .replies
                .iter()
                .min_by_key(|(_, reply)| reply.at)
                .map(|(key, _)| key.clone())
            {
                state.replies.remove(&oldest);
            }
        }
        state.replies.insert(
            key,
            CachedReply {
                at: now,
                ttl,
                text,
                validators,
                must_revalidate,
            },
        );
    }

    /// 写操作成功了：清掉受影响的回答，并让它们的限流槽立刻可用。
    ///
    /// 只清缓存不够：间隔还在，紧接着的重拉（例如打分后重新拉地图页）会被本地
    /// 限流挡住或排队，界面要么报「太频繁」，要么等几秒才看到自己刚改的内容。
    /// 间隔本来是「这段时间里不会有新东西」的礼貌值，写了之后这个前提不成立了，
    /// 所以一起放开。每分钟 120 次的窗口不动。
    ///
    /// 还在路上的同一个读也一样：它的排队号从表里摘掉（之后的重拉拿新号，不会排在
    /// 写之前就发出去的那个读后面、拿到它分来的旧回答，见 [`Self::share_with_queued`]），
    /// 槽也放开（不用等它的间隔）。
    fn forget_replies(&self, affected: impl Fn(&str, &str) -> bool) {
        let mut slots = Vec::new();
        {
            let mut flights = self.flights.lock().unwrap_or_else(|e| e.into_inner());
            flights.retain(|key, _| {
                let hit = affected(key.key, &key.path);
                if hit {
                    slots.push(limiter_key(key.key, &key.path));
                }
                !hit
            });
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.replies.retain(|key, _| {
            let hit = affected(key.key, &key.path);
            if hit {
                slots.push(limiter_key(key.key, &key.path));
            }
            !hit
        });
        for slot in slots {
            state.limiter.forget(&slot);
        }
    }

    /// 把客户端指到本地测试服务器上。测试用。
    #[cfg(test)]
    pub(crate) fn set_base(&mut self, base: String) {
        self.base = base;
    }

    /// 两个句柄是不是同一个客户端（共享限流、额度与缓存）。测试用。
    #[cfg(test)]
    pub(crate) fn shares_state_with(&self, other: &VrccClient) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }

    /// 会话换了令牌（连上、断开、失效）：丢掉所有带令牌的回答，并忘掉各接口的
    /// 间隔记录。
    ///
    /// - 回答按令牌指纹分键，换了账号本来就读不到；清掉是把内存及时还回去。
    /// - 间隔记录**必须一起清**：槽名不带令牌，上一个令牌占着的槽留着的话，新令牌在
    ///   间隔内的第一次读手上没有回答（刚清掉），只能拿到本机的「太频繁」——断开后
    ///   一分钟内重连、换一个也连了星遇同频的号，都会碰上。每分钟的窗口不动；提醒轮询
    ///   的 25 秒是按授权算的，换了令牌就是另一份授权。
    /// - 带令牌的读的排队号也摘掉：之后来的拿新号，不会排在旧令牌的那个读后面。
    pub fn forget_session_state(&self) {
        {
            let mut flights = self.flights.lock().unwrap_or_else(|e| e.into_inner());
            flights.retain(|key, _| key.scope == 0);
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.replies.retain(|key, _| key.scope == 0);
        state.limiter.forget_gaps();
    }

    /// 这个读请求的排队号。
    ///
    /// 客户端共享以后，同一个读两处同时发起（两个组件一起挂载）时，两个都没在缓存
    /// 里找到，第二个就会撞上第一个刚占住的间隔，报「太频繁」。排队以后，后来的
    /// 等前面那个做完：前面那个把回答分过来了（[`Self::share_with_queued`]）就直接
    /// 用，没有再去缓存里找。
    fn flight(&self, key: &ReplyKey) -> Flight {
        let mut flights = self.flights.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(flight) = flights.get(key).and_then(Weak::upgrade) {
            return flight;
        }
        // 顺手清掉已经没人排的键，免得表随逛过的世界一直长。
        flights.retain(|_, flight| flight.strong_count() > 0);
        let flight = Arc::new(tokio::sync::Mutex::new(None));
        flights.insert(key.clone(), Arc::downgrade(&flight));
        flight
    }

    /// 把这次成功的回答分给正排在同一个读后面的调用方，并把这个排队号从表里摘掉。
    ///
    /// 服务器给个人接口的回答都带 `no-store`，存不下：只靠排队的话，两个组件一起
    /// 挂载时后来的那个拿到号、缓存里没有，还得再发一次。分给它就只花一次。
    ///
    /// 只分给「请求在路上时就已经拿了号」的调用方：号一摘，之后来的拿到的是新号，
    /// 看不到这份，照样自己发；手里攥着旧号的都走了，这份回答也就跟着没了，不为
    /// 以后的请求存。所以和 `no-store` 不冲突；`no-cache` 的也照分——排队者是在这次
    /// 请求发出之后才来的，拿到的不会比它自己发一次旧。
    ///
    /// 调用方保证：只在正文已经解出来之后调用；标已读的读（`mark=1`）不调用——
    /// 那种读带着写，各自真的发出去。
    fn share_with_queued(
        &self,
        key: &ReplyKey,
        flight: &Flight,
        turn: &mut Option<String>,
        text: &str,
    ) {
        *turn = Some(text.to_string());
        let mut flights = self.flights.lock().unwrap_or_else(|e| e.into_inner());
        if flights
            .get(key)
            .is_some_and(|entry| std::ptr::eq(entry.as_ptr(), Arc::as_ptr(flight)))
        {
            flights.remove(key);
        }
    }

    /// 记下服务器在 `RateLimit-*` 里说的一份额度。
    ///
    /// 只存内存。这份数就是界面能看到的全部依据——不再拿它去覆盖什么本地计数，
    /// 因为本地计数已经没有了。
    fn note_quota(&self, bucket: QuotaBucket, quota: ServerQuota) {
        let now = Instant::now();
        let reset = if quota.reset_secs > 0 {
            quota.reset_secs
        } else {
            secs_until_beijing_midnight()
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.quotas.insert(
            bucket,
            SeenQuota {
                limit: quota.limit,
                remaining: quota.remaining,
                until: now + Duration::from_secs(reset),
            },
        );
    }

    /// 某一份额度今天用完了：在重置之前，这一份的请求不再发出去。
    fn mark_exhausted(&self, bucket: QuotaBucket, wait: Duration) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let limit = state.quotas.get(&bucket).map_or(0, |quota| quota.limit);
        state.quotas.insert(
            bucket,
            SeenQuota {
                limit,
                remaining: 0,
                until: Instant::now() + wait,
            },
        );
    }

    /// 发请求前过一下额度闸门。
    ///
    /// **只在拿到过服务端 `RateLimit-*` 时才拦。** 没拿到过就放行——不能按本地自然日预扣到
    /// 1000 就自己停下：官方的时间基准是北京时间零点，
    /// 本地推算的日期对非 UTC+8 的玩家必然错开，结果是「服务端明明还有余量，
    /// 客户端自己先拒绝」。宁可让服务端回 429（我们原样展示），也不要自己编。
    fn spend(&self, bucket: QuotaBucket) -> Result<(), VrccError> {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(live) = state.live_quota(bucket, now) else {
            return Ok(());
        };
        if live.remaining <= 0 {
            // 这一份额度按服务端上一次给的头已经用满。仍属本机拦截：
            // 请求没有发出去，服务端也没计数。
            return Err(VrccError::new(
                ErrorLayer::Client,
                ErrorDetail::ThrottledLocally {
                    retry_after_secs: live.until.saturating_duration_since(now).as_secs().max(1),
                    reason: LocalThrottle::Backoff,
                },
            ));
        }
        // 先扣着，免得两次响应之间连发越界；下一个响应头会覆盖它。
        if let Some(seen) = state.quotas.get_mut(&bucket) {
            seen.remaining -= 1;
        }
        Ok(())
    }

    /// 把 `spend` 预扣的那一次退回去。**只用于请求没发出去的情况。**
    ///
    /// 服务器根本没收到这个请求，本地就不该占着这次额度。
    fn refund(&self, bucket: QuotaBucket) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(seen) = state.quotas.get_mut(&bucket) {
            seen.remaining = seen.remaining.saturating_add(1);
        }
    }

    /// 处理一次 `429` 的**本地记账**：停用哪一份额度、退避多久。
    ///
    /// 它不返回错误，也不构造错误。早先这个函数叫 `on_too_many`，返回
    /// 6 种不同的错误变体，等于把「该等多久」和「怎么措辞」都焊进了
    /// client 层——文案没法本地化，`device_code` 的特殊说明也只能硬编码成
    /// 一段中文。现在它只更新 [`VrccClientState`]，错误由调用方按 HTTP 事实
    /// 统一构造，等待时长由 `Retry-After` 随错误一起带出去。
    ///
    /// 官方的 `429` 分几种，记账方式不同：
    /// - `too_soon`：两次轮询间隔不到 25 秒。**不退避**，这不是配额问题。
    /// - 每天的次数用完（`RateLimit-Remaining` 是 0，或 `Retry-After` 长到
    ///   只能是「到北京时间零点」）：只停这一份额度，不让别的接口跟着停。
    /// - 其余（每分钟的限制）：照 `Retry-After` 全局退避；旧服务器没给头时
    ///   按指数退避猜。
    ///
    /// `429 busy`（全站连接功能暂停到次日）也走这里，但它和本机额度无关：
    /// 它不停用任何额度，只会因为 `Retry-After` 很长而让 [`Self::acquire`]
    /// 自然退避。判断哪种 `busy` 是消费方的事，见 [`VrccError::is_not_ready`]
    /// 与 [`HttpDetail::op`]。
    fn record_too_many(&self, error: &str, rate: RateHeaders, bucket: Option<QuotaBucket>) {
        if error == "too_soon" {
            // 问勤了而已，不消耗额度也不该长时间退避。
            return;
        }
        let daily = rate.quota.is_some_and(|quota| quota.remaining <= 0)
            || rate
                .retry_after_secs
                .is_some_and(|secs| Duration::from_secs(secs) > MAX_BACKOFF);
        if daily {
            if let Some(bucket) = bucket {
                let wait = rate
                    .retry_after_secs
                    .or(rate.quota.map(|quota| quota.reset_secs))
                    .filter(|secs| *secs > 0)
                    .unwrap_or_else(secs_until_beijing_midnight);
                self.mark_exhausted(bucket, Duration::from_secs(wait));
            }
            return;
        }
        match rate.retry_after_secs {
            Some(secs) => {
                self.note_retry_after(Duration::from_secs(secs.max(1)));
            }
            None => {
                self.note_rate_limited();
            }
        }
    }

    /// 把请求真正发出去之前的所有步骤。
    ///
    /// 单独抽一层，是为了让「没发出去就要退配额」只写一次：这一层返回的
    /// `Err` 一律表示**服务器没有收到这次请求**，调用方据此退还预扣。
    ///
    /// 注意 `send()` 失败也算没发出去——连接都建不起来，服务端不可能计数。
    ///
    /// 成功时连同还槽守卫一起交回：可复用的读，守卫一直带到响应处理完（见
    /// [`SlotRelease`]）；其余请求发出去以后槽就留着，交回的守卫是空的。
    async fn send_request(
        &self,
        call: &Call<'_>,
        token: Option<&str>,
        conditional: Option<&Validators>,
        wait_budget: Duration,
    ) -> Result<(reqwest::Response, SlotRelease<'_>), VrccError> {
        // 优先级请求（吊销）不排队：被退避挡住就永远吊销不了了。
        //
        // 槽名默认按**资源**算（接口 + 保留表达资源的查询参数），见
        // [`limiter_key`]。少数调用方给了显式的 `limiter`，那是「同一份
        // 资源、不同意图」——例如可见性探测，它不该占住玩家随后那一次
        // 带 `mark=1` 的读取。
        //
        // 从占到槽起，`release` 管着这次预占：等待时被取消、睡醒碰上退避、连接建不
        // 起来（下面 `send_reserved` 返回 `Err`），都会撤回去——服务器一次都没见过
        // 这个请求，不该占着间隔，否则玩家在间隔内点「重试」只会得到「太频繁」。
        // 每分钟的窗口照算。
        let mut release = if call.bypass_gate {
            SlotRelease::none(self)
        } else {
            self.acquire(&slot_of(call), call.min_gap, wait_budget)
                .await?
        };
        if call.reusable() {
            // 可复用的读：等响应时被取消也撤（守卫跟着 future 一起丢掉）。拿到响应
            // 以后由 `request()` 决定留不留。
            let response = self.send_reserved(call, token, conditional).await?;
            return Ok((response, release));
        }
        // 写操作、提醒轮询：开始发了就不撤——等响应时被取消，服务器可能已经收到，
        // 它们的间隔是官方的。只有确定没发出去（`Err`）才撤。
        let held = release.held.take();
        match self.send_reserved(call, token, conditional).await {
            Ok(response) => Ok((response, SlotRelease::none(self))),
            Err(error) => {
                // 把预占交回守卫，返回时撤掉。
                release.held = held;
                Err(error)
            }
        }
    }

    /// 已经拿到发送许可之后，真正把请求发出去。失败一律表示请求没有到服务器。
    async fn send_reserved(
        &self,
        call: &Call<'_>,
        token: Option<&str>,
        conditional: Option<&Validators>,
    ) -> Result<reqwest::Response, VrccError> {
        let mut builder = self
            .http
            .request(
                call.method.parse().map_err(|_| {
                    // 方法名是我们自己写死的，拼错属于本机 bug，不是网络问题。
                    VrccError::new(
                        ErrorLayer::Input,
                        ErrorDetail::AbsolutePath(format!("不支持的请求方法 {}", call.method)),
                    )
                })?,
                format!("{}{}", self.base, call.path),
            )
            .header("X-Client", &self.client_id)
            .header("Accept", "application/json");

        if let Some(token) = token {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }

        // 手上有一份过期但带校验值的回答：问服务器「变了没有」。
        if let Some(validators) = conditional {
            if let Some(etag) = &validators.etag {
                builder = builder.header("If-None-Match", etag);
            }
            if let Some(last_modified) = &validators.last_modified {
                builder = builder.header("If-Modified-Since", last_modified);
            }
        }

        if let Some(payload) = &call.body {
            let text = payload.to_string();
            check_body_size(text.len())?;
            builder = builder
                .header("Content-Type", "application/json; charset=utf-8")
                .body(text);
        }

        builder.send().await.map_err(transport_error)
    }

    /// 核心请求。
    async fn request<T: DeserializeOwned>(&self, call: Call<'_>) -> Result<T, VrccError> {
        Self::validate_path(call.path)?;
        // 令牌只在这里读一次：缓存按它分账号，请求头也用它。分两次读的话，等限流
        // 的那几秒里换了号，就会拿 B 的令牌去问、却把回答记在 A 的名下。
        let token =
            if call.authed {
                Some((self.token)().ok_or_else(|| {
                    VrccError::new(ErrorLayer::Session, ErrorDetail::TokenMissing)
                })?)
            } else {
                None
            };
        let reply_key = self.reply_key(&call, token.as_deref());
        // 同一个读同时只有一个在路上；后来的拿到号以后，先看前面那个有没有把回答
        // 分过来（见 `share_with_queued`），再查缓存。
        let flight = reply_key.as_ref().map(|key| self.flight(key));
        let mut turn = match &flight {
            Some(flight) => Some(flight.lock().await),
            None => None,
        };
        if let Some(text) = turn.as_ref().and_then(|turn| turn.as_ref()) {
            return decode(text);
        }
        // 间隔内的同一个读请求：先看有没有能复用的回答。放在扣额度之前——
        // 复用不发请求，不该占服务器的次数。
        //
        // 标已读的读（`mark=1`）例外：优先真的发出去，手上那份只在被本地挡下时
        // 顶上（见下面 `fallback`）。
        let mut stale = None;
        let mut fallback = None;
        if let Some(key) = &reply_key {
            match self.lookup_reply(key) {
                Cached::Fresh(text) if call.marks_read() => fallback = Some(text),
                Cached::Fresh(text) => return decode(&text),
                // 标已读的读不发条件请求：服务器要是回 `304`，可能就不执行「标已读」
                // 这个副作用了。眼下服务器不给校验值，碰不到；上游已经在讨论加 `ETag`
                // （vrcc-cc/xingyu-tongpin-api#3），加上之后这里就会碰到。
                Cached::Stale { .. } if call.marks_read() => {}
                Cached::Stale {
                    text,
                    validators,
                    must_revalidate,
                } => stale = Some((text, validators, must_revalidate)),
                Cached::Missing => {}
            }
        }
        let bucket = call.bucket;
        if let Some(bucket) = bucket {
            self.spend(bucket)?;
        }
        // `spend` 是**预扣**：先记上一次，免得两次响应头之间连发越界。但预扣
        // 只有「请求真的发出去」才成立——限流没过、令牌没有、连接就失败，
        // 服务器一次都没数，这地方必须退回去。
        //
        // 不退的后果很实在：断网时后台轮询每分钟扣一次，本地计数很快跑满，
        // 网络恢复后会被自己拦到第二天，而服务端一次都没记过。
        let conditional = stale.as_ref().map(|(_, validators, _)| validators);
        // 手上有能顶上的回答（标已读的读）时不为间隔干等：等的这几秒里玩家看到的是
        // 转圈，而顶上的那份本来就是这个间隔里刚拿到的。
        let wait_budget = if fallback.is_some() {
            Duration::ZERO
        } else {
            MAX_ACQUIRE_WAIT
        };
        let (response, mut release) = match self
            .send_request(&call, token.as_deref(), conditional, wait_budget)
            .await
        {
            Ok(sent) => sent,
            Err(error) => {
                if let Some(bucket) = bucket {
                    self.refund(bucket);
                }
                // 标已读的读被本机挡下（间隔或退避）：拿上一份顶上。玩家每次切到
                // 那一页都会发它，报错会变成一条「太频繁」的提示；上一份回答是这个
                // 间隔里刚拿到的，服务器那边也已经标过了。
                if let Some(text) = fallback {
                    if matches!(error.detail, ErrorDetail::ThrottledLocally { .. }) {
                        return decode(&text);
                    }
                }
                if call.reusable() && error.is_local_gap() {
                    // 可复用的读被本地间隔挡下、手上又没有回答可给：调用方只能拿到
                    // 「太频繁」。复用回答和 `SlotRelease` 就是为了不出现这种情况，
                    // 碰到了多半是哪条路径漏了，留个线索。
                    tracing::warn!(
                        key = call.key,
                        "vrcc: a read was turned away by the local gap with no reply to reuse"
                    );
                }
                return Err(error);
            }
        };
        // 从这里起拿到了响应。可复用的读要是最后没存下能直接给出的回答（正文解不
        // 出来、读到一半断了、服务器回错误、`no-store` / `no-cache`），`release` 在
        // 函数返回时把槽还回去；只有存下了才 `keep`。其余请求的 `release` 是空的。
        let status = response.status();
        let rate = RateHeaders::from_headers(response.headers());
        let cache = CacheHeaders::from_headers(response.headers());
        if let Some(sunset) = response
            .headers()
            .get("sunset")
            .and_then(|value| value.to_str().ok())
        {
            // 1.14.0 起，带着弃用字段的回答有 `Deprecation` / `Sunset` 头。
            // 契约层已经不读那些字段，这里只留个线索，方便以后核对。
            tracing::debug!(
                key = call.key,
                sunset,
                "vrcc: response carries deprecated fields"
            );
        }
        let text = response.text().await.map_err(transport_error)?;

        if let (Some(bucket), Some(quota)) = (call.bucket, rate.quota) {
            self.note_quota(bucket, quota);
        }

        if status == reqwest::StatusCode::NOT_MODIFIED {
            if let (Some(key), Some((text, validators, was_no_cache))) =
                (reply_key.as_ref(), stale.as_ref())
            {
                // 服务器说没变：接着用手上那份，重新算有效期。`304` 里给了新的
                // 校验值就换上，没给就沿用原来的。
                self.note_success();
                let value = decode(text)?;
                if let (Some(flight), Some(turn), false) =
                    (flight.as_ref(), turn.as_mut(), call.marks_read())
                {
                    self.share_with_queued(key, flight, turn, text);
                }
                let renewed = Validators {
                    etag: cache
                        .validators
                        .etag
                        .clone()
                        .or_else(|| validators.etag.clone()),
                    last_modified: cache
                        .validators
                        .last_modified
                        .clone()
                        .or_else(|| validators.last_modified.clone()),
                };
                // `304` 不带 `Cache-Control` 时沿用上一份的 `no-cache`：不沿用的话，这份
                // 回答会重新变成「间隔内新鲜」，下一次不验证就直接给出去。
                let policy = ReplyPolicy {
                    ttl: reply_ttl(call.min_gap, call.authed, &cache),
                    validators: renewed,
                    must_revalidate: cache.no_cache || (!cache.has_control && *was_no_cache),
                    no_store: cache.no_store,
                };
                if self.keep_reply(&call, key.clone(), text.clone(), policy) {
                    release.keep();
                }
                return Ok(value);
            }
        }

        if status.is_success() {
            self.note_success();
            let value = decode(&text)?;
            // 排在后面的同一个读直接拿这份，不再发第二次（标已读的读除外）。
            if let (Some(key), Some(flight), Some(turn), false) = (
                reply_key.as_ref(),
                flight.as_ref(),
                turn.as_mut(),
                call.marks_read(),
            ) {
                self.share_with_queued(key, flight, turn, &text);
            }
            let policy = ReplyPolicy {
                ttl: reply_ttl(call.min_gap, call.authed, &cache),
                validators: cache.validators.clone(),
                must_revalidate: cache.no_cache,
                no_store: cache.no_store,
            };
            if call.marks_read() {
                // 服务器上的已读往前推了：未读数和通知中心的回答过时了。
                self.forget_replies(|other, _| matches!(other, "unread" | "notes"));
                // 同一个接口不标已读的那份**不清，而是用这份覆盖**：两者是同一份
                // 资源，这份更新。清掉的话，可见性探测（自己一个 `crossings/probe`
                // 槽，和不标已读的读共用这份回答）在间隔内再来时，槽还占着、手上却
                // 没有回答，只能报「太频繁」。
                //
                // 服务器不让存（`no-store`）时这一步跳过。
                if let (Some(key), false) = (&reply_key, policy.no_store) {
                    let plain = ReplyKey {
                        path: without_mark(&key.path),
                        ..key.clone()
                    };
                    self.keep_reply(&call, plain, text.clone(), policy.clone());
                }
            }
            if let Some(key) = reply_key {
                if self.keep_reply(&call, key, text, policy) {
                    release.keep();
                }
            }
            Ok(value)
        } else {
            let code = status.as_u16();
            let parsed: VrccErrorBody = serde_json::from_str(&text).unwrap_or_default();
            let error = parsed
                .error
                .clone()
                .unwrap_or_else(|| format!("http_{code}"));
            // 429 的记账（停用哪一份额度、退避多久）在这一步做完。**错误本身
            // 只记录 HTTP 事实**：哪些码要退避、退避多久由 `VrccClientState`
            // 承担，不是错误对象的职责。
            // 错误不进缓存，槽由 `release` 还回去（429 也是：退避照旧挡着重试，退避
            // 结束后不该再被本地间隔挡着）。
            if code == 429 {
                self.record_too_many(&error, rate, call.bucket);
                // 只有退避或额度闸门真的挡着重试时才还槽。`Retry-After` 很长、接口又
                // 不属于任何一份额度时，`record_too_many` 什么都不记，这时槽就是唯一
                // 挡着连发的东西，留着。
                if !self.retries_held_back(call.bucket) {
                    release.keep();
                }
            }
            Err(VrccError::new(
                ErrorLayer::Server,
                ErrorDetail::Http(HttpDetail {
                    status: code,
                    error,
                    need: parsed.need,
                    why: parsed.why,
                    retry_after: rate.retry_after_secs,
                    op: Some(call.key),
                }),
            ))
        }
    }

    // -- 开放数据（不需要令牌） --------------------------------------------

    /// 地图榜。官方说明服务器最多 5 分钟更新一次，问得再勤也是同一份。
    ///
    /// `sort`（1.13.0 起）：总榜、本周榜、新上榜。旧服务器不认 `sort`、照旧
    /// 回总榜，这时回答里没有 `sort` 字段，调用方据此判断「还不支持」。
    pub async fn boards(&self, limit: u32, sort: VrccBoardSort) -> Result<VrccBoard, VrccError> {
        let limit = limit.clamp(1, 100);
        self.request(Call {
            method: "GET",
            path: &boards_path(limit, sort),
            body: None,
            authed: false,
            // 每种排序各算各的间隔：切到别的排序不用等上一种的 60 秒。
            key: match sort {
                VrccBoardSort::Top => "boards",
                VrccBoardSort::Week => "boards_week",
                VrccBoardSort::New => "boards_new",
            },
            min_gap: Duration::from_secs(60),
            bucket: None,
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 模型榜（`GET /boards/models`，1.17.0 起）。不需要令牌。
    ///
    /// 和 [`Self::boards`] 同一套约定：`sort` 是 `top` / `week` / `new`，旧服务器
    /// 认不出这个接口会回 `404`，认 `sort` 却不回 `sort` 时调用方要当成
    /// 「服务器还不支持排序」。服务器同样最多 5 分钟更新一次。
    ///
    /// 每种排序单独一个限流槽：切排序本来看的就是不同列表，不该被上一种的
    /// 60 秒间隔挡住。
    pub async fn models_board(
        &self,
        limit: u32,
        sort: VrccBoardSort,
    ) -> Result<VrccModelBoard, VrccError> {
        let limit = limit.clamp(1, 100);
        self.request(Call {
            method: "GET",
            path: &models_board_path(limit, sort),
            body: None,
            authed: false,
            key: match sort {
                VrccBoardSort::Top => "models",
                VrccBoardSort::Week => "models_week",
                VrccBoardSort::New => "models_new",
            },
            min_gap: Duration::from_secs(60),
            bucket: None,
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 单个模型（`GET /models/{id}`）。不需要令牌。
    ///
    /// 不在榜上时服务器回 `404 not_found`——那是**正常状态**（这个模型还没上榜
    /// 或没通过审核），转成 `Ok(None)`，不让它看起来像故障。
    ///
    /// `id` 是 BOOTH 商品编号，要拼进路径，所以只接受正整数：负数、0 本地就
    /// 挡住，没有理由替它发一次请求。
    pub async fn model(&self, id: i64) -> Result<Option<VrccModelDetail>, VrccError> {
        if id <= 0 {
            return Ok(None);
        }
        match self
            .request(Call {
                method: "GET",
                path: &format!("models/{id}"),
                body: None,
                authed: false,
                key: "model",
                min_gap: Duration::from_secs(30),
                bucket: None,
                limiter: None,
                bypass_gate: false,
            })
            .await
        {
            Ok(detail) => Ok(Some(detail)),
            // 只有 `404 not_found` 才是「没有这个模型」；别的 404（比如令牌失效的
            // `gone`）在这条不需要令牌的路径上不该出现，真出现了也不该被吞掉。
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// 服务探活。不需要 client 标识，也不计入个人额度。
    pub async fn status(&self) -> Result<VrccStatus, VrccError> {
        self.request(Call {
            method: "GET",
            path: "status",
            body: None,
            authed: false,
            key: "status",
            min_gap: Duration::from_secs(30),
            bucket: None,
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 取站点上的一个静态资源（例如星遇企划图标）。
    ///
    /// 走 `VRCC_SITE_ORIGIN` 而不是 API 基址：站点资源不在 `/api/v1/` 下。
    /// 仍然复用同一个 http 客户端，所以**代理设置一致**——否则会出现
    /// 「接口通、图标取不到」这种看起来像图标坏了、实际是没走代理的情况。
    ///
    /// 同样硬拦绝对地址：这个客户端的承诺是「只和 vrcc.cc 说话」，
    /// 不能因为多了一个取图方法就开个口子把令牌或请求发到别处去。
    ///
    /// 不计入个人接口的日配额：那不是带令牌的 API 调用，服务端也不按令牌算。
    pub async fn fetch_site_asset(&self, path: &str) -> Result<Vec<u8>, VrccError> {
        Self::validate_path(path)?;
        let url = format!("{VRCC_SITE_ORIGIN}{path}");
        let response = self.http.get(&url).send().await.map_err(transport_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(VrccError::new(
                ErrorLayer::Server,
                ErrorDetail::Http(HttpDetail {
                    status: status.as_u16(),
                    error: "asset".to_string(),
                    need: None,
                    why: None,
                    retry_after: None,
                    op: Some("asset"),
                }),
            ));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(transport_error)
    }

    /// 单张地图详情。未上榜或未过审返回 `404 not_found`。
    pub async fn map(&self, world: &str) -> Result<VrccMapDetail, VrccError> {
        Self::validate_world(world)?;
        self.request(Call {
            method: "GET",
            path: &format!("maps/{world}"),
            body: None,
            authed: false,
            key: "map",
            min_gap: Duration::from_secs(30),
            bucket: None,
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 按世界 ID 批量查地图（`GET /maps?ids=`）。
    ///
    /// 官方上限 20 张，**只返回在榜上的那些**——所以返回的条数可能少于传入
    /// 的数量，调用方不能按下标一一对应，要按 `world` 去配。这里在发请求前
    /// 就先截到 20，免得服务器回 `400`。
    ///
    /// 空列表直接发一个空请求没有意义，也会白白占一次读取，所以本地短路。
    pub async fn maps(&self, worlds: &[String]) -> Result<VrccBoard, VrccError> {
        // 形状不对的 ID 直接丢掉：它要拼进查询串，留着只会污染整条路径。
        // 批量接口的语义本来就是「有多少算多少」，少一张不算失败。
        let valid: Vec<String> = worlds
            .iter()
            .filter(|world| Self::is_valid_world(world))
            .cloned()
            .collect();
        let Some(path) = batch_maps_path(&valid) else {
            // 空列表没什么可问的，也不该白占一次读取。
            return Ok(VrccBoard::default());
        };
        self.request(Call {
            method: "GET",
            path: &path,
            body: None,
            authed: false,
            // 和单张查询共用一个间隔：两处问的都是同一份地图数据，
            // 分开计时只会让连点两次时多打一次网络。
            key: "map",
            min_gap: Duration::from_secs(30),
            bucket: None,
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 一个社团的公开页（`GET /clubs/{code}`，1.18.0 起）。**不需要令牌。**
    ///
    /// 内容和 `https://vrcc.cc/c/<短码>` 一样（就是社团放在 VRChat 群组链接
    /// 栏里的那个链接）。`{code}` 是群组短码，形如 `KIPFEL.2104`，**大小写
    /// 都收**（实测 `kipfel.2104` 同样 200）。
    ///
    /// 短码就是 VRChat 群组自己的 `shortCode` + `.` + `discriminator`，
    /// 所以群组详情里现成能拼出来，不需要额外接口反查。
    ///
    /// # 404 是常态
    ///
    /// 没入驻、审核中、下架、只给成员看、不展示的，**一律同样的
    /// `404 not_found`**——看不出是哪一种。调用方应当把它当成「这个群组没有
    /// 星遇同频社团」这一条**正常结论**，而不是错误弹给玩家。
    ///
    /// 服务器最多 1 分钟更新一次，所以本机最小间隔给 60 秒：同一分钟里反复
    /// 打开同一个群组详情，问到的本来就是同一份。
    pub async fn club(&self, code: &str) -> Result<VrccClubResponse, VrccError> {
        Self::validate_club_code(code)?;
        let path = format!("clubs/{}", encode_query_value(code.trim()));
        self.request(Call {
            method: "GET",
            path: &path,
            body: None,
            authed: false,
            key: "club",
            min_gap: Duration::from_secs(60),
            bucket: None,
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 所有公开社团（`GET /clubs`，1.19.2 起）。
    ///
    /// 不用令牌，只要 `client` 头。每个社团的字段和 [`Self::club`] 的 `club`
    /// **完全一样**——同一个 [`VrccClub`] 契约。
    ///
    /// # 顺序改不了
    ///
    /// 服务器把顺序定死：点赞多的在前，再按群组人数、短码。接口**没有 `sort`
    /// 参数**，客户端只能翻页，不能重排。
    ///
    /// # 没有点赞数
    ///
    /// 榜是按点赞排的，但 [`VrccClub`] 里**不带 `likes`**。要显示「为什么排这个
    /// 位置」只能再用带令牌的 [`Self::my_clubs`] 叠一层。
    ///
    /// 服务器最多 5 分钟更新一次（和地图榜一样），本机最小间隔给 30 秒——比
    /// 服务器刷新快没意义，但比 `club()` 的 60 秒宽松，因为翻页会连着来。
    pub async fn clubs(&self, limit: u32, offset: u32) -> Result<VrccClubs, VrccError> {
        let path = clubs_path(limit, offset);
        self.request(Call {
            method: "GET",
            path: &path,
            body: None,
            authed: false,
            key: "clubs",
            min_gap: Duration::from_secs(30),
            bucket: None,
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 玩家所在星池里能看到的社团（`GET /me/clubs`，1.18.0 起）。
    ///
    /// **需要 `clubs` 敏感权限**。只读：不算社团曝光、不消红点。16–17 岁星池
    /// 或社团功能关着时回 `404 club_off`，那是**预期的结果**，调用方该把
    /// 「没有社团」和「网络坏了」分开说。
    ///
    /// 算第二份读（`MoreReads`，每天 1000 次）。顺序由服务器定（点赞多的在
    /// 前，再按群组人数），网页上每天换一次，接口不换。
    ///
    /// 本机最小间隔 60 秒：这个列表一天里几乎不变，而打开任何群组详情都可能
    /// 要查它一次，问太勤只是白花额度。
    pub async fn my_clubs(&self) -> Result<VrccMyClubs, VrccError> {
        self.request(Call {
            method: "GET",
            path: "me/clubs",
            body: None,
            authed: true,
            key: "my_clubs",
            min_gap: Duration::from_secs(60),
            bucket: Some(QuotaBucket::MoreReads),
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 一个社团的社团页（`GET /me/clubs/{code}`，1.18.0 起）。
    ///
    /// **需要 `clubs` 敏感权限。** 比公开的 [`Self::club`] 多简介、公告、
    /// 正在开的公开房间、下次活动。
    ///
    /// 玩家看不到的社团（不在 TA 的星池、只给成员看但 TA 没挂、不展示、
    /// 下架）回 `404 club_gone`；16–17 岁星池回 `404 club_off`。这两种和
    /// 公开页的 `not_found` 一样都算**正常结论**。
    ///
    /// 限流键按短码分开：连续打开三个社团的详情不该互相卡住。
    pub async fn my_club(&self, code: &str) -> Result<VrccMyClubResponse, VrccError> {
        Self::validate_club_code(code)?;
        let path = format!("me/clubs/{}", encode_query_value(code.trim()));
        self.request(Call {
            method: "GET",
            path: &path,
            body: None,
            authed: true,
            key: "my_club",
            min_gap: Duration::from_secs(30),
            bucket: Some(QuotaBucket::MoreReads),
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 公开地图页（`GET /maps/{world}/page`，1.13.0 起）。
    ///
    /// 不需要令牌。不在榜上或没过审时 `404 not_found`——这是正常状态
    /// （这张图还没上榜），调用方应当把它当成「没有这一页」，而不是报故障。
    ///
    /// 限流器按路径分组，所以世界与排序自然分开。若它和所有别的世界共用同一个
    /// 键，连续打开第三个世界详情时那张图会直接没有分数——「按资源查询的接口用了
    /// 全局键」的典型症状，和接口本身的频率无关。服务器最多 5 分钟才更新一次，
    /// 5 秒足够挡手抖。
    pub async fn map_page(&self, world: &str) -> Result<VrccMapPage, VrccError> {
        Self::validate_world(world)?;
        self.request(Call {
            method: "GET",
            path: &format!("maps/{world}/page"),
            body: None,
            authed: false,
            key: "map_page",
            min_gap: Duration::from_secs(5),
            bucket: None,
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 带令牌的地图页（`GET /me/maps/{world}/page`）。需要 `maps` 普通权限。
    ///
    /// 比公开版 [`Self::map_page`] 多出：**完整短评列表**、地图介绍、正在开的
    /// 房，以及我自己打的分与写的短评。公开版按官方说明「短评和介绍不给」，
    /// 所以想看别人的评价只能走这一条。
    ///
    /// `sort` 只认 `hot` / `new`，别的值由服务器按默认处理。
    ///
    /// 算个人每天的次数，记在**第二份读**（`MoreReads`）上：官方《频率限制》把
    /// 「地图榜和地图页」和「全部介绍」都划进第二份，和提醒、连线、所在地图那一份
    /// （第一份）分开算。
    ///
    /// 限流器按**路径**分组（见 [`limiter_key`]），所以世界和排序自然分开：这条
    /// 按世界查，若所有世界共用一个 30 秒的槽，20 秒内打开第二个世界的页签、
    /// 或者切一次 `hot` / `new`，就直接「请求太频繁」。而前端本来就有 5 分钟
    /// `staleTime`，Rust 侧再压一个全局间隔只会误伤。
    pub async fn my_map_page(
        &self,
        world: &str,
        sort: Option<&str>,
    ) -> Result<VrccMyMapPage, VrccError> {
        Self::validate_world(world)?;
        let sort = sort.map(str::trim).filter(|sort| !sort.is_empty());
        let path = match sort {
            Some(sort) => format!("me/maps/{world}/page?sort={sort}"),
            None => format!("me/maps/{world}/page"),
        };
        // `hot` / `new` 是两份不同的列表，不同世界更是不同的资源：限流器按
        // 「接口 + 路径」分组（见 [`limiter_key`]），所以这里不用担心串味。
        self.request(Call {
            method: "GET",
            path: &path,
            body: None,
            authed: true,
            key: "my_map_page",
            min_gap: Duration::from_secs(3),
            // 第二份读。官方《频率限制》把「地图榜和地图页」划进第二份，
            // 和提醒、连线、所在地图那一份（第一份）分开算。
            bucket: Some(QuotaBucket::MoreReads),
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 给地图打分 / 写短评（`POST /me/maps/{world}/rating`）。需要 `maps.rate`。
    ///
    /// ## `text` 的三种语义必须分清
    ///
    /// 见 [`VrccRatingText`]：不传 = 保留原短评，传空串 = 删短评，传文字 = 更新。
    /// 这里**只在需要时才把 `text` 放进 JSON**，放一个 `null` 进去与官方语义不同。
    ///
    /// ## 这是写操作
    ///
    /// 官方要求这类写操作「由玩家本人**每次**主动点击触发」，所以：
    /// - 不做本地限流的「合并请求」——两次点击就是两次意图；
    /// - `min_gap` 仍然设一个小的值，只为挡住连点造成的重复提交。
    ///
    /// 返回更新后的地图状态（形状同 `GET /me/maps/{world}`），调用方可以直接
    /// 用它刷新界面，不必再发一次读请求。
    pub async fn rate_map(
        &self,
        world: &str,
        score: i64,
        text: &VrccRatingText,
    ) -> Result<VrccMyMap, VrccError> {
        Self::validate_world(world)?;
        let score = score.clamp(VrccRatingInput::MIN_SCORE, VrccRatingInput::MAX_SCORE);
        let mut body = json!({ "score": score });
        if let Some(text) = text.as_body_text() {
            body["text"] = json!(text);
        }
        let map = self
            .request(Call {
                method: "POST",
                path: &format!("me/maps/{world}/rating"),
                body: Some(body),
                authed: true,
                key: "rate_map",
                // 连点防重：服务器自己的额度才是权威，这里只挡手抖。
                min_gap: Duration::from_secs(2),
                // 打分有自己的一份额度（官方《频率限制》：「和网页地图榜共用每天的
                // 打分与添图次数」），**不算读**。
                bucket: None,
                limiter: None,
                bypass_gate: false,
            })
            .await?;
        self.forget_after_rating(world);
        Ok(map)
    }

    /// 打分、删分之后，哪些读到的东西已经过时：这张图的地图页（两份排序都算）、
    /// 地图状态、公开地图页，以及列着「我打的分」的玩家视角地图榜和我打过分的图。
    fn forget_after_rating(&self, world: &str) {
        self.forget_replies(|key, path| {
            path.contains(world) || matches!(key, "my_board" | "my_maps")
        });
    }

    /// 删掉自己给这张图的评分（`POST /me/maps/{world}/rating/delete`）。
    ///
    /// 需要 `maps.rate`。官方没说返回什么，所以这里只判断成功与否，
    /// 调用方刷新页面即可。
    pub async fn delete_map_rating(&self, world: &str) -> Result<(), VrccError> {
        Self::validate_world(world)?;
        let response = self
            .request::<serde_json::Value>(Call {
                method: "POST",
                path: &format!("me/maps/{world}/rating/delete"),
                body: None,
                authed: true,
                key: "delete_map_rating",
                min_gap: Duration::from_secs(2),
                // 删除算写（官方《频率限制》：「各种删除和合照合计 50」），
                // 和读的 1000 次不是同一份额度。
                bucket: None,
                limiter: None,
                bypass_gate: false,
            })
            .await?;
        // 只要拿到了 2xx 就算成功；正文可能是什么都没有，或者是一份地图状态。
        drop(response);
        self.forget_after_rating(world);
        Ok(())
    }

    /// 给短评或地图介绍投票（`POST /me/map-votes`）。需要 `maps.write`。
    ///
    /// ## 三条规则都在服务端
    ///
    /// - `v` 为 `0` 表示取消（用 [`VrccVote::None`]）；
    /// - **不能给自己的内容投票**，服务端会拒。界面应提前禁用，别让玩家
    ///   点了才发现不行；
    /// - `id` 是 `VrccMapRate::id`，不是世界 ID，也不是星号。
    ///
    /// 返回更新后的 `up` / `down` 与我当前的票，调用方直接拿来刷新那一行即可。
    ///
    /// 写操作：不做重试（重试等于投两次）。
    pub async fn vote_map_content(
        &self,
        kind: VrccVoteKind,
        id: i64,
        vote: VrccVote,
    ) -> Result<VrccVoteResult, VrccError> {
        let result = self
            .request(Call {
                method: "POST",
                path: "me/map-votes",
                body: Some(json!({
                    "kind": kind.as_str(),
                    "id": id,
                    "v": vote.as_body_value(),
                })),
                authed: true,
                key: "vote_map_content",
                // 连点防重。服务端自己也有额度，这里只挡手抖。
                min_gap: Duration::from_secs(1),
                // 赞踩算写（官方《频率限制》：「地图赞踩 200」，每份授权每天），
                // 不占读。
                bucket: None,
                limiter: None,
                bypass_gate: false,
            })
            .await?;
        // 票数显示在带令牌的地图页里。`id` 是短评编号，看不出是哪张图，
        // 所以所有地图页一起清。
        self.forget_replies(|key, _| key == "my_map_page");
        Ok(result)
    }

    /// 公告和停站（1.12.0 起）。不用令牌，停站时也能问，服务器缓存 1 分钟。
    ///
    /// 旧服务器没有这个接口，回答 `404 not_found`。
    pub async fn announcements(&self) -> Result<VrccAnnouncements, VrccError> {
        self.request(Call {
            method: "GET",
            path: "announcements",
            body: None,
            authed: false,
            key: "announcements",
            min_gap: Duration::from_secs(30),
            bucket: None,
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    // -- 贴纸素材 ----------------------------------------------------------

    /// 所有贴纸和材质名（`GET /stickers`）。
    ///
    /// 这个接口**不需要 `X-Client`**（官方 `security: []`）。
    /// `send_request` 会统一带一个客户端标识——官方只是"不要求"，并没有
    /// "禁止"，多带一个不改变回答，所以这里不为此开特例；真正影响的是
    /// `authed: false`：不带令牌。
    ///
    /// 官方说缓存一天，所以本地间隔也放长：这份数据一天内不会变。
    pub async fn stickers(&self) -> Result<VrccStickers, VrccError> {
        self.request(Call {
            method: "GET",
            path: "stickers",
            body: None,
            authed: false,
            key: "stickers",
            min_gap: Duration::from_secs(3600),
            bucket: None,
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    // -- 设备授权 ----------------------------------------------------------

    /// 申请连接码。
    ///
    /// 官方定义（`POST /api/v1/device/code`）：正文可以不带；要指定权限就带
    /// `scope`。不写 `scope` 时按客户端登记状态取默认值（`dev-` 是
    /// `profile notifications maps`，`profile` 总会加上）。
    ///
    /// 所以这里传 `None` 表示「用官方默认」，而不是「不要权限」。
    ///
    /// **`scope` 走查询串，不走 JSON 正文。** 这一点是实测出来的，和官方
    /// `openapi.yaml` 不一致：
    ///
    /// | 写法 | 实测结果 |
    /// | --- | --- |
    /// | 不带正文 | `200` |
    /// | JSON 正文 `{"scope": …}` | `400 bad_request` |
    /// | 查询串 `?scope=…` | `200` |
    /// | 表单 `scope=…` | `200` |
    ///
    /// 文档说 400 只表示 `invalid_scope`，但要了 JSON 正文时现网回的是
    /// `bad_request`——也就是说服务器根本没解析 JSON 里的 scope。参考实现
    /// （`xingshi-app`）里「遇 400 退回查询串」的兼容逻辑是对的，这里直接
    /// 从一开始就发查询串，省掉一次注定失败的往返。
    pub async fn device_code(&self, scope: Option<&str>) -> Result<VrccDeviceCode, VrccError> {
        let path = match scope.map(str::trim).filter(|scope| !scope.is_empty()) {
            Some(scope) => format!("device/code?scope={}", encode_query_value(scope)),
            None => "device/code".to_string(),
        };
        self.request(Call {
            method: "POST",
            path: &path,
            body: None,
            authed: false,
            key: DEVICE_CODE_KEY,
            min_gap: Duration::from_secs(5),
            bucket: None,
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 用设备码换令牌。玩家还没点同意时返回 `authorization_pending`，
    /// 调用方应按 `interval` 秒后再问；`access_denied` / `expired_token` 要停下。
    pub async fn device_token(&self, device_code: &str) -> Result<VrccTokenResponse, VrccError> {
        self.request(Call {
            method: "POST",
            path: "device/token",
            body: Some(json!({ "device_code": device_code })),
            authed: false,
            key: "device_token",
            min_gap: Duration::from_secs(5),
            bucket: None,
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 断开：让服务器吊销当前令牌。
    ///
    /// **不受退避限制**（`bypass_gate`）：吊销是玩家明确要求的动作，而退避是
    /// 全局的——别的接口吃到一次 429 就会进入退避。被它挡住的话，本地令牌
    /// 已经清掉，这场授权就再也没有机会在服务器上吊销了。
    pub async fn disconnect(&self) -> Result<(), VrccError> {
        let _: serde_json::Value = self
            .request(Call {
                method: "POST",
                path: "disconnect",
                body: None,
                authed: true,
                key: "disconnect",
                min_gap: Duration::from_secs(5),
                bucket: None,
                limiter: None,
                bypass_gate: true,
            })
            .await?;
        Ok(())
    }

    // -- 个人接口 ----------------------------------------------------------

    /// 增量轮询提醒。首次不传 `since`，之后回传上次的 `last`。
    ///
    /// 官方硬性要求两次之间至少 25 秒，这里在限流层强制。它**没有每天上限**
    /// （官方《频率限制》），所以不扣任何一份额度——后台轮询就靠它。
    pub async fn notifications(&self, since: Option<i64>) -> Result<VrccNotifications, VrccError> {
        let path = match since {
            Some(cursor) => format!("me/notifications?since={cursor}"),
            None => "me/notifications".to_string(),
        };
        self.request(Call {
            method: "GET",
            path: &path,
            body: None,
            authed: true,
            key: "notifications",
            min_gap: NOTIFICATIONS_MIN_GAP,
            bucket: None,
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 最近一天的最后几条提醒（第一份读）。
    pub async fn recent_notifications(
        &self,
        mark: bool,
    ) -> Result<VrccRecentNotifications, VrccError> {
        self.request(Call {
            method: "GET",
            path: recent_path(mark),
            body: None,
            authed: true,
            key: "recent",
            min_gap: Duration::from_secs(10),
            bucket: Some(QuotaBucket::Reads),
            // 标已读的那次单独一个槽：和不标已读的那次共用资源槽的话，先拉了列表、
            // 再切到这一页时就会被挡在本地，已读推不上去。同 `crossings/probe`。
            limiter: mark.then_some("recent/mark"),
            bypass_gate: false,
        })
        .await
    }

    /// 几处未读数（1.13.0 起，`notifications` 权限）。读了不标已读。
    ///
    /// 每个授权每天 1500 次，单独计数，官方建议 1–2 分钟问一次。旧服务器没有
    /// 这个接口，回答 `404 not_found`。
    pub async fn unread(&self) -> Result<VrccUnread, VrccError> {
        self.request(Call {
            method: "GET",
            path: "me/unread",
            body: None,
            authed: true,
            key: "unread",
            min_gap: Duration::from_secs(25),
            bucket: Some(QuotaBucket::Unread),
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 通知中心（1.13.0 起，`notifications` 权限，第二份读）。读了不标已读。
    pub async fn notes(&self) -> Result<VrccNotes, VrccError> {
        self.request(Call {
            method: "GET",
            path: "me/notes",
            body: None,
            authed: true,
            key: "notes",
            min_gap: Duration::from_secs(10),
            bucket: Some(QuotaBucket::MoreReads),
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 当前所在地图在地图榜上的状态与我自己的评分（第一份读）。
    ///
    /// 需要 `maps`（普通权限，`dev-` 就能用）。这是官方插件「所在地图」那块
    /// 的数据来源：能看分、看人数、看自己打了几分。
    ///
    /// `world` 只能是 `wrld_…`，不要带房间实例号。
    ///
    /// 限流器按路径分组，所以不同世界互不相干。若它们共用一个键，玩家在 10 秒
    /// 内打开第二个世界详情就会被自己的限流器挡住。
    ///
    /// **形状不对的 ID 直接拒掉**，见 [`Self::validate_world`]。
    pub async fn my_map(&self, world: &str) -> Result<VrccMyMap, VrccError> {
        Self::validate_world(world)?;
        self.request(Call {
            method: "GET",
            path: &format!("me/maps/{world}"),
            body: None,
            authed: true,
            key: "my_map",
            min_gap: Duration::from_secs(3),
            bucket: Some(QuotaBucket::Reads),
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 玩家视角地图榜（`GET /me/board`）。需要 `maps` 普通权限。
    ///
    /// 比公开榜多出「我的视角」：去过几次、热评、名次、介绍首行。
    /// `sort` 走 `top` / `week` / `new` / `went`——比公开榜多一个 `went`。
    ///
    /// `q` 非空时走搜索（地图名或作者）。**搜索会覆盖排序**：服务器此时把回答
    /// 里的 `sort` 改写成 `"find"`，请求带的 `sort` 只影响限流分组，不影响结果
    /// 顺序。所以调用方不要拿回答里的 `sort` 回填界面状态。
    ///
    /// # 限流键里保留 `q`
    ///
    /// 按 [`limiter_key`] 的既定规则，只有**不改变资源身份**的参数（`mark` /
    /// `limit`）才剥掉。`q` 换一个词就是另一份列表，属于改身份，所以留在键里
    /// 意味着**每个新的搜索词都是新的槽**——本地不会因为上一个词刚查过就把这次
    /// 拦下来。
    ///
    /// 代价是「搜索不受本地 60 秒间隔保护」。这是有意的：把 `q` 剥掉会让玩家
    /// 改一个词就撞上假的「请求太频繁」，那比多发一次请求糟得多。防重复请求
    /// 交给界面侧的输入防抖，不在这里做。
    pub async fn my_board(
        &self,
        sort: VrccMyBoardSort,
        q: Option<&str>,
    ) -> Result<VrccMyBoard, VrccError> {
        let path = my_board_path(sort, q);
        // `top` / `week` / `new` / `went` 是四份不同的列表；限流器按「接口 +
        // 路径」分组，切排序不会撞上上一份的间隔。
        self.request(Call {
            method: "GET",
            path: &path,
            body: None,
            authed: true,
            key: "my_board",
            min_gap: Duration::from_secs(60),
            // 地图榜是第二份读（官方《频率限制》：「地图榜和地图页」）。
            bucket: Some(QuotaBucket::MoreReads),
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 我打过分的地图（`GET /me/maps`）。最近的在前，最多 200 张。
    ///
    /// 需要 `maps` 普通权限，计入个人每天的读取次数。这个接口只有 `GET`；
    /// 同一个路径的 `POST` 是「添加地图」，要 `maps.write` 敏感权限，本客户端
    /// 不申请，所以那条路走不通是**设计如此**，不是漏了。
    pub async fn my_maps(&self) -> Result<VrccRatedMaps, VrccError> {
        self.request(Call {
            method: "GET",
            path: "me/maps",
            body: None,
            authed: true,
            key: "my_maps",
            min_gap: Duration::from_secs(30),
            bucket: Some(QuotaBucket::Reads),
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 今天这一轮路过的星 + 轮次进度。需要 `today` 权限（敏感，要审核）。
    ///
    /// 它是「网页上的动作」，算第二份读；停站时回答 `503 stop`。
    pub async fn today(&self) -> Result<VrccToday, VrccError> {
        self.request(Call {
            method: "GET",
            path: "me/today",
            body: None,
            authed: true,
            key: "today",
            min_gap: Duration::from_secs(30),
            bucket: Some(QuotaBucket::MoreReads),
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 「互相连上」的星友列表（第一份读）。需要 `mutuals` 权限（敏感，要审核）。
    ///
    /// 条目里的 `vrcUser` 是对方的 VRChat 用户 ID，只在对方验证过且公开
    /// 「已验证」标志时才有。客户端拿它只能打开自己的玩家详情页，
    /// **不得代玩家发好友请求**——这条是官方的硬性合规要求。
    pub async fn mutuals(&self) -> Result<VrccMutuals, VrccError> {
        self.request(Call {
            method: "GET",
            path: "me/mutuals",
            body: None,
            authed: true,
            key: "mutuals",
            min_gap: Duration::from_secs(30),
            bucket: Some(QuotaBucket::Reads),
            limiter: None,
            bypass_gate: false,
        })
        .await
    }

    /// 星轨相交：在同房间偶遇过的玩家列表（`GET /me/crossings`）。
    ///
    /// **需要 `crossings` 敏感权限**。`dev-` 标识要不到，会回
    /// `403 scope_unreviewed`——这是预期行为，不是故障，所以调用方应当把
    /// `MissingScope` 当成「功能没开通」处理，而不是「连接坏了」。
    ///
    /// `mark` 只在玩家真的打开这一页时为 true：它会把已读同步到账号，
    /// 网页和手机上的红点跟着消。后台只为算数字时**必须**传 false，
    /// 否则等于替玩家把提醒标成看过了。
    ///
    /// 计入第一份读的额度（与 `/me/mutuals`、`/me/notifications/recent`
    /// 共享每授权每天 1000 次）。
    pub async fn crossings(&self, mark: bool) -> Result<VrccCrossings, VrccError> {
        self.request(Call {
            method: "GET",
            path: crossings_path(mark),
            body: None,
            authed: true,
            key: "crossings",
            min_gap: Duration::from_secs(30),
            bucket: Some(QuotaBucket::Reads),
            // 标已读的那次单独一个槽，理由见 `recent_notifications`：手动刷新过列表
            // （不标已读）再切回这一页时，不能被那次挡下。
            limiter: mark.then_some("crossings/mark"),
            bypass_gate: false,
        })
        .await
    }

    /// 只看「星轨相交对这个玩家可见吗」，不标已读。
    ///
    /// 请求和 [`Self::crossings`]`(false)` 完全一样，区别只在**本地限流槽**：
    /// 这条用独立的一个槽（`crossings/probe`）。
    ///
    /// 为什么必须分开：探测发生在打开页面时，紧接着玩家就可能切到这个页签，
    /// 那一次要带 `mark=1` 把红点清掉。如果两者共用一个 30 秒的槽，探测会
    /// 把玩家那次读取挡在本地——请求根本发不出去，`mark` 永远到不了服务器，
    /// 红点消不掉，玩家还会看到一句「请求太频繁」，而那句话是他点开页签时
    /// 弹出来的，跟他的动作对不上。
    ///
    /// 探测本身仍然受 30 秒间隔约束（同一个页面反复重挂不会连发）。
    pub async fn crossings_probe(&self) -> Result<VrccCrossings, VrccError> {
        self.request(Call {
            method: "GET",
            path: crossings_path(false),
            body: None,
            authed: true,
            key: "crossings",
            min_gap: Duration::from_secs(30),
            bucket: Some(QuotaBucket::Reads),
            limiter: Some("crossings/probe"),
            bypass_gate: false,
        })
        .await
    }

    /// 星轨相交的完整星卡（`GET /me/crossings/cards`，1.13.0 起）。
    ///
    /// 与 [`Self::crossings`] 的区别：那个是「谁跟我擦过肩」的列表，这个是
    /// 网页「连向我 → 星轨相交」那种完整星卡，另带今天还能连几颗。
    ///
    /// **算第二份读的额度**（`MoreReads`）。注意是**共享**而不是独立：官方
    /// 《频率限制》把它和 `/me/today`、`/me/notes`、地图榜一起划进「每个授权
    /// 每天 1000 次」那一份，不是另一份额度。所以拉取频率要比列表更克制——
    /// 每拉一次星卡，就少一次留给地图页的次数。
    ///
    /// 只有带 `?mark=1` 才标成看过；调用方实际只用 `left` / `max` 两个整数，
    /// 这层没做裁剪是因为服务器只给整份 `cards[]`。
    pub async fn crossing_cards(&self, mark: bool) -> Result<VrccCrossingCards, VrccError> {
        self.request(Call {
            method: "GET",
            path: crossing_cards_path(mark),
            body: None,
            authed: true,
            key: "crossings/cards",
            // 列表是 30 秒，星卡**更宽**：它是第二份读，别跟列表抢窗口。
            min_gap: Duration::from_secs(60),
            bucket: Some(QuotaBucket::MoreReads),
            // 标已读的那次单独一个槽，理由见 `recent_notifications`。
            limiter: mark.then_some("crossings/cards/mark"),
            bypass_gate: false,
        })
        .await
    }
}

/// 公开社团列表的路径。
///
/// `offset=0` 时**不带这个参数**：第一页的和 1.19.2 之前的老写法一致，少一个
/// 查询参数就少一处和服务器对不齐的机会。
fn clubs_path(limit: u32, offset: u32) -> String {
    let limit = limit.clamp(1, 100);
    if offset == 0 {
        format!("clubs?limit={limit}")
    } else {
        format!("clubs?limit={limit}&offset={offset}")
    }
}

/// 地图榜的路径。总榜不带 `sort`，和旧服务器上的写法一模一样。
fn boards_path(limit: u32, sort: VrccBoardSort) -> String {
    match sort {
        VrccBoardSort::Top => format!("boards/maps?limit={limit}"),
        other => format!("boards/maps?limit={limit}&sort={}", other.as_query()),
    }
}

/// 模型榜的路径。同 [`boards_path`]：总榜不带 `sort`。
///
/// 复用 [`VrccBoardSort::as_query`] 而不是再写一份 `match`：两个榜单的排序取值
/// 是同一套（`top` / `week` / `new`），分开写迟早会有一边漏改。
fn models_board_path(limit: u32, sort: VrccBoardSort) -> String {
    match sort {
        VrccBoardSort::Top => format!("boards/models?limit={limit}"),
        other => format!("boards/models?limit={limit}&sort={}", other.as_query()),
    }
}

/// 批量查地图的路径；一条有效 ID 都没有时返回 `None`，让调用方短路。
///
/// 官方一次最多 20 张，超了直接 `400 bad_request`，所以在本地先截断。
/// 空串和重复项也在这里去掉：调用方传来的列表来自别处，不能指望它干净。
fn batch_maps_path(worlds: &[String]) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let ids: Vec<&str> = worlds
        .iter()
        .map(|world| world.trim())
        .filter(|world| !world.is_empty() && seen.insert(*world))
        .take(MAX_BATCH_WORLDS)
        .collect();
    if ids.is_empty() {
        return None;
    }
    Some(format!("maps?ids={}", ids.join(",")))
}

/// 「最近的提醒」的路径。
///
/// 1.9.0 起 `?mark` 按字面算：只有 `1` / `true` 才标已读，`?mark=0` 不标。
/// 这里本来就只在要标的时候带 `?mark=1`，不标的时候整个参数都不带，新旧
/// 服务器上都对。
fn recent_path(mark: bool) -> &'static str {
    if mark {
        "me/notifications/recent?mark=1"
    } else {
        "me/notifications/recent"
    }
}

/// 星轨相交列表的路径。
///
/// 与 `recent_path` 同一套 `mark` 约定：要标才带 `?mark=1`，不标整个参数
/// 都不带。这样 1.9.0 前后两种服务器语义都正确。
fn crossings_path(mark: bool) -> &'static str {
    if mark {
        "me/crossings?mark=1"
    } else {
        "me/crossings"
    }
}

/// 星轨相交完整星卡的路径。`mark` 约定同上。
fn crossing_cards_path(mark: bool) -> &'static str {
    if mark {
        "me/crossings/cards?mark=1"
    } else {
        "me/crossings/cards"
    }
}

/// 解析一份成功回答的正文。
fn decode<T: DeserializeOwned>(text: &str) -> Result<T, VrccError> {
    serde_json::from_str(text).map_err(|error| {
        // 不把正文塞进错误里：里面可能有玩家数据。
        //
        // 归传输层而不是服务器层：拿到了 2xx 却不是 JSON，通常意味着
        // 中间设备改写了响应（门户页、劫持），或者代理返回了 HTML
        // 错误页。那不是「服务器按协议拒绝了」，排查方向完全不同。
        VrccError::new(
            ErrorLayer::Transport,
            ErrorDetail::Decode(error.to_string()),
        )
    })
}

/// 一次限流预占的还槽守卫：`acquire` 占到槽就交出它，没被 `keep` 就在丢掉时把这次
/// 预占撤回去（[`RateLimiter::undo`]：只撤自己那一截，预占前就在的间隔照旧）。
///
/// 撤回的时机分两段：
///
/// - **请求还没发出去**：在 `acquire` 里等着时被取消、睡醒发现开始退避了、连接
///   建不起来——服务器一次都没见过，所有请求都撤。
/// - **请求发出去以后**：只有可复用的读撤。本地间隔只在「间隔内有回答可给」时才有
///   意义：槽占着、手上却没有能直接给出的回答，下一次读只能拿到本机的「太频繁」，
///   而重构前（客户端每次新建、间隔从不生效）它会真的去问。所以一条规则：**这次没
///   存下能直接复用的回答，就把槽还回去**——正文解不出来、读到一半断了、服务器回
///   错误（429 只在退避或额度闸门挡着重试时）、`no-store`、`no-cache`、在等响应时
///   被取消，都在这一条里。写操作、提醒轮询发出去以后就不撤：服务器可能已经收到，
///   它们的间隔是官方的。
///
/// ## 这条规则的后果：带令牌接口实际上没有本地最小间隔
///
/// 2026-11-16 在运行中的应用里确认：个人接口的 `200` 一律回
/// `Cache-Control: no-store`，也没有 `ETag` / `Last-Modified`。于是它们的每次读都
/// 会走到「没存下 → 还槽」这一条，间隔内的第二次读不会被挡住：挡着连发的只剩每分钟
/// `120` 次的窗口、每天额度，以及同一个读的在途排队。
///
/// 这是**有意的选择**，由 #23 评审提出、#24 拍板：服务器既然不让复用，就不该拿一个
/// 「手上没有回答可给」的间隔去换玩家的「请求太频繁」。开放数据不受影响——回答能
/// 存下，间隔照旧生效。
///
/// 服务器哪天开始给个人接口发可复用的回答（或给出 `ETag`），这条会自动回到「间隔内
/// 复用」，不用改代码。
#[must_use]
struct SlotRelease<'a> {
    client: &'a VrccClient,
    held: Option<Reserved>,
}

/// 守卫手上的那次预占：槽名、预占到的时间、预占前槽上记的时间。
struct Reserved {
    slot: String,
    at: Instant,
    previous: Option<Instant>,
}

impl<'a> SlotRelease<'a> {
    /// 手上没有预占（绕过限流的请求）：什么都不撤。
    fn none(client: &'a VrccClient) -> Self {
        Self { client, held: None }
    }

    /// 槽留着：存下了能直接给出的回答，或者这次请求已经发出去、不该撤。
    fn keep(&mut self) {
        self.held = None;
    }
}

impl Drop for SlotRelease<'_> {
    fn drop(&mut self) {
        if let Some(Reserved { slot, at, previous }) = self.held.take() {
            let mut state = self.client.state.lock().unwrap_or_else(|e| e.into_inner());
            state.limiter.undo(&slot, at, previous);
        }
    }
}

/// 这次请求占哪个限流槽：调用方给了显式的 `limiter` 就用它，否则按资源算
/// （见 [`limiter_key`]）。
fn slot_of(call: &Call<'_>) -> String {
    match call.limiter {
        Some(slot) => slot.to_string(),
        None => limiter_key(call.key, call.path),
    }
}

/// 去掉路径里的 `mark=1`：标已读的读和不标已读的读是同一份资源。
fn without_mark(path: &str) -> String {
    let (base, query) = match path.split_once('?') {
        Some((base, query)) => (base, query),
        None => return path.to_string(),
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.is_empty() && *pair != "mark=1")
        .collect();
    if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

/// 令牌的指纹，只用来给「间隔内复用」的回答分账号。
///
/// 不存令牌本身；末位置 1，保证不会和开放数据的 `0` 撞上。
fn token_scope(token: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    token.hash(&mut hasher);
    hasher.finish() | 1
}

/// 本地限流槽的键：接口标识 + **路径，只保留表达资源的那些查询参数**。
///
/// ## 为什么不能只用接口标识
///
/// `me/maps/{world}/page` 对不同的世界、不同的排序是**四个不同的资源**，共用
/// 一个键意味着「刚看过 A 就看不到 B」——玩家看到的是「请求太频繁」，而真实
/// 原因只是两次请求之间要隔三秒。`06e6aeb` 就是修这个的。
///
/// ## 为什么也不能整条剥掉查询串
///
/// 那样会把 `sort` 和 `ids` 一起并进来，于是：
///
/// - `me/board?sort=top|week|new|went` 共用一个 60 秒的槽——切一次排序要等
///   一分钟才能看到，而排序之间本来就是不同的列表；
/// - `me/maps/{world}/page?sort=hot|new` 并成一个 3 秒的槽；
/// - 批量查地图时 `maps?ids=...` 换一批 ID 也算同一个资源，榜单换排序后补
///   封面会自己撞自己。
///
/// 这是从另一侧把 `06e6aeb` 修掉的问题又撞了回来。
///
/// ## 剥哪些
///
/// 只剥**不改变资源身份**的参数：`mark`——同一条列表，这次要不要顺手标已读。
///
/// `limit` **不剥**，尽管它也不改变「这是哪份列表」。槽和「间隔内复用」的回答
/// 缓存是一对搭档：槽说「这个资源刚问过」，缓存说「这份回答给你」。`limit` 留在
/// 缓存键里（`limit=20` 的回答顶不了 100 条的请求），却曾经从槽名里剥掉，于是
/// 同一个接口用两种 `limit` 调用时，第二次会被本地间隔挡下、手上又没有能顶上的
/// 回答，只能报「请求太频繁」。两边现在给同一个答案。
///
/// 其余（`sort` / `ids` / 以后新增的）一律保留——默认「留在键里」比默认
/// 「剥掉」安全：多一个槽最多多一次请求，少一个槽会让玩家看到一个假的
/// 「请求太频繁」。
///
/// `op`（进错误详情的那份）仍然用不带路径的 `key`：那是**接口**的身份，消费方
/// 靠它区分「同一个错误码在不同接口下的不同含义」，路径里的世界 ID 不属于那里。
fn limiter_key(key: &str, path: &str) -> String {
    let (path, query) = match path.split_once('?') {
        Some((path, query)) => (path, query),
        None => (path, ""),
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            let name = pair.split('=').next().unwrap_or(pair);
            // 只剥 `mark`。`limit` 看上去也不改身份，但回答不能跨 `limit` 顶替，
            // 细节见 [`limiter_key`]。
            name != "mark"
        })
        .collect();
    if kept.is_empty() {
        format!("{key} {path}")
    } else {
        format!("{key} {path}?{}", kept.join("&"))
    }
}

/// 离北京时间下一个零点还有几秒。服务器每天的次数都在这时重置。
fn secs_until_beijing_midnight() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    secs_until_beijing_midnight_from(now)
}

fn secs_until_beijing_midnight_from(unix_secs: u64) -> u64 {
    const DAY: u64 = 86_400;
    let beijing = unix_secs + 8 * 3600;
    (DAY - beijing % DAY).max(1)
}

/// 把 scope 编成查询串可用形式。
///
/// scope 是空格分隔的权限名（profile notifications maps），查询串里空格要
/// 编成 %20（官方示例用的是 +，两者都收，%20 更不容易被别的层误解）。
fn encode_query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `GET /me/board` 的路径。
///
/// `q` 先 trim 再判空：界面上把输入框清空后传进来的可能是 `""` 或 `"   "`，
/// 两者都该走「不搜索」，而不是发一个 `q=` 让服务器去找空字符串。
///
/// 中文、空格、`&`、`=` 全部按 UTF-8 逐字节百分号编码——地图名里带 `&`
/// 并不罕见，不编码会被下游当成参数分隔符，查出来是另一个词。
fn my_board_path(sort: VrccMyBoardSort, q: Option<&str>) -> String {
    let trimmed = q.map(str::trim).filter(|value| !value.is_empty());
    match trimmed {
        Some(q) => format!(
            "me/board?sort={}&q={}",
            sort.as_query(),
            encode_query_value(q)
        ),
        None => format!("me/board?sort={}", sort.as_query()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vrcx_0_vrcc_contracts::{is_dev_client, VRCC_BUILD_CLIENT, VRCC_DEV_CLIENT};

    /// 服务器拒绝（拿到了响应，状态码不是 2xx）。
    fn http_error(status: u16, error: &str) -> VrccError {
        VrccError::new(
            ErrorLayer::Server,
            ErrorDetail::Http(HttpDetail {
                status,
                error: error.to_string(),
                need: None,
                why: None,
                retry_after: None,
                op: None,
            }),
        )
    }

    /// 构建期没注入时，解析结果必须就是开发态标识。
    ///
    /// 这条是「克隆下来直接能跑」的保证：不能解析出空串。
    fn expected_client() -> String {
        if VRCC_BUILD_CLIENT.trim().is_empty() {
            VRCC_DEV_CLIENT.to_string()
        } else {
            VRCC_BUILD_CLIENT.trim().to_string()
        }
    }

    fn client() -> VrccClient {
        VrccClient::new("dev-test", Arc::new(|| Some("tok".to_string())), None)
    }

    /// 世界 ID 必须合官方 `World` 的结构（`wrld_` 加 36 位十六进制）。
    ///
    /// 这个值要拼进 URL 路径，`wrld_x/../../me/notifications` 这种在规范化之后会
    /// 落到另一个接口上。写接口尤其不能靠「来源可信」来兜底。
    ///
    /// **大小写不管**：官方 schema 写的是小写，但大小写不影响路径安全，服务器两种
    /// 都认；玩家从别处复制来的大写 ID 不该因为格式洁癖被挡掉。
    #[test]
    fn only_world_ids_of_the_documented_shape_pass() {
        assert!(VrccClient::is_valid_world(
            "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd"
        ));
        // 大写只是不规范，不是危险，照收。
        assert!(VrccClient::is_valid_world(
            "wrld_4432EA9B-729C-46E3-8EAF-846AA0A37FDD"
        ));
        for bad in [
            "",
            "wrld_",
            "wrld_x",
            // 少一位、多一位
            "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fd",
            "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdda",
            // 多一段、路径穿越
            "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd/page",
            "wrld_x/../../me/notifications",
            "notwrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd",
        ] {
            assert!(!VrccClient::is_valid_world(bad), "{bad} 不该通过");
        }
    }

    /// 短码的形状检查。
    ///
    /// 短码会被直接拼进路径（`clubs/{code}`、`me/clubs/{code}`），而
    /// [`VrccClient::validate_path`] **只拒绝绝对地址**，不管路径穿越。
    /// 所以这一层必须自己挡：`../me/card` 拼出来的
    /// `clubs/../me/card` 在规范化之后会落到另一个接口上。
    #[test]
    fn only_club_codes_of_the_documented_shape_pass() {
        // 实测存在的两个。
        assert!(VrccClient::validate_club_code("KIPFEL.2104").is_ok());
        assert!(VrccClient::validate_club_code("VRCN.0483").is_ok());
        // 全数字的头部也合法（实测有 `520.6088`、`114514.0398` 里的 114514
        // 其实是 6 位，`520` 是 3 位）。
        assert!(VrccClient::validate_club_code("520.6088").is_ok());
        // 大小写都收：服务器两种都认。
        assert!(VrccClient::validate_club_code("kipfel.2104").is_ok());
        // 前后空白先 trim 再说。
        assert!(VrccClient::validate_club_code("  KIPFEL.2104  ").is_ok());

        for bad in [
            "",
            "   ",
            // 没有点、点多一个、尾部不是 4 位
            "KIPFEL2104",
            "KIPFEL.2104.1",
            "KIPFEL.210",
            "KIPFEL.21045",
            // 头部长度不在 3..=6
            "KI.2104",
            "KIPFELX.2104",
            // 非 ASCII、路径穿越
            "中文.2104",
            "../me/card",
            "KIPFEL.2104/../../me/card",
            "KIPFEL.2104?x=1",
        ] {
            assert!(
                VrccClient::validate_club_code(bad).is_err(),
                "{bad} 不该通过"
            );
        }
    }

    /// 形状不对的短码不会让请求跑出去（公开版与带令牌版都要挡）。
    #[tokio::test]
    async fn a_club_code_of_the_wrong_shape_never_reaches_the_network() {
        let client = client();
        for code in ["../me/card", "KIPFEL2104", ""] {
            let error = client.club(code).await.expect_err("形状不对应当本地拒绝");
            assert!(error.is_our_bug(), "{code} → {error:?}");
            let error = client
                .my_club(code)
                .await
                .expect_err("形状不对应当本地拒绝");
            assert!(error.is_our_bug(), "{code} → {error:?}");
        }
    }

    /// 形状不对的 ID 不会让请求跑出去。
    ///
    /// 这条走的是写接口：评分请求带着一个坏 ID 发出去，落点就是另一个 `me/…`
    /// 接口，而且是以玩家的名义。
    #[tokio::test]
    async fn a_world_id_of_the_wrong_shape_never_reaches_the_network() {
        let client = client();
        let error = client
            .rate_map("wrld_x/../../me/notifications", 8, &VrccRatingText::Clear)
            .await
            .expect_err("形状不对应当本地拒掉");
        assert_eq!(error.layer, ErrorLayer::Input);
        assert!(matches!(
            error.detail,
            ErrorDetail::InvalidArgument("world")
        ));
    }

    /// 路径里的 `..`、反斜杠和片段标识都进不去。
    #[test]
    fn a_path_that_climbs_out_of_the_api_prefix_is_refused() {
        for bad in [
            "http://evil.example/me",
            "//evil.example/me",
            "me/maps/wrld_x/../../me/notifications",
            "me\\maps",
            "me/maps/x#frag",
        ] {
            assert!(VrccClient::validate_path(bad).is_err(), "{bad} 不该通过");
        }
        // 正常的相对路径与查询串照常放行。
        assert!(VrccClient::validate_path("me/maps/wrld_x/page?sort=new").is_ok());
        assert!(VrccClient::validate_path("boards?limit=20&sort=hot").is_ok());
    }

    /// 起一个本地服务器，按顺序回答 `connections` 次相同的响应，返回它的基址。
    ///
    /// 用真实 socket 而不是桩：这几条测的是「**哪一份**额度被扣了」「哪个键被
    /// 限流了」，而那是 `request()` 根据状态码、响应头和限流键做的决定，只测
    /// 纯函数碰不到。
    async fn serve(status_line: &'static str, body: &'static str, connections: usize) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            for _ in 0..connections {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });
        format!("http://{addr}/")
    }

    /// 把客户端指到那个一次性服务器上。
    fn client_at(base: String) -> VrccClient {
        let mut client = client();
        client.base = base;
        client
    }

    /// 「这一份今天用完了」只停它自己，不能顺手把第一份读也停掉。
    ///
    /// 地图榜和地图页在官方《频率限制》里属于**第二份读**，不能和提醒、
    /// 连线、所在地图共用 `Reads` 桶：否则 `record_too_many` 会把吃到的 429 当成
    /// 「这一份今天没了」标记到北京时间零点——于是玩家把地图榜刷满，连提醒都
    /// 收不到。
    #[tokio::test]
    async fn burning_the_map_boards_daily_reads_does_not_stop_the_first_read_bucket() {
        let base = serve(
            "429 Too Many Requests\r\nRetry-After: 7200\r\nRateLimit-Limit: 1000\r\nRateLimit-Remaining: 0\r\nRateLimit-Reset: 7200",
            r#"{"error":"too_many"}"#,
            1,
        )
        .await;
        let client = client_at(base);

        let error = client
            .my_board(VrccMyBoardSort::Top, None)
            .await
            .expect_err("服务器回的是 429");
        // 这是**服务端**说的「这一份今天没了」，不是本机拦下的。
        assert!(
            matches!(&error.detail, ErrorDetail::Http(http) if http.status == 429),
            "{error:?}"
        );

        // 第一份读一点没受影响。
        assert!(client.spend(QuotaBucket::Reads).is_ok());
        // 第二份读按服务端的说法已经用满。
        assert!(client.spend(QuotaBucket::MoreReads).is_err());
    }

    /// 不同的世界、不同的排序，不该被同一个限流键互相挡住。
    ///
    /// 键原来是一个固定的 `"my_map_page"`、间隔 30 秒，而 `MAX_ACQUIRE_WAIT`
    /// 只有 10 秒：10 秒内打开第二个世界的页签就直接得到 `ThrottledLocally`，
    /// 界面显示成「请求太频繁」。前端本来就有 5 分钟 `staleTime`，这个全局间隔
    /// 只会误伤。键带上世界和排序之后，这三者互不相干。
    #[tokio::test]
    async fn a_second_world_and_a_second_sort_are_not_throttled_by_the_first() {
        let base = serve("200 OK", "{}", 3).await;
        let client = client_at(base);
        let first = "wrld_11111111-1111-4111-8111-111111111111";
        let second = "wrld_22222222-2222-4222-8222-222222222222";

        client
            .my_map_page(first, Some("hot"))
            .await
            .expect("第一个世界");
        // 同一个世界、另一份排序
        client
            .my_map_page(first, Some("new"))
            .await
            .expect("同一世界的另一份排序");
        // 另一个世界、同一个排序
        client
            .my_map_page(second, Some("hot"))
            .await
            .expect("第二个世界");
    }

    /// 可见性探测不能占住玩家随后那一次读取。
    ///
    /// 两条请求问的是同一份列表，路径只差一个 `?mark=1`，而 [`limiter_key`]
    /// **只剥 `mark`**——所以按默认规则它们本来就是同一个槽。探测发生在打开
    /// 页面时，紧接着玩家就可能切到这一页，那一次要带 `mark=1` 去清红点：
    /// 探测先把 30 秒的槽占住，玩家那次就被挡在本地，请求根本没发出去，
    /// `mark` 永远到不了服务器，红点消不掉，界面上还会弹一句和他动作对不上的
    /// 「请求太频繁」。
    ///
    /// 修复是给探测一个自己的槽（`Call.limiter`）。槽是 `request()` 在本地做的
    /// 决定，只测纯函数看不到它——所以这条用真实 socket 数「发出去几次」：
    /// 探测之后紧接着的那次读取必须真的发到服务器，否则 `serve` 只收到一次
    /// 连接，而这一步会拿到 `ThrottledLocally` 而不是 200。
    #[tokio::test]
    async fn the_visibility_probe_does_not_block_the_players_own_read() {
        let base = serve("200 OK", r#"{"stars":[],"n":0,"why":null,"me":null}"#, 2).await;
        let client = client_at(base);

        client.crossings_probe().await.expect("探测");
        // 探测刚发生过，这一步仍然要发得出去。
        client.crossings(true).await.expect("点开页签");
    }

    /// 写接口吃到当天的 429，不该动任何一份读额度，也不该让所有接口一起退避。
    ///
    /// 赞踩是「地图赞踩 200」、打分是「每天的打分与添图次数」、删除是「各种删除
    /// 合计 50」——都是写，和读的 1000 次毫无关系。
    #[tokio::test]
    async fn a_vote_hitting_its_daily_limit_leaves_both_read_buckets_alone() {
        let base = serve(
            "429 Too Many Requests\r\nRetry-After: 7200\r\nRateLimit-Limit: 200\r\nRateLimit-Remaining: 0\r\nRateLimit-Reset: 7200",
            r#"{"error":"too_many"}"#,
            1,
        )
        .await;
        let client = client_at(base);

        let error = client
            .vote_map_content(VrccVoteKind::Rate, 7, VrccVote::Up)
            .await
            .expect_err("服务器回的是 429");
        assert!(
            matches!(&error.detail, ErrorDetail::Http(http) if http.status == 429),
            "{error:?}"
        );

        assert!(client.spend(QuotaBucket::Reads).is_ok());
        assert!(client.spend(QuotaBucket::MoreReads).is_ok());
        // 也不该把所有接口一起退避到明天：那只会发生在「全站 429」时。
        assert_eq!(client.local_usage().backoff_remaining, Duration::ZERO);
    }

    /// 标识的来源只有两层：构建期注入 > 源码默认，**没有运行时覆盖**。
    ///
    /// 这条钉住的是「同一个二进制永远用同一个标识」。留运行时覆盖就等于允许
    /// 用配置文件冒充已审核客户端，所以这里连参数都不该有。
    #[test]
    fn client_id_is_locked_at_build_time() {
        // 解析结果就是构建期那一层，不接受任何外部输入。
        assert_eq!(resolve_client_id(), expected_client());
        // 没注入时必须落到开发态，不能是空串——空 `X-Client` 每个请求都 403。
        if VRCC_BUILD_CLIENT.trim().is_empty() {
            assert_eq!(resolve_client_id(), VRCC_DEV_CLIENT);
        }
    }

    /// 无论注入成什么样，**发出去的那个值**必须是干净的非空串。
    ///
    /// 这是唯一真正重要的不变式：`option_env!` 拿到的是构建环境的原始串，
    /// CI 拼接时带空白或换行很常见，而带空白的 `X-Client` 会让**每一个**
    /// 接口都 `403 client_unknown`，报错里又看不出是空格的问题。
    ///
    /// 注意断言的是解析结果、不是常量本身：注入了纯空白时常量确实还是
    /// `"   "`，但 `resolve_client_id` 会回落到开发态——这种「常量脏、结果
    /// 干净」的状态是合法的，不该让构建失败。
    #[test]
    fn resolved_client_id_is_always_clean() {
        let resolved = resolve_client_id();
        assert!(!resolved.is_empty(), "X-Client must never be empty");
        assert_eq!(
            resolved,
            resolved.trim(),
            "resolved X-Client must not carry surrounding whitespace"
        );
    }

    /// 开发态判定只看 `dev-` 前缀，这是官方的判定方式。
    #[test]
    fn dev_client_is_detected_by_prefix() {
        assert!(is_dev_client(VRCC_DEV_CLIENT));
        assert!(is_dev_client("  dev-anything  "));
        assert!(!is_dev_client("app_0123456789abcdef"));
        assert!(!is_dev_client(""));
    }

    /// 不写 `scope` 时自助登记的 `app_…` 只有 `profile`，所以两种标识都要
    /// 显式地要；`dev-` 要了敏感权限会连码都拿不到。
    #[test]
    fn device_auth_scope_depends_on_the_client_kind() {
        use vrcx_0_vrcc_contracts::{
            device_auth_scope, VRCC_DEVICE_AUTH_SCOPE, VRCC_DEV_DEVICE_AUTH_SCOPE,
        };
        assert_eq!(
            device_auth_scope(VRCC_DEV_CLIENT),
            VRCC_DEV_DEVICE_AUTH_SCOPE
        );
        assert_eq!(
            device_auth_scope("app_0123456789abcdef"),
            VRCC_DEVICE_AUTH_SCOPE
        );
        assert!(!VRCC_DEV_DEVICE_AUTH_SCOPE.contains("mutuals"));
        // 敏感权限只出现在正式标识那一串；`dev-` 要了会连码都拿不到。
        assert!(VRCC_DEVICE_AUTH_SCOPE.contains("crossings"));
        assert!(!VRCC_DEV_DEVICE_AUTH_SCOPE.contains("crossings"));
        // 社团（1.18.0）同样是敏感权限：只给正式标识。
        assert!(VRCC_DEVICE_AUTH_SCOPE.contains("clubs"));
        assert!(!VRCC_DEV_DEVICE_AUTH_SCOPE.contains("clubs"));
        // `clubs` 必须作为**独立词**出现——写成 `clubss` 或 `xclubs`
        // 都能过 `contains`，但服务器会整串判权限，等于没申请。
        assert!(VRCC_DEVICE_AUTH_SCOPE
            .split(' ')
            .any(|item| item == "clubs"));
        for scope in [VRCC_DEVICE_AUTH_SCOPE, VRCC_DEV_DEVICE_AUTH_SCOPE] {
            assert!(scope.starts_with("profile notifications maps"));
            // 权限之间用**单个空格**分隔：多空格或逗号都不会让服务器认错，
            // 但会让「按空格 split 再比对」的本地判断（上面那条）出错。
            assert!(!scope.contains("  "));
        }
    }

    #[test]
    fn scope_grants_reads_the_token_scope() {
        use vrcx_0_vrcc_contracts::scope_grants;
        assert!(!scope_grants(Some("profile"), "notifications"));
        assert!(scope_grants(
            Some("profile notifications maps"),
            "notifications"
        ));
        assert!(scope_grants(Some("profile,notifications"), "notifications"));
        assert!(scope_grants(Some("profile+notifications"), "notifications"));
        // 只认整项，不认前缀：`maps.rate` 不等于 `maps`。
        assert!(!scope_grants(Some("profile maps.rate"), "maps"));
        // 旧存档没记权限：交给服务器判断。
        assert!(scope_grants(None, "notifications"));
    }

    /// 缺哪些权限，决定界面说的是「重新连一次可以多要一项」还是「连接已失效」。
    #[test]
    fn missing_scopes_lists_only_what_is_actually_missing() {
        use vrcx_0_vrcc_contracts::missing_scopes;
        // 够：什么都不缺。
        assert!(missing_scopes(Some("profile notifications maps"), "profile maps").is_empty());
        // 多出来的权限不触发任何结论。
        assert!(missing_scopes(Some("profile maps crossings"), "profile").is_empty());
        // 缺两项就报两项，顺序按要求那一串。
        assert_eq!(
            missing_scopes(Some("profile"), "profile notifications crossings"),
            vec!["notifications", "crossings"]
        );
        // 只认整项：`maps.rate` 不等于 `maps`。
        assert_eq!(
            missing_scopes(Some("profile maps.rate"), "maps"),
            vec!["maps"]
        );
        // 旧存档没记权限：不知道就不下结论，别凭空让人重新授权。
        assert!(missing_scopes(None, "profile notifications").is_empty());
    }

    /// 开发态常量本身必须符合官方的 `dev-` 前缀要求——
    /// 否则每个请求都会吃 `403 client_unknown`。
    #[test]
    fn dev_client_constant_is_a_valid_dev_id() {
        assert!(
            VRCC_DEV_CLIENT.starts_with("dev-"),
            "dev client must start with `dev-`, got {VRCC_DEV_CLIENT:?}"
        );
        assert!(
            VRCC_DEV_CLIENT.len() > 4,
            "dev client must have a name after the prefix"
        );
    }

    #[test]
    fn absolute_paths_are_rejected() {
        assert!(matches!(
            VrccClient::validate_path("https://evil.example/x"),
            Err(VrccError {
                layer: ErrorLayer::Input,
                detail: ErrorDetail::AbsolutePath(_)
            })
        ));
        assert!(matches!(
            VrccClient::validate_path("//evil.example/x"),
            Err(VrccError {
                layer: ErrorLayer::Input,
                detail: ErrorDetail::AbsolutePath(_)
            })
        ));
        assert!(VrccClient::validate_path("boards/maps?limit=5").is_ok());
        assert!(VrccClient::validate_path("me/notifications?since=1").is_ok());
    }

    #[test]
    fn gone_error_is_detected() {
        let error = http_error(404, "gone");
        assert!(error.is_gone());
        assert!(error.needs_reauth());
    }

    #[test]
    fn star_gone_and_not_found_do_not_need_reauth() {
        // 官方错误码表明确区分：对方不在了 / 地图不在榜上，令牌都没事。
        for code in ["star_gone", "not_found"] {
            let error = http_error(404, code);
            assert!(!error.is_gone(), "{code} 不该被当成令牌失效");
            assert!(!error.needs_reauth(), "{code} 不该触发重新授权");
        }
    }

    #[test]
    fn reauth_matches_official_error_table() {
        // 官方表里这些都要玩家重新授权。
        for (status, code) in [
            (403u16, "client_mismatch"),
            (403, "client_unknown"),
            (403, "client_off"),
            (403, "client_missing"),
            (403, "scope"),
            (403, "scope_unreviewed"),
            (401, "token"),
        ] {
            let error = VrccError::new(
                ErrorLayer::Server,
                ErrorDetail::Http(HttpDetail {
                    status,
                    error: code.to_string(),
                    need: Some("card".to_string()),
                    why: None,
                    retry_after: None,
                    op: None,
                }),
            );
            assert!(error.needs_reauth(), "{status} {code} 应要求重新授权");
        }
        // 这些不需要。
        assert!(!http_error(429, "too_many").needs_reauth());
        let map_not_went = VrccError::new(
            ErrorLayer::Server,
            ErrorDetail::Http(HttpDetail {
                status: 403,
                error: "map_not_went".to_string(),
                need: Some("verify".to_string()),
                why: None,
                retry_after: None,
                op: None,
            }),
        );
        assert!(!map_not_went.needs_reauth(), "打分前置条件不要求重新授权");
    }

    #[test]
    fn device_flow_errors_are_classified() {
        let pending = http_error(400, "authorization_pending");
        assert!(pending.is_authorization_pending());
        assert!(!pending.is_device_auth_terminal());

        for code in ["access_denied", "expired_token"] {
            let error = http_error(400, code);
            assert!(error.is_device_auth_terminal(), "{code}");
            assert!(!error.is_authorization_pending(), "{code}");
        }
    }

    /// 钉住 `scope` 的编码方式。
    ///
    /// 相关内容：`device/code` 的 `scope` 必须走查询串——实测 JSON 正文里带
    /// `scope` 会被回 `400 bad_request`，查询串与表单都回 `200`。
    ///
    /// scope 是空格分隔的权限名，查询串里空格必须编码。编码函数写成净函数，
    /// 这样不依赖网络也能钉住行为。
    #[test]
    fn scope_is_encoded_for_the_query_string() {
        assert_eq!(
            encode_query_value("profile notifications maps"),
            "profile%20notifications%20maps"
        );
        // 权限名本身只含安全字符，不该被过度编码。
        assert_eq!(encode_query_value("maps.rate"), "maps.rate");
        // 其它字符一律编码，避免拼进 URL 时被误解。
        assert_eq!(encode_query_value("a&b=c"), "a%26b%3Dc");
    }

    /// `device/code` 的 400 分类：不是终态、也不是「等待中」。
    ///
    /// 现网对「JSON 正文带 scope」回的是 `bad_request`，对不认识的权限名回
    /// `invalid_scope`。两者都不是必须放弃授权流程的错误——玩家可以改了
    /// scope 再试，和 `access_denied` / `expired_token` 那种必须停下的不同。
    #[test]
    fn device_code_bad_request_is_recoverable() {
        for code in ["invalid_scope", "bad_request"] {
            let error = http_error(400, code);
            assert!(!error.is_device_auth_terminal(), "{code}");
            assert!(!error.is_authorization_pending(), "{code}");
            assert!(!error.needs_reauth(), "{code}");
        }
        // 缺的权限要能带出去，界面才能告诉玩家是哪个名字写错了。
        let invalid_scope = VrccError::new(
            ErrorLayer::Server,
            ErrorDetail::Http(HttpDetail {
                status: 400,
                error: "invalid_scope".to_string(),
                need: Some("maps.rate".to_string()),
                why: None,
                retry_after: None,
                op: None,
            }),
        );
        assert_eq!(invalid_scope.needed_scope(), Some("maps.rate"));
    }

    #[test]
    fn too_soon_is_not_a_quota_problem() {
        // 官方把 429 分成两种：too_soon 是问勤了，too_many 是额度用完。
        // 前者不该被当成需要长时间退避的配额问题。
        //
        // 两者都是 `Http { status: 429, .. }`，靠 `error` 串区分；
        // 退避时长由 `retry_after` 带出去，不再拼进中文文案里。
        let too_soon = VrccError::new(
            ErrorLayer::Server,
            ErrorDetail::Http(HttpDetail {
                status: 429,
                error: "too_soon".to_string(),
                need: None,
                why: None,
                retry_after: Some(25),
                op: Some("notifications"),
            }),
        );
        assert!(!too_soon.needs_reauth());
        assert!(too_soon.to_string().contains("25"));

        let too_many = http_error(429, "too_many");
        assert!(!too_many.needs_reauth());
    }

    #[test]
    fn recent_notification_urls_are_relative_and_get_prefixed() {
        use vrcx_0_vrcc_contracts::VrccRecentNotificationItem;
        // 官方 recent 的 url 是相对路径（以 /match/ 开头）。
        let item = VrccRecentNotificationItem {
            url: Some("/match/?tab=inbox".to_string()),
            ..Default::default()
        };
        assert_eq!(item.absolute_url(), "https://vrcc.cc/match/?tab=inbox");

        // 已经是绝对地址就原样用。
        let absolute = VrccRecentNotificationItem {
            url: Some("https://vrcc.cc/match/?x=1".to_string()),
            ..Default::default()
        };
        assert_eq!(absolute.absolute_url(), "https://vrcc.cc/match/?x=1");

        // 没有 url 回到站点首页。
        let none = VrccRecentNotificationItem::default();
        assert_eq!(none.absolute_url(), "https://vrcc.cc/");
    }

    /// 没拿到过服务端配额时，`spend` 一律放行。
    ///
    /// 按本地自然日预扣到 1000 就自己停下是错的：官方按北京时间
    /// 零点重置，本地推算的日期对非 UTC+8 的玩家必然错开，结果就是「服务端
    /// 明明还有余量，客户端自己先拒绝」。现在宁可让服务端回 429。
    #[tokio::test]
    async fn spend_passes_when_no_server_quota_is_known() {
        let client = client();
        assert!(client.spend(QuotaBucket::Reads).is_ok());
        assert!(client.local_usage().reads.is_none());
    }

    /// 服务端说过余量是 0 时，不再把请求发出去。
    #[tokio::test]
    async fn spend_stops_once_the_server_says_the_day_is_used_up() {
        let client = client();
        client.note_quota(
            QuotaBucket::Reads,
            ServerQuota {
                limit: 1000,
                remaining: 0,
                reset_secs: 3600,
            },
        );
        let error = client.spend(QuotaBucket::Reads).unwrap_err();
        // 这一份额度按服务端上一次的头已经用满：本机拦下，请求没发出去。
        assert_eq!(error.layer, ErrorLayer::Client);
        assert!(error.is_throttled(), "{error:?}");
    }

    /// `spend` 与 `refund` 必须对称：请求没发出去时不能白占一次额度。
    #[test]
    fn refunding_a_spend_restores_the_server_count() {
        let client = client();
        client.note_quota(
            QuotaBucket::Reads,
            ServerQuota {
                limit: 1000,
                remaining: 10,
                reset_secs: 3600,
            },
        );
        let before = client.local_usage().reads.expect("server quota").0;
        client.spend(QuotaBucket::Reads).expect("spend");
        let after = client.local_usage().reads.expect("server quota").0;
        assert_eq!(after, before + 1);
        client.refund(QuotaBucket::Reads);
        assert_eq!(client.local_usage().reads.expect("server quota").0, before);
    }

    /// 没预扣就退还，不该把余量撑到超过上限。
    #[test]
    fn refunding_never_exceeds_the_server_limit() {
        let client = client();
        client.note_quota(
            QuotaBucket::Reads,
            ServerQuota {
                limit: 1000,
                remaining: 1000,
                reset_secs: 3600,
            },
        );
        client.refund(QuotaBucket::Reads);
        let (used, limit) = client.local_usage().reads.expect("server quota");
        assert_eq!(used, 0);
        assert_eq!(limit, 1000);
    }

    /// `/device/code` 不再有本地计数，也不再本地拦截。
    ///
    /// 那个额度是**按网络出口 IP** 算的（每天 30 个，同一路由下所有人共用），
    /// 本客户端无从得知、也不该假装知道。玩家换网络本来能继续用，本地拦着
    /// 就成了凭空拒绝。
    #[test]
    fn device_code_is_not_counted_locally() {
        let client = client();
        let usage = client.local_usage();
        // 快照里已经没有这一项了——不存在「本客户端今日申请了几个码」这个数。
        assert_eq!(usage.reads, None);
        assert_eq!(usage.unread, None);
    }

    #[test]
    fn body_over_2kb_is_rejected() {
        // 官方限制 POST 正文 2 KB，客户端在发送前就拦。
        assert!(
            matches!(
                check_body_size(MAX_BODY_BYTES + 1),
                Err(ref error) if error.is_our_bug()
            ),
            "超 1 字节就该拦，且要标成我们自己的 bug"
        );
        // 正好等于上限要通过：官方说的是「最大 2 KB」，边界不算超。
        assert!(check_body_size(MAX_BODY_BYTES).is_ok());
        assert!(check_body_size(0).is_ok());
        // 断言上限本身没被手滑改小。
        assert_eq!(MAX_BODY_BYTES, 2048);
    }

    // -- 1.9.0–1.14.0：限流响应头、错误码、按字面算的 mark ----------------

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, value.parse().unwrap());
        }
        map
    }

    fn api_error(status: u16, code: &str) -> VrccError {
        http_error(status, code)
    }

    #[test]
    fn rate_headers_are_read_when_present() {
        let rate = RateHeaders::from_headers(&headers(&[
            ("Retry-After", "42"),
            ("RateLimit-Limit", "1000"),
            ("RateLimit-Remaining", "588"),
            ("RateLimit-Reset", "3600"),
        ]));
        assert_eq!(rate.retry_after_secs, Some(42));
        assert_eq!(
            rate.quota,
            Some(ServerQuota {
                limit: 1000,
                remaining: 588,
                reset_secs: 3600
            })
        );
        assert_eq!(rate.quota.unwrap().used(), 412);
    }

    #[test]
    fn missing_or_broken_rate_headers_mean_nothing() {
        // 旧服务器（1.8.x）什么都不给。
        assert_eq!(
            RateHeaders::from_headers(&HeaderMap::new()),
            RateHeaders::default()
        );
        // 写坏的头不该让请求失败，也不该被当成「用完了」。
        let rate = RateHeaders::from_headers(&headers(&[
            ("Retry-After", "Wed, 21 Oct 2026 07:28:00 GMT"),
            ("RateLimit-Limit", "lots"),
            ("RateLimit-Remaining", "1"),
        ]));
        assert_eq!(rate, RateHeaders::default());
        // 只有上限、没有余量，等于没说。
        let half = RateHeaders::from_headers(&headers(&[("RateLimit-Limit", "1000")]));
        assert_eq!(half.quota, None);
    }

    /// 同一个 `busy` 串在 409 和 429 下含义相反，只能按状态码分开。
    ///
    /// 重构后这个判断不再写死在 client 里（早先有个 `classify_status_error`
    /// 专门做这件事，等于把业务语义焊在了网络层）。现在错误只忠实记录
    /// `Http { status, error }`，判断落在 `is_not_ready()` 和
    /// `classify_vrcc_error()` 上。这条测试钉住「消费者仍能分清」：
    /// 409 是「星卡还在生成」，429 是「连接功能暂停到次日」。
    #[test]
    fn busy_is_told_apart_by_status_code() {
        // 409：这一轮的星卡还在生成，等一下就好。
        let not_ready = http_error(409, "busy");
        assert!(not_ready.is_not_ready());
        assert_eq!(not_ready.layer, ErrorLayer::Server);

        // 429：全站连接功能暂停到次日。同一个串，但**不是** is_not_ready。
        let connect_paused = http_error(429, "busy");
        assert!(!connect_paused.is_not_ready());
        assert_eq!(connect_paused.layer, ErrorLayer::Server);

        // 两者都不该被当成令牌问题。
        for error in [not_ready, connect_paused] {
            assert!(!error.needs_reauth());
            assert!(!error.is_session_lost());
        }

        // 别的码不会被 busy 规则误伤。
        for (code, error) in [
            (429u16, "too_many"),
            (429, "too_soon"),
            (429, "quota"),
            (409, "yq_wait"),
            (409, "msg_wait"),
            (503, "stop"),
            (403, "scope"),
        ] {
            assert!(!http_error(code, error).is_not_ready(), "{code} {error}");
        }
    }

    /// `too_soon` 只是问勤了，不该触发长时间退避。
    ///
    /// 它不是独立变体：错误统一是 `Http { status: 429, error:
    /// "too_soon", retry_after }`，等待时长由 `retry_after` 带给消费方。
    #[test]
    fn too_soon_does_not_trigger_backoff() {
        let client = client();
        let rate = RateHeaders {
            retry_after_secs: Some(7),
            quota: None,
        };
        client.record_too_many("too_soon", rate, None);
        assert!(client.local_usage().backoff_remaining.is_zero());
        // 旧服务器没给头也不能退避。
        client.record_too_many("too_soon", RateHeaders::default(), None);
        assert!(client.local_usage().backoff_remaining.is_zero());
    }

    #[test]
    fn minute_limit_backs_off_exactly_as_told() {
        let client = client();
        let rate = RateHeaders {
            retry_after_secs: Some(60),
            quota: None,
        };
        client.record_too_many("too_many", rate, None);
        let backoff = client.local_usage().backoff_remaining;
        assert!(backoff > Duration::from_secs(55) && backoff <= Duration::from_secs(60));
        // 第二次照样按服务器说的等，不再自己翻倍。
        client.record_too_many("too_many", rate, None);
        assert!(client.local_usage().backoff_remaining <= Duration::from_secs(60));
    }

    #[test]
    fn old_server_429_without_retry_after_still_backs_off() {
        let client = client();
        client.record_too_many("too_many", RateHeaders::default(), None);
        assert!(client.local_usage().backoff_remaining > Duration::from_secs(55));
    }

    #[test]
    fn a_used_up_day_stops_only_that_quota() {
        let client = client();
        // 第二份读今天用完了：服务器回 429，余量 0，`Retry-After` 到北京时间零点。
        let rate = RateHeaders {
            retry_after_secs: Some(5 * 3600),
            quota: Some(ServerQuota {
                limit: 1000,
                remaining: 0,
                reset_secs: 5 * 3600,
            }),
        };
        client.record_too_many("too_many", rate, Some(QuotaBucket::MoreReads));
        // 不让所有接口一起退避五个小时。
        assert!(client.local_usage().backoff_remaining.is_zero());
        // 这一份在零点前不再发请求……
        assert!(client.spend(QuotaBucket::MoreReads).is_err());
        // ……别的几份照常。
        assert!(client.spend(QuotaBucket::Reads).is_ok());
        assert!(client.spend(QuotaBucket::Unread).is_ok());
    }

    #[test]
    fn long_retry_after_without_quota_headers_is_a_daily_limit() {
        // `device/code` 每个网络地址每天 30 个：429 只带 `Retry-After`（到零点）。
        let client = client();
        let rate = RateHeaders {
            retry_after_secs: Some(8 * 3600),
            quota: None,
        };
        client.record_too_many("too_many", rate, None);
        // 长到只能是「到零点」的等待，不该变成全局退避。
        assert!(client.local_usage().backoff_remaining.is_zero());
    }

    /// 服务端给的数直接进快照，不再覆盖什么本地计数。
    #[test]
    fn server_quota_shows_up_in_the_snapshot() {
        let client = client();
        // 还没拿到过头：什么都不显示，也绝不编一个数出来。
        let before = client.local_usage();
        assert_eq!(before.reads, None);
        assert_eq!(before.reset_secs, None);

        // 服务器说这个授权今天已经用了 412 次。
        client.note_quota(
            QuotaBucket::Reads,
            ServerQuota {
                limit: 1000,
                remaining: 588,
                reset_secs: 3600,
            },
        );
        let after = client.local_usage();
        assert_eq!(after.reads, Some((412, 1000)));
        // 重置时间来自服务端（到北京时间零点的秒数），不是本地推算的。
        assert!(after.reset_secs.is_some_and(|secs| secs <= 3600));
        // 未读数那一份还没拿到过头，不显示。
        assert_eq!(after.unread, None);

        client.note_quota(
            QuotaBucket::Unread,
            ServerQuota {
                limit: 1500,
                remaining: 1400,
                reset_secs: 3600,
            },
        );
        assert_eq!(client.local_usage().unread, Some((100, 1500)));
    }

    /// 服务器哪天把上限调高了，本地不该有任何理由拦住。
    ///
    /// 现在本地根本没有上限这个概念了，这条钉住「没有本地上限」。
    #[test]
    fn a_raised_server_limit_is_never_capped_locally() {
        let client = client();
        client.note_quota(
            QuotaBucket::Reads,
            ServerQuota {
                limit: 2000,
                remaining: 1000,
                reset_secs: 3600,
            },
        );
        assert!(client.spend(QuotaBucket::Reads).is_ok());
        assert!(client.spend(QuotaBucket::Reads).is_ok());
    }

    #[test]
    fn server_saying_none_left_stops_before_the_request() {
        let client = client();
        client.note_quota(
            QuotaBucket::Unread,
            ServerQuota {
                limit: 1500,
                remaining: 0,
                reset_secs: 600,
            },
        );
        let error = client.spend(QuotaBucket::Unread).unwrap_err();
        assert!(error.is_throttled(), "{error:?}");
    }

    #[test]
    fn expired_method_and_star_gone_are_not_token_loss() {
        // 1.9.0：签名图片失效改成 `404 expired`，`gone` 只表示令牌失效。
        // 1.11.0：方法不对是 `405 method`（原来是 `404 not_found`）。
        for (status, code) in [
            (404u16, "expired"),
            (405, "method"),
            (404, "star_gone"),
            (404, "map_gone"),
            (404, "yq_gone"),
            (404, "not_found"),
        ] {
            let error = api_error(status, code);
            assert!(!error.is_gone(), "{status} {code}");
            assert!(!error.needs_reauth(), "{status} {code}");
        }
        assert!(api_error(404, "not_found").is_not_found());
        assert!(!api_error(404, "gone").is_not_found());
        assert!(!api_error(405, "method").is_not_found());
    }

    /// `503 stop`（停站维护）不是令牌问题。
    ///
    /// 它不是独立变体，而是 `Http { status: 503, error: "stop" }`。
    /// **停站说明和恢复时间不再拼进错误文案**——那两个字段本来就能从
    /// `/announcements` 的 `stop` 拿到，错误对象只记 HTTP 事实即可。
    #[test]
    fn stop_reads_as_maintenance_not_as_reauth() {
        let stop = http_error(503, "stop");
        assert!(!stop.needs_reauth(), "停站不该让玩家重新授权");
        assert!(!stop.is_session_lost());
        assert_eq!(stop.layer, ErrorLayer::Server);
        // 文案只保留状态码与官方错误码，不含中文解释。
        let text = stop.to_string();
        assert!(text.contains("503"), "{text}");
        assert!(text.contains("stop"), "{text}");
    }

    #[test]
    fn mark_is_only_sent_when_marking() {
        // 1.9.0 起 `?mark=0` 不再标已读；这里不标时干脆不带这个参数。
        assert_eq!(recent_path(true), "me/notifications/recent?mark=1");
        assert_eq!(recent_path(false), "me/notifications/recent");
        assert!(!recent_path(false).contains("mark"));
    }

    #[test]
    fn board_paths_keep_the_old_form_for_the_top_board() {
        assert_eq!(boards_path(20, VrccBoardSort::Top), "boards/maps?limit=20");
        assert_eq!(
            boards_path(20, VrccBoardSort::Week),
            "boards/maps?limit=20&sort=week"
        );
        assert_eq!(
            boards_path(5, VrccBoardSort::New),
            "boards/maps?limit=5&sort=new"
        );
    }

    /// 社团列表的第一页不带 `offset`：和 1.19.2 之前的老写法一致。
    ///
    /// `limit` 也夹到 1–100——服务器对超出范围的写法要么按 100 处理要么回 400，
    /// 与其让它替我们猜，不如自己夹住。
    #[test]
    fn club_paths_omit_the_offset_on_the_first_page() {
        assert_eq!(clubs_path(20, 0), "clubs?limit=20");
        assert_eq!(clubs_path(20, 40), "clubs?limit=20&offset=40");
        assert_eq!(clubs_path(0, 0), "clubs?limit=1");
        assert_eq!(clubs_path(1000, 0), "clubs?limit=100");
    }

    /// 星轨相交的两个端点都要遵守「要标才带 `mark=1`」。
    ///
    /// 这条值得单独钉住：`crossings` 是**唯一**会把已读同步到玩家账号、
    /// 连带清掉网页与手机红点的读接口。后台只为算未读数时若误传 true，
    /// 等于替玩家把提醒标成看过了——那个损失不可撤销。
    #[test]
    fn crossing_paths_only_mark_when_asked() {
        assert_eq!(crossings_path(true), "me/crossings?mark=1");
        assert_eq!(crossings_path(false), "me/crossings");
        assert!(!crossings_path(false).contains("mark"));

        assert_eq!(crossing_cards_path(true), "me/crossings/cards?mark=1");
        assert_eq!(crossing_cards_path(false), "me/crossings/cards");
        assert!(!crossing_cards_path(false).contains("mark"));
    }

    /// 批量查地图的路径。
    ///
    /// 三件事必须同时成立：去掉空串、去重、截到 20 —— 官方超过 20 个直接回
    /// `400 bad_request`，而调用方传进来的列表来自别处，不能指望它已经干净。
    #[test]
    fn batch_map_paths_dedupe_and_stop_at_twenty() {
        let one = vec!["wrld_a".to_string()];
        assert_eq!(batch_maps_path(&one), Some("maps?ids=wrld_a".to_string()));

        // 空白与重复都不该进查询串。
        let noisy = vec![
            "wrld_a".to_string(),
            "  wrld_a ".to_string(),
            String::new(),
            "   ".to_string(),
            "wrld_b".to_string(),
        ];
        assert_eq!(
            batch_maps_path(&noisy),
            Some("maps?ids=wrld_a,wrld_b".to_string())
        );

        // 一条都没有时返回 None，让调用方短路，不发空请求。
        assert_eq!(batch_maps_path(&[]), None);
        assert_eq!(batch_maps_path(&[String::new()]), None);

        // 25 个只发前 20 个。
        let many: Vec<String> = (0..25).map(|index| format!("wrld_{index}")).collect();
        let path = batch_maps_path(&many).expect("should build a path");
        assert_eq!(path.matches(',').count(), MAX_BATCH_WORLDS - 1);
        assert!(path.contains("wrld_19"));
        assert!(!path.contains("wrld_20"));
    }

    /// `me/board` 的路径与搜索词编码。
    ///
    /// 中文必须按 UTF-8 逐字节编码：地图名和作者名大量是中文，直接拼进去会
    /// 让 `reqwest` 拒发或让服务器查成另一个词。
    #[test]
    fn my_board_path_encodes_the_search_term() {
        // 不搜索：只有排序。
        assert_eq!(
            my_board_path(VrccMyBoardSort::Top, None),
            "me/board?sort=top"
        );
        assert_eq!(
            my_board_path(VrccMyBoardSort::Went, None),
            "me/board?sort=went"
        );

        // 空串和纯空白都当「不搜索」。界面清空输入框后传的就是这两种。
        assert_eq!(
            my_board_path(VrccMyBoardSort::Top, Some("")),
            "me/board?sort=top"
        );
        assert_eq!(
            my_board_path(VrccMyBoardSort::Top, Some("   ")),
            "me/board?sort=top"
        );

        // 前后空白要 trim，否则服务器会拿 "  孤 " 去匹配。
        assert_eq!(
            my_board_path(VrccMyBoardSort::Top, Some("  孤 ")),
            "me/board?sort=top&q=%E5%AD%A4"
        );

        // `&` 和 `=` 不编码会被下游当成参数分隔符/键值对。
        assert_eq!(
            my_board_path(VrccMyBoardSort::New, Some("a&b=c")),
            "me/board?sort=new&q=a%26b%3Dc"
        );

        // 搜索词留在限流键里：换一个词就是另一份列表，不该被上一个词挡住。
        assert_ne!(
            limiter_key("my_board", &my_board_path(VrccMyBoardSort::Top, Some("孤"))),
            limiter_key("my_board", &my_board_path(VrccMyBoardSort::Top, Some("天")))
        );
        // 带不带搜索也是两条不同的路径。
        assert_ne!(
            limiter_key("my_board", &my_board_path(VrccMyBoardSort::Top, None)),
            limiter_key("my_board", &my_board_path(VrccMyBoardSort::Top, Some("孤")))
        );
    }

    /// 北京时间零点换算。
    ///
    /// `human_wait`（「5 小时 20 分钟」这种中文格式化）随重构删掉了：
    /// 现在错误只带 `retry_after` 秒数，文案由前端按语言渲染。
    #[test]
    fn beijing_midnight_reads_right() {
        // 2026-10-27 00:00 北京 = 2026-10-26 16:00 UTC。
        let midnight_utc = 1_793_030_400;
        assert_eq!(secs_until_beijing_midnight_from(midnight_utc - 60), 60);
        assert_eq!(secs_until_beijing_midnight_from(midnight_utc), 86_400);
    }

    /// 限流槽只剥掉「不改变资源身份」的参数。
    ///
    /// 三条相反的错误都出现过，这里一起钉住：
    ///
    /// - 用整条路径（含 `mark`）时，「刷新」和「切页签」变成两个槽，能在间隔
    ///   内连发两次；
    /// - 把整条查询串剥掉时，`sort` 和 `ids` 又被并进同一个槽——切一次排序要
    ///   等一整个 `min_gap`（地图榜是 60 秒），批量换一批 ID 也自己撞自己；
    /// - 把 `limit` 剥掉时，同一个接口的两种 `limit` 共用一个槽，而缓存里只有
    ///   另一种 `limit` 的回答，第二种调用只能拿到「太频繁」。
    #[test]
    fn limiter_key_keeps_identity_and_drops_the_rest() {
        // `mark` 不改身份：同一条列表，只是这次要不要标已读。
        assert_eq!(
            limiter_key("crossings", "me/crossings?mark=1"),
            limiter_key("crossings", "me/crossings")
        );
        // `limit` 改槽：它不改变「这是哪份列表」，但**回答不能跨 `limit` 顶替**，
        // 并成一个槽就等于保证第二种 `limit` 在间隔内拿不到东西、只能报错。
        assert_ne!(
            limiter_key("boards", "boards?limit=25"),
            limiter_key("boards", "boards?limit=50")
        );
        // `sort` 改身份：不同排序是不同列表。
        assert_ne!(
            limiter_key("my_board", "me/board?sort=top"),
            limiter_key("my_board", "me/board?sort=new")
        );
        // `ids` 改身份：换一批世界就是另一次查询。
        assert_ne!(
            limiter_key("maps", "maps?ids=wrld_a,wrld_b"),
            limiter_key("maps", "maps?ids=wrld_c,wrld_d")
        );
        // 前两行剩下的那一个具体形状，别让它悄悄变成带 `?` 的怪串。
        assert_eq!(
            limiter_key("crossings", "me/crossings?mark=1"),
            "crossings me/crossings"
        );
        assert_eq!(
            limiter_key("my_board", "me/board?sort=top"),
            "my_board me/board?sort=top"
        );
        // 没有查询串时不多出一个 `?`。
        assert_eq!(limiter_key("status", "status"), "status status");
    }

    // -- 间隔内复用回答 ----------------------------------------------------

    /// 和 [`serve`] 一样，只是数着到底来了几个连接。
    async fn serve_counted(
        body: &'static str,
        connections: usize,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        tokio::spawn(async move {
            for _ in 0..connections {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });
        (format!("http://{addr}/"), seen)
    }

    const WORLD: &str = "wrld_11111111-1111-4111-8111-111111111111";

    /// 客户端共享以后，`status` 的 30 秒间隔真的生效了。间隔内再问一次不能报
    /// 「太频繁」（那是打开过的页面切回来就报错），也不该再发请求：直接给上一次
    /// 的回答。服务器只肯接一个连接，第二次要是真发出去就会失败。
    #[tokio::test]
    async fn a_second_read_within_the_gap_reuses_the_first_reply() {
        let base = serve("200 OK", r#"{"at":1,"state":"ok"}"#, 1).await;
        let client = client_at(base);

        let first = client.status().await.expect("第一次");
        let second = client.status().await.expect("间隔内第二次直接复用");
        assert_eq!(first.state.as_deref(), Some("ok"));
        assert_eq!(second.state.as_deref(), Some("ok"));
        assert_eq!(second.at, first.at);
    }

    /// 带令牌的回答按令牌分开：换了账号，不能拿上一个账号的回答顶上。
    #[tokio::test]
    async fn a_reply_is_not_reused_for_another_token() {
        let base = serve("200 OK", "{}", 1).await;
        let token = Arc::new(std::sync::Mutex::new("tok-a".to_string()));
        let current = Arc::clone(&token);
        let mut client = VrccClient::new(
            "dev-test",
            Arc::new(move || Some(current.lock().unwrap().clone())),
            None,
        );
        client.base = base;

        client
            .my_map_page(WORLD, Some("hot"))
            .await
            .expect("账号 A");
        // 同一个令牌、间隔内：复用。
        client
            .my_map_page(WORLD, Some("hot"))
            .await
            .expect("账号 A 再问一次，复用");

        *token.lock().unwrap() = "tok-b".to_string();
        // 换了令牌：必须真的去问。服务器已经不接了，所以这一次失败。
        let error = client
            .my_map_page(WORLD, Some("hot"))
            .await
            .expect_err("账号 B 不能拿到 A 的回答");
        assert_eq!(error.layer, ErrorLayer::Transport, "{error:?}");
    }

    /// 提醒轮询带游标，复用会让后台把同一批提醒再弹一遍：间隔内照旧被本地挡下。
    #[tokio::test]
    async fn the_notification_poll_is_never_reused() {
        let base = serve("200 OK", r#"{"items":[],"last":7}"#, 1).await;
        let client = client_at(base);

        client.notifications(None).await.expect("第一次");
        let error = client
            .notifications(None)
            .await
            .expect_err("25 秒内第二次不复用");
        assert!(
            matches!(
                error.detail,
                ErrorDetail::ThrottledLocally {
                    reason: LocalThrottle::Gap,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    /// 打完分紧接着重拉地图页：不能拿到提交前的那一份，也不该被 3 秒间隔卡住。
    #[tokio::test]
    async fn rating_a_map_forgets_its_page_and_lets_the_refetch_through() {
        use std::sync::atomic::Ordering;
        let (base, seen) = serve_counted("{}", 3).await;
        let client = client_at(base);

        client
            .my_map_page(WORLD, Some("hot"))
            .await
            .expect("先看一眼");
        client
            .rate_map(WORLD, 8, &VrccRatingText::Keep)
            .await
            .expect("打分");
        let started = Instant::now();
        client
            .my_map_page(WORLD, Some("hot"))
            .await
            .expect("打完分重拉");

        assert_eq!(seen.load(Ordering::SeqCst), 3, "重拉必须真的发出去");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "间隔已经放开，不该等：{:?}",
            started.elapsed()
        );
    }

    /// 连不上的那一次没有到服务器，不该占着最小间隔：间隔内重试要真的再试一次，
    /// 而不是得到「太频繁」。
    #[tokio::test]
    async fn a_request_that_never_reached_the_server_does_not_hold_its_gap() {
        // 绑一个端口再放掉：之后连它一定被拒。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local addr");
        drop(listener);
        let client = client_at(format!("http://{addr}/"));

        let first = client.status().await.expect_err("连不上");
        assert_eq!(first.layer, ErrorLayer::Transport, "{first:?}");
        let second = client.status().await.expect_err("还是连不上");
        assert_eq!(
            second.layer,
            ErrorLayer::Transport,
            "间隔内重试应该真的再试一次：{second:?}"
        );
    }

    /// 第一个连接收下请求后一直不回答，模拟「等响应时被取消」；之后的连接照常回答。
    /// 第一个请求到达时 `received` 通知一次，测试据此丢掉还在等的那个 future。
    async fn serve_holding_first(
        body: &'static str,
    ) -> (
        String,
        Arc<tokio::sync::Notify>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let received = Arc::new(tokio::sync::Notify::new());
        let seen = Arc::new(AtomicUsize::new(0));
        let (notify, counter) = (Arc::clone(&received), Arc::clone(&seen));
        tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    notify.notify_one();
                    held.push(socket);
                    continue;
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });
        (format!("http://{addr}/"), received, seen)
    }

    /// 可复用的读在等响应时被取消（调用方丢掉了 future）：手上没有回答，槽不能
    /// 占着。`status` 的间隔是 30 秒，比 `MAX_ACQUIRE_WAIT` 长，槽要是还占着，
    /// 紧接着的那次读只能拿到「太频繁」。
    #[tokio::test]
    async fn a_read_cancelled_while_waiting_for_its_reply_gives_its_slot_back() {
        use std::sync::atomic::Ordering;
        let (base, received, seen) = serve_holding_first(r#"{"at":1,"state":"ok"}"#).await;
        let client = client_at(base);

        tokio::select! {
            result = client.status() => panic!("服务器没回答，不该返回：{result:?}"),
            () = received.notified() => {}
        }
        // 上面那个 future 已经丢掉了：请求到了服务器，回答没等到。
        let reply = client
            .status()
            .await
            .expect("取消的那次不该占着 30 秒的间隔");
        assert_eq!(reply.state.as_deref(), Some("ok"));
        assert_eq!(seen.load(Ordering::SeqCst), 2, "第二次要真的发出去");
    }

    /// 提醒轮询不一样：发出去以后被取消，服务器可能已经收到，25 秒的官方间隔
    /// 照旧挡着，不能因为取消就放开。
    #[tokio::test]
    async fn a_poll_cancelled_mid_flight_still_holds_its_gap() {
        let (base, received, _) = serve_holding_first(r#"{"items":[],"last":7}"#).await;
        let client = client_at(base);

        tokio::select! {
            result = client.notifications(None) => panic!("服务器没回答，不该返回：{result:?}"),
            () = received.notified() => {}
        }
        let error = client
            .notifications(None)
            .await
            .expect_err("25 秒内第二次轮询仍被本地挡下");
        assert!(
            matches!(
                error.detail,
                ErrorDetail::ThrottledLocally {
                    reason: LocalThrottle::Gap,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    /// 等在 `acquire` 里的请求被取消：撤掉的只能是它自己那次预占。前面那次请求
    /// 留下的间隔（这里模拟一个发出去、槽留着的请求）还该挡着下一次——既不能像
    /// 以前那样一直占着取消掉的那截，也不能连前面那段一起抹掉。
    #[tokio::test]
    async fn a_request_cancelled_while_queued_leaves_the_earlier_gap_alone() {
        let client = client_at("http://127.0.0.1:9/".to_string());
        let gap = Duration::from_secs(4);
        let budget = Duration::from_secs(10);

        let mut first = client.acquire("slot", gap, budget).await.expect("第一次");
        first.keep();
        let queued = tokio::time::timeout(
            Duration::from_millis(100),
            client.acquire("slot", gap, budget),
        )
        .await;
        assert!(
            queued.is_err(),
            "第二次要排到间隔之后，测试在它等着时丢掉它"
        );

        let wait = {
            let state = client.state.lock().unwrap();
            state
                .limiter
                .check("slot", gap, Instant::now())
                .expect_err("第一次的间隔还在")
        };
        // 以前取消掉的那截一直占着：要等将近两个间隔。
        assert!(
            wait <= gap,
            "只剩第一次的间隔，取消掉的那截已经撤了：{wait:?}"
        );
    }

    /// 排着队时开始退避了：睡醒拿到「退避中」，这次没发出去，预占要撤回去。
    #[tokio::test]
    async fn a_request_turned_away_by_backoff_after_queueing_gives_its_slot_back() {
        let client = client_at("http://127.0.0.1:9/".to_string());
        let gap = Duration::from_millis(1500);
        let budget = Duration::from_secs(10);

        let mut first = client.acquire("slot", gap, budget).await.expect("第一次");
        first.keep();
        let queued = client.acquire("slot", gap, budget);
        let backoff = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            client.state.lock().unwrap().backoff_until =
                Some(Instant::now() + Duration::from_secs(5));
        };
        let (queued, ()) = tokio::join!(queued, backoff);
        let error = queued.err().expect("睡醒碰上退避");
        assert!(
            matches!(
                error.detail,
                ErrorDetail::ThrottledLocally {
                    reason: LocalThrottle::Backoff,
                    ..
                }
            ),
            "{error:?}"
        );

        // 第一次的间隔已经过了（排着的那次就是等到它过了才醒）：撤回以后槽是空的。
        let state = client.state.lock().unwrap();
        assert_eq!(
            state.limiter.check("slot", gap, Instant::now()),
            Ok(()),
            "没发出去的那次不该还占着槽"
        );
    }

    /// 每个连接都回答 `response`，但第一个连接收下请求后先通知 `received`，等 `go`
    /// 才回答：测试借这段时间让第二个调用方排到同一个读后面。连接各自处理，第一个
    /// 卡着时后面的照样能连上、拿到回答。
    async fn serve_gated(
        response: String,
    ) -> (
        String,
        Arc<tokio::sync::Notify>,
        Arc<tokio::sync::Notify>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let received = Arc::new(tokio::sync::Notify::new());
        let go = Arc::new(tokio::sync::Notify::new());
        let seen = Arc::new(AtomicUsize::new(0));
        let (notify, gate, counter) = (Arc::clone(&received), Arc::clone(&go), Arc::clone(&seen));
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let first = counter.fetch_add(1, Ordering::SeqCst) == 0;
                let (notify, gate, response) =
                    (Arc::clone(&notify), Arc::clone(&gate), response.clone());
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    if first {
                        notify.notify_one();
                        gate.notified().await;
                    }
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        (format!("http://{addr}/"), received, go, seen)
    }

    /// 两个组件一起挂载：同一个读，第二个在第一个还在路上时就排上了。回答是
    /// `no-store`，存不下，以前第二个拿到号以后还得再发一次；现在直接拿第一个的。
    /// 之后再来的不在路上的那一批里，照样自己发——这份回答不为以后的请求留着。
    #[tokio::test]
    async fn a_read_queued_behind_the_same_read_in_flight_takes_its_reply() {
        use std::sync::atomic::Ordering;
        let (base, received, go, seen) = serve_gated(ok_with(
            "Cache-Control: no-store\r\n",
            r#"{"at":1,"state":"ok"}"#,
        ))
        .await;
        let client = client_at(base);
        let queued = Arc::new(tokio::sync::Notify::new());

        let first = client.status();
        let second = async {
            received.notified().await;
            queued.notify_one();
            client.status().await
        };
        let open_the_gate = async {
            queued.notified().await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            go.notify_one();
        };
        let (first, second, ()) = tokio::join!(first, second, open_the_gate);
        assert_eq!(first.expect("第一个").state.as_deref(), Some("ok"));
        assert_eq!(second.expect("第二个").state.as_deref(), Some("ok"));
        assert_eq!(seen.load(Ordering::SeqCst), 1, "排在后面的那个不该再发一次");

        client.status().await.expect("回答到了以后才来的");
        assert_eq!(
            seen.load(Ordering::SeqCst),
            2,
            "回答到了以后才来的照样自己发：no-store 的回答不留给以后"
        );
    }

    /// 打分前就发出去的地图页还在路上，打完分紧接着重拉：重拉不能排在它后面、拿它
    /// 分来的打分前的那一份。写操作把这个读的排队号摘掉了，重拉拿新号，自己发。
    #[tokio::test]
    async fn a_refetch_after_a_write_does_not_take_a_reply_requested_before_it() {
        use std::sync::atomic::Ordering;
        let (base, received, go, seen) =
            serve_gated(ok_with("Cache-Control: no-store\r\n", "{}")).await;
        let client = client_at(base);
        let queued = Arc::new(tokio::sync::Notify::new());

        let before = client.my_map_page(WORLD, Some("hot"));
        let write_then_refetch = async {
            received.notified().await;
            client
                .rate_map(WORLD, 8, &VrccRatingText::Keep)
                .await
                .expect("打分");
            queued.notify_one();
            client.my_map_page(WORLD, Some("hot")).await
        };
        let open_the_gate = async {
            queued.notified().await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            go.notify_one();
        };
        let (before, after, ()) = tokio::join!(before, write_then_refetch, open_the_gate);
        before.expect("打分前的那次");
        after.expect("打分后重拉");
        assert_eq!(
            seen.load(Ordering::SeqCst),
            3,
            "打分前的读、打分、打分后的重拉，三次都要到服务器"
        );
    }

    /// 分完回答，排队号就从表里摘掉：之后来的拿到的是新号，里面什么都没有，
    /// 看不到上一批的回答。
    #[tokio::test]
    async fn sharing_a_reply_retires_the_flight() {
        let client = client();
        let key = ReplyKey {
            scope: 0,
            key: "status",
            path: "status".to_string(),
        };
        let flight = client.flight(&key);
        let mut turn = flight.lock().await;
        client.share_with_queued(&key, &flight, &mut turn, "{}");

        let later = client.flight(&key);
        assert!(!Arc::ptr_eq(&flight, &later), "之后来的要拿新号");
        assert!(later.lock().await.is_none(), "新号里没有上一批的回答");
    }

    /// 会话换了令牌：带令牌的读的排队号摘掉，开放数据的留着。
    #[tokio::test]
    async fn a_session_change_retires_the_flights_of_personal_reads() {
        let client = client();
        let personal = ReplyKey {
            scope: 7,
            key: "mutuals",
            path: "me/mutuals".to_string(),
        };
        let open = ReplyKey {
            scope: 0,
            key: "status",
            path: "status".to_string(),
        };
        let (personal_flight, open_flight) = (client.flight(&personal), client.flight(&open));

        client.forget_session_state();

        assert!(!Arc::ptr_eq(&personal_flight, &client.flight(&personal)));
        assert!(Arc::ptr_eq(&open_flight, &client.flight(&open)));
    }

    /// 标已读的读带着写，不合并：两次都真的发出去。
    #[tokio::test]
    async fn reads_that_mark_as_read_are_not_shared() {
        use std::sync::atomic::Ordering;
        let (base, received, go, seen) =
            serve_gated(ok_with("Cache-Control: no-store\r\n", "{}")).await;
        let client = client_at(base);
        let queued = Arc::new(tokio::sync::Notify::new());

        let first = client.recent_notifications(true);
        let second = async {
            received.notified().await;
            queued.notify_one();
            client.recent_notifications(true).await
        };
        let open_the_gate = async {
            queued.notified().await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            go.notify_one();
        };
        let (first, second, ()) = tokio::join!(first, second, open_the_gate);
        first.expect("第一次标已读");
        second.expect("第二次标已读");
        assert_eq!(seen.load(Ordering::SeqCst), 2, "标已读的两次都要真的发出去");
    }

    // -- 服务器的缓存头 ----------------------------------------------------

    /// 按顺序回答事先写好的几份完整 HTTP 响应，并记下每次收到的请求。
    async fn serve_script(responses: Vec<String>) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let requests = Arc::clone(&seen);
        tokio::spawn(async move {
            for response in responses {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = [0u8; 4096];
                let read = socket.read(&mut buf).await.unwrap_or(0);
                requests
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..read]).to_string());
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });
        (format!("http://{addr}/"), seen)
    }

    fn ok_with(headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn cache_headers_read_max_age_minus_age_and_the_validators() {
        let mut headers = HeaderMap::new();
        headers.insert("cache-control", "public, max-age=60".parse().unwrap());
        headers.insert("age", "20".parse().unwrap());
        headers.insert("etag", "W/\"v1\"".parse().unwrap());
        let cache = CacheHeaders::from_headers(&headers);
        assert_eq!(cache.fresh_for, Some(Duration::from_secs(40)));
        assert!(!cache.no_store);
        assert_eq!(cache.validators.etag.as_deref(), Some("W/\"v1\""));
        assert_eq!(cache.validators.last_modified, None);

        let mut headers = HeaderMap::new();
        headers.insert("cache-control", "no-store".parse().unwrap());
        assert!(CacheHeaders::from_headers(&headers).no_store);
        assert_eq!(
            CacheHeaders::from_headers(&HeaderMap::new()),
            CacheHeaders::default()
        );
    }

    #[test]
    fn the_server_may_lengthen_reuse_but_never_below_the_local_gap() {
        let gap = Duration::from_secs(30);
        let fresh = |secs| CacheHeaders {
            fresh_for: Some(Duration::from_secs(secs)),
            ..CacheHeaders::default()
        };
        assert_eq!(reply_ttl(gap, false, &fresh(60)), Duration::from_secs(60));
        assert_eq!(reply_ttl(gap, false, &fresh(5)), gap);
        assert_eq!(reply_ttl(gap, false, &CacheHeaders::default()), gap);
        let no_store = CacheHeaders {
            no_store: true,
            ..fresh(600)
        };
        assert_eq!(reply_ttl(gap, false, &no_store), gap);
        // 带令牌的回答没核对过服务器的头，不延长。
        assert_eq!(reply_ttl(gap, true, &fresh(600)), gap);
    }

    /// 开放数据：服务器说 60 秒内都新鲜，本地间隔（公开地图页 5 秒）过了也接着用。
    #[tokio::test]
    async fn a_server_max_age_keeps_open_data_past_the_local_gap() {
        let page = r#"{"map":{"world":"wrld_11111111-1111-4111-8111-111111111111"}}"#;
        let (base, seen) =
            serve_script(vec![ok_with("Cache-Control: public, max-age=60\r\n", page)]).await;
        let client = client_at(base);

        client.map_page(WORLD).await.expect("第一次");
        tokio::time::sleep(Duration::from_millis(5_100)).await;
        client
            .map_page(WORLD)
            .await
            .expect("本地间隔过了，但服务器说还新鲜");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    /// 带令牌的回答不听服务器的 `max-age`：本地间隔一过就重新问。
    #[tokio::test]
    async fn a_personal_reply_does_not_follow_the_server_max_age() {
        let (base, seen) = serve_script(vec![
            ok_with("Cache-Control: private, max-age=600\r\n", "{}"),
            ok_with("Cache-Control: private, max-age=600\r\n", "{}"),
        ])
        .await;
        let client = client_at(base);

        client
            .my_map_page(WORLD, Some("hot"))
            .await
            .expect("第一次");
        tokio::time::sleep(Duration::from_millis(3_100)).await;
        client
            .my_map_page(WORLD, Some("hot"))
            .await
            .expect("间隔过了，重新问");
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    /// 带 `ETag` 的回答过期以后发条件请求；服务器回 `304` 就接着用手上那份。
    #[tokio::test]
    async fn a_stale_reply_with_an_etag_is_revalidated_and_reused_on_304() {
        let (base, seen) = serve_script(vec![
            ok_with("ETag: \"v1\"\r\n", r#"{"sort":"hot"}"#),
            "HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_string(),
        ])
        .await;
        let client = client_at(base);

        let first = client
            .my_map_page(WORLD, Some("hot"))
            .await
            .expect("第一次");
        // 等本地间隔（3 秒）过去，回答过期。
        tokio::time::sleep(Duration::from_millis(3_100)).await;
        let second = client
            .my_map_page(WORLD, Some("hot"))
            .await
            .expect("304 也是成功");

        assert_eq!(second.sort, first.sort);
        assert_eq!(second.sort.as_deref(), Some("hot"));
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1]
                .to_ascii_lowercase()
                .contains("if-none-match: \"v1\""),
            "第二次要带上校验值：{}",
            requests[1]
        );
    }

    /// 同一个读两处同时发起（两个组件一起挂载、开发模式下 effect 跑两遍）：
    /// 第二个要等第一个的回答，而不是撞上本地间隔报「太频繁」。
    #[tokio::test]
    async fn two_identical_reads_at_once_share_one_request() {
        let (base, seen) = serve_counted(r#"{"at":1,"state":"ok"}"#, 1).await;
        let client = client_at(base);

        let (first, second) = tokio::join!(client.status(), client.status());
        assert_eq!(first.expect("第一个").state.as_deref(), Some("ok"));
        assert_eq!(
            second.expect("第二个等第一个的回答").state.as_deref(),
            Some("ok")
        );
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// 先刷新了列表（不标已读），紧接着切到这一页（标已读）：标已读那次必须
    /// 真的发出去；再切回来一次，被自己的间隔挡下时拿上一份顶上，不报错、
    /// 也不再请求。
    #[tokio::test]
    async fn a_mark_read_goes_out_after_a_plain_read_and_falls_back_inside_its_gap() {
        use std::sync::atomic::Ordering;
        let (base, seen) = serve_counted("{}", 3).await;
        let client = client_at(base);

        client.crossings(false).await.expect("先刷新列表");
        client.crossings(true).await.expect("切到这一页，标已读");
        assert_eq!(seen.load(Ordering::SeqCst), 2, "标已读那次要真的发出去");

        client
            .crossings(true)
            .await
            .expect("间隔内再切回来：拿上一份，不报太频繁");
        assert_eq!(seen.load(Ordering::SeqCst), 2, "间隔内不再请求");
    }

    /// 标已读之后，未读数的那份回答过时了：下一次读未读数要马上去问。
    #[tokio::test]
    async fn a_mark_read_forgets_the_unread_counts() {
        use std::sync::atomic::Ordering;
        let (base, seen) = serve_counted("{}", 3).await;
        let client = client_at(base);

        client.unread().await.expect("先看一眼未读数");
        client.crossings(true).await.expect("标已读");
        let started = Instant::now();
        client.unread().await.expect("再看未读数");

        assert_eq!(seen.load(Ordering::SeqCst), 3, "未读数要重新问");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// 一次带令牌的请求只读一次令牌：缓存分账号用的和请求头里带的是同一个。
    #[tokio::test]
    async fn a_personal_request_reads_the_token_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let base = serve("200 OK", "{}", 1).await;
        let reads = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&reads);
        let mut client = VrccClient::new(
            "dev-test",
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Some("tok".to_string())
            }),
            None,
        );
        client.base = base;

        client.my_map(WORLD).await.expect("请求");
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    /// 打开页面时先探测（`crossings/probe` 槽），紧接着切到星轨相交页（标已读），
    /// 离开后 30 秒内再回来又探测一次：第三次不能被探测自己的槽挡下、手上却没
    /// 有回答可给。
    #[tokio::test]
    async fn a_probe_after_a_mark_read_still_has_a_reply() {
        let (base, seen) =
            serve_script(vec![ok_with("", r#"{"n":1}"#), ok_with("", r#"{"n":2}"#)]).await;
        let client = client_at(base);

        client.crossings_probe().await.expect("打开页面时探测");
        client.crossings(true).await.expect("切到这一页，标已读");
        let again = client
            .crossings_probe()
            .await
            .expect("30 秒内再探测：不能报太频繁");
        // 用的是标已读那次（更新的）回答，不是第一次探测那份。
        assert_eq!(again.n, Some(2));
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    /// 标已读的读手上有上一份时，不为间隔干等：直接用上一份。
    #[tokio::test]
    async fn a_mark_read_inside_its_gap_does_not_wait() {
        use std::sync::atomic::Ordering;
        let (base, seen) = serve_counted("{}", 2).await;
        let client = client_at(base);

        client.recent_notifications(true).await.expect("第一次");
        let started = Instant::now();
        client
            .recent_notifications(true)
            .await
            .expect("10 秒内再切回来");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "不该等满间隔：{:?}",
            started.elapsed()
        );
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    /// 服务器回了错误（例如 `dev-` 标识拿到的 `403 scope`）：间隔内再问一次，
    /// 调用方看到的必须还是服务器的回答，不能变成本机的「太频繁」。
    #[tokio::test]
    async fn a_server_error_does_not_turn_the_next_try_into_too_frequent() {
        let body = r#"{"error":"scope","need":"mutuals"}"#;
        let refused = format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (base, seen) = serve_script(vec![refused.clone(), refused]).await;
        let client = client_at(base);

        let first = client.mutuals().await.expect_err("第一次 403");
        let second = client.mutuals().await.expect_err("第二次也该是 403");
        for error in [&first, &second] {
            assert!(
                matches!(&error.detail, ErrorDetail::Http(http) if http.status == 403),
                "{error:?}"
            );
        }
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn no_store_and_no_cache_are_read_apart() {
        let mut headers = HeaderMap::new();
        headers.insert("cache-control", "private, no-cache".parse().unwrap());
        let cache = CacheHeaders::from_headers(&headers);
        assert!(cache.no_cache && !cache.no_store && cache.has_control);

        let mut headers = HeaderMap::new();
        headers.insert("cache-control", "no-store".parse().unwrap());
        let cache = CacheHeaders::from_headers(&headers);
        assert!(cache.no_store && !cache.no_cache);

        assert!(!CacheHeaders::from_headers(&HeaderMap::new()).has_control);
    }

    /// `no-store` 的回答不存：间隔内再读要真的去问，而且不能被本地间隔挡成「太频繁」。
    #[tokio::test]
    async fn a_no_store_reply_is_not_kept_and_does_not_hold_the_gap() {
        // `status` 的本地间隔是 30 秒，超过等待上限：槽不还的话第二次只能报太频繁。
        let body = r#"{"at":1,"state":"ok"}"#;
        let (base, seen) = serve_script(vec![
            ok_with("Cache-Control: no-store\r\n", body),
            ok_with("Cache-Control: no-store\r\n", body),
        ])
        .await;
        let client = client_at(base);

        client.status().await.expect("第一次");
        client
            .status()
            .await
            .expect("间隔内第二次：去问服务器，不报太频繁");
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    /// `no-cache` 加 `ETag`：每次复用前都先验证。服务器的 `304` 不重复
    /// `Cache-Control` 时也一样——第三次读仍然要带着 `If-None-Match` 去问。
    #[tokio::test]
    async fn a_no_cache_reply_is_revalidated_every_time_even_after_a_bare_304() {
        let body = r#"{"at":1,"state":"ok"}"#;
        let bare_304 = "HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .to_string();
        let (base, seen) = serve_script(vec![
            ok_with("Cache-Control: no-cache\r\nETag: \"v1\"\r\n", body),
            bare_304.clone(),
            bare_304,
        ])
        .await;
        let client = client_at(base);

        client.status().await.expect("第一次");
        client.status().await.expect("第二次：验证后复用");
        client.status().await.expect("第三次：还是要验证");

        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 3, "每次都要先问服务器");
        for request in &requests[1..] {
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("if-none-match: \"v1\""),
                "{request}"
            );
        }
    }

    /// 标已读的读不发条件请求：服务器回 `304` 的话可能就不执行「标已读」了。
    #[tokio::test]
    async fn a_mark_read_never_sends_a_conditional_request() {
        let (base, seen) = serve_script(vec![
            // `no-cache` 让这份回答立刻需要验证（否则要等 30 秒间隔过去）。
            ok_with("Cache-Control: no-cache\r\nETag: \"v1\"\r\n", "{}"),
            ok_with("Cache-Control: no-cache\r\nETag: \"v1\"\r\n", "{}"),
        ])
        .await;
        let client = client_at(base);

        client.crossings(true).await.expect("第一次标已读");
        client.crossings(true).await.expect("再标一次");

        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            !requests[1].to_ascii_lowercase().contains("if-none-match"),
            "标已读的读不能带校验值：{}",
            requests[1]
        );
    }

    // -- 每个可复用的读，间隔内都还拿得到东西 ------------------------------

    /// 把「同一个读连做两次」写成一条断言。
    ///
    /// 正文由类型的 `Default` 序列化而来：必填字段都有值，所以一定解得回来，也
    /// 不用跟着 schema 手写一份 fixture（改了字段这里不用动）。
    ///
    /// 断言两件事：两次都成功，且**第二次一个请求都没发**。后者才是真正的重点——
    /// 复用要是没生效，玩家的额度已经被偷偷花掉了，界面却看不出来。
    ///
    /// 参数是「方法名 + 实参」而不是闭包：闭包返回的 future 借着自己那个参数，
    /// 同一份字面量被调用两次时推断不出足够的生命周期。
    macro_rules! reused_twice {
        ($ty:ty, $method:ident ( $($arg:expr),* $(,)? )) => {{
            let body = serde_json::to_string(&<$ty>::default()).expect("默认值应能序列化");
            let response = ok_with("", &body);
            let (base, seen) = serve_script(vec![response.clone(), response]).await;
            let client = client_at(base);
            let first = client.$method($($arg),*).await;
            assert!(
                first.is_ok(),
                concat!(stringify!($method), " 第一次就失败了：{:?}"),
                first.err()
            );
            let second = client.$method($($arg),*).await;
            assert!(
                second.is_ok(),
                concat!(stringify!($method), " 第二次没拿到可复用的回答：{:?}"),
                second.err()
            );
            assert_eq!(
                seen.lock().unwrap().len(),
                1,
                concat!(
                    stringify!($method),
                    " 第二次又发了请求，间隔内那份回答没有复用上"
                )
            );
        }};
    }

    /// 每个可复用的读，连调两次都还拿得到东西。
    ///
    /// 这条守的是**一整类**漏洞，不是某一个场景：客户端在会话里共享之后，各接口的
    /// 最小间隔才真正生效，而「间隔内的第二次读」能不能过，取决于前面那层有没有
    /// 回答可以给。前几轮抓到的漏洞全是同一类——限流槽被占着、手上却没有能顶上的
    /// 回答，玩家只看到「太频繁」。逐个场景去补，总会漏掉下一个。
    ///
    /// 不在这张表里的：
    ///
    /// - `notifications`：带游标，[`Call::reusable`] 明确排除（复用会让后台把同一批
    ///   提醒再弹一遍）；
    /// - `fetch_site_asset`：拿的是图片字节，不是 JSON；
    /// - 所有写操作，以及 `disconnect`（`bypass_gate`）。
    #[tokio::test]
    async fn every_reusable_read_survives_a_second_call() {
        const WORLD: &str = "wrld_11111111-1111-4111-8111-111111111111";
        // 短码的形状是「3–6 位字母数字 + `.` + 4 位数字」，形状不对会在本地被拒。
        const CLUB: &str = "abc.1234";
        let worlds = vec![WORLD.to_string()];

        reused_twice!(VrccBoard, boards(100, VrccBoardSort::Top));
        reused_twice!(VrccModelBoard, models_board(100, VrccBoardSort::Top));
        reused_twice!(VrccModelDetail, model(42));
        reused_twice!(VrccStatus, status());
        reused_twice!(VrccMapDetail, map(WORLD));
        reused_twice!(VrccBoard, maps(&worlds));
        reused_twice!(VrccClubResponse, club(CLUB));
        reused_twice!(VrccClubs, clubs(100, 0));
        reused_twice!(VrccMyClubs, my_clubs());
        reused_twice!(VrccMyClubResponse, my_club(CLUB));
        reused_twice!(VrccMapPage, map_page(WORLD));
        reused_twice!(VrccMyMapPage, my_map_page(WORLD, Some("hot")));
        reused_twice!(VrccAnnouncements, announcements());
        reused_twice!(VrccStickers, stickers());
        reused_twice!(VrccRecentNotifications, recent_notifications(false));
        // 标已读的读：第二次优先真的发出去，被本地间隔挡下才退回手上那份。不管走
        // 哪条，都不该报错，也不该把请求发出去。
        reused_twice!(VrccRecentNotifications, recent_notifications(true));
        reused_twice!(VrccUnread, unread());
        reused_twice!(VrccNotes, notes());
        reused_twice!(VrccMyMap, my_map(WORLD));
        reused_twice!(VrccMyBoard, my_board(VrccMyBoardSort::Top, None));
        reused_twice!(VrccRatedMaps, my_maps());
        reused_twice!(VrccToday, today());
        reused_twice!(VrccMutuals, mutuals());
        reused_twice!(VrccCrossings, crossings(false));
        reused_twice!(VrccCrossings, crossings(true));
        // 可见性探测和上面共用一个回答，但占的是自己的限流槽。
        reused_twice!(VrccCrossings, crossings_probe());
        reused_twice!(VrccCrossingCards, crossing_cards(false));
        reused_twice!(VrccCrossingCards, crossing_cards(true));
    }

    /// 带令牌的读、服务器回 `no-store`：两次都要成功，且两次都真的发了请求。
    ///
    /// 和 [`reused_twice`] 相反：这种回答存不下，`SlotRelease` 每次都把槽还回去，
    /// 所以第二次不该被本地间隔挡下，也不该复用任何东西。
    macro_rules! no_store_twice {
        ($ty:ty, $method:ident ( $($arg:expr),* $(,)? )) => {{
            let body = serde_json::to_string(&<$ty>::default()).expect("默认值应能序列化");
            let response = ok_with("Cache-Control: no-store\r\n", &body);
            let (base, seen) = serve_script(vec![response.clone(), response]).await;
            let client = client_at(base);
            let first = client.$method($($arg),*).await;
            assert!(
                first.is_ok(),
                concat!(stringify!($method), " 第一次就失败了：{:?}"),
                first.err()
            );
            let second = client.$method($($arg),*).await;
            assert!(
                second.is_ok(),
                concat!(stringify!($method), " 第二次被本地间隔挡下了：{:?}"),
                second.err()
            );
            assert_eq!(
                seen.lock().unwrap().len(),
                2,
                concat!(
                    stringify!($method),
                    " 的第二次没有真的发出去：no-store 的回答不该让槽占着"
                )
            );
        }};
    }

    /// 带令牌的读在真实服务器的缓存头下，第二次仍然发得出去。
    ///
    /// 2026-11-16 在运行中的应用里实测：个人接口的 `200` 一律回
    /// `Cache-Control: no-store`，也没有校验值。所以它们的回答存不下，
    /// [`SlotRelease`] 每次都会把槽还回去——间隔内的第二次读要真的再问一次，而不是
    /// 拿到本机的「太频繁」。
    ///
    /// [`every_reusable_read_survives_a_second_call`] 用不带缓存头的回答（等于
    /// 「可复用」），守的是复用那一路；这条守的是真实服务器走的那一路。两张表的带
    /// 令牌部分一一对应。
    #[tokio::test]
    async fn every_personal_read_survives_a_no_store_reply() {
        const WORLD: &str = "wrld_11111111-1111-4111-8111-111111111111";
        const CLUB: &str = "abc.1234";

        no_store_twice!(VrccRecentNotifications, recent_notifications(false));
        no_store_twice!(VrccRecentNotifications, recent_notifications(true));
        no_store_twice!(VrccUnread, unread());
        no_store_twice!(VrccNotes, notes());
        no_store_twice!(VrccMyMap, my_map(WORLD));
        no_store_twice!(VrccMyBoard, my_board(VrccMyBoardSort::Top, None));
        no_store_twice!(VrccRatedMaps, my_maps());
        no_store_twice!(VrccToday, today());
        no_store_twice!(VrccMutuals, mutuals());
        no_store_twice!(VrccCrossings, crossings(false));
        no_store_twice!(VrccCrossings, crossings(true));
        // 可见性探测：自己的槽，同样要还。
        no_store_twice!(VrccCrossings, crossings_probe());
        no_store_twice!(VrccCrossingCards, crossing_cards(false));
        no_store_twice!(VrccCrossingCards, crossing_cards(true));
        no_store_twice!(VrccMyClubs, my_clubs());
        no_store_twice!(VrccMyClubResponse, my_club(CLUB));
    }

    // -- 拿到了响应、却没存下回答：槽不能占着 ------------------------------

    /// 2xx 但正文解不出来（门户页、代理改写）：这次没有能复用的回答，间隔内重试
    /// 要真的再问一次，而不是拿到本机的「太频繁」。
    #[tokio::test]
    async fn an_undecodable_reply_does_not_hold_the_gap() {
        let (base, seen) = serve_script(vec![
            ok_with("", "<html>captive portal</html>"),
            ok_with("", r#"{"at":1,"state":"ok"}"#),
        ])
        .await;
        let client = client_at(base);

        let first = client.status().await.expect_err("第一次解不出来");
        assert!(matches!(first.detail, ErrorDetail::Decode(_)), "{first:?}");
        client.status().await.expect("间隔内重试：真的去问");
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    /// 响应头到了、正文读到一半连接断了：同上。
    #[tokio::test]
    async fn a_reply_cut_off_mid_body_does_not_hold_the_gap() {
        let cut = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{\"at\":"
            .to_string();
        let (base, seen) = serve_script(vec![cut, ok_with("", r#"{"at":1,"state":"ok"}"#)]).await;
        let client = client_at(base);

        let first = client.status().await.expect_err("第一次正文断了");
        assert_eq!(first.layer, ErrorLayer::Transport, "{first:?}");
        client.status().await.expect("间隔内重试：真的去问");
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    /// 429：退避照旧（`Retry-After`），但退避结束后不该再被本地间隔挡着——贴纸的
    /// 间隔是一小时，不还槽的话，玩家要等一小时才能再试。
    #[tokio::test]
    async fn after_a_429_backoff_the_read_is_not_held_by_its_gap() {
        let too_many = "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 2\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
            .to_string();
        let stickers = serde_json::to_string(&VrccStickers::default()).expect("序列化");
        let (base, seen) = serve_script(vec![too_many, ok_with("", &stickers)]).await;
        let client = client_at(base);

        client.stickers().await.expect_err("第一次 429");
        // 退避期间照旧被挡。
        let held = client.stickers().await.expect_err("退避中");
        assert!(
            matches!(
                held.detail,
                ErrorDetail::ThrottledLocally {
                    reason: LocalThrottle::Backoff,
                    ..
                }
            ),
            "{held:?}"
        );
        tokio::time::sleep(Duration::from_millis(2_200)).await;
        client.stickers().await.expect("退避过了：真的去问");
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    /// 换了令牌（断开后重连、换号）：上一个令牌的回答清掉了，它占着的间隔也不能
    /// 留着。不然新令牌在间隔内的第一次读手上没有回答，只能拿到本机的「太频繁」。
    #[tokio::test]
    async fn a_new_token_is_not_held_by_the_old_tokens_gaps() {
        let (base, seen) = serve_script(vec![ok_with("", "{}"), ok_with("", "{}")]).await;
        let mut session = crate::session::VrccSession::new("dev-test");
        session.point_client_at(base);

        session.connect("tok-a".into(), None, None, None);
        session.client().mutuals().await.expect("账号 A");
        session.connect("tok-b".into(), None, None, None);
        session
            .client()
            .mutuals()
            .await
            .expect("账号 B 的第一次读：真的去问，不被 A 的间隔挡下");
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    /// 429 的 `Retry-After` 很长、接口又不属于任何一份额度时，没有退避也没有额度
    /// 闸门挡着重试：这时槽留着，不能让紧接着的重试直接发出去。
    #[tokio::test]
    async fn a_429_with_nothing_else_holding_retries_keeps_the_gap() {
        let too_many = "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7200\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
            .to_string();
        let stickers = serde_json::to_string(&VrccStickers::default()).expect("序列化");
        let (base, seen) = serve_script(vec![too_many, ok_with("", &stickers)]).await;
        let client = client_at(base);

        client.stickers().await.expect_err("第一次 429");
        let held = client.stickers().await.expect_err("紧接着重试：被本地挡下");
        assert!(
            matches!(held.detail, ErrorDetail::ThrottledLocally { .. }),
            "{held:?}"
        );
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}
