// SPDX-License-Identifier: MPL-2.0

//! 应用层测试：用内存仓库替换持久化，验证编排逻辑。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use vrcx_0_contracts::vrcc::{
    HttpDetail, LocalThrottle, VrccBoardMap, VrccBoardModel, VrccCrossingCards, VrccCrossingStar,
    VrccCrossings, VrccModelBoard, VrccModelDetail, VrccUnread,
};
// ---------------------------------------------------------------------------
// 造错误的几个便捷函数
//
// 重构后错误是「层级 + 细节」两段，直接写全每次都很啰嗦。这几个函数只用于
// 测试，名字直接说明造的是哪一层的错。
// ---------------------------------------------------------------------------

/// 传输层失败（没拿到响应）。
fn transport(message: &str) -> VrccError {
    VrccError::new(
        ErrorLayer::Transport,
        ErrorDetail::Transport {
            kind: TransportKind::Connect,
            message: message.to_string(),
        },
    )
}

/// 服务器拒绝（拿到了响应，状态码不是 2xx）。
fn http(status: u16, error: &str) -> VrccError {
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

/// 缺权限的 403。
fn missing_scope(need: &str) -> VrccError {
    VrccError::new(
        ErrorLayer::Server,
        ErrorDetail::Http(HttpDetail {
            status: 403,
            error: "scope".to_string(),
            need: Some(need.to_string()),
            why: None,
            retry_after: None,
            op: None,
        }),
    )
}

/// 本机限流（请求没发出去）。
fn throttled_locally(secs: u64, reason: LocalThrottle) -> VrccError {
    VrccError::new(
        ErrorLayer::Client,
        ErrorDetail::ThrottledLocally {
            retry_after_secs: secs,
            reason,
        },
    )
}

/// 没有令牌。
fn token_missing() -> VrccError {
    VrccError::new(ErrorLayer::Session, ErrorDetail::TokenMissing)
}

/// 内存令牌仓库。
#[derive(Default)]
struct MemoryStore {
    inner: Mutex<Option<StoredCredential>>,
    /// 「上一个令牌的结局」。和令牌分开：`clear` 不动它，这也是真仓库的行为。
    outcome: Mutex<VrccCredentialState>,
    /// 「首连引导讲到哪一步」。同样按账号，和凭据互不影响。
    guidance: Mutex<VrccGuidanceState>,
}

impl MemoryStore {
    fn with(credential: StoredCredential) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Some(credential)),
            ..Self::default()
        })
    }

    /// 造一个「只剩结局、没有令牌」的仓库：模拟重启之后的样子。
    fn with_outcome(state: VrccCredentialState) -> Arc<Self> {
        Arc::new(Self {
            outcome: Mutex::new(state),
            ..Self::default()
        })
    }

    fn outcome(&self) -> VrccCredentialState {
        *self.outcome.lock().unwrap()
    }

    fn guidance(&self) -> VrccGuidanceState {
        *self.guidance.lock().unwrap()
    }
}

impl VrccTokenStore for MemoryStore {
    fn load(&self) -> Result<Option<StoredCredential>> {
        Ok(self.inner.lock().unwrap().clone())
    }

    fn save(&self, credential: &StoredCredential) -> Result<()> {
        *self.inner.lock().unwrap() = Some(credential.clone());
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        *self.inner.lock().unwrap() = None;
        Ok(())
    }

    fn load_state(&self) -> Result<VrccCredentialState> {
        Ok(self.outcome())
    }

    fn save_state(&self, state: VrccCredentialState) -> Result<()> {
        // 与真实仓库一致：`Active` / `Never` 落盘等于把记录清掉。
        *self.outcome.lock().unwrap() = match state {
            VrccCredentialState::Active | VrccCredentialState::Never => VrccCredentialState::Never,
            other => other,
        };
        Ok(())
    }

    fn load_guidance(&self) -> Result<VrccGuidanceState> {
        Ok(self.guidance())
    }

    fn save_guidance(&self, state: VrccGuidanceState) -> Result<()> {
        *self.guidance.lock().unwrap() = state;
        Ok(())
    }
}

/// 假网络端口：只记录调用、按需返回预设结果。
///
/// 这正是端口层的意义：应用层的编排（未读怎么算、什么时候清令牌、
/// 配额报什么数）可以完全不碰网络就测掉。过去这些测试靠真的
/// `VrccSession`，一联网就拖慢、还会碰真实限流。
#[derive(Default)]
struct FakeRemote {
    /// 当前连接状态，由 `connect`/`disconnect`/`expire` 改。
    connection: Mutex<VrccConnection>,
    /// 下一次个人调用返回的错误。
    next_error: Mutex<Option<VrccError>>,
    /// `my_map` 的返回。设了就用它，没设就按「令牌缺失」处理。
    my_map: Mutex<Option<VrccMyMap>>,
    /// `recent_notifications` 的返回。
    recent: Mutex<Option<VrccRecentNotifications>>,
    /// `unread` 的返回。
    unread: Mutex<Option<VrccUnread>>,
    /// 记录 `note_poll` 收到的参数。
    notes_polled: Mutex<Vec<(Option<i64>, Option<i64>)>>,
    /// 记录 `device_code` 收到的 `scope`。
    device_code_scopes: Mutex<Vec<Option<String>>>,
    /// `device_token` 成功时返回的令牌；没设就按传输失败处理。
    device_token_response: Mutex<Option<VrccTokenResponse>>,
    /// `crossings` / `crossing_cards` 的返回。
    crossings: Mutex<Option<VrccCrossings>>,
    cards: Mutex<Option<VrccCrossingCards>>,
    /// 记录两个星轨接口收到的 `mark`。
    crossings_mark: Mutex<Option<bool>>,
    cards_mark: Mutex<Option<bool>>,
    /// `models_board` 的返回；`None` 时按「旧服务器没这个接口」处理。
    models: Mutex<Option<VrccModelBoard>>,
    /// 记录 `models_board` 收到的 `(limit, sort)`。
    models_calls: Mutex<Vec<(u32, VrccBoardSort)>>,
    /// `model()` 的返回；`None` 时按「不在榜上」处理。
    model_detail: Mutex<Option<VrccModelDetail>>,
    /// 记录 `model()` 收到的 id，用来验证缓存是否真的省掉了请求。
    model_calls: Mutex<Vec<i64>>,
}

impl FakeRemote {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            connection: Mutex::new(VrccConnection {
                client: "dev-test".to_string(),
                poll_every_secs: 60,
                ..VrccConnection::default()
            }),
            ..Self::default()
        })
    }

    /// 让假端口进入「已连接」。
    ///
    /// 接收 `&self` 而不是 `self`：`new()` 已经返回 `Arc`，再取所有权就没法用了。
    fn mark_connected(&self) {
        self.connection.lock().unwrap().connected = true;
    }

    /// 预设下一次 `recent_notifications` 的返回。
    fn set_recent(&self, recent: VrccRecentNotifications) {
        *self.recent.lock().unwrap() = Some(recent);
    }

    /// 预设下一次 `unread` 的返回。
    fn set_unread(&self, unread: VrccUnread) {
        *self.unread.lock().unwrap() = Some(unread);
    }

    /// 预设星轨相交列表的返回。
    fn set_crossings(&self, crossings: VrccCrossings) {
        *self.crossings.lock().unwrap() = Some(crossings);
    }

    /// 预设星轨相交星卡的返回。
    fn set_crossing_cards(&self, cards: VrccCrossingCards) {
        *self.cards.lock().unwrap() = Some(cards);
    }

    /// 预设 `device_token` 成功时返回的令牌。
    fn set_device_token(&self, response: VrccTokenResponse) {
        *self.device_token_response.lock().unwrap() = Some(response);
    }

    /// 让下一次个人调用失败。
    fn set_error(&self, error: VrccError) {
        *self.next_error.lock().unwrap() = Some(error);
    }

    fn take_error(&self) -> Option<VrccError> {
        self.next_error.lock().unwrap().take()
    }

    /// 预设模型榜的返回。
    fn set_models(&self, board: VrccModelBoard) {
        *self.models.lock().unwrap() = Some(board);
    }

    /// 预设单模型详情的返回。
    fn set_model_detail(&self, detail: VrccModelDetail) {
        *self.model_detail.lock().unwrap() = Some(detail);
    }
}

impl VrccRemote for FakeRemote {
    fn boards<'a>(&'a self, _limit: u32, _sort: VrccBoardSort) -> VrccFuture<'a, VrccBoard> {
        Box::pin(async move { Err(transport("fake")) })
    }

    /// 模型榜：记下调用参数，返回预设值（没预设就当旧服务器没这个接口）。
    ///
    /// `next_error` 同样生效：测试「限流窗口内回退缓存」需要它。
    fn models_board<'a>(
        &'a self,
        limit: u32,
        sort: VrccBoardSort,
    ) -> VrccFuture<'a, VrccModelBoard> {
        self.models_calls.lock().unwrap().push((limit, sort));
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            match self.models.lock().unwrap().clone() {
                Some(board) => Ok(board),
                // 没有预设：模拟旧服务器，回答 `404 not_found`。
                None => Err(VrccError::new(
                    ErrorLayer::Server,
                    ErrorDetail::Http(HttpDetail {
                        status: 404,
                        error: "not_found".to_string(),
                        need: None,
                        why: None,
                        retry_after: None,
                        op: Some("models"),
                    }),
                )),
            }
        })
    }

    /// 单模型：记下这一次 id，返回预设详情（没预设就是「不在榜上」）。
    fn model<'a>(&'a self, id: i64) -> VrccFuture<'a, Option<VrccModelDetail>> {
        self.model_calls.lock().unwrap().push(id);
        Box::pin(async move { Ok(self.model_detail.lock().unwrap().clone()) })
    }
    fn maps<'a>(&'a self, _worlds: &'a [String]) -> VrccFuture<'a, VrccBoard> {
        Box::pin(async move { Err(transport("fake")) })
    }
    fn status<'a>(&'a self) -> VrccFuture<'a, VrccStatus> {
        Box::pin(async move { Err(transport("fake")) })
    }
    fn fetch_site_asset<'a>(&'a self, _path: &'a str) -> VrccFuture<'a, Vec<u8>> {
        Box::pin(async move { Err(transport("fake")) })
    }
    fn announcements<'a>(&'a self) -> VrccFuture<'a, VrccAnnouncements> {
        Box::pin(async move { Err(transport("fake")) })
    }
    fn stickers<'a>(&'a self) -> VrccFuture<'a, VrccStickers> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            Ok(VrccStickers::default())
        })
    }
    fn device_code<'a>(&'a self, scope: Option<&'a str>) -> VrccFuture<'a, VrccDeviceCode> {
        self.device_code_scopes
            .lock()
            .unwrap()
            .push(scope.map(str::to_string));
        Box::pin(async move { Err(transport("fake")) })
    }
    fn device_token<'a>(&'a self, _code: &'a str) -> VrccFuture<'a, VrccTokenResponse> {
        let response = self.device_token_response.lock().unwrap().clone();
        Box::pin(async move { response.ok_or_else(|| transport("fake")) })
    }
    fn revoke<'a>(&'a self) -> VrccFuture<'a, ()> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            Ok(())
        })
    }
    fn recent_notifications<'a>(&'a self, _mark: bool) -> VrccFuture<'a, VrccRecentNotifications> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            self.recent
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(token_missing)
        })
    }
    fn notifications<'a>(&'a self, _since: Option<i64>) -> VrccFuture<'a, VrccNotifications> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            Err(token_missing())
        })
    }
    fn my_map<'a>(&'a self, _world: &'a str) -> VrccFuture<'a, VrccMyMap> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            if let Some(map) = self.my_map.lock().unwrap().clone() {
                return Ok(map);
            }
            Err(token_missing())
        })
    }
    fn my_maps<'a>(&'a self) -> VrccFuture<'a, VrccRatedMaps> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            Err(token_missing())
        })
    }
    fn map_page<'a>(&'a self, _world: &'a str) -> VrccFuture<'a, VrccMapPage> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            // 公开接口：没有令牌也能答。默认给一份空页面，让「404 转 None」
            // 这类测试只要注入错误就能覆盖到。
            Ok(VrccMapPage::default())
        })
    }
    fn my_map_page<'a>(
        &'a self,
        _world: &'a str,
        _sort: Option<&'a str>,
    ) -> VrccFuture<'a, VrccMyMapPage> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            // 带令牌的页面：默认给一份空的，让「404 转 None」「未连接先拒绝」
            // 这类测试只要注入错误就能覆盖到。
            Ok(VrccMyMapPage::default())
        })
    }
    fn rate_map<'a>(
        &'a self,
        _world: &'a str,
        _score: i64,
        _text: &'a VrccRatingText,
    ) -> VrccFuture<'a, VrccMyMap> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            // 写操作默认成功，返回一份空的地图状态。
            Ok(VrccMyMap::default())
        })
    }
    fn delete_map_rating<'a>(&'a self, _world: &'a str) -> VrccFuture<'a, ()> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            Ok(())
        })
    }
    fn vote_map_content<'a>(
        &'a self,
        _kind: VrccVoteKind,
        _id: i64,
        _vote: VrccVote,
    ) -> VrccFuture<'a, VrccVoteResult> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            Ok(VrccVoteResult::default())
        })
    }
    fn my_board<'a>(&'a self, _sort: VrccMyBoardSort) -> VrccFuture<'a, VrccMyBoard> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            Err(token_missing())
        })
    }
    fn unread<'a>(&'a self) -> VrccFuture<'a, VrccUnread> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            self.unread
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(token_missing)
        })
    }
    fn notes<'a>(&'a self) -> VrccFuture<'a, VrccNotes> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            Err(token_missing())
        })
    }
    fn today<'a>(&'a self) -> VrccFuture<'a, VrccToday> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            Err(token_missing())
        })
    }
    fn mutuals<'a>(&'a self) -> VrccFuture<'a, VrccMutuals> {
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            Err(token_missing())
        })
    }

    /// 星轨相交列表。假实现记录收到的 `mark`，并按预设返回。
    fn crossings<'a>(&'a self, mark: bool) -> VrccFuture<'a, VrccCrossings> {
        *self.crossings_mark.lock().unwrap() = Some(mark);
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            let stars = self
                .crossings
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(token_missing)?;
            Ok(stars)
        })
    }

    /// 星轨相交的完整星卡。同样记录 `mark`，并按预设返回。
    fn crossing_cards<'a>(&'a self, mark: bool) -> VrccFuture<'a, VrccCrossingCards> {
        *self.cards_mark.lock().unwrap() = Some(mark);
        Box::pin(async move {
            if let Some(error) = self.take_error() {
                return Err(error);
            }
            self.cards.lock().unwrap().clone().ok_or_else(token_missing)
        })
    }

    fn connect(
        &self,
        _token: String,
        star: Option<i64>,
        vrc_name: Option<String>,
        scope: Option<String>,
    ) {
        let mut connection = self.connection.lock().unwrap();
        connection.connected = true;
        connection.expired = false;
        connection.star = star;
        connection.vrc_name = vrc_name;
        connection.scope = scope;
        connection.notifications_cursor = None;
    }
    fn disconnect(&self) {
        let mut connection = self.connection.lock().unwrap();
        *connection = VrccConnection {
            client: connection.client.clone(),
            poll_every_secs: 60,
            ..VrccConnection::default()
        };
    }
    fn expire(&self) {
        let mut connection = self.connection.lock().unwrap();
        connection.connected = false;
        connection.expired = true;
        connection.notifications_cursor = None;
    }
    fn connection(&self) -> VrccConnection {
        self.connection.lock().unwrap().clone()
    }
    fn client_id(&self) -> String {
        self.connection.lock().unwrap().client.clone()
    }
    fn local_usage(&self) -> VrccLocalUsage {
        VrccLocalUsage {
            reads: Some((12, 1000)),
            reset_secs: Some(3600),
            ..VrccLocalUsage::default()
        }
    }
    fn note_poll(&self, _at_ms: i64, cursor: Option<i64>, every_secs: Option<i64>) {
        self.notes_polled.lock().unwrap().push((cursor, every_secs));
    }
    fn window_limit(&self) -> i64 {
        120
    }
    fn now_millis(&self) -> i64 {
        1_700_000_000_000
    }
}

#[test]
fn new_restores_connected_session_from_store() {
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        star: Some(77),
        vrc_name: Some("拾光".into()),
        scope: Some("profile maps".into()),
        client_id: None,
    });
    let service = VrccService::new(store, FakeRemote::new());
    let snapshot = service.snapshot();
    assert!(snapshot.connected);
    assert_eq!(snapshot.star, Some(77));
    assert_eq!(snapshot.vrc_name.as_deref(), Some("拾光"));
    assert_eq!(snapshot.client, "dev-test");
}

#[test]
fn new_without_stored_token_is_disconnected() {
    let store = Arc::new(MemoryStore::default());
    let service = VrccService::new(store, FakeRemote::new());
    assert!(!service.snapshot().connected);
}

#[test]
fn usage_reports_what_the_server_said() {
    // 口径要求：只说「今日已用 N/上限」，不说「还剩」。
    let service = VrccService::new(Arc::new(MemoryStore::default()), FakeRemote::new());
    let usage = service.usage();
    // 服务端 `RateLimit-*` 说的那一份。
    assert_eq!(usage.personal_used_today, Some(12));
    assert_eq!(usage.personal_daily_limit, Some(1000));
    assert_eq!(usage.window_limit, 120);
    // 重置时间来自服务端（到北京时间零点的秒数），不是本地推算的日期。
    assert_eq!(usage.reset_secs, Some(3600));
    // 未读数那一份还没拿到过响应头，不显示。
    assert_eq!(usage.unread_used_today, None);
    assert_eq!(usage.unread_daily_limit, None);
}

#[test]
fn a_gone_token_is_dropped_and_remembered_as_expired() {
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        star: Some(77),
        vrc_name: None,
        scope: Some("profile notifications maps".into()),
        client_id: None,
    });
    let remote = FakeRemote::new();
    remote.mark_connected();
    let service = VrccService::new(store.clone(), remote);
    service.note_unread(&VrccUnread {
        inbox: 2,
        ..Default::default()
    });

    let error = service.settle(http(404, "gone"));
    assert!(error.is_gone());

    let snapshot = service.snapshot();
    assert!(!snapshot.connected);
    assert!(snapshot.expired, "界面要能说出是失效，不是玩家自己断开的");
    // 跨重启的结论也要是「失效」，而不是退化成「从没连过」。
    assert_eq!(snapshot.credential, VrccCredentialState::Expired);
    // 本地存的令牌也清掉了，重启后不会拿着死令牌再问。
    assert!(store.load().unwrap().is_none());
    // 未读数不再属于任何人；令牌也没了，腕表整块消失（底栏回落到玩家数）。
    assert!(service.cached_unread().is_none());
    assert!(
        service.wrist_summary(None).is_none(),
        "令牌失效后腕表不该再显示"
    );
}

#[test]
fn other_404s_leave_the_token_alone() {
    // `star_gone`（对方不在了）、`expired`（签名图片失效）、`not_found`（旧服务器
    // 没有这个接口）都不是令牌的事。
    for code in ["star_gone", "expired", "not_found"] {
        let store = MemoryStore::with(StoredCredential {
            token: "tok".into(),
            ..Default::default()
        });
        let remote = FakeRemote::new();
        remote.mark_connected();
        let service = VrccService::new(store.clone(), remote);
        service.settle(http(404, code));
        assert!(service.snapshot().connected, "{code}");
        assert!(!service.snapshot().expired, "{code}");
        assert!(store.load().unwrap().is_some(), "{code}");
    }
}

// ---------------------------------------------------------------------------
// 凭据状态：界面靠它决定「要不要把玩家当新人讲一遍」
//
// 这一组测的都是**跨重启**的结论：只有 `connected` / `expired` 两个布尔时，
// 「从没连过」和「连过又断开」长得一模一样。
// ---------------------------------------------------------------------------

#[test]
fn a_fresh_account_has_never_connected() {
    let service = VrccService::new(Arc::new(MemoryStore::default()), FakeRemote::new());
    assert_eq!(service.snapshot().credential, VrccCredentialState::Never);
}

#[test]
fn a_stored_token_reads_as_active() {
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        ..Default::default()
    });
    let service = VrccService::new(store, FakeRemote::new());
    assert_eq!(service.snapshot().credential, VrccCredentialState::Active);
}

#[test]
fn a_token_from_another_client_is_a_mismatch_not_a_first_run() {
    // 换了正式标识、或改了构建期的 `X-Client`：老令牌对每个请求都会吃
    // `403 client_mismatch`，令牌整个作废，只能重新授权。
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        client_id: Some("app_someone_else".into()),
        ..Default::default()
    });
    let service = VrccService::new(store.clone(), FakeRemote::new());
    let snapshot = service.snapshot();
    assert_eq!(snapshot.credential, VrccCredentialState::GrantMismatch);
    assert!(!snapshot.connected);
    // 死令牌清掉了，结局留下了：重启后仍然是「不匹配」，不能退化成新人。
    assert!(store.load().unwrap().is_none());
    assert_eq!(store.outcome(), VrccCredentialState::GrantMismatch);
    assert_eq!(
        VrccService::new(store, FakeRemote::new())
            .snapshot()
            .credential,
        VrccCredentialState::GrantMismatch
    );
}

#[test]
fn a_token_with_fewer_scopes_stays_connected() {
    // 权限是签发时定死的。这个构建要的比令牌拿到的多，缺的那几个接口会一直
    // `403 scope`——但令牌本身还能用，**不能清掉**，那是白扔一份授权。
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        client_id: Some("dev-test".into()),
        // `dev-` 标识要的是 `profile notifications maps`，这份令牌只有 `profile`。
        scope: Some("profile".into()),
        ..Default::default()
    });
    let service = VrccService::new(store.clone(), FakeRemote::new());
    let snapshot = service.snapshot();
    assert_eq!(snapshot.credential, VrccCredentialState::ScopeMismatch);
    assert!(
        snapshot.connected,
        "权限不够不等于没连上：提醒和榜单照样能用"
    );
    assert!(store.load().unwrap().is_some(), "令牌必须留着");
    // 这一档不落盘：令牌在，重启后重新比一遍就有同样的结论。
    assert_eq!(store.outcome(), VrccCredentialState::Never);
}

#[test]
fn a_token_with_enough_scopes_is_plain_active() {
    // 权限刚好够（甚至更多）时不该报任何问题。
    for scope in [
        "profile notifications maps",
        "profile notifications maps mutuals",
    ] {
        let store = MemoryStore::with(StoredCredential {
            token: "tok".into(),
            client_id: Some("dev-test".into()),
            scope: Some(scope.into()),
            ..Default::default()
        });
        let service = VrccService::new(store, FakeRemote::new());
        assert_eq!(
            service.snapshot().credential,
            VrccCredentialState::Active,
            "{scope}"
        );
    }
}

#[test]
fn an_archive_without_a_recorded_scope_is_not_called_a_mismatch() {
    // 旧存档没记 `scope`：不知道就说不知道，不要凭空让玩家重新授权。
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        client_id: Some("dev-test".into()),
        scope: None,
        ..Default::default()
    });
    let service = VrccService::new(store, FakeRemote::new());
    assert_eq!(service.snapshot().credential, VrccCredentialState::Active);
}

#[tokio::test]
async fn a_manual_disconnect_survives_a_restart() {
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        ..Default::default()
    });
    let remote = FakeRemote::new();
    remote.mark_connected();
    let service = VrccService::new(store.clone(), remote);
    service.disconnect().await.unwrap();
    assert_eq!(
        service.snapshot().credential,
        VrccCredentialState::Disconnected
    );

    // 重启：只凭仓库重建，仍然是「玩家自己断的」。
    let restarted = VrccService::new(store, FakeRemote::new());
    assert_eq!(
        restarted.snapshot().credential,
        VrccCredentialState::Disconnected
    );
}

#[test]
fn connecting_again_clears_the_recorded_outcome() {
    let store = MemoryStore::with_outcome(VrccCredentialState::Expired);
    let service = VrccService::new(store.clone(), FakeRemote::new());
    assert_eq!(service.snapshot().credential, VrccCredentialState::Expired);

    // 设备授权成功的等价动作：记录被清掉，状态回到「已连接」。
    service.set_credential_state(VrccCredentialState::Active);
    assert_eq!(store.outcome(), VrccCredentialState::Never);
    assert_eq!(service.snapshot().credential, VrccCredentialState::Active);
}

#[test]
fn guidance_starts_unseen_and_survives_a_restart() {
    let store = Arc::new(MemoryStore::default());
    let service = VrccService::new(
        Arc::clone(&store) as Arc<dyn VrccTokenStore>,
        FakeRemote::new(),
    );
    assert_eq!(service.guidance_state(), VrccGuidanceState::Unseen);

    service
        .set_guidance_state(VrccGuidanceState::Postponed)
        .expect("save guidance");
    assert_eq!(store.guidance(), VrccGuidanceState::Postponed);

    // 重启：只凭仓库重建，表态还在。
    let restarted = VrccService::new(store, FakeRemote::new());
    assert_eq!(restarted.guidance_state(), VrccGuidanceState::Postponed);
}

#[test]
fn connecting_and_disconnecting_leaves_the_guidance_alone() {
    // 引导说的是「讲过了没有」，不是「连没连上」：连上不该把它抹掉，
    // 按「看过引导」记下之后也不该因为断开又回到未读。
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        ..Default::default()
    });
    let service = VrccService::new(store.clone(), FakeRemote::new());
    service
        .set_guidance_state(VrccGuidanceState::Seen)
        .expect("save guidance");
    service.set_credential_state(VrccCredentialState::Disconnected);
    assert_eq!(service.guidance_state(), VrccGuidanceState::Seen);
    assert_eq!(store.guidance(), VrccGuidanceState::Seen);
}

#[test]
fn wrist_prefers_the_unread_counts_over_recent() {
    let remote = FakeRemote::new();
    remote.mark_connected();
    let service = VrccService::new(
        Arc::new(MemoryStore::default()),
        Arc::clone(&remote) as Arc<dyn VrccRemote>,
    );
    // 旧服务器：只有「最近的提醒」的未读条数。
    service.note_recent(2);
    assert_eq!(service.wrist_summary(None).unwrap().unread, 2);

    // 有了 `me/unread`：腕表显示几处红点的合计。
    let counts = VrccUnread {
        inbox: 1,
        cross: Some(1),
        chats: 2,
        notes: 3,
    };
    assert!(service.note_unread(&counts), "第一次拿到算变化");
    assert_eq!(service.wrist_summary(None).unwrap().unread, 7);
    // 之后「最近的提醒」不再覆盖它。
    service.note_recent(0);
    assert_eq!(service.wrist_summary(None).unwrap().unread, 7);
    // 一样的数不算变化，后台就不必通知前端。
    assert!(!service.note_unread(&counts));
    assert_eq!(service.cached_unread(), Some(counts));
}

/// 重新授权（换账号）后不能先显示上一个账号的腕表摘要。
///
/// `poll_device_auth` 会把连接状态重新置为 true，若不清缓存，
/// `wrist_summary` 的 `connected` 守卫就放行，旧账号的 `●N` 立刻又可见。
#[tokio::test]
async fn reconnecting_clears_the_previous_accounts_wrist_summary() {
    let (service, remote, _store) = connected_service();
    // 造出上一个账号的未读数与腕表缓存。
    service.note_unread(&VrccUnread {
        inbox: 5,
        ..Default::default()
    });
    assert!(service.wrist_summary(None).is_some());

    remote.set_device_token(VrccTokenResponse {
        access_token: "new-token".into(),
        token_type: None,
        scope: Some("profile notifications maps".into()),
        me: None,
    });
    let prompt = VrccDeviceAuthPrompt {
        user_code: "ABCD-2345".into(),
        verification_uri: None,
        verification_uri_complete: None,
        expires_in_secs: 600,
        interval_secs: 5,
        device_code: "dev-code".into(),
        scope: None,
    };
    let step = service.poll_device_auth(&prompt).await.expect("connected");
    assert!(matches!(step, VrccDeviceAuthStep::Connected(_)));

    // 新账号还没拉到任何数据：腕表整块不显示，底栏回落玩家数。
    assert!(service.cached_unread().is_none());
    assert!(service.wrist_summary(None).is_none());
}

#[test]
fn wrist_shows_nothing_until_the_player_connects() {
    // 没连接就不该在腕表上占位：底栏原本显示的是玩家数，
    // 不该被一个从没连过的功能顶掉。
    let service = VrccService::new(Arc::new(MemoryStore::default()), FakeRemote::new());
    assert!(service.wrist_summary(None).is_none());

    // 就算拉到了开放榜单（不需要连接），也不该突然冒出摘要。
    service.note_boards(&VrccBoard {
        maps: vec![VrccBoardMap {
            world: "wrld_x".into(),
            rank: Some(3),
            avg: Some(8.8),
            ..Default::default()
        }],
        ..Default::default()
    });
    assert!(
        service.wrist_summary(Some("wrld_x")).is_none(),
        "没连接时榜单不该让腕表出现"
    );
}

#[test]
fn wrist_shows_the_map_rank_once_connected() {
    let remote = FakeRemote::new();
    remote.mark_connected();
    let service = VrccService::new(
        Arc::new(MemoryStore::default()),
        Arc::clone(&remote) as Arc<dyn VrccRemote>,
    );
    // 连接后先有一条提醒，腕表才有数据可显示。
    service.note_recent(0);
    service.note_boards(&VrccBoard {
        maps: vec![VrccBoardMap {
            world: "wrld_x".into(),
            rank: Some(3),
            avg: Some(8.8),
            ..Default::default()
        }],
        ..Default::default()
    });
    let summary = service.wrist_summary(Some("wrld_x")).expect("connected");
    assert_eq!(summary.map_rank, Some(3));
    assert_eq!(summary.map_avg_x10, Some(88));
}

#[test]
fn stop_is_classified_as_a_passing_problem() {
    // 停站维护：令牌没事，等 `until` 就好，和网络问题一样处理。
    let paused = http(503, "stop");
    assert_eq!(classify_vrcc_error(&paused).0, VrccErrorKind::Transport);
    assert_eq!(paused.layer, ErrorLayer::Server);
}

#[tokio::test]
async fn unread_and_notes_require_connection() {
    let service = VrccService::new(Arc::new(MemoryStore::default()), FakeRemote::new());
    assert!(service
        .refresh_unread()
        .await
        .unwrap_err()
        .is_session_lost());
    assert!(service.notes().await.unwrap_err().is_session_lost());
}

#[test]
fn usage_snapshot_reflects_the_remote_counters() {
    // 快照必须是真实读上来的，不是写死的 0。
    let service = VrccService::new(Arc::new(MemoryStore::default()), FakeRemote::new());
    let usage = service.usage();
    assert_eq!(usage.personal_used_today, Some(12));
    assert_eq!(usage.window_used, 0);
    assert_eq!(usage.backoff_remaining_ms, 0);
}

#[test]
fn wrist_summary_is_none_before_any_data_arrives() {
    // 「还没拿到数据」和「未读 0 条」是两件事，前者不该在腕表上显示数字。
    let service = VrccService::new(Arc::new(MemoryStore::default()), FakeRemote::new());
    assert!(service.wrist_summary(None).is_none());
    assert!(service.wrist_summary(Some("wrld_abc")).is_none());
}

#[test]
fn wrist_summary_reports_map_rank_from_board_cache() {
    let remote = FakeRemote::new();
    remote.mark_connected();
    let service = VrccService::new(
        Arc::new(MemoryStore::default()),
        Arc::clone(&remote) as Arc<dyn VrccRemote>,
    );
    // 腕表要先有数据（比如拉过一次提醒）才会显示；榜单负责补名次。
    service.note_recent(0);
    // 直接喂一份榜单，模拟页面拉过地图榜。
    let board = VrccBoard {
        board: Some("maps".into()),
        sort: None,
        min: Some(5),
        maps: vec![
            VrccBoardMap {
                world: "wrld_target".into(),
                rank: Some(3),
                avg: Some(8.75),
                ..Default::default()
            },
            VrccBoardMap {
                world: "wrld_other".into(),
                rank: Some(9),
                avg: Some(6.0),
                ..Default::default()
            },
        ],
        at: Some(1),
    };
    service.note_boards(&board);

    let summary = service
        .wrist_summary(Some("wrld_target"))
        .expect("榜单已加载，应当有摘要");
    assert_eq!(summary.map_rank, Some(3));
    // 均分放大 10 倍存整数，8.75 → 88（四舍五入到一位小数）。
    assert_eq!(summary.map_avg_x10, Some(88));
    // 没拉过 recent，未读数还没数据，保持 0（未连接时收不到提醒）。
    assert_eq!(summary.unread, 0);

    // 不在榜上的图不报名次，而不是报一个假名次。
    let off_board = service
        .wrist_summary(Some("wrld_missing"))
        .expect("仍应有摘要");
    assert_eq!(off_board.map_rank, None);
    assert_eq!(off_board.map_avg_x10, None);
}

#[test]
fn wrist_summary_unread_comes_from_recent_read_cursor() {
    let remote = FakeRemote::new();
    remote.mark_connected();
    let service = VrccService::new(
        Arc::new(MemoryStore::default()),
        Arc::clone(&remote) as Arc<dyn VrccRemote>,
    );
    service.note_recent(4);
    let summary = service.wrist_summary(None).expect("有未读数据了");
    assert_eq!(summary.unread, 4);
}

#[test]
fn classify_splits_reauth_from_throttle() {
    // 本机限流：请求没发出去，玩家该等。
    let client_side = throttled_locally(30, LocalThrottle::Gap);
    assert_eq!(
        classify_vrcc_error(&client_side).0,
        VrccErrorKind::Throttled
    );

    // 服务器说 429：同样是「问太多了」，也归 Throttled。
    let too_many = http(429, "too_many");
    assert_eq!(classify_vrcc_error(&too_many).0, VrccErrorKind::Throttled);

    let quota = http(429, "quota");
    assert_eq!(classify_vrcc_error(&quota).0, VrccErrorKind::Throttled);

    let gone = http(404, "gone");
    assert_eq!(classify_vrcc_error(&gone).0, VrccErrorKind::NeedsReauth);

    let missing = missing_scope("today");
    let (kind, need) = classify_vrcc_error(&missing);
    assert_eq!(kind, VrccErrorKind::MissingScope);
    assert_eq!(need.as_deref(), Some("today"));

    // 服务器漏带 `need` 时也必须归 MissingScope，而不是 NeedsReauth：
    // 后者会让 settle() 把这份仍然有效的令牌清掉。
    let missing_without_need = http(403, "scope");
    let (kind, need) = classify_vrcc_error(&missing_without_need);
    assert_eq!(kind, VrccErrorKind::MissingScope);
    assert_eq!(need, None);

    let unreviewed_without_need = http(403, "scope_unreviewed");
    assert_eq!(
        classify_vrcc_error(&unreviewed_without_need).0,
        VrccErrorKind::MissingScope
    );

    let transport = transport("timeout");
    assert_eq!(classify_vrcc_error(&transport).0, VrccErrorKind::Transport);
}

/// `409 busy` 和 `429 busy` 用的是同一个 `error` 串，含义却相反：
/// 前者是「那边的星卡还在生成，等一下」，后者是「全站连接功能今天关了」。
///
/// 两者都不该被当成需要重新授权，也不该说成「今天的次数用完了」。
/// 这正是把 HTTP 语义收敛成一个 `Http` 变体之后必须守住的一条：
/// **官方的 `busy` 只能按状态码区分**。
#[test]
fn busy_never_reads_as_throttled_or_reauth() {
    let not_ready = http(409, "busy");
    assert_eq!(classify_vrcc_error(&not_ready).0, VrccErrorKind::NotReady);
    assert!(not_ready.is_not_ready());

    let connect_paused = http(429, "busy");
    // 429 busy 也是「那边没准备好」，不是「你额度用完了」。
    assert_eq!(
        classify_vrcc_error(&connect_paused).0,
        VrccErrorKind::NotReady
    );
    // 但 429 busy 不是 is_not_ready（那只认 409），两者靠状态码分开。
    assert!(!connect_paused.is_not_ready());

    for error in [not_ready, connect_paused] {
        assert_ne!(classify_vrcc_error(&error).0, VrccErrorKind::NeedsReauth);
        assert_ne!(classify_vrcc_error(&error).0, VrccErrorKind::Throttled);
        assert!(!error.needs_reauth());
    }
}

/// 我们自己构造请求出的错，不该走到玩家面前。
#[test]
fn our_own_bugs_are_marked_as_input_layer() {
    let bad_path = VrccError::new(
        ErrorLayer::Input,
        ErrorDetail::AbsolutePath("http://elsewhere".into()),
    );
    assert!(bad_path.is_our_bug());
    assert_eq!(classify_vrcc_error(&bad_path).0, VrccErrorKind::Other);

    let too_big = VrccError::new(ErrorLayer::Input, ErrorDetail::BodyTooLarge);
    assert!(too_big.is_our_bug());

    // 反过来：网络问题不是我们的锅。
    assert!(!transport("timeout").is_our_bug());
}

#[test]
fn map_not_went_is_not_a_reauth_case() {
    // 打分前置条件不满足，令牌没问题。
    let error = missing_scope("verify");
    let (kind, _) = classify_vrcc_error(&error);
    assert_ne!(kind, VrccErrorKind::NeedsReauth);
}

#[tokio::test]
async fn connected_guard_blocks_personal_calls_when_disconnected() {
    let store = Arc::new(MemoryStore::default());
    let service = VrccService::new(store, FakeRemote::new());
    // 未连接时不应发请求，直接报没令牌。
    let error = service.recent_notifications(false).await.unwrap_err();
    assert!(error.is_session_lost());
}

/// 不传 `scope` 时要按标识显式地要：交给服务器默认的话，自助登记的
/// `app_…` 只有 `profile`，提醒和「连上」全是 `403 scope`。
#[tokio::test]
async fn device_auth_asks_for_the_scope_that_fits_the_client() {
    let cases = [
        ("dev-test", "profile notifications maps"),
        (
            "app_0123456789abcdef",
            // 三项敏感权限都在：`crossings` 是「星轨相交」页要用的，漏了它，
            // 那个页签对每个玩家都永远是「缺权限」，功能等于没做。
            "profile notifications maps mutuals crossings maps.rate maps.write",
        ),
    ];
    for (client, expected) in cases {
        let remote = FakeRemote::new();
        remote.connection.lock().unwrap().client = client.to_string();
        let service = VrccService::new(Arc::new(MemoryStore::default()), remote.clone());

        let _ = service.begin_device_auth(None).await;
        let _ = service.begin_device_auth(Some("  ")).await;

        let asked = remote.device_code_scopes.lock().unwrap().clone();
        assert_eq!(
            asked,
            vec![Some(expected.to_string()), Some(expected.to_string())],
            "{client}"
        );
    }
}

#[tokio::test]
async fn device_auth_keeps_an_explicit_scope() {
    let remote = FakeRemote::new();
    let service = VrccService::new(Arc::new(MemoryStore::default()), remote.clone());

    let _ = service.begin_device_auth(Some("profile card")).await;

    let asked = remote.device_code_scopes.lock().unwrap().clone();
    assert_eq!(asked, vec![Some("profile card".to_string())]);
}

#[tokio::test]
async fn poll_notifications_requires_connection() {
    let service = VrccService::new(Arc::new(MemoryStore::default()), FakeRemote::new());
    let error = service.poll_notifications().await.unwrap_err();
    assert!(error.is_session_lost());
}

#[tokio::test]
async fn today_requires_connection() {
    let service = VrccService::new(Arc::new(MemoryStore::default()), FakeRemote::new());
    let error = service.today().await.unwrap_err();
    assert!(error.is_session_lost());
}

#[tokio::test]
async fn unread_is_counted_from_the_read_cursor_not_the_item_count() {
    // 官方的「未读」是 `id > read`，不是「返回了几条」。
    // 过去这里靠真的 `VrccSession` 才能测，现在端口一换就能脱离网络验证。
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        ..Default::default()
    });
    let remote = FakeRemote::new();
    remote.mark_connected();
    remote.set_recent(VrccRecentNotifications {
        items: vec![recent_item(3), recent_item(2), recent_item(1)],
        read: Some(2),
    });
    let service = VrccService::new(store, Arc::clone(&remote) as Arc<dyn VrccRemote>);

    let recent = service.recent_notifications(false).await.expect("recent");
    assert_eq!(recent.items.len(), 3);
    // read=2 时只有 id=3 是未读。
    assert_eq!(
        service.wrist_summary(None).map(|summary| summary.unread),
        Some(1)
    );
}

#[tokio::test]
async fn a_missing_read_cursor_treats_every_item_as_unread() {
    // 账号上还没记过已读时，只能把返回的都当未读——这是唯一诚实的解读。
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        ..Default::default()
    });
    let remote = FakeRemote::new();
    remote.mark_connected();
    remote.set_recent(VrccRecentNotifications {
        items: vec![recent_item(2), recent_item(1)],
        read: None,
    });
    let service = VrccService::new(store, Arc::clone(&remote) as Arc<dyn VrccRemote>);

    service.recent_notifications(false).await.expect("recent");
    assert_eq!(
        service.wrist_summary(None).map(|summary| summary.unread),
        Some(2)
    );
}

#[tokio::test]
async fn unread_prefers_the_server_counts_over_the_recent_items() {
    // 服务器有 `me/unread` 时，腕表用它；「最近的提醒」的条数不再覆盖它。
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        ..Default::default()
    });
    let remote = FakeRemote::new();
    remote.mark_connected();
    remote.set_unread(VrccUnread {
        inbox: 2,
        chats: 3,
        ..Default::default()
    });
    let service = VrccService::new(store, Arc::clone(&remote) as Arc<dyn VrccRemote>);

    let (counts, changed) = service
        .refresh_unread()
        .await
        .expect("unread")
        .expect("counts");
    assert!(changed);
    assert_eq!(counts.total(), 5);
    assert_eq!(
        service.wrist_summary(None).map(|summary| summary.unread),
        Some(5)
    );
}

#[tokio::test]
async fn a_transport_failure_is_reported_but_keeps_the_token() {
    // 网络错误不是令牌的事：报出去，但不清凭据。
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        ..Default::default()
    });
    let remote = FakeRemote::new();
    remote.mark_connected();
    remote.set_error(transport("dns timeout"));
    let service = VrccService::new(store.clone(), Arc::clone(&remote) as Arc<dyn VrccRemote>);

    let error = service.refresh_unread().await.unwrap_err();
    assert_eq!(error.layer, ErrorLayer::Transport);
    assert!(service.snapshot().connected);
    assert!(store.load().unwrap().is_some());
}

/// 造一条「最近的提醒」条目。只关心 id，其余字段取默认。
fn recent_item(id: i64) -> vrcx_0_contracts::vrcc::VrccRecentNotificationItem {
    vrcx_0_contracts::vrcc::VrccRecentNotificationItem {
        id,
        title: Some(format!("提醒 {id}")),
        body: None,
        at: Some(1_700_000_000_000),
        url: None,
    }
}

// -- 断开时的吊销 ------------------------------------------------------------

/// **吊销请求没发出去时必须保留令牌。**
///
/// 这是真实风险：网络不通、或恰好在全局退避里，`POST /disconnect` 到不了
/// 服务器。如果这时候把本地令牌删了，服务器上那份授权就再也吊销不了——
/// 玩家在网页「第三方客户端」里会一直看到一个早就该消失的客户端。
#[tokio::test]
async fn a_retriable_revoke_failure_keeps_the_token() {
    for error in [
        transport("network unreachable"),
        throttled_locally(30, LocalThrottle::Backoff),
        // 最小间隔没到也是「没发出去」，同样该保留令牌。
        throttled_locally(12, LocalThrottle::Gap),
    ] {
        let store = MemoryStore::with(StoredCredential {
            token: "tok".into(),
            ..Default::default()
        });
        let remote = FakeRemote::new();
        remote.mark_connected();
        remote.set_error(error.clone());
        let service = VrccService::new(store.clone(), Arc::clone(&remote) as Arc<dyn VrccRemote>);

        let returned = service.disconnect().await.unwrap_err();
        // `VrccError` 现在是 struct，不能再用 `discriminant` 比「同一个变体」；
        // 改成比层级，这才是「原样交出去」在结构上唯一说得通的等值判据。
        assert_eq!(returned.layer, error.layer, "错误要原样交出去");
        // 令牌还在，服务器上那份授权还有机会被吊销。
        assert!(
            store.load().unwrap().is_some(),
            "可重试的吊销失败不该清掉令牌：{error}"
        );
        // 也仍然算连着：界面要如实显示，不能骗玩家说已经断开了。
        assert!(service.snapshot().connected, "本地也不该断开：{error}");
    }
}

/// 吊销成功 → 清本地。
#[tokio::test]
async fn a_successful_revoke_clears_the_token() {
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        ..Default::default()
    });
    let remote = FakeRemote::new();
    remote.mark_connected();
    let service = VrccService::new(store.clone(), Arc::clone(&remote) as Arc<dyn VrccRemote>);

    service.disconnect().await.expect("disconnect");
    assert!(store.load().unwrap().is_none());
    assert!(!service.snapshot().connected);
}

/// 服务器说「这个令牌早就不算数了」→ 当作已断开，清本地。
#[tokio::test]
async fn a_gone_token_counts_as_already_disconnected() {
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        ..Default::default()
    });
    let remote = FakeRemote::new();
    remote.mark_connected();
    remote.set_error(http(404, "gone"));
    let service = VrccService::new(store.clone(), Arc::clone(&remote) as Arc<dyn VrccRemote>);

    service
        .disconnect()
        .await
        .expect("令牌本来就无效，等于已经断开了");
    assert!(store.load().unwrap().is_none());
    assert!(!service.snapshot().connected);
}

/// 服务器答复里的错误不算「可重试」：请求已经到了，重试也不会变。
#[test]
fn only_pre_send_failures_are_retriable() {
    // 没发出去的：传输失败、本机限流。
    assert!(retriable_revoke_failure(&transport("x")));
    assert!(retriable_revoke_failure(&throttled_locally(
        1,
        LocalThrottle::Backoff
    )));
    assert!(retriable_revoke_failure(&throttled_locally(
        1,
        LocalThrottle::Gap
    )));

    // 服务器已经答复的：重试没用。
    assert!(!retriable_revoke_failure(&http(500, "server_error")));
    assert!(!retriable_revoke_failure(&http(429, "quota")));
    assert!(!retriable_revoke_failure(&http(503, "stop")));
}

// ---------------------------------------------------------------------------
// 地图页 / 我打过的图 / 玩家视角榜 / 贴纸
//
// 这四条都随 A 批一起进来，行为都是「某种失败要变成可显示的状态」，
// 最容易在以后的重构里被悄悄改掉，所以钉住。
// ---------------------------------------------------------------------------

fn connected_service() -> (VrccService, Arc<FakeRemote>, Arc<MemoryStore>) {
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        ..Default::default()
    });
    let remote = FakeRemote::new();
    remote.mark_connected();
    let service = VrccService::new(store.clone(), Arc::clone(&remote) as Arc<dyn VrccRemote>);
    (service, remote, store)
}

/// 地图页是没有令牌也能读的公开接口。
///
/// 带令牌的地图页：404 同样是「不在榜上」，不是故障。
///
/// 这一条和公开版分开测，因为两条路径的 `ensure_connected` 行为不同：
/// 公开版不要求连接，带令牌版**要求**。
#[tokio::test]
async fn the_signed_in_map_page_treats_404_as_absent() {
    let (service, remote, _store) = connected_service();
    remote.set_error(http(404, "not_found"));

    assert!(
        service.my_map_page("wrld_x", None).await.unwrap().is_none(),
        "404 should read as 'this world is not on the board', not as a failure"
    );

    // 真的故障仍然要报出去。
    remote.set_error(http(500, "server"));
    let error = service.my_map_page("wrld_x", None).await.unwrap_err();
    assert_eq!(error.layer, ErrorLayer::Server);
}

/// `off` 的地图要**原样交给界面**，不能折成 `None`。
///
/// 折成 `None` 会把「被合规隐藏」「没连接」「查不到」压成同一个结果，界面
/// 只能一律说「不在榜上」。界面拿不到 `off` 就没法说明真正的原因。
///
/// 代价是每个调用点都必须自己判断 `off`——所以这条测试钉住「服务层不会替
/// 调用方挡掉」这个约定，免得有人又把它折回去。
#[tokio::test]
async fn a_hidden_map_is_returned_with_its_off_flag_intact() {
    let (service, remote, _store) = connected_service();
    *remote.my_map.lock().unwrap() = Some(VrccMyMap {
        world: Some("wrld_x".into()),
        off: Some(true),
        ..VrccMyMap::default()
    });

    let map = service
        .my_map("wrld_x")
        .await
        .expect("a hidden map is not an error")
        .expect("the caller needs the object to explain why it is hidden");

    assert_eq!(map.off, Some(true));
    assert!(!map.is_visible());
}

/// 未连接时直接拒绝，不把一个必然 `TokenMissing` 的请求发出去。
#[tokio::test]
async fn the_signed_in_map_page_refuses_before_sending_when_disconnected() {
    // 空的 store + 没 mark_connected 的 remote = 从没连过的玩家。
    // **不能**用 `connected_service()` 再 `disconnect()`：那个 store 里已经有
    // 令牌，`ensure_connected` 看的是 store，断开并不会让检查失败。
    let store: Arc<dyn VrccTokenStore> = Arc::new(MemoryStore::default());
    let remote = FakeRemote::new();
    let service = VrccService::new(store, Arc::clone(&remote) as Arc<dyn VrccRemote>);

    let error = service.my_map_page("wrld_x", None).await.unwrap_err();
    assert!(error.needs_reauth());
    // 没注入过错误且没有走到网络，说明请求是被本地拦下的。
    assert!(remote.take_error().is_none());
}

/// 写操作遇到空世界 ID 必须**失败**，不能安静成功。
///
/// 读接口在这种情况下返回 `Ok(None)` 是合理的（「没有这一页」），但写操作
/// 「什么都没发生」是最坏的结局——玩家会以为打分提交了。
#[tokio::test]
async fn rating_a_map_without_a_world_id_fails_instead_of_silently_succeeding() {
    let (service, _remote, _store) = connected_service();

    let error = service
        .rate_map("", 8, &VrccRatingText::Keep)
        .await
        .unwrap_err();
    assert_eq!(error.layer, ErrorLayer::Input);

    let error = service.delete_map_rating("").await.unwrap_err();
    assert_eq!(error.layer, ErrorLayer::Input);
}

/// 未连接时写操作先拒绝，不把必然失败的请求发出去。
#[tokio::test]
async fn rating_refuses_before_sending_when_disconnected() {
    let store: Arc<dyn VrccTokenStore> = Arc::new(MemoryStore::default());
    let remote = FakeRemote::new();
    let service = VrccService::new(store, Arc::clone(&remote) as Arc<dyn VrccRemote>);

    let error = service
        .rate_map("wrld_x", 8, &VrccRatingText::Keep)
        .await
        .unwrap_err();
    assert!(error.needs_reauth());
    assert!(remote.take_error().is_none());
}

/// 删评分遇到 `404` 当作成功：目标本来就已达成。
///
/// 玩家看到「删除失败」会以为评分还在，然后反复点。
#[tokio::test]
async fn deleting_a_rating_that_is_already_gone_is_not_an_error() {
    let (service, remote, _store) = connected_service();
    remote.set_error(http(404, "not_found"));

    assert!(service.delete_map_rating("wrld_x").await.is_ok());
}

/// 短评被审核拒绝时，错误要能一路带到界面。
///
/// `yq_words` 不是网络故障，服务器会在 `why` 里说明原因，那句话对玩家比
/// 「操作失败」有用得多。
#[tokio::test]
async fn a_rejected_comment_keeps_its_reason() {
    let (service, remote, _store) = connected_service();
    remote.set_error(http(400, "yq_words"));

    let error = service
        .rate_map("wrld_x", 8, &VrccRatingText::Write("留个联系方式".into()))
        .await
        .unwrap_err();
    // 这是服务器拒绝，不是传输失败；界面据此显示「提交失败」+ 原因
    assert_eq!(error.layer, ErrorLayer::Server);
}

/// 不在榜上时服务器回 `404 not_found`——那是**这张图还没上榜**，不是故障。
/// 如果这里改成把 404 当错误抛出去，一张新图就会显示成「服务坏了」。
#[tokio::test]
async fn a_map_that_is_not_on_the_board_is_absent_not_broken() {
    let (service, remote, _store) = connected_service();
    remote.set_error(http(404, "not_found"));

    assert!(
        service.map_page("wrld_x").await.unwrap().is_none(),
        "404 should read as 'no page for this world', not as a failure"
    );

    // 真的故障（5xx）仍然要报出去，别把两者都吞掉。
    remote.set_error(http(500, "server"));
    let error = service.map_page("wrld_x").await.unwrap_err();
    assert_eq!(error.layer, ErrorLayer::Server);
}

/// 空的 world 直接短路，不发请求。
#[tokio::test]
async fn an_empty_world_never_reaches_the_network() {
    let (service, remote, _store) = connected_service();
    // 留一个「下一次一定失败」的错误：真发出去就会拿到它。
    remote.set_error(transport("should not be sent"));

    assert!(service.map_page("").await.unwrap().is_none());
    assert!(service.map_page("   ").await.unwrap().is_none());
}

/// 贴纸是装饰素材：旧服务器没有这个接口（404）时给 `None`，
/// 「我的」页签不该因此报错。
#[tokio::test]
async fn stickers_go_missing_quietly_on_an_old_server() {
    let (service, remote, _store) = connected_service();

    // 正常有数据。
    assert!(service.stickers().await.unwrap().is_some());

    remote.set_error(http(404, "not_found"));
    assert!(service.stickers().await.unwrap().is_none());

    // 其它错误照常上报。
    remote.set_error(http(500, "server"));
    assert!(service.stickers().await.unwrap_err().layer == ErrorLayer::Server);
}

/// 需要令牌的两条：没连接就当场拒绝，不把必然 `TokenMissing` 的请求发出去。
///
/// 发出去的后果很实在：那个错误会被判成「要重新授权」，于是还没连过的玩家
/// 打开页面就看见一句莫名其妙的「请重新连接」。
#[tokio::test]
async fn personal_map_lists_refuse_before_sending_when_disconnected() {
    // 注意这里**不**调用 `mark_connected`。
    let store = MemoryStore::with(StoredCredential::default());
    let remote = FakeRemote::new();
    let service = VrccService::new(store, Arc::clone(&remote) as Arc<dyn VrccRemote>);

    let error = service.my_maps().await.unwrap_err();
    assert_eq!(error.layer, ErrorLayer::Session);

    let error = service.my_board(VrccMyBoardSort::Went).await.unwrap_err();
    assert_eq!(error.layer, ErrorLayer::Session);
}

/// 连上之后这两条照常可用。
///
/// fake 在这两条上回 `token_missing`，但连接状态已经就绪——所以拿到的错误
/// 必须来自**远端**，而不是本地那道提前拒绝。两者同为 `Session` 层，这里
/// 靠「换一个远端错误能否透传」来区分。
#[tokio::test]
async fn personal_map_lists_work_once_connected() {
    let (service, remote, _store) = connected_service();

    // 注入一个远端错误：能原样拿到，就说明请求确实走到了远端。
    remote.set_error(http(403, "scope"));
    let error = service.my_maps().await.unwrap_err();
    assert_eq!(error.layer, ErrorLayer::Server);

    remote.set_error(http(403, "scope"));
    let error = service.my_board(VrccMyBoardSort::Top).await.unwrap_err();
    assert_eq!(error.layer, ErrorLayer::Server);
}

// ---------------------------------------------------------------------------
// 星轨相交
// ---------------------------------------------------------------------------

#[tokio::test]
async fn crossings_requires_connection() {
    // 星轨相交是个人接口：没连接时不能发请求，直接报没令牌。
    // 与 mutuals / today 共用同一条守卫，但值得单独钉住——它是新加的
    // 敏感权限接口，漏掉守卫就会拿空令牌发出去，服务器回 401 让玩家
    // 误以为自己的账号出了问题。
    let service = VrccService::new(Arc::new(MemoryStore::default()), FakeRemote::new());
    let error = service.crossings(false).await.unwrap_err();
    assert!(error.is_session_lost());

    let error = service.crossing_cards(false).await.unwrap_err();
    assert!(error.is_session_lost());
}

/// 连接好、`crossings` 能返回一条的测试服务。
fn crossings_service(reason: Option<&str>) -> (VrccService, Arc<FakeRemote>) {
    let remote = FakeRemote::new();
    remote.mark_connected();
    remote.set_crossings(VrccCrossings {
        stars: vec![VrccCrossingStar {
            n: 77,
            name: Some("拾光旅人".into()),
            ..Default::default()
        }],
        n: Some(1),
        why: reason.map(str::to_string),
        ..Default::default()
    });
    (
        VrccService::new(Arc::new(MemoryStore::default()), remote.clone()),
        remote,
    )
}

#[tokio::test]
async fn crossings_passes_the_mark_flag_through() {
    // `mark` 决定要不要把星轨相交标成已读、连带清掉网页与手机的红点。
    // 应用层**不做判断**，只原样透传给端口——但这条链路必须通：如果哪
    // 一层把 mark 丢成 false，玩家打开页面后红点永远消不掉，而且看不出
    // 任何报错。
    let (service, remote) = crossings_service(None);

    service.crossings(true).await.expect("mark=true 应该成功");
    assert_eq!(
        *remote.crossings_mark.lock().unwrap(),
        Some(true),
        "mark 必须透传成 true，否则玩家打开页面后红点消不掉"
    );

    service.crossings(false).await.expect("mark=false 应该成功");
    assert_eq!(*remote.crossings_mark.lock().unwrap(), Some(false));
}

#[tokio::test]
async fn crossings_only_hide_when_the_server_says_hide() {
    // `hidden` 由应用层按 `why` 定好，消费方不再各自去比字符串。
    // 这条判定的两边后果不对称：把 `hide` 当成普通空列表，等于向不该
    // 看到的人确认「这里有星轨相交」这个事实本身。
    let (service, _) = crossings_service(Some("hide"));
    assert!(
        service.crossings(false).await.expect("应成功").hidden,
        "why=hide 必须整块隐藏"
    );

    for reason in [
        "verify",
        "wait",
        "queued",
        "sent",
        "gone",
        "friend",
        "something-new",
    ] {
        let (service, _) = crossings_service(Some(reason));
        assert!(
            !service.crossings(false).await.expect("应成功").hidden,
            "why={reason} 只是暂时不可用，不能藏起来"
        );
    }

    let (service, _) = crossings_service(None);
    assert!(!service.crossings(false).await.expect("应成功").hidden);
}

#[tokio::test]
async fn crossing_cards_falls_back_to_none_on_old_servers() {
    // `/me/crossings/cards` 是 1.13.0 才有的。旧服务器回 `404 not_found`，
    // 这只影响「今天还能连几颗」那一小块——**不能让整个列表跟着失败**，
    // 也不能把令牌当成失效（`not_found` 不是 `gone`）。
    let (service, remote) = crossings_service(None);
    remote.set_error(http(404, "not_found"));

    let cards = service
        .crossing_cards(false)
        .await
        .expect("旧服务器不该报错");
    assert!(
        cards.is_none(),
        "没有这个接口时返回 None，界面藏起额度那一行"
    );
    assert!(
        service.snapshot().connected,
        "`not_found` 不是令牌失效，不能把连接弄断"
    );

    // 真的拿到时照常返回，`mark` 同样透传。
    remote.set_crossing_cards(VrccCrossingCards {
        left: Some(3),
        max: Some(5),
        ..Default::default()
    });
    let cards = service
        .crossing_cards(true)
        .await
        .expect("应成功")
        .expect("应返回星卡");
    assert_eq!(cards.left, Some(3));
    assert_eq!(*remote.cards_mark.lock().unwrap(), Some(true));
}

// ---------------------------------------------------------------------------
// 模型榜（1.17.0 起）
// ---------------------------------------------------------------------------

/// 旧服务器上没有 `boards/models`：`404 not_found` 收成 `None`，不是错误。
///
/// 这和一个真实存在的故障必须分开：地图榜还在，模型榜只是还没上线，
/// 把它抛成错误会让整个页面顶上挂一条红横幅。
#[tokio::test]
async fn model_board_missing_on_old_server_returns_none() {
    let store = Arc::new(MemoryStore::default());
    let remote = FakeRemote::new();
    let service = VrccService::new(store, remote.clone());

    // `FakeRemote` 没预设时回 404 not_found。
    let board = service
        .models_board(20, VrccBoardSort::Top)
        .await
        .expect("旧服务器不该被当成故障");
    assert!(board.is_none());
    assert_eq!(
        *remote.models_calls.lock().unwrap(),
        vec![(20, VrccBoardSort::Top)]
    );
}

/// 拿到榜单时原样返回，字段照契约读出来。
#[tokio::test]
async fn model_board_returns_the_board_it_received() {
    let store = Arc::new(MemoryStore::default());
    let remote = FakeRemote::new();
    remote.set_models(VrccModelBoard {
        board: Some("models".to_string()),
        sort: Some("top".to_string()),
        models: vec![VrccBoardModel {
            id: 8962830,
            name: "オリジナル3Dモデル「星」".to_string(),
            short: Some("星".to_string()),
            likes: 42,
            rank: Some(1),
            ..Default::default()
        }],
        at: Some(1),
        ..Default::default()
    });
    let service = VrccService::new(store, remote.clone());

    let board = service
        .models_board(20, VrccBoardSort::Top)
        .await
        .expect("应成功")
        .expect("应返回榜单");
    assert_eq!(board.models.len(), 1);
    assert_eq!(board.models[0].display_name(), "星");
    assert_eq!(board.models[0].likes, 42);
}

/// 限流窗口内再问：把上次那份还给玩家，而不是报「请求太频繁」。
///
/// 缓存命中要求**排序和条数都一样**：请求 50 条却拿到 20 条的旧数据，界面
/// 会以为这就是全部内容。
#[tokio::test]
async fn model_board_falls_back_to_cache_when_locally_throttled() {
    let store = Arc::new(MemoryStore::default());
    let remote = FakeRemote::new();
    remote.set_models(VrccModelBoard {
        board: Some("models".to_string()),
        models: vec![VrccBoardModel {
            id: 1,
            name: "モデル".to_string(),
            likes: 7,
            ..Default::default()
        }],
        ..Default::default()
    });
    let service = VrccService::new(store, remote.clone());

    // 第一次：真的拿到，并进缓存。
    let first = service
        .models_board(20, VrccBoardSort::Top)
        .await
        .expect("应成功")
        .expect("应返回榜单");
    assert_eq!(first.models.len(), 1);

    // 第二次：本地限流拦下（`is_local_gap`），服务层拿缓存顶上。
    remote.set_error(throttled_locally(30, LocalThrottle::Gap));
    let second = service
        .models_board(20, VrccBoardSort::Top)
        .await
        .expect("限流窗口内不该报错，应回退到缓存")
        .expect("缓存里有这一份");
    assert_eq!(second.models.len(), 1);
    assert_eq!(second.models[0].id, 1);

    // 条数不同就不给：请求 50 条却拿到 20 条的旧数据会误导界面。
    // `set_error` 取一次就清空，所以要重新设一次限流。
    remote.set_error(throttled_locally(30, LocalThrottle::Gap));
    let mismatch = service.models_board(50, VrccBoardSort::Top).await;
    assert!(mismatch.is_err(), "条数不一致时不能拿旧数据冒充");
}

/// `model()` 不在榜上时是 `None`，不是故障。
#[tokio::test]
async fn model_lookup_returns_none_when_not_listed() {
    let store = Arc::new(MemoryStore::default());
    let remote = FakeRemote::new();
    let service = VrccService::new(store, remote.clone());

    let detail: Option<VrccModelDetail> = service.model(8962830).await.expect("不该失败");
    assert!(detail.is_none());
}

/// 单模型详情要进缓存：反复点开同一条不该反复请求。
///
/// 玩家常做的动作就是「点开、关掉、再点开」。每次都走一次网络，手感是
/// 「慢」而不是「快」；官方也说这些信息最多 5 分钟才更新一次。
#[tokio::test]
async fn model_detail_is_cached_between_calls() {
    let store = Arc::new(MemoryStore::default());
    let remote = FakeRemote::new();
    let service = VrccService::new(store, remote.clone());

    // 先放一份真实详情：这次要验的是「拿到了就别再问」。
    remote.set_model_detail(VrccModelDetail {
        model: Some(VrccBoardModel {
            id: 4242,
            name: "モデル".to_string(),
            likes: 9,
            ..Default::default()
        }),
        hots: vec![],
        at: Some(1),
    });

    let first = service.model(4242).await.expect("should not fail");
    assert!(first.is_some());
    assert_eq!(remote.model_calls.lock().unwrap().len(), 1);

    // 第二次：命中缓存，请求次数不涨。
    let second = service.model(4242).await.expect("should not fail");
    assert!(second.is_some());
    assert_eq!(
        remote.model_calls.lock().unwrap().len(),
        1,
        "缓存命中时不该再发请求"
    );

    // 换个 id 必须重新问。这一条钉的是「缓存按 BOOTH 编号分」——如果哪天
    // 缓存退化成只存一份，这里会立刻变成 1 次。
    let other = service.model(9999).await.expect("should not fail");
    assert!(other.is_some());
    let calls = remote.model_calls.lock().unwrap().clone();
    assert_eq!(calls, vec![4242, 9999], "缓存必须按 id 分开");
}

/// 「不在榜上」只记 MODEL_MISSING_TTL（30 秒）：窗口内别问，久了要重新问。
///
/// 这条不是「记住没有」，而是挡住本地限流：`models/{id}` 的 `min_gap` 就是
/// 30 秒，同一个 id 在这段窗口里再问会被 `MAX_ACQUIRE_WAIT` 直接拒掉，而病根
/// 只是玩家关掉又点开了同一个已下架的模型——那不该表现成「评论加载失败」。
#[tokio::test]
async fn model_404_is_remembered_only_for_the_throttle_window() {
    let store = Arc::new(MemoryStore::default());
    let remote = FakeRemote::new();
    let service = VrccService::new(store, remote.clone());

    // FakeRemote 默认就是「不在榜上」。
    let first = service.model(4242).await.expect("should not fail");
    assert!(first.is_none());
    assert_eq!(remote.model_calls.lock().unwrap().len(), 1);

    let second = service.model(4242).await.expect("should not fail");
    assert!(second.is_none(), "窗口内仍然是「不在榜上」");
    assert_eq!(
        remote.model_calls.lock().unwrap().len(),
        1,
        "限流窗口内不该再问同一个 id"
    );
}

/// 详情缓存会过期：TTL 一到就重新问服务器。
///
/// 时钟是 std `Instant`，没有注入点，`tokio::time::pause()` 也推不动它。
/// 所以这里**直接把一条已经过期的时间戳塞进缓存**——比给生产代码开一个
/// 只在测试里用的时钟口子便宜得多，而且验的还是真实的过期判定。
#[tokio::test]
async fn model_detail_expires_after_ttl() {
    let store = Arc::new(MemoryStore::default());
    let remote = FakeRemote::new();
    remote.set_model_detail(VrccModelDetail {
        model: None,
        hots: vec![],
        at: Some(1),
    });
    let service = VrccService::new(store, remote.clone());

    // 键要带当前登录态。`FakeRemote` 默认未连接，所以分区是 `None`。
    let key = OwnedModelKey::new(service.cache_scope(), 777);
    {
        let mut cache = service.model_details.write().unwrap();
        cache.insert(
            key,
            (
                Instant::now() - MODEL_DETAIL_TTL - Duration::from_secs(1),
                VrccModelDetail {
                    model: None,
                    hots: vec![],
                    at: Some(1),
                },
            ),
        );
    }

    let hit = service.model(777).await.expect("should not fail");
    assert!(hit.is_some(), "FakeRemote 配了详情");
    assert_eq!(
        remote.model_calls.lock().unwrap().len(),
        1,
        "过期的那份不该被当成命中"
    );
}

/// 缓存满了淘汰最旧的，而不是整表清空。
///
/// 整表清空会让「刚看过的一屏模型」一起失效，下一次点开又是满屏转圈。
#[tokio::test]
async fn model_detail_cache_evicts_the_oldest_not_everything() {
    let store = Arc::new(MemoryStore::default());
    let remote = FakeRemote::new();
    remote.set_model_detail(VrccModelDetail {
        model: None,
        hots: vec![],
        at: Some(1),
    });
    let service = VrccService::new(store, remote.clone());

    let max = MODEL_DETAIL_CACHE_MAX as i64;
    let scope = service.cache_scope();
    {
        let mut cache = service.model_details.write().unwrap();
        for id in 0..max {
            // id 越小越旧：0 是最早写进来的那条。
            cache.insert(
                OwnedModelKey::new(scope, id),
                (
                    Instant::now() - Duration::from_secs(1_000 - id as u64),
                    VrccModelDetail {
                        model: None,
                        hots: vec![],
                        at: Some(1),
                    },
                ),
            );
        }
    }

    let fresh = max + 1;
    let _ = service.model(fresh).await.expect("should not fail");

    let cache = service.model_details.read().unwrap();
    assert!(
        cache.contains_key(&OwnedModelKey::new(scope, fresh)),
        "新条目一定进得去"
    );
    assert!(
        !cache.contains_key(&OwnedModelKey::new(scope, 0)),
        "最旧的那批被淘汰"
    );
    assert!(
        cache.contains_key(&OwnedModelKey::new(scope, max - 1)),
        "只淘汰最旧的 1/4，不是整表清空"
    );
}

/// 换账号之后同一张卡的缓存必须分开取。
///
/// 这条钉的是**未来**：`GET /models/{id}` 今天是公开数据，但权限可以再申请，
/// 接口也可能开始返回跟登录态绑定的东西。键里若没有账号维度，那一天到来时
/// A 的详情会直接给 B 看。
#[tokio::test]
async fn model_detail_cache_is_partitioned_by_account() {
    let store = Arc::new(MemoryStore::default());
    let remote = FakeRemote::new();
    remote.set_model_detail(VrccModelDetail {
        model: Some(VrccBoardModel {
            id: 4242,
            name: "モデル".to_string(),
            likes: 9,
            ..Default::default()
        }),
        hots: vec![],
        at: Some(1),
    });
    let service = VrccService::new(store, remote.clone());

    // 账号 A 看到的。
    remote.connect("tok-a".into(), Some(111), Some("A".into()), None);
    let _ = service.model(4242).await.expect("should not fail");
    assert_eq!(remote.model_calls.lock().unwrap().len(), 1);

    // 换成账号 B：同一个 id，但分区不同，必须重新问。
    remote.connect("tok-b".into(), Some(222), Some("B".into()), None);
    let _ = service.model(4242).await.expect("should not fail");
    assert_eq!(
        remote.model_calls.lock().unwrap().len(),
        2,
        "换了账号就不该命中上一个账号的缓存"
    );

    // 切回 A：这次命中 A 自己那份，不再请求。
    remote.connect("tok-a".into(), Some(111), Some("A".into()), None);
    let _ = service.model(4242).await.expect("should not fail");
    assert_eq!(
        remote.model_calls.lock().unwrap().len(),
        2,
        "切回原账号应该命中它自己那份"
    );
}

/// 榜单缓存也要按账号分区。
///
/// 用模型榜来验（`FakeRemote` 对它既有预设又有调用计数）。榜单今天不含个人
/// 字段，但这条约束不该靠「今天」来维持：接口以后完全可能带上「你投过票没有」。
#[tokio::test]
async fn board_cache_is_partitioned_by_account() {
    let store = Arc::new(MemoryStore::default());
    let remote = FakeRemote::new();
    remote.set_models(VrccModelBoard {
        board: Some("models".to_string()),
        sort: None,
        total: Some(1),
        ..Default::default()
    });
    let service = VrccService::new(store, remote.clone());

    // 账号 A 拉一次，进缓存。
    remote.connect("tok-a".into(), Some(111), Some("A".into()), None);
    let _ = service.models_board(20, VrccBoardSort::Top).await;
    let a_calls = remote.models_calls.lock().unwrap().len();
    assert_eq!(a_calls, 1);

    // 换账号 B：缓存里没有 B 的那份。
    remote.connect("tok-b".into(), Some(222), Some("B".into()), None);
    let b_key = OwnedModelBoardKey::new(service.cache_scope(), VrccBoardSort::Top);
    assert!(
        service.cached_models(20, b_key).is_none(),
        "换账号之后不该能读到上一个账号的榜单缓存"
    );

    // 切回 A：A 自己那份还在，而且不用重新请求。
    remote.connect("tok-a".into(), Some(111), Some("A".into()), None);
    let a_key = OwnedModelBoardKey::new(service.cache_scope(), VrccBoardSort::Top);
    assert!(
        service.cached_models(20, a_key).is_some(),
        "切回原账号应该还能命中它自己那份"
    );
    assert_eq!(
        remote.models_calls.lock().unwrap().len(),
        a_calls,
        "这一段只查缓存，不该再发请求"
    );
}
