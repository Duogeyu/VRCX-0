// SPDX-License-Identifier: MPL-2.0

//! 令牌与连接状态的持有者。
//!
//! 令牌只存在本机：由调用方（应用层）从加密存储里取出并注入，这一层不碰磁盘。
//! 这样 `integrations` 保持无持久化依赖，加密策略留在持久化层。

use std::sync::{Arc, RwLock};

use vrcx_0_vrcc_contracts::{is_dev_client, VrccConnectionSnapshot, VrccCredentialState};

use super::client::VrccClient;

/// 一次连接的全部状态。
#[derive(Clone, Debug, Default)]
pub struct VrccSessionState {
    pub token: Option<String>,
    pub star: Option<i64>,
    pub vrc_name: Option<String>,
    pub scope: Option<String>,
    /// 上次成功轮询提醒的毫秒时间戳。
    pub last_poll_at: Option<i64>,
    /// 提醒轮询游标。
    pub notifications_cursor: Option<i64>,
    /// 官方建议的轮询间隔（秒）。
    pub poll_every_secs: i64,
    /// 上一个令牌被服务器判了失效（`404 gone`）后清掉的。
    pub expired: bool,
    /// 会话代次：连上、断开、被判失效时各推进一次。
    ///
    /// 请求发出前记下它，响应回来时如果已经变了，说明那个响应属于**上一个**
    /// 会话——例如换号或重新授权之后才回来的轮询。这时不能再拿它写游标、清
    /// 令牌，见 [`VrccSession::note_poll`] 与 [`VrccSession::expire`]。
    pub generation: u64,
}

impl VrccSessionState {
    pub fn is_connected(&self) -> bool {
        self.token.as_deref().is_some_and(|token| !token.is_empty())
    }
}

/// 可克隆的会话句柄。
#[derive(Clone)]
pub struct VrccSession {
    state: Arc<RwLock<VrccSessionState>>,
    client_id: String,
    /// 这个会话**唯一**的客户端。
    ///
    /// 构造时建一次，之后每次 [`Self::client`] 都返回它的克隆（内部状态是
    /// `Arc` 共享的）。早先每个请求都 `VrccClient::new` 一个新的：每分钟窗口、
    /// 每个接口的最小间隔、429 退避、服务端下发的额度全都跟着新客户端从零开始，
    /// 等于没有；`reqwest::Client` 也每次新建，连接池用不上。
    ///
    /// 代理在这里定死：同一会话下所有请求必须走同一条网络路径，否则会出现
    /// 「某几个接口通、某几个不通」这种极难排查的状态。令牌经回调现取，换令牌
    /// 不用重建客户端。
    client: VrccClient,
}

impl VrccSession {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self::with_proxy(client_id, None)
    }

    /// 带代理设置构造。`proxy_url` 来自宿主的统一代理配置。
    pub fn with_proxy(client_id: impl Into<String>, proxy_url: Option<String>) -> Self {
        let client_id = client_id.into();
        let state = Arc::new(RwLock::new(default_state()));
        let proxy_url = proxy_url.filter(|url| !url.trim().is_empty());
        // 回调只持有状态，不持有会话本身：会话里存着客户端，客户端再回头持有
        // 会话就成了引用环。
        let token_state = Arc::clone(&state);
        let client = VrccClient::new(
            client_id.clone(),
            Arc::new(move || {
                token_state
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .token
                    .clone()
            }),
            proxy_url.as_deref(),
        );
        Self {
            state,
            client_id,
            client,
        }
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn snapshot(&self) -> VrccSessionSnapshot {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        VrccSessionSnapshot {
            connected: state.is_connected(),
            star: state.star,
            vrc_name: state.vrc_name.clone(),
            scope: state.scope.clone(),
            client: self.client_id.clone(),
            last_poll_at: state.last_poll_at,
            notifications_cursor: state.notifications_cursor,
            poll_every_secs: state.poll_every_secs,
            expired: state.expired,
            generation: state.generation,
        }
    }

    /// 当前会话代次，见 [`VrccSessionState::generation`]。
    pub fn generation(&self) -> u64 {
        self.state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .generation
    }

    /// 当前令牌，供客户端回调取用。
    pub fn token(&self) -> Option<String> {
        self.state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .token
            .clone()
    }

    /// 连接成功：写入令牌与玩家身份。
    pub fn connect(
        &self,
        token: String,
        star: Option<i64>,
        vrc_name: Option<String>,
        scope: Option<String>,
    ) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        state.token = Some(token);
        state.star = star;
        state.vrc_name = vrc_name;
        state.scope = scope;
        state.expired = false;
        // 换了授权，游标作废，下次从头问。
        state.notifications_cursor = None;
        state.last_poll_at = None;
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        // 上一个令牌的回答和它占着的间隔都不再属于任何人，见
        // `VrccClient::forget_session_state`。
        self.client.forget_session_state();
    }

    /// 断开：只清本地状态。服务器侧吊销由 `client.disconnect()` 负责。
    pub fn disconnect(&self) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        *state = VrccSessionState {
            generation: state.generation.wrapping_add(1),
            ..default_state()
        };
        drop(state);
        self.client.forget_session_state();
    }

    /// 令牌被服务器判了失效（`404 gone`）：清掉本地状态，并记下是这个原因。
    ///
    /// 和玩家自己点「断开」分开记，界面才能说清楚「连接已失效，请重新连接」。
    ///
    /// **只在代次没变时清**，返回清了没有：失败是拿 `generation` 那一代的令牌
    /// 换来的。之后会话如果已经换过（重新授权、换号），失败说的是旧令牌，不能
    /// 拿它清掉现在这一个。判断和清理在同一把锁里，中间插不进别的连接。
    pub fn expire(&self, generation: u64) -> bool {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if state.generation != generation {
            return false;
        }
        *state = VrccSessionState {
            expired: true,
            generation: state.generation.wrapping_add(1),
            ..default_state()
        };
        drop(state);
        self.client.forget_session_state();
        true
    }

    /// 记录一次成功的提醒轮询。
    ///
    /// **只在代次没变时记**，返回记了没有：轮询是拿 `generation` 那一代的游标
    /// 发出去的，换号或重新授权之后才回来的结果不能写进新会话（新会话的游标要
    /// 从头问）。
    pub fn note_poll(
        &self,
        generation: u64,
        at_ms: i64,
        cursor: Option<i64>,
        every_secs: Option<i64>,
    ) -> bool {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if state.generation != generation {
            return false;
        }
        state.last_poll_at = Some(at_ms);
        if let Some(cursor) = cursor {
            state.notifications_cursor = Some(cursor);
        }
        if let Some(every) = every_secs.filter(|value| *value > 0) {
            // 官方建议值，但不低于 25 秒硬下限。
            state.poll_every_secs = every.max(25);
        }
        true
    }

    /// 把会话的客户端指到本地测试服务器上。测试用。
    #[cfg(test)]
    pub(crate) fn point_client_at(&mut self, base: String) {
        self.client.set_base(base);
    }

    /// 这个会话的客户端。每次返回的都是**同一个**客户端的克隆，限流、退避、
    /// 额度和连接池都共享，见 [`Self::client`] 字段的说明。
    pub fn client(&self) -> VrccClient {
        self.client.clone()
    }
}

fn default_state() -> VrccSessionState {
    VrccSessionState {
        poll_every_secs: 60,
        ..Default::default()
    }
}

/// 给前端看的会话快照。
#[derive(Clone, Debug)]
pub struct VrccSessionSnapshot {
    pub connected: bool,
    pub star: Option<i64>,
    pub vrc_name: Option<String>,
    pub scope: Option<String>,
    pub client: String,
    pub last_poll_at: Option<i64>,
    pub notifications_cursor: Option<i64>,
    pub poll_every_secs: i64,
    pub expired: bool,
    /// 会话代次，见 [`VrccSessionState::generation`]。
    pub generation: u64,
}

impl From<VrccSessionSnapshot> for VrccConnectionSnapshot {
    /// 会话层只说得出现在有没有令牌、以及这次会话里令牌有没有被服务器判失效。
    ///
    /// `Disconnected` / `ClientMismatch` 这里永远不会出现：那是跨重启的记录，
    /// 只有应用层的 `VrccService::snapshot` 知道。前端拿到的快照走那条路径，
    /// 这个转换是给会话层自己的测试用的，所以宁可少说，也不编一个结局出来。
    fn from(snapshot: VrccSessionSnapshot) -> Self {
        let client_is_dev = is_dev_client(&snapshot.client);
        let credential = if snapshot.connected {
            VrccCredentialState::Active
        } else if snapshot.expired {
            VrccCredentialState::Expired
        } else {
            VrccCredentialState::Never
        };
        VrccConnectionSnapshot {
            connected: snapshot.connected,
            expired: snapshot.expired,
            credential,
            star: snapshot.star,
            vrc_name: snapshot.vrc_name,
            scope: snapshot.scope,
            client: snapshot.client,
            client_is_dev,
            last_poll_at: snapshot.last_poll_at,
            next_poll_at: snapshot
                .last_poll_at
                .map(|last| last + snapshot.poll_every_secs * 1000),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_and_disconnect_round_trip() {
        let session = VrccSession::new("dev-test");
        assert!(!session.snapshot().connected);

        session.connect(
            "tok".into(),
            Some(77),
            Some("拾光".into()),
            Some("profile".into()),
        );
        let snapshot = session.snapshot();
        assert!(snapshot.connected);
        assert_eq!(snapshot.star, Some(77));
        assert_eq!(snapshot.vrc_name.as_deref(), Some("拾光"));

        session.disconnect();
        assert!(!session.snapshot().connected);
        assert!(session.token().is_none());
    }

    #[test]
    fn connect_resets_cursor_and_poll_time() {
        let session = VrccSession::new("dev-test");
        session.connect("tok".into(), None, None, None);
        session.note_poll(session.generation(), 1_000, Some(42), Some(60));
        assert_eq!(session.snapshot().notifications_cursor, Some(42));

        // 重新授权后游标作废，避免用旧游标问到空结果。
        session.connect("tok2".into(), None, None, None);
        let snapshot = session.snapshot();
        assert_eq!(snapshot.notifications_cursor, None);
        assert_eq!(snapshot.last_poll_at, None);
    }

    #[test]
    fn poll_interval_never_below_hard_floor() {
        let session = VrccSession::new("dev-test");
        session.connect("tok".into(), None, None, None);
        // 官方建议 60，但服务器可能给个很小的值；25 秒是硬下限。
        let generation = session.generation();
        session.note_poll(generation, 1_000, None, Some(5));
        assert_eq!(session.snapshot().poll_every_secs, 25);
        session.note_poll(generation, 2_000, None, Some(90));
        assert_eq!(session.snapshot().poll_every_secs, 90);
    }

    #[test]
    fn expiring_clears_the_token_and_says_why() {
        let session = VrccSession::new("dev-test");
        session.connect("tok".into(), Some(77), None, Some("profile".into()));
        session.note_poll(session.generation(), 1_000, Some(42), Some(60));

        assert!(session.expire(session.generation()));
        let snapshot = session.snapshot();
        assert!(!snapshot.connected);
        assert!(snapshot.expired);
        assert!(session.token().is_none());
        assert_eq!(snapshot.notifications_cursor, None);

        // 重新连上后不再提示失效。
        session.connect("tok2".into(), None, None, None);
        assert!(!session.snapshot().expired);

        // 玩家自己点断开，不算失效。
        session.disconnect();
        assert!(!session.snapshot().expired);
    }

    #[test]
    fn next_poll_at_derives_from_last_poll() {
        let session = VrccSession::new("dev-test");
        session.connect("tok".into(), None, None, None);
        session.note_poll(session.generation(), 1_000_000, None, Some(60));
        let projection: VrccConnectionSnapshot = session.snapshot().into();
        assert_eq!(projection.next_poll_at, Some(1_060_000));
    }

    #[test]
    fn every_client_from_a_session_is_the_same_client() {
        // 早先每次 `client()` 都新建一个：限流、退避、额度从零开始，等于没有。
        let session = VrccSession::new("dev-test");
        assert!(session.client().shares_state_with(&session.client()));
        // 换了令牌也还是同一个：令牌经回调现取。
        session.connect("tok".into(), None, None, None);
        let before = session.client();
        session.connect("tok2".into(), None, None, None);
        assert!(before.shares_state_with(&session.client()));
    }

    #[test]
    fn the_generation_moves_on_every_session_change() {
        let session = VrccSession::new("dev-test");
        let start = session.generation();
        session.connect("tok".into(), None, None, None);
        let connected = session.generation();
        assert_ne!(connected, start);
        session.disconnect();
        let disconnected = session.generation();
        assert_ne!(disconnected, connected);
        session.connect("tok".into(), None, None, None);
        assert!(session.expire(session.generation()));
        assert_ne!(session.generation(), disconnected);
    }

    #[test]
    fn a_poll_from_an_earlier_session_does_not_write_the_cursor() {
        let session = VrccSession::new("dev-test");
        session.connect("tok-a".into(), None, None, None);
        let stale = session.generation();
        // 轮询还在路上，换成了另一个令牌（换号或重新授权）。
        session.connect("tok-b".into(), None, None, None);

        assert!(!session.note_poll(stale, 1_000, Some(42), Some(60)));
        assert_eq!(session.snapshot().notifications_cursor, None);
        assert_eq!(session.snapshot().last_poll_at, None);

        assert!(session.note_poll(session.generation(), 1_000, Some(42), Some(60)));
        assert_eq!(session.snapshot().notifications_cursor, Some(42));
    }

    #[test]
    fn a_failure_from_an_earlier_session_does_not_expire_the_new_token() {
        let session = VrccSession::new("dev-test");
        session.connect("old".into(), None, None, None);
        let stale = session.generation();
        // 重新授权：旧令牌随之失效，它还在路上的请求会回来 `404 gone`。
        session.connect("new".into(), None, None, None);

        assert!(!session.expire(stale));
        assert!(session.snapshot().connected);
        assert_eq!(session.token().as_deref(), Some("new"));
    }
}
