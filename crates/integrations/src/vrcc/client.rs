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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::header::HeaderMap;
use serde::de::DeserializeOwned;
use serde_json::json;
use vrcx_0_contracts::vrcc::{
    VrccAnnouncements, VrccBoard, VrccBoardSort, VrccDeviceCode, VrccErrorBody, VrccMapDetail,
    VrccMutuals, VrccMyMap, VrccNotes, VrccNotifications, VrccRecentNotifications, VrccStatus,
    VrccToday, VrccTokenResponse, VrccUnread, VRCC_API_BASE, VRCC_DEFAULT_CLIENT,
};
use vrcx_0_core::proxy::with_remote_dns;

use super::throttle::{
    backoff_for, DailyUsage, RateLimiter, MAX_BACKOFF, NOTIFICATIONS_MIN_GAP, PERSONAL_DAILY_LIMIT,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(25);
/// 官方限制 POST 正文最大 2 KB。
const MAX_BODY_BYTES: usize = 2048;

/// 客户端错误。`status` 保留 HTTP 状态码，前端要靠它区分可重试与不可重试。
#[derive(Debug, thiserror::Error)]
pub enum VrccError {
    #[error("星遇同频 token 缺失，请先连接")]
    TokenMissing,
    #[error("路径必须相对 /api/v1/：{0}")]
    AbsolutePath(String),
    #[error("请求正文超过 {MAX_BODY_BYTES} 字节上限")]
    BodyTooLarge,
    #[error("星遇同频限流：{0}")]
    RateLimited(String),
    /// 429 `too_soon`：两次轮询间隔太短。等 `retry_after_secs` 再问，
    /// 不触发长时间退避——这不是配额问题，只是问勤了。
    #[error("星遇同频轮询太快，{retry_after_secs} 秒后再问")]
    TooSoon { retry_after_secs: u64 },
    #[error("星遇同频额度已用尽：{0}")]
    QuotaExhausted(String),
    /// `503 stop`：停站维护。令牌没事，等到 `until` 以后再试。
    ///
    /// 只有「网页上的动作」那一批（今天的星等）会这样回答；提醒、星卡、
    /// 地图榜停站时照常。
    #[error("星遇同频停站维护中{}", stop_suffix(.say))]
    Paused {
        say: Option<String>,
        until: Option<i64>,
    },
    #[error("星遇同频接口返回 HTTP {status}（{error}）")]
    Api {
        status: u16,
        error: String,
        need: Option<String>,
        why: Option<String>,
    },
    #[error("星遇同频接口暂时不可用：{0}")]
    Transport(String),
    #[error("星遇同频返回无法解析：{0}")]
    Decode(String),
}

fn stop_suffix(say: &Option<String>) -> String {
    match say.as_deref().map(str::trim).filter(|say| !say.is_empty()) {
        Some(say) => format!("：{say}"),
        None => String::new(),
    }
}

impl VrccError {
    /// 官方错误码表里的 `404 gone`：**令牌失效**（玩家断开了、同一个客户端又
    /// 授权了一次、星被删除，1.10.0 起还有「90 天没调用过任何接口自动断开」）。
    /// 要停下轮询并让玩家重新授权。
    ///
    /// 注意区分 `star_gone`（连线/私聊的对方不在了，令牌没事）、`expired`
    /// （1.9.0 起签名图片失效用这个码，原来也叫 `gone`）和 `not_found`
    /// （查的地图不在榜上、或旧服务器还没有这个接口），这些都**不该**触发重新授权。
    pub fn is_gone(&self) -> bool {
        matches!(
            self,
            VrccError::Api {
                status: 404,
                error,
                ..
            } if error == "gone"
        )
    }

    /// `404 not_found`：没有这个接口，或查的东西不在。
    ///
    /// 新接口（`me/unread`、`me/notes`、`announcements`）在还没更新的服务器上
    /// 就是这样回答的，调用方据此安静地不显示，而不是报错。
    pub fn is_not_found(&self) -> bool {
        matches!(
            self,
            VrccError::Api {
                status: 404,
                error,
                ..
            } if error == "not_found"
        )
    }

    /// 需要重新授权的情况。照官方错误码表逐条来：
    /// - `404 gone`：令牌失效
    /// - `403 client_mismatch`：令牌不是发给这个 `X-Client` 的
    /// - `403 client_unknown` / `client_off` / `client_missing`：标识本身有问题
    /// - `401 token`：没带令牌或格式不对
    /// - `403 scope`：缺权限，要玩家重新授权时勾上 `need` 那一项
    pub fn needs_reauth(&self) -> bool {
        if self.is_gone() {
            return true;
        }
        matches!(self, VrccError::Api { status: 401, .. })
            || matches!(
                self,
                VrccError::Api {
                    status: 403,
                    error,
                    ..
                } if matches!(
                    error.as_str(),
                    "client_mismatch"
                        | "client_unknown"
                        | "client_off"
                        | "client_missing"
                        | "scope"
                        | "scope_unreviewed"
                )
            )
            || matches!(self, VrccError::TokenMissing)
    }

    /// 缺哪个权限。前端据此提示「需要重新授权并勾上 X」。
    pub fn needed_scope(&self) -> Option<&str> {
        match self {
            VrccError::Api { need, .. } => need.as_deref(),
            _ => None,
        }
    }

    /// 设备授权期间「玩家还没点同意」，调用方应等 `interval` 秒再来问。
    pub fn is_authorization_pending(&self) -> bool {
        matches!(
            self,
            VrccError::Api {
                status: 400,
                error,
                ..
            } if error == "authorization_pending"
        )
    }

    /// 设备授权走到终态：玩家取消，或码过期/已用过。
    pub fn is_device_auth_terminal(&self) -> bool {
        matches!(
            self,
            VrccError::Api {
                status: 400,
                error,
                ..
            } if error == "access_denied" || error == "expired_token"
        )
    }
}

/// 带每天次数的接口各自算在哪一份额度里（官方《频率限制》）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuotaBucket {
    /// 第一份读：最近的提醒、星轨相交、互相连上、所在的地图、我打过分的地图。
    Reads,
    /// 第二份读：今天的星、通知中心等，和第一份分开算。
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

/// 客户端共享状态：限流器与用量计数。
#[derive(Debug, Default)]
pub struct VrccClientState {
    limiter: RateLimiter,
    usage: DailyUsage,
    /// 429 退避截止时间。
    backoff_until: Option<Instant>,
    /// 连续 429 次数，用于指数退避。
    backoff_attempt: u32,
    /// 服务器在 `RateLimit-*` 里说的各份额度。
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

    pub fn personal_used_today(&self) -> i64 {
        self.usage.personal_used()
    }

    pub fn device_code_used_today(&self) -> i64 {
        self.usage.device_code_used()
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
    /// 第一份读今天已用多少。拿到过服务器的 `RateLimit-*` 就是服务器的数。
    pub personal_used_today: i64,
    /// 第一份读每天的上限：服务器给的，没给时是 [`PERSONAL_DAILY_LIMIT`]。
    pub personal_daily_limit: i64,
    /// 上面两个数是不是服务器给的。
    pub personal_from_server: bool,
    /// `me/unread` 的（已用，上限），只有服务器给过响应头时才有。
    pub unread: Option<(i64, i64)>,
    pub device_code_used_today: i64,
    pub window_used: usize,
    pub backoff_remaining: Duration,
    /// 用量计数的日期（本地时区 `YYYY-MM-DD`）。
    pub date: String,
}

/// 日配额计数的持久化端口。
///
/// `integrations` 不依赖数据库，所以这里只定接口，实现在 `outbound-adapters`。
/// 需要落盘的原因是：日配额是**按天**算的，旧服务器不回 `RateLimit-*` 时，
/// 重启后清零会让本地预扣完全失效。服务器给了头以后，落盘的是服务器的准数。
pub trait DailyUsageStore: Send + Sync {
    /// 读取上次保存的计数。没有记录或读失败时返回 `None`。
    fn load(&self) -> Option<DailyUsage>;
    /// 保存计数。失败只该记日志，不该让请求失败。
    fn save(&self, usage: &DailyUsage);
}

/// 一次请求的全部参数。
struct Call<'a> {
    method: &'static str,
    path: &'a str,
    body: Option<serde_json::Value>,
    /// 是否带 `Bearer`。
    authed: bool,
    /// 本地限流用的接口标识。
    key: &'a str,
    /// 同一个接口两次之间至少隔多久。
    min_gap: Duration,
    /// 算在哪一份每天的额度里；`None` 是没有每天上限的接口。
    bucket: Option<QuotaBucket>,
}

/// 一个可克隆的客户端句柄。
#[derive(Clone)]
pub struct VrccClient {
    http: reqwest::Client,
    base: String,
    client_id: String,
    token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    state: Arc<std::sync::Mutex<VrccClientState>>,
    /// 日配额的落盘端口；`None` 时只在内存里记。
    usage_store: Option<Arc<dyn DailyUsageStore>>,
}

impl VrccClient {
    /// 新建客户端。`token` 是个取令牌的回调，这样令牌续期不用重建客户端。
    ///
    /// `proxy_url` 走宿主应用统一的代理设置（`WebClient::proxy_url`）。
    /// **必须传**：本机实测 `vrcc.cc` 直连会 DNS 超时，走代理才通。原来这里
    /// 自己 `build()` 一个不带代理的客户端，结果请求发不出去，界面上表现为
    /// 「接口暂时不可用」——看起来像服务挂了，其实是本地网络。
    ///
    /// 代理的构造方式照 `llm.rs` 的既有做法：`Proxy::all(with_remote_dns(url))`，
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
            Some(proxy_url) => match reqwest::Proxy::all(with_remote_dns(proxy_url).as_ref()) {
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
            usage_store: None,
        }
    }

    /// 接上日配额的落盘端口，并把上次的计数恢复进内存。
    ///
    /// 不恢复的后果是真的：旧服务器不回 `RateLimit-*` 时，重启清零等于本地
    /// 预扣形同虚设，直到撞上 `too_many` 才暴露——那时只能等第二天。
    ///
    /// 恢复时会把日期一起带进来，`roll_to` 会在下次取用量时判断是否跨天，
    /// 所以跨天重启不会把昨天的数字带到今天。
    pub fn with_usage_store(mut self, store: Arc<dyn DailyUsageStore>) -> Self {
        if let Some(restored) = store.load() {
            if let Ok(mut state) = self.state.lock() {
                state.usage = restored;
            }
        }
        self.usage_store = Some(store);
        self
    }

    /// 用默认标识新建。
    pub fn with_default_client(
        token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        proxy_url: Option<&str>,
    ) -> Self {
        Self::new(VRCC_DEFAULT_CLIENT, token, proxy_url)
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// 读用量。
    ///
    /// 会顺带把跨天的计数清零——调用方通常是界面，正好一天看一次。
    pub fn local_usage(&self) -> VrccLocalUsage {
        let today = super::throttle::local_date_today();
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.usage.roll_to(&today);
        let reads = state.live_quota(QuotaBucket::Reads, now);
        let unread = state
            .live_quota(QuotaBucket::Unread, now)
            .map(|quota| ((quota.limit - quota.remaining).max(0), quota.limit));
        VrccLocalUsage {
            personal_used_today: state.personal_used_today(),
            personal_daily_limit: reads.map_or(PERSONAL_DAILY_LIMIT, |quota| quota.limit),
            personal_from_server: reads.is_some(),
            unread,
            device_code_used_today: state.device_code_used_today(),
            window_used: state.window_used(now),
            backoff_remaining: state.backoff_remaining(now),
            date: today,
        }
    }

    /// 只允许 `/api/v1/` 下的相对路径。这是硬约束，不提供绕过开关。
    fn validate_path(path: &str) -> Result<(), VrccError> {
        let lowered = path.to_ascii_lowercase();
        if lowered.starts_with("http") || path.starts_with("//") {
            return Err(VrccError::AbsolutePath(path.to_string()));
        }
        Ok(())
    }

    /// 检查退避与限流，必要时等待。
    async fn acquire(&self, key: &str, min_gap: Duration) -> Result<(), VrccError> {
        let now = Instant::now();
        let (backoff, wait) = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let backoff = state.backoff_remaining(now);
            let wait = state
                .limiter
                .check(key, min_gap, now + backoff)
                .err()
                .unwrap_or_default();
            (backoff, wait)
        };
        if !backoff.is_zero() {
            return Err(VrccError::RateLimited(format!(
                "退避中，还需 {} 秒",
                backoff.as_secs().max(1)
            )));
        }
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.limiter.record(key, now);
        Ok(())
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

    /// 记下服务器在 `RateLimit-*` 里说的一份额度。
    ///
    /// 第一份读还会把本地计数改成服务器的准数并落盘：这就是「用服务器的数
    /// 代替自己落盘猜的数」。
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
        if bucket != QuotaBucket::Reads {
            return;
        }
        state.usage.roll_to(&super::throttle::local_date_today());
        state.usage.set_personal(quota.used());
        let snapshot = state.usage.clone();
        drop(state);
        self.persist_usage(&snapshot);
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

    /// 发请求前扣一次额度；已知用完时直接返回 `QuotaExhausted`，不发请求。
    ///
    /// 服务器给过 `RateLimit-*` 的，以服务器的余量为准；第一份读在没拿到过
    /// 服务器的数时，退回本地计数（上限 [`PERSONAL_DAILY_LIMIT`]）。
    fn spend(&self, bucket: QuotaBucket) -> Result<(), VrccError> {
        let today = super::throttle::local_date_today();
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.usage.roll_to(&today);
        let live = state.live_quota(bucket, now);
        if let Some(quota) = live {
            if quota.remaining <= 0 {
                return Err(VrccError::QuotaExhausted(format!(
                    "今天的次数用完了，约 {} 后恢复",
                    human_wait(quota.until.saturating_duration_since(now).as_secs())
                )));
            }
            // 先扣着，免得两次响应之间连发越界；下一个响应头会覆盖它。
            if let Some(seen) = state.quotas.get_mut(&bucket) {
                seen.remaining -= 1;
            }
        }
        if bucket != QuotaBucket::Reads {
            return Ok(());
        }
        if live.is_some() {
            state.usage.add_personal(1);
        } else if !state.usage.spend_personal(1) {
            return Err(VrccError::QuotaExhausted(format!(
                "本客户端今日个人接口已用满 {PERSONAL_DAILY_LIMIT} 次"
            )));
        }
        let snapshot = state.usage.clone();
        drop(state);
        self.persist_usage(&snapshot);
        Ok(())
    }

    pub fn spend_device_code(&self) -> Result<(), VrccError> {
        let today = super::throttle::local_date_today();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.usage.roll_to(&today);
        if !state.usage.spend_device_code() {
            return Err(VrccError::QuotaExhausted(
                "本客户端今日申请连接码次数已用尽".to_string(),
            ));
        }
        let snapshot = state.usage.clone();
        drop(state);
        self.persist_usage(&snapshot);
        Ok(())
    }

    /// 把当前计数写回存储。
    ///
    /// 先放锁再落盘：写数据库可能耗时，不该在持有状态锁时做。
    /// 写失败只记日志——配额记不准比让请求失败轻得多。
    fn persist_usage(&self, usage: &DailyUsage) {
        if let Some(store) = &self.usage_store {
            store.save(usage);
        }
    }

    /// 把一个 `429` 变成错误，顺带决定要不要退避、退避多久。
    ///
    /// 官方的 `429` 分几种，处理完全不同：
    /// - `too_soon`：两次轮询间隔不到 25 秒。等 `Retry-After` 秒（旧服务器
    ///   没给就用 `every`）再问就行，**不该**触发长时间退避。
    /// - 每天的次数用完（`RateLimit-Remaining` 是 0，或 `Retry-After` 长到
    ///   只能是「到北京时间零点」）：只停这一份额度，不让别的接口跟着停。
    /// - 其余（每分钟的限制）：照 `Retry-After` 全局退避；旧服务器没给头时
    ///   按指数退避猜。
    fn on_too_many(
        &self,
        error: &str,
        body: &VrccErrorBody,
        rate: RateHeaders,
        bucket: Option<QuotaBucket>,
    ) -> VrccError {
        if error == "too_soon" {
            let gap = rate
                .retry_after_secs
                .filter(|secs| *secs > 0)
                .or_else(|| {
                    body.every
                        .filter(|every| *every > 0)
                        .map(|every| every as u64)
                })
                .unwrap_or(NOTIFICATIONS_MIN_GAP.as_secs());
            return VrccError::TooSoon {
                retry_after_secs: gap,
            };
        }
        let daily = rate.quota.is_some_and(|quota| quota.remaining <= 0)
            || rate
                .retry_after_secs
                .is_some_and(|secs| Duration::from_secs(secs) > MAX_BACKOFF);
        if daily {
            let wait = rate
                .retry_after_secs
                .or(rate.quota.map(|quota| quota.reset_secs))
                .filter(|secs| *secs > 0)
                .unwrap_or_else(secs_until_beijing_midnight);
            if let Some(bucket) = bucket {
                self.mark_exhausted(bucket, Duration::from_secs(wait));
            }
            return VrccError::QuotaExhausted(format!(
                "{error}，今天的次数用完了，约 {} 后恢复",
                human_wait(wait)
            ));
        }
        let wait = match rate.retry_after_secs {
            Some(secs) => self.note_retry_after(Duration::from_secs(secs.max(1))),
            None => self.note_rate_limited(),
        };
        VrccError::RateLimited(format!("{error}，退避 {} 秒", wait.as_secs()))
    }

    /// 核心请求。
    async fn request<T: DeserializeOwned>(&self, call: Call<'_>) -> Result<T, VrccError> {
        Self::validate_path(call.path)?;
        if let Some(bucket) = call.bucket {
            self.spend(bucket)?;
        }
        self.acquire(call.key, call.min_gap).await?;

        let mut builder = self
            .http
            .request(
                call.method.parse().map_err(|_| {
                    VrccError::Transport(format!("不支持的请求方法 {}", call.method))
                })?,
                format!("{}{}", self.base, call.path),
            )
            .header("X-Client", &self.client_id)
            .header("Accept", "application/json");

        if call.authed {
            let token = (self.token)().ok_or(VrccError::TokenMissing)?;
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }

        if let Some(payload) = call.body {
            let text = payload.to_string();
            if text.len() > MAX_BODY_BYTES {
                return Err(VrccError::BodyTooLarge);
            }
            builder = builder
                .header("Content-Type", "application/json; charset=utf-8")
                .body(text);
        }

        let response = builder
            .send()
            .await
            .map_err(|error| VrccError::Transport(error.to_string()))?;
        let status = response.status();
        let rate = RateHeaders::from_headers(response.headers());
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
        let text = response
            .text()
            .await
            .map_err(|error| VrccError::Transport(error.to_string()))?;

        if let (Some(bucket), Some(quota)) = (call.bucket, rate.quota) {
            self.note_quota(bucket, quota);
        }

        if status.is_success() {
            self.note_success();
            serde_json::from_str(&text).map_err(|error| {
                // 不把正文塞进错误里：里面可能有玩家数据。
                VrccError::Decode(error.to_string())
            })
        } else {
            let code = status.as_u16();
            let parsed: VrccErrorBody = serde_json::from_str(&text).unwrap_or_default();
            let error = parsed
                .error
                .clone()
                .unwrap_or_else(|| format!("http_{code}"));
            if code == 429 {
                return Err(self.on_too_many(&error, &parsed, rate, call.bucket));
            }
            if code == 503 && error == "stop" {
                return Err(VrccError::Paused {
                    say: parsed.say,
                    until: parsed.until,
                });
            }
            Err(VrccError::Api {
                status: code,
                error,
                need: parsed.need,
                why: parsed.why,
            })
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
        })
        .await
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
        })
        .await
    }

    /// 单张地图详情。未上榜或未过审返回 `404 not_found`。
    pub async fn map(&self, world: &str) -> Result<VrccMapDetail, VrccError> {
        self.request(Call {
            method: "GET",
            path: &format!("maps/{world}"),
            body: None,
            authed: false,
            key: "map",
            min_gap: Duration::from_secs(30),
            bucket: None,
        })
        .await
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
        self.spend_device_code()?;
        let path = match scope.map(str::trim).filter(|scope| !scope.is_empty()) {
            Some(scope) => format!("device/code?scope={}", encode_query_value(scope)),
            None => "device/code".to_string(),
        };
        self.request(Call {
            method: "POST",
            path: &path,
            body: None,
            authed: false,
            key: "device_code",
            min_gap: Duration::from_secs(5),
            bucket: None,
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
        })
        .await
    }

    /// 断开：让服务器吊销当前令牌。
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
        })
        .await
    }

    /// 当前所在地图在地图榜上的状态与我自己的评分（第一份读）。
    ///
    /// 需要 `maps`（普通权限，`dev-` 就能用）。这是官方插件「所在地图」那块
    /// 的数据来源：能看分、看人数、看自己打了几分。
    ///
    /// `world` 只能是 `wrld_…`，不要带房间实例号。
    pub async fn my_map(&self, world: &str) -> Result<VrccMyMap, VrccError> {
        self.request(Call {
            method: "GET",
            path: &format!("me/maps/{world}"),
            body: None,
            authed: true,
            key: "my_map",
            min_gap: Duration::from_secs(10),
            bucket: Some(QuotaBucket::Reads),
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
        })
        .await
    }
}

/// 地图榜的路径。总榜不带 `sort`，和旧服务器上的写法一模一样。
fn boards_path(limit: u32, sort: VrccBoardSort) -> String {
    match sort {
        VrccBoardSort::Top => format!("boards/maps?limit={limit}"),
        other => format!("boards/maps?limit={limit}&sort={}", other.as_query()),
    }
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

/// 给人看的等待时长：`3 小时 20 分钟`、`5 分钟`、`40 秒`。
fn human_wait(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs} 秒");
    }
    let total_minutes = secs.div_ceil(60);
    match (total_minutes / 60, total_minutes % 60) {
        (0, minutes) => format!("{minutes} 分钟"),
        (hours, 0) => format!("{hours} 小时"),
        (hours, minutes) => format!("{hours} 小时 {minutes} 分钟"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use vrcx_0_contracts::vrcc::{
        is_dev_client, resolve_client_id, VRCC_DEV_CLIENT, VRCC_PROD_CLIENT,
    };

    fn client() -> VrccClient {
        VrccClient::new("dev-test", Arc::new(|| Some("tok".to_string())), None)
    }

    /// 标识解析的优先级：运行时覆盖 > 正式常量 > 开发常量。
    ///
    /// 这三者的关系值得钉住，因为正式标识是平台发的、**不是** `dev-` 那个字符串
    /// 去掉前缀——`dev-…` 与 `app_…` 是两种东西。写死一处常量迟早有人手改出错。
    #[test]
    fn client_id_resolution_prefers_override_then_prod_then_dev() {
        // 覆盖值优先。
        assert_eq!(
            resolve_client_id(Some("app_0123456789abcdef")),
            "app_0123456789abcdef"
        );
        // 空白覆盖值不该被采用（配置项被清成空串是很常见的事）。
        assert_eq!(resolve_client_id(Some("   ")), {
            if VRCC_PROD_CLIENT.is_empty() {
                VRCC_DEV_CLIENT.to_string()
            } else {
                VRCC_PROD_CLIENT.to_string()
            }
        });
        assert_eq!(resolve_client_id(None), {
            if VRCC_PROD_CLIENT.is_empty() {
                VRCC_DEV_CLIENT.to_string()
            } else {
                VRCC_PROD_CLIENT.to_string()
            }
        });
    }

    /// 开发态判定只看 `dev-` 前缀，这是官方的判定方式。
    #[test]
    fn dev_client_is_detected_by_prefix() {
        assert!(is_dev_client(VRCC_DEV_CLIENT));
        assert!(is_dev_client("  dev-anything  "));
        assert!(!is_dev_client("app_0123456789abcdef"));
        assert!(!is_dev_client(""));
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
            Err(VrccError::AbsolutePath(_))
        ));
        assert!(matches!(
            VrccClient::validate_path("//evil.example/x"),
            Err(VrccError::AbsolutePath(_))
        ));
        assert!(VrccClient::validate_path("boards/maps?limit=5").is_ok());
        assert!(VrccClient::validate_path("me/notifications?since=1").is_ok());
    }

    #[test]
    fn gone_error_is_detected() {
        let error = VrccError::Api {
            status: 404,
            error: "gone".to_string(),
            need: None,
            why: None,
        };
        assert!(error.is_gone());
        assert!(error.needs_reauth());
    }

    #[test]
    fn star_gone_and_not_found_do_not_need_reauth() {
        // 官方错误码表明确区分：对方不在了 / 地图不在榜上，令牌都没事。
        for code in ["star_gone", "not_found"] {
            let error = VrccError::Api {
                status: 404,
                error: code.to_string(),
                need: None,
                why: None,
            };
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
            let error = VrccError::Api {
                status,
                error: code.to_string(),
                need: Some("card".to_string()),
                why: None,
            };
            assert!(error.needs_reauth(), "{status} {code} 应要求重新授权");
        }
        // 这些不需要。
        assert!(!VrccError::RateLimited("too_many".into()).needs_reauth());
        let map_not_went = VrccError::Api {
            status: 403,
            error: "map_not_went".to_string(),
            need: Some("verify".to_string()),
            why: None,
        };
        assert!(!map_not_went.needs_reauth(), "打分前置条件不要求重新授权");
    }

    #[test]
    fn device_flow_errors_are_classified() {
        let pending = VrccError::Api {
            status: 400,
            error: "authorization_pending".to_string(),
            need: None,
            why: None,
        };
        assert!(pending.is_authorization_pending());
        assert!(!pending.is_device_auth_terminal());

        for code in ["access_denied", "expired_token"] {
            let error = VrccError::Api {
                status: 400,
                error: code.to_string(),
                need: None,
                why: None,
            };
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
            let error = VrccError::Api {
                status: 400,
                error: code.to_string(),
                need: None,
                why: None,
            };
            assert!(!error.is_device_auth_terminal(), "{code}");
            assert!(!error.is_authorization_pending(), "{code}");
            assert!(!error.needs_reauth(), "{code}");
        }
        // 缺的权限要能带出去，界面才能告诉玩家是哪个名字写错了。
        let invalid_scope = VrccError::Api {
            status: 400,
            error: "invalid_scope".to_string(),
            need: Some("maps.rate".to_string()),
            why: None,
        };
        assert_eq!(invalid_scope.needed_scope(), Some("maps.rate"));
    }

    #[test]
    fn too_soon_is_not_a_quota_problem() {
        // 官方把 429 分成两种：too_soon 是问勤了，too_many 是额度用完。
        // 前者不该被当成需要长时间退避的配额问题。
        let too_soon = VrccError::TooSoon {
            retry_after_secs: 25,
        };
        assert!(!too_soon.needs_reauth());
        assert!(too_soon.to_string().contains("25"));

        let too_many = VrccError::RateLimited("too_many".into());
        assert!(!too_many.needs_reauth());
    }

    #[test]
    fn recent_notification_urls_are_relative_and_get_prefixed() {
        use vrcx_0_contracts::vrcc::VrccRecentNotificationItem;
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

    #[tokio::test]
    async fn personal_quota_stops_before_request() {
        let client = client();
        // 把额度用光
        {
            let today = super::super::throttle::local_date_today();
            let mut state = client.state.lock().unwrap();
            state.usage.roll_to(&today);
            assert!(state.usage.spend_personal(PERSONAL_DAILY_LIMIT));
        }
        let error = client.spend(QuotaBucket::Reads).unwrap_err();
        assert!(matches!(error, VrccError::QuotaExhausted(_)));
    }

    #[test]
    fn body_over_2kb_is_rejected() {
        // 官方限制 POST 正文 2 KB，客户端在发送前就拦。
        let big = "x".repeat(MAX_BODY_BYTES + 1);
        assert!(big.len() > MAX_BODY_BYTES);
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
        VrccError::Api {
            status,
            error: code.to_string(),
            need: None,
            why: None,
        }
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

    #[test]
    fn too_soon_waits_for_retry_after_over_every() {
        let client = client();
        let body = VrccErrorBody {
            every: Some(60),
            ..Default::default()
        };
        let rate = RateHeaders {
            retry_after_secs: Some(7),
            quota: None,
        };
        // 1.9.0 起 `Retry-After` 是 25 秒间隔还剩的秒数，比 `every` 准。
        match client.on_too_many("too_soon", &body, rate, None) {
            VrccError::TooSoon { retry_after_secs } => assert_eq!(retry_after_secs, 7),
            other => panic!("expected TooSoon, got {other:?}"),
        }
        // 旧服务器没有头：退回 `every`。
        match client.on_too_many("too_soon", &body, RateHeaders::default(), None) {
            VrccError::TooSoon { retry_after_secs } => assert_eq!(retry_after_secs, 60),
            other => panic!("expected TooSoon, got {other:?}"),
        }
        // too_soon 不触发全局退避。
        assert!(client.local_usage().backoff_remaining.is_zero());
    }

    #[test]
    fn minute_limit_backs_off_exactly_as_told() {
        let client = client();
        let rate = RateHeaders {
            retry_after_secs: Some(60),
            quota: None,
        };
        let error = client.on_too_many("too_many", &VrccErrorBody::default(), rate, None);
        assert!(matches!(error, VrccError::RateLimited(_)), "{error:?}");
        let backoff = client.local_usage().backoff_remaining;
        assert!(backoff > Duration::from_secs(55) && backoff <= Duration::from_secs(60));
        // 第二次照样按服务器说的等，不再自己翻倍。
        client.on_too_many("too_many", &VrccErrorBody::default(), rate, None);
        assert!(client.local_usage().backoff_remaining <= Duration::from_secs(60));
    }

    #[test]
    fn old_server_429_without_retry_after_still_backs_off() {
        let client = client();
        let error = client.on_too_many(
            "too_many",
            &VrccErrorBody::default(),
            RateHeaders::default(),
            None,
        );
        assert!(matches!(error, VrccError::RateLimited(_)));
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
        let error = client.on_too_many(
            "too_many",
            &VrccErrorBody::default(),
            rate,
            Some(QuotaBucket::MoreReads),
        );
        assert!(matches!(error, VrccError::QuotaExhausted(_)), "{error:?}");
        assert!(error.to_string().contains("5 小时"), "{error}");
        // 不让所有接口一起退避五个小时。
        assert!(client.local_usage().backoff_remaining.is_zero());
        // 这一份在零点前不再发请求……
        assert!(matches!(
            client.spend(QuotaBucket::MoreReads),
            Err(VrccError::QuotaExhausted(_))
        ));
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
        let error = client.on_too_many("too_many", &VrccErrorBody::default(), rate, None);
        assert!(matches!(error, VrccError::QuotaExhausted(_)), "{error:?}");
        assert!(client.local_usage().backoff_remaining.is_zero());
    }

    #[test]
    fn server_quota_replaces_the_local_count() {
        let client = client();
        // 本地先记了 3 次（比如重启前的落盘数）。
        for _ in 0..3 {
            client.spend(QuotaBucket::Reads).unwrap();
        }
        let before = client.local_usage();
        assert_eq!(before.personal_used_today, 3);
        assert!(!before.personal_from_server);
        assert_eq!(before.personal_daily_limit, PERSONAL_DAILY_LIMIT);

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
        assert_eq!(after.personal_used_today, 412);
        assert_eq!(after.personal_daily_limit, 1000);
        assert!(after.personal_from_server);
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

    #[test]
    fn server_quota_lets_a_raised_limit_through() {
        // 本地兜底上限是 1000；服务器哪天把上限调高了，不能被本地数拦住。
        let client = client();
        {
            let today = super::super::throttle::local_date_today();
            let mut state = client.state.lock().unwrap();
            state.usage.roll_to(&today);
            assert!(state.usage.spend_personal(PERSONAL_DAILY_LIMIT));
        }
        client.note_quota(
            QuotaBucket::Reads,
            ServerQuota {
                limit: 2000,
                remaining: 1000,
                reset_secs: 3600,
            },
        );
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
        assert!(matches!(
            client.spend(QuotaBucket::Unread),
            Err(VrccError::QuotaExhausted(_))
        ));
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

    #[test]
    fn stop_reads_as_maintenance_not_as_reauth() {
        let paused = VrccError::Paused {
            say: Some("升级数据库，约半小时".to_string()),
            until: Some(1_793_000_000_000),
        };
        assert!(!paused.needs_reauth());
        assert_eq!(
            paused.to_string(),
            "星遇同频停站维护中：升级数据库，约半小时"
        );
        let silent = VrccError::Paused {
            say: None,
            until: None,
        };
        assert_eq!(silent.to_string(), "星遇同频停站维护中");
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

    #[test]
    fn beijing_midnight_and_waits_read_right() {
        // 2026-10-27 00:00 北京 = 2026-10-26 16:00 UTC。
        let midnight_utc = 1_793_030_400;
        assert_eq!(secs_until_beijing_midnight_from(midnight_utc - 60), 60);
        assert_eq!(secs_until_beijing_midnight_from(midnight_utc), 86_400);
        assert_eq!(human_wait(40), "40 秒");
        assert_eq!(human_wait(300), "5 分钟");
        assert_eq!(human_wait(3599), "1 小时");
        assert_eq!(human_wait(5 * 3600 + 20 * 60), "5 小时 20 分钟");
    }
}
