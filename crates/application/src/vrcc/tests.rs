//! 应用层测试：用内存仓库替换持久化，验证编排逻辑。

use std::sync::{Arc, Mutex};

use super::*;
use vrcx_0_contracts::vrcc::{VrccBoardMap, VrccUnread};

/// 内存令牌仓库。
#[derive(Default)]
struct MemoryStore {
    inner: Mutex<Option<StoredCredential>>,
}

impl MemoryStore {
    fn with(credential: StoredCredential) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Some(credential)),
        })
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
}

#[test]
fn new_restores_connected_session_from_store() {
    let store = MemoryStore::with(StoredCredential {
        token: "tok".into(),
        star: Some(77),
        vrc_name: Some("拾光".into()),
        scope: Some("profile maps".into()),
    });
    let service = VrccService::new(store, "dev-test");
    let snapshot = service.snapshot();
    assert!(snapshot.connected);
    assert_eq!(snapshot.star, Some(77));
    assert_eq!(snapshot.vrc_name.as_deref(), Some("拾光"));
    assert_eq!(snapshot.client, "dev-test");
}

#[test]
fn new_without_stored_token_is_disconnected() {
    let store = Arc::new(MemoryStore::default());
    let service = VrccService::new(store, "dev-test");
    assert!(!service.snapshot().connected);
}

#[test]
fn usage_reports_used_not_remaining() {
    // 口径要求：只说「今日已用 N/上限」，不说「还剩」。
    let service = VrccService::new(Arc::new(MemoryStore::default()), "dev-test");
    let usage = service.usage();
    // 1.8.4 起第一份读是每个授权每天 1000 次。
    assert_eq!(usage.personal_daily_limit, 1000);
    assert_eq!(usage.device_code_daily_limit, 30);
    assert_eq!(usage.window_limit, 120);
    assert!(!usage.date.is_empty());
    // 还没拿到过服务器的 `RateLimit-*`：本地数，未读数那一份不显示。
    assert!(!usage.personal_from_server);
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
    });
    let service = VrccService::new(store.clone(), "dev-test");
    service.note_unread(&VrccUnread {
        inbox: 2,
        ..Default::default()
    });

    let error = service.settle(VrccError::Api {
        status: 404,
        error: "gone".into(),
        need: None,
        why: None,
    });
    assert!(error.is_gone());

    let snapshot = service.snapshot();
    assert!(!snapshot.connected);
    assert!(snapshot.expired, "界面要能说出是失效，不是玩家自己断开的");
    // 本地存的令牌也清掉了，重启后不会拿着死令牌再问。
    assert!(store.load().unwrap().is_none());
    // 未读数不再属于任何人。
    assert!(service.cached_unread().is_none());
    assert_eq!(
        service.wrist_summary(None).map(|summary| summary.unread),
        Some(0)
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
        let service = VrccService::new(store.clone(), "dev-test");
        service.settle(VrccError::Api {
            status: 404,
            error: code.into(),
            need: None,
            why: None,
        });
        assert!(service.snapshot().connected, "{code}");
        assert!(!service.snapshot().expired, "{code}");
        assert!(store.load().unwrap().is_some(), "{code}");
    }
}

#[test]
fn wrist_prefers_the_unread_counts_over_recent() {
    let service = VrccService::new(Arc::new(MemoryStore::default()), "dev-test");
    // 旧服务器：只有「最近的提醒」的未读条数。
    service.note_recent(2);
    assert_eq!(service.wrist_summary(None).unwrap().unread, 2);

    // 有了 `me/unread`：腕表显示几处红点的合计。
    let counts = VrccUnread {
        inbox: 1,
        cross: 1,
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

#[test]
fn stop_is_classified_as_a_passing_problem() {
    let paused = VrccError::Paused {
        say: Some("维护".into()),
        until: None,
    };
    assert_eq!(classify_vrcc_error(&paused).0, VrccErrorKind::Transport);
}

#[tokio::test]
async fn unread_and_notes_require_connection() {
    let service = VrccService::new(Arc::new(MemoryStore::default()), "dev-test");
    assert!(matches!(
        service.refresh_unread().await.unwrap_err(),
        VrccError::TokenMissing
    ));
    assert!(matches!(
        service.notes().await.unwrap_err(),
        VrccError::TokenMissing
    ));
}

#[test]
fn usage_starts_at_zero_and_reflects_real_counters() {
    // 新客户端还没发过请求，计数应当是真实的 0（而不是被写死的 0）。
    let service = VrccService::new(Arc::new(MemoryStore::default()), "dev-test");
    let usage = service.usage();
    assert_eq!(usage.personal_used_today, 0);
    assert_eq!(usage.device_code_used_today, 0);
    assert_eq!(usage.window_used, 0);
    assert_eq!(usage.backoff_remaining_ms, 0);
}

#[test]
fn wrist_summary_is_none_before_any_data_arrives() {
    // 「还没拿到数据」和「未读 0 条」是两件事，前者不该在腕表上显示数字。
    let service = VrccService::new(Arc::new(MemoryStore::default()), "dev-test");
    assert!(service.wrist_summary(None).is_none());
    assert!(service.wrist_summary(Some("wrld_abc")).is_none());
}

#[test]
fn wrist_summary_reports_map_rank_from_board_cache() {
    let service = VrccService::new(Arc::new(MemoryStore::default()), "dev-test");
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
    let service = VrccService::new(Arc::new(MemoryStore::default()), "dev-test");
    service.note_recent(4);
    let summary = service.wrist_summary(None).expect("有未读数据了");
    assert_eq!(summary.unread, 4);
}

#[test]
fn classify_splits_reauth_from_throttle() {
    let throttled = VrccError::RateLimited("too_many".into());
    assert_eq!(classify_vrcc_error(&throttled).0, VrccErrorKind::Throttled);

    let quota = VrccError::QuotaExhausted("用尽".into());
    assert_eq!(classify_vrcc_error(&quota).0, VrccErrorKind::Throttled);

    let gone = VrccError::Api {
        status: 404,
        error: "gone".into(),
        need: None,
        why: None,
    };
    assert_eq!(classify_vrcc_error(&gone).0, VrccErrorKind::NeedsReauth);

    let missing = VrccError::Api {
        status: 403,
        error: "scope".into(),
        need: Some("today".into()),
        why: None,
    };
    let (kind, need) = classify_vrcc_error(&missing);
    assert_eq!(kind, VrccErrorKind::MissingScope);
    assert_eq!(need.as_deref(), Some("today"));

    let transport = VrccError::Transport("timeout".into());
    assert_eq!(classify_vrcc_error(&transport).0, VrccErrorKind::Transport);
}

#[test]
fn map_not_went_is_not_a_reauth_case() {
    // 打分前置条件不满足，令牌没问题。
    let error = VrccError::Api {
        status: 403,
        error: "map_not_went".into(),
        need: Some("verify".into()),
        why: None,
    };
    let (kind, _) = classify_vrcc_error(&error);
    assert_ne!(kind, VrccErrorKind::NeedsReauth);
}

#[tokio::test]
async fn connected_guard_blocks_personal_calls_when_disconnected() {
    let store = Arc::new(MemoryStore::default());
    let service = VrccService::new(store, "dev-test");
    // 未连接时不应发请求，直接报没令牌。
    let error = service.recent_notifications(false).await.unwrap_err();
    assert!(matches!(error, VrccError::TokenMissing));
}

#[tokio::test]
async fn poll_notifications_requires_connection() {
    let service = VrccService::new(Arc::new(MemoryStore::default()), "dev-test");
    let error = service.poll_notifications().await.unwrap_err();
    assert!(matches!(error, VrccError::TokenMissing));
}

#[tokio::test]
async fn today_requires_connection() {
    let service = VrccService::new(Arc::new(MemoryStore::default()), "dev-test");
    let error = service.today().await.unwrap_err();
    assert!(matches!(error, VrccError::TokenMissing));
}
