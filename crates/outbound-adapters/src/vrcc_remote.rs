//! 星遇同频网络端口的适配器。
//!
//! **为什么需要这一层**：应用层（`vrcx_0_application::vrcc`）要编排业务，
//! 但协议细节在 `vrcx_0_integrations::vrcc`。架构测试
//! `application_crates_do_not_depend_on_outbound_implementations` 明确禁止
//! 应用层依赖出向实现，所以应用层只声明 [`VrccRemote`] 端口，实现放在这里。
//!
//! 这一层很薄：把应用层的调用翻译成 `VrccSession` / `VrccClient` 的调用，
//! 并把两边形状不同的类型对上（`VrccSessionSnapshot` → [`VrccConnection`]、
//! `VrccLocalUsage` → 应用层的用量快照、`DailyUsage` → [`VrccUsageRecord`]）。
//! 不在这里做业务判断——要不要重连、未读怎么算，都属于应用层。

use std::sync::Arc;

use vrcx_0_application::vrcc::{
    VrccConnection, VrccFuture, VrccLocalUsage, VrccRemote, VrccUsageRecord, VrccUsageStore,
};
use vrcx_0_contracts::vrcc::{
    VrccAnnouncements, VrccBoard, VrccBoardSort, VrccDeviceCode, VrccMutuals, VrccMyMap, VrccNotes,
    VrccNotifications, VrccRecentNotifications, VrccStatus, VrccToday, VrccTokenResponse,
    VrccUnread,
};
use vrcx_0_integrations::vrcc::{
    now_millis, DailyUsage, DailyUsageStore, VrccSession, DEVICE_CODE_DAILY_LIMIT, WINDOW_LIMIT,
};

/// 把应用层的用量端口接到 `integrations` 的 `DailyUsageStore` 上。
///
/// 两者搬运的是同一组数（日期、第一份读、设备码），只是类型分属两层。
struct UsageBridge(Arc<dyn VrccUsageStore>);

impl DailyUsageStore for UsageBridge {
    fn load(&self) -> Option<DailyUsage> {
        let record = self.0.load()?;
        Some(DailyUsage::restore(
            &record.date,
            record.personal,
            record.device_code,
        ))
    }

    fn save(&self, usage: &DailyUsage) {
        self.0.save(&VrccUsageRecord {
            date: usage.date().to_string(),
            personal: usage.personal_used(),
            device_code: usage.device_code_used(),
        });
    }
}

/// 星遇同频网络端口的实现。
///
/// 持有 `VrccSession`：它同时管令牌状态和「按当前令牌造客户端」，所以
/// 每个请求都现造一个客户端，令牌换了立刻生效，不用额外同步。
pub struct VrccRemoteAdapter {
    session: VrccSession,
}

impl VrccRemoteAdapter {
    /// 构造适配器。
    ///
    /// `proxy_url` 用宿主应用的统一代理设置：不传的话在需要代理才能出网的
    /// 环境里，所有请求都会 DNS 超时。
    pub fn new(
        client_id: impl Into<String>,
        proxy_url: Option<String>,
        usage_store: Option<Arc<dyn VrccUsageStore>>,
    ) -> Self {
        let mut session = VrccSession::with_proxy(client_id, proxy_url);
        if let Some(store) = usage_store {
            session = session.with_usage_store(Arc::new(UsageBridge(store)));
        }
        Self { session }
    }

    /// 按当前会话造一个客户端。每个请求一次，确保用的是当前令牌。
    fn client(&self) -> vrcx_0_integrations::vrcc::VrccClient {
        self.session.client()
    }
}

impl VrccRemote for VrccRemoteAdapter {
    fn boards<'a>(&'a self, limit: u32, sort: VrccBoardSort) -> VrccFuture<'a, VrccBoard> {
        Box::pin(async move { self.client().boards(limit, sort).await })
    }

    fn status<'a>(&'a self) -> VrccFuture<'a, VrccStatus> {
        Box::pin(async move { self.client().status().await })
    }

    fn fetch_site_asset<'a>(&'a self, path: &'a str) -> VrccFuture<'a, Vec<u8>> {
        Box::pin(async move { self.client().fetch_site_asset(path).await })
    }

    fn announcements<'a>(&'a self) -> VrccFuture<'a, VrccAnnouncements> {
        Box::pin(async move { self.client().announcements().await })
    }

    fn device_code<'a>(&'a self, scope: Option<&'a str>) -> VrccFuture<'a, VrccDeviceCode> {
        Box::pin(async move { self.client().device_code(scope).await })
    }

    fn device_token<'a>(&'a self, device_code: &'a str) -> VrccFuture<'a, VrccTokenResponse> {
        Box::pin(async move { self.client().device_token(device_code).await })
    }

    fn revoke<'a>(&'a self) -> VrccFuture<'a, ()> {
        Box::pin(async move { self.client().disconnect().await })
    }

    fn recent_notifications<'a>(&'a self, mark: bool) -> VrccFuture<'a, VrccRecentNotifications> {
        Box::pin(async move { self.client().recent_notifications(mark).await })
    }

    fn notifications<'a>(&'a self, since: Option<i64>) -> VrccFuture<'a, VrccNotifications> {
        Box::pin(async move { self.client().notifications(since).await })
    }

    fn my_map<'a>(&'a self, world: &'a str) -> VrccFuture<'a, VrccMyMap> {
        Box::pin(async move { self.client().my_map(world).await })
    }

    fn unread<'a>(&'a self) -> VrccFuture<'a, VrccUnread> {
        Box::pin(async move { self.client().unread().await })
    }

    fn notes<'a>(&'a self) -> VrccFuture<'a, VrccNotes> {
        Box::pin(async move { self.client().notes().await })
    }

    fn today<'a>(&'a self) -> VrccFuture<'a, VrccToday> {
        Box::pin(async move { self.client().today().await })
    }

    fn mutuals<'a>(&'a self) -> VrccFuture<'a, VrccMutuals> {
        Box::pin(async move { self.client().mutuals().await })
    }

    fn connect(
        &self,
        token: String,
        star: Option<i64>,
        vrc_name: Option<String>,
        scope: Option<String>,
    ) {
        self.session.connect(token, star, vrc_name, scope);
    }

    fn disconnect(&self) {
        self.session.disconnect();
    }

    fn expire(&self) {
        self.session.expire();
    }

    fn connection(&self) -> VrccConnection {
        let snapshot = self.session.snapshot();
        VrccConnection {
            connected: snapshot.connected,
            expired: snapshot.expired,
            star: snapshot.star,
            vrc_name: snapshot.vrc_name,
            scope: snapshot.scope,
            client: snapshot.client,
            notifications_cursor: snapshot.notifications_cursor,
            last_poll_at: snapshot.last_poll_at,
            poll_every_secs: snapshot.poll_every_secs,
        }
    }

    fn client_id(&self) -> String {
        self.session.client_id().to_string()
    }

    fn local_usage(&self) -> VrccLocalUsage {
        let usage = self.client().local_usage();
        VrccLocalUsage {
            personal_used_today: usage.personal_used_today,
            personal_daily_limit: usage.personal_daily_limit,
            personal_from_server: usage.personal_from_server,
            unread: usage.unread,
            device_code_used_today: usage.device_code_used_today,
            window_used: usage.window_used,
            backoff_remaining_ms: usage.backoff_remaining.as_millis() as i64,
            date: usage.date,
        }
    }

    fn note_poll(&self, at_ms: i64, cursor: Option<i64>, every_secs: Option<i64>) {
        self.session.note_poll(at_ms, cursor, every_secs);
    }

    fn window_limit(&self) -> i64 {
        WINDOW_LIMIT as i64
    }

    fn device_code_daily_limit(&self) -> i64 {
        DEVICE_CODE_DAILY_LIMIT
    }

    fn now_millis(&self) -> i64 {
        now_millis()
    }
}

/// 编译期确认：适配器必须能跨线程共享。
///
/// 端口注入靠 `Arc<dyn VrccRemote>` 进应用层；如果适配器不是 `Send + Sync`，
/// 那个注入点在多线程 runtime 下会出问题。宁可在编译期钉住。
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<VrccRemoteAdapter>();
};
