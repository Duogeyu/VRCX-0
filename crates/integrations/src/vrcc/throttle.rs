//! 客户端侧的频率与配额控制。
//!
//! 官方《频率限制》表格里对我们有效的几条（开放接口 1.14.0）：
//!
//! | 范围 | 限制 |
//! | --- | --- |
//! | 每个网络地址（总计） | 每分钟约 120 次 |
//! | 轮询提醒 `/me/notifications` | 两次之间至少 25 秒，建议按返回的 `every`（60 秒）；**没有每天上限** |
//! | 第一份读：最近提醒 / 星轨 / 互相连上 / 所在图 / 我打过的图 | 每个授权每天 1000 次（1.8.4 起，原来 300） |
//! | 第二份读：今天的星 / 通知中心 / … | 每个授权每天 1000 次，和第一份分开算 |
//! | 未读数 `/me/unread` | 每个授权每天 1500 次，单独算 |
//! | 设备授权拿码 `/device/code` | 每网络地址每天 30 个 |
//!
//! 1.11.0 起，带每天次数的接口回答 `RateLimit-Limit` / `RateLimit-Remaining`
//! / `RateLimit-Reset`；1.9.0 起所有 `429` 都带 `Retry-After`。服务器给了就以
//! 服务器为准（见 `client.rs`），这里的本地计数只在旧服务器没给头时兜底。

use std::collections::HashMap;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// 每分钟请求总数上限（官方「120 次左右」，取 120 整数）。
pub const WINDOW_LIMIT: usize = 120;
/// 限流窗口长度。
pub const WINDOW: Duration = Duration::from_secs(60);
/// 「第一份读」每个授权每天的上限（1.8.4 起从每颗星 300 次改成每个授权 1000 次）。
///
/// 只是服务器没给 `RateLimit-Limit` 时的兜底；给了就用服务器的。
pub const PERSONAL_DAILY_LIMIT: i64 = 1000;
/// 设备授权拿码每日上限。
pub const DEVICE_CODE_DAILY_LIMIT: i64 = 30;
/// 轮询提醒的最小间隔（官方硬性要求）。
pub const NOTIFICATIONS_MIN_GAP: Duration = Duration::from_secs(25);

/// 429 之后的最大退避。
///
/// 也是「短等」和「等到明天」的分界：`Retry-After` 比它长的，是某一项每天的
/// 次数用完了，只停那一项，不让全部接口一起退避（见 `client.rs`）。
pub const MAX_BACKOFF: Duration = Duration::from_secs(600);
/// 旧服务器的 429 不带 `Retry-After` 时的最小退避（当时官方写「至少等一分钟」）。
pub const MIN_BACKOFF: Duration = Duration::from_secs(60);

/// 滑动窗口限流器 + 每接口最小间隔。
///
/// 逻辑照 `xingshi-app` 的 `Throttle`，但改为同步：这里的调用点本来就在
/// 异步任务里，用 `Instant` 记账比 `delay` 更容易测试。
#[derive(Debug, Default)]
pub struct RateLimiter {
    /// 最近一分钟内每次放行的时间点，最旧的在前。
    stamps: VecDeque<Instant>,
    /// 每个接口标识的上次放行时间。
    last_at: HashMap<String, Instant>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 窗口里已经不可能再算进任何未来窗口的记录，直接丢掉。
    ///
    /// 比 `now` 早一分钟以上的时间点，对**将来**的任何 `at`（`at >= now`）
    /// 都有 `stamp <= now - 60s <= at - 60s`，永远落在窗口外。
    fn drop_expired(&mut self, now: Instant) {
        while let Some(front) = self.stamps.front() {
            if now.saturating_duration_since(*front) >= WINDOW {
                self.stamps.pop_front();
            } else {
                break;
            }
        }
    }

    /// 最早能在哪个时刻发一个 `key` 请求。只读，不改状态。
    ///
    /// 同时满足两个约束：
    /// - 同一个接口两次之间至少隔 `min_gap`；
    /// - 任意 60 秒窗口内不超过 [`WINDOW_LIMIT`] 次。
    fn earliest(&self, key: &str, min_gap: Duration, now: Instant) -> Instant {
        let gap_ready = self
            .last_at
            .get(key)
            .map(|last| *last + min_gap)
            .unwrap_or(now)
            .max(now);
        // 窗口约束：以 `at` 结尾的 60 秒里已有记录必须少于上限（要给它留位置）。
        // 不够就往后推，推到最老的几条滑出窗口为止。
        let mut at = gap_ready;
        loop {
            let window_start = at.checked_sub(WINDOW).unwrap_or(at);
            let mut in_window: Vec<Instant> = self
                .stamps
                .iter()
                .filter(|stamp| **stamp > window_start && **stamp <= at)
                .copied()
                .collect();
            if in_window.len() < WINDOW_LIMIT {
                return at;
            }
            in_window.sort_unstable();
            // 要多腾出一个位置，就得让最老的 `overflow` 条滑出窗口。
            let overflow = in_window.len() + 1 - WINDOW_LIMIT;
            at = in_window[overflow - 1] + WINDOW;
        }
    }

    /// 判断现在能否发一个 `key` 接口的请求。不修改状态。
    ///
    /// 返回 `Ok(())` 可以发；`Err(wait)` 需要等这么久。
    pub fn check(&self, key: &str, min_gap: Duration, now: Instant) -> Result<(), Duration> {
        let wait = self
            .earliest(key, min_gap, now)
            .saturating_duration_since(now);
        if wait.is_zero() {
            Ok(())
        } else {
            Err(wait)
        }
    }

    /// **原子地**预占一个发送时间槽。
    ///
    /// 返回 `Ok(at)`：槽已占用，等到 `at` 就能发。
    /// 返回 `Err(wait)`：本来要等 `wait`，因为超过 `budget` 而**没有**占槽。
    ///
    /// 存在的理由：原来调用方先 `check` 拿到 `wait`、放锁、睡完再加锁
    /// `record`。并发调用会拿到**同一个** `wait`，然后一起醒来一起发出去，
    /// `min_gap` 与窗口上限都不是原子的。预占把这两步收进同一把锁，
    /// 每个调用者拿到互不相同的时间点。
    pub fn reserve(
        &mut self,
        key: &str,
        min_gap: Duration,
        now: Instant,
        budget: Duration,
    ) -> Result<Instant, Duration> {
        self.drop_expired(now);
        let at = self.earliest(key, min_gap, now);
        let wait = at.saturating_duration_since(now);
        if wait > budget {
            // 不占槽：调用方可以选择用缓存，而不是干等。
            return Err(wait);
        }
        // 按时间有序插入：`at` 未必大于最后一条（别的接口的 gap 更短时
        // 可能插到前面），而窗口统计依赖有序。
        let stamps = self.stamps.make_contiguous();
        let position = stamps.partition_point(|stamp| *stamp <= at);
        self.stamps.insert(position, at);
        self.last_at.insert(key.to_string(), at);
        Ok(at)
    }

    /// 记录一次放行。调用点应先用 `check` 确认可以发。
    pub fn record(&mut self, key: &str, now: Instant) {
        self.drop_expired(now);
        let stamps = self.stamps.make_contiguous();
        let position = stamps.partition_point(|stamp| *stamp <= now);
        self.stamps.insert(position, now);
        self.last_at.insert(key.to_string(), now);
    }

    /// 当前窗口内已用次数。
    pub fn window_used(&self, now: Instant) -> usize {
        self.stamps
            .iter()
            .filter(|stamp| now.saturating_duration_since(**stamp) < WINDOW)
            .count()
    }
}

/// 429 退避的指数增长：60s、120s、240s、480s，封顶 600s。
pub fn backoff_for(attempt: u32) -> Duration {
    let shifted = MIN_BACKOFF.saturating_mul(1u32.checked_shl(attempt.min(4)).unwrap_or(16));
    shifted.min(MAX_BACKOFF)
}

/// 按本地日期滚动的用量计数。
///
/// 只记「已用」，不记「剩余」。服务器回过 `RateLimit-*` 时，`personal` 会被
/// 改成服务器的准数（见 [`DailyUsage::set_personal`]）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsage {
    date: String,
    personal: i64,
    device_code: i64,
}

impl DailyUsage {
    /// 从落盘的快照恢复计数。
    ///
    /// 不用在这里判断日期是否已过期：下一次 `roll_to` 会负责清零，
    /// 而 `roll_to` 每次取用量前都会被调用。
    pub fn restore(date: &str, personal: i64, device_code: i64) -> Self {
        Self {
            date: date.to_string(),
            personal: personal.max(0),
            device_code: device_code.max(0),
        }
    }

    /// 这份计数属于哪一天（本地时区 `YYYY-MM-DD`）。
    pub fn date(&self) -> &str {
        &self.date
    }

    /// 换到 `today`（`YYYY-MM-DD`）时清零。返回是否发生了跨天。
    pub fn roll_to(&mut self, today: &str) -> bool {
        if self.date == today {
            return false;
        }
        self.date = today.to_string();
        self.personal = 0;
        self.device_code = 0;
        true
    }

    pub fn personal_used(&self) -> i64 {
        self.personal
    }

    pub fn device_code_used(&self) -> i64 {
        self.device_code
    }

    /// 个人接口还能不能再用 `amount` 次。
    pub fn can_spend_personal(&self, amount: i64) -> bool {
        self.personal + amount <= PERSONAL_DAILY_LIMIT
    }

    /// 扣一次个人接口额度。返回是否扣成功（额度不足时不动账）。
    pub fn spend_personal(&mut self, amount: i64) -> bool {
        if !self.can_spend_personal(amount) {
            return false;
        }
        self.personal += amount;
        true
    }

    /// 只记账、不拦：服务器已经告诉我们还有余量时用，拦不拦由服务器的数说了算。
    pub fn add_personal(&mut self, amount: i64) {
        self.personal = self.personal.saturating_add(amount);
    }

    /// 用服务器 `RateLimit-*` 算出的已用次数覆盖本地计数。
    ///
    /// 服务器按这个授权数，是准数；本地数只是兜底。覆盖之后落盘的也是准数，
    /// 重启后在拿到下一次响应头之前，显示的不会是自己猜的。
    pub fn set_personal(&mut self, used: i64) {
        self.personal = used.max(0);
    }

    /// 退一次「第一份读」的预扣。
    ///
    /// 用在**请求没发出去**的情况（还没过限流、或连接就失败了）：服务器
    /// 一次都没数，本地不该记。不退的话，断网时后台轮询每分钟扣一次，
    /// 本地很快跑满，网络恢复后会被自己拦到第二天。
    ///
    /// 下限夹在 0：`i64::saturating_sub` 是在 `i64::MIN` 饱和而不是 0，
    /// 直接用会让计数变成负数。
    pub fn refund_personal(&mut self, amount: i64) {
        self.personal = self.personal.saturating_sub(amount.max(0)).max(0);
    }

    /// 退一次设备码的预扣。同理，只在请求没发出去时调，并且夹在 0。
    pub fn refund_device_code(&mut self) {
        self.device_code = self.device_code.saturating_sub(1).max(0);
    }

    pub fn can_spend_device_code(&self) -> bool {
        self.device_code < DEVICE_CODE_DAILY_LIMIT
    }

    pub fn spend_device_code(&mut self) -> bool {
        if !self.can_spend_device_code() {
            return false;
        }
        self.device_code += 1;
        true
    }
}

/// 当前时间的毫秒时间戳。会话记录轮询时间用。
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// 本地时区的 `YYYY-MM-DD`。
///
/// 官方按自然日算额度，我们跟着本地时区走，跨天即清零。
/// 这里不引入 `chrono`：`integrations` 目前没有这个依赖，而算一个本地日期
/// 用 `libc` 的 `localtime_r` 语义太长；直接按 UTC+本地偏移手算了。
pub fn local_date_today() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_local_date(now, local_offset_secs())
}

/// 本地时区相对 UTC 的秒数（东八区为 `+28800`）。
///
/// Windows 上没有简单可用的 tz 查询，这里读环境变量 `TZ_OFFSET_SECS`，
/// 缺省按 UTC 处理——日期只在跨天时用一次，误差最多影响当天的计数清零时机。
fn local_offset_secs() -> i64 {
    std::env::var("TZ_OFFSET_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(8 * 3600)
}

/// 把 Unix 秒按给定偏移折算成 `YYYY-MM-DD`。
pub fn format_local_date(unix_secs: i64, offset_secs: i64) -> String {
    let shifted = unix_secs + offset_secs;
    let days = shifted.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant 的 `civil_from_days`：把「1970-01-01 起的天数」转成公历年月日。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn window_blocks_after_limit() {
        let base = Instant::now();
        let mut limiter = RateLimiter::new();
        for _ in 0..WINDOW_LIMIT {
            assert!(limiter.check("x", Duration::ZERO, base).is_ok());
            limiter.record("x", base);
        }
        assert!(limiter.check("x", Duration::ZERO, base).is_err());
        // 一分钟后窗口滑走，重新放行。
        assert!(limiter.check("x", Duration::ZERO, at(base, 61)).is_ok());
    }

    #[test]
    fn per_key_gap_is_enforced() {
        let base = Instant::now();
        let mut limiter = RateLimiter::new();
        assert!(limiter
            .check("notifications", NOTIFICATIONS_MIN_GAP, base)
            .is_ok());
        limiter.record("notifications", base);
        // 25 秒内同一个接口不放行。
        assert!(limiter
            .check("notifications", NOTIFICATIONS_MIN_GAP, at(base, 24))
            .is_err());
        // 别的接口不受影响。
        assert!(limiter
            .check("boards", NOTIFICATIONS_MIN_GAP, at(base, 1))
            .is_ok());
        // 满 25 秒放行。
        assert!(limiter
            .check("notifications", NOTIFICATIONS_MIN_GAP, at(base, 25))
            .is_ok());
    }

    /// **并发预占拿到的是互不相同的时间点。**
    ///
    /// 这是这次改造的核心：原来 `check` + `record` 分两步，两个并发调用会
    /// 拿到同一个 `wait`，一起睡醒、一起发出去——`min_gap` 形同虚设。
    #[test]
    fn concurrent_reservations_get_distinct_slots() {
        let base = Instant::now();
        let mut limiter = RateLimiter::new();
        let gap = Duration::from_secs(5);
        let budget = Duration::from_secs(600);

        let first = limiter
            .reserve("boards", gap, base, budget)
            .expect("first slot");
        // 同一时刻再要一次：不能也拿到 `first`。
        let second = limiter
            .reserve("boards", gap, base, budget)
            .expect("second slot");
        let third = limiter
            .reserve("boards", gap, base, budget)
            .expect("third slot");

        assert_eq!(first, base, "第一个请求立刻就能发");
        assert_eq!(second, base + gap, "第二个要等一个 gap");
        assert_eq!(third, base + gap * 2, "第三个再等一个 gap");
        // 三者互不相同，这才是原子性要保证的东西。
        assert_ne!(first, second);
        assert_ne!(second, third);
    }

    /// 预占会占住槽位：同一时刻再要，就得等满一个 gap。
    #[test]
    fn a_reserved_slot_is_not_handed_out_twice() {
        let base = Instant::now();
        let mut limiter = RateLimiter::new();
        let gap = Duration::from_secs(30);
        let _ = limiter
            .reserve("x", gap, base, Duration::from_secs(600))
            .expect("slot");
        // 同一时刻再要，且只肯等 1 秒：必须被拒，且拒的理由是「要等满 30 秒」。
        let again = limiter.reserve("x", gap, base, Duration::from_secs(1));
        assert_eq!(again, Err(gap));
    }

    /// 超过预算就不占槽，让调用方改用缓存。
    #[test]
    fn budget_exceeded_reserves_nothing() {
        let base = Instant::now();
        let mut limiter = RateLimiter::new();
        let gap = Duration::from_secs(60);
        // 先占一个，使下一个至少要等 60 秒。
        let _ = limiter
            .reserve("boards", gap, base, Duration::from_secs(600))
            .expect("first");

        // 只肯等 2 秒：直接拒绝，且**不留痕**。
        let denied = limiter.reserve("boards", gap, base, Duration::from_secs(2));
        assert_eq!(denied, Err(gap));
        assert_eq!(limiter.window_used(base), 1, "被拒绝的请求不该占用窗口名额");

        // 别的接口不受影响，照样能立刻发（窗口没满）。
        assert!(limiter
            .reserve(
                "status",
                Duration::from_secs(30),
                base,
                Duration::from_secs(1)
            )
            .is_ok());
    }

    /// 窗口上限也要在预占里成立：第 121 个请求必须被推后。
    #[test]
    fn reservations_respect_the_window_limit() {
        let base = Instant::now();
        let mut limiter = RateLimiter::new();
        for _ in 0..WINDOW_LIMIT {
            assert!(limiter
                .reserve("x", Duration::ZERO, base, Duration::from_secs(600))
                .is_ok());
        }
        // 窗口满了：这一个得等到最老的滑出去。
        let next = limiter
            .reserve("x", Duration::ZERO, base, Duration::from_secs(600))
            .expect("room after the window slides");
        assert!(
            next >= base + WINDOW,
            "第 {WINDOW_LIMIT} 之后的请求必须等到窗口滑动，实际 {next:?}"
        );
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_for(0), Duration::from_secs(60));
        assert_eq!(backoff_for(1), Duration::from_secs(120));
        assert_eq!(backoff_for(2), Duration::from_secs(240));
        assert_eq!(backoff_for(4), MAX_BACKOFF);
        assert_eq!(backoff_for(9), MAX_BACKOFF);
    }

    #[test]
    fn daily_usage_rolls_over_on_new_date() {
        let mut usage = DailyUsage::default();
        assert!(usage.roll_to("2026-10-25"));
        assert!(usage.spend_personal(PERSONAL_DAILY_LIMIT - 1));
        assert!(!usage.spend_personal(2)); // 到上限了，超了不给
        assert_eq!(usage.personal_used(), PERSONAL_DAILY_LIMIT - 1);
        // 跨天清零。
        assert!(usage.roll_to("2026-10-26"));
        assert_eq!(usage.personal_used(), 0);
        assert!(usage.spend_personal(PERSONAL_DAILY_LIMIT));
    }

    #[test]
    fn personal_limit_matches_the_1_8_4_quota() {
        // 1.8.4 起「第一份读」是每个授权每天 1000 次。还按 300 拦的话，
        // 本客户端会在服务器还有大半余量时自己停下。
        assert_eq!(PERSONAL_DAILY_LIMIT, 1000);
    }

    /// 退还预扣：请求没发出去时不该白扣一次。
    #[test]
    fn refunding_undoes_a_spend() {
        let mut usage = DailyUsage::default();
        usage.roll_to("2026-10-27");
        assert!(usage.spend_personal(1));
        assert_eq!(usage.personal_used(), 1);
        usage.refund_personal(1);
        assert_eq!(usage.personal_used(), 0);

        // 退多了不会变负数：计数错一点比下溢成天文数字好。
        usage.refund_personal(5);
        assert_eq!(usage.personal_used(), 0);

        assert!(usage.spend_device_code());
        assert_eq!(usage.device_code_used(), 1);
        usage.refund_device_code();
        assert_eq!(usage.device_code_used(), 0);
        usage.refund_device_code();
        assert_eq!(usage.device_code_used(), 0);
    }

    /// 退还必须能解开「本地已满」的死结。
    ///
    /// 这正是断网时的场景：每轮轮询都扣一次、又都退回来，计数不该往上涨。
    #[test]
    fn refunding_keeps_the_local_limit_from_filling_up_offline() {
        let mut usage = DailyUsage::default();
        usage.roll_to("2026-10-27");
        // 模拟 500 轮「断网 → 预扣 → 退还」。
        for _ in 0..500 {
            assert!(usage.spend_personal(1));
            usage.refund_personal(1);
        }
        assert_eq!(usage.personal_used(), 0, "断网不该把本地额度烧掉");
        // 网恢复后照样能用满。
        assert!(usage.spend_personal(PERSONAL_DAILY_LIMIT));
    }

    #[test]
    fn server_count_overrides_the_local_guess() {
        let mut usage = DailyUsage::default();
        usage.roll_to("2026-10-27");
        assert!(usage.spend_personal(3));
        // 服务器说这个授权今天已经用了 412 次（别处的会话、重启前的请求都算在内）。
        usage.set_personal(412);
        assert_eq!(usage.personal_used(), 412);
        // 只记账不拦。
        usage.add_personal(1);
        assert_eq!(usage.personal_used(), 413);
        // 负数（服务器头写坏了）不该让计数变负。
        usage.set_personal(-5);
        assert_eq!(usage.personal_used(), 0);
    }

    #[test]
    fn device_code_has_its_own_counter() {
        let mut usage = DailyUsage::default();
        usage.roll_to("2026-10-25");
        for _ in 0..DEVICE_CODE_DAILY_LIMIT {
            assert!(usage.spend_device_code());
        }
        assert!(!usage.spend_device_code());
        // 和个人接口额度互不影响。
        assert!(usage.can_spend_personal(1));
    }

    #[test]
    fn date_formatting_matches_known_dates() {
        // 1970-01-01 00:00 UTC
        assert_eq!(format_local_date(0, 0), "1970-01-01");
        // 2024-02-29 00:00 UTC（闰日）
        assert_eq!(format_local_date(1_709_164_800, 0), "2024-02-29");
        // 2026-10-26 08:00 UTC = 16:00 UTC+8
        assert_eq!(format_local_date(1_793_001_600, 0), "2026-10-26");
        assert_eq!(format_local_date(1_793_001_600, 8 * 3600), "2026-10-26");
        // 2026-10-26 20:00 UTC = 2026-10-27 04:00 UTC+8，跨天
        assert_eq!(format_local_date(1_793_044_800, 0), "2026-10-26");
        assert_eq!(format_local_date(1_793_044_800, 8 * 3600), "2026-10-27");
    }
}
