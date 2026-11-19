// SPDX-License-Identifier: MPL-2.0

//! 健康探测的编排测试。
//!
//! 这里只测「什么情况下换结论、什么情况下不换」——判定本身在前端
//! （`src/lib/vrccHealth.ts`），那边有覆盖全部分支的测试。把判定搬到 Rust 会
//! 让两份规则漂移，所以本文件刻意只管时序与取舍。

use std::sync::Arc;

use vrcx_0_application_core::{RuntimeEventBus, VrccHealthProbe};
use vrcx_0_contracts::vrcc::{ErrorDetail, ErrorLayer, HttpDetail, VrccError, VrccStatus};

use super::health::{VrccHealthService, ISSUE_POLL_MS, OK_POLL_MS};
use super::tests::FakeRemote;
use super::VrccRemote;

fn service(remote: &Arc<FakeRemote>) -> VrccHealthService {
    VrccHealthService::new(
        Arc::clone(remote) as Arc<dyn VrccRemote>,
        RuntimeEventBus::new(),
    )
}

fn ok_status() -> VrccStatus {
    VrccStatus {
        at: Some(1_794_453_729_283),
        state: Some("ok".to_string()),
    }
}

fn http_error(status: u16) -> VrccError {
    VrccError::new(
        ErrorLayer::Server,
        ErrorDetail::Http(HttpDetail {
            status,
            error: "too_many".to_string(),
            need: None,
            why: None,
            retry_after: Some(30),
            op: Some("status"),
        }),
    )
}

fn transport_error() -> VrccError {
    VrccError::new(
        ErrorLayer::Transport,
        ErrorDetail::Transport {
            kind: vrcx_0_contracts::vrcc::TransportKind::Dns,
            message: "dns failed".to_string(),
        },
    )
}

#[tokio::test]
async fn a_successful_probe_records_ok_and_the_normal_interval() {
    let remote = FakeRemote::new();
    remote.set_status(Ok(ok_status()));

    let snapshot = service(&remote).refresh().await.expect("refresh");

    assert!(matches!(snapshot.probe, VrccHealthProbe::Ok { .. }));
    assert_eq!(snapshot.polling_interval_ms, OK_POLL_MS);
    assert!(snapshot.checked_at.is_some());
    assert!(!snapshot.refreshing);
}

/// 契约里 `state` 的 enum 只有 `"ok"`。别的值说明服务端加了新语义，
/// 我们读不懂 —— 读不懂就不能说它正常。
#[tokio::test]
async fn an_unknown_state_is_not_reported_as_ok() {
    let remote = FakeRemote::new();
    remote.set_status(Ok(VrccStatus {
        at: None,
        state: Some("degraded".to_string()),
    }));

    let snapshot = service(&remote).refresh().await.expect("refresh");

    assert!(matches!(snapshot.probe, VrccHealthProbe::StateNotOk { .. }));
    // 服务端明确回了「不是 ok」，这是站方那边的事，按异常档问得更勤。
    assert_eq!(snapshot.polling_interval_ms, ISSUE_POLL_MS);
}

#[tokio::test]
async fn a_transport_failure_becomes_a_conclusion_and_shortens_the_interval() {
    let remote = FakeRemote::new();
    remote.set_status(Err(transport_error()));

    let service = service(&remote);
    let result = service.refresh().await;

    assert!(result.is_err(), "失败要能被循环看见，好切到异常间隔");
    let snapshot = service.snapshot();
    assert!(matches!(snapshot.probe, VrccHealthProbe::Failed { .. }));
    assert_eq!(snapshot.polling_interval_ms, ISSUE_POLL_MS);
}

/// 本机限流：请求根本没发出去，服务状态没变。
#[tokio::test]
async fn local_throttling_keeps_the_previous_conclusion() {
    let remote = FakeRemote::new();
    remote.set_status(Ok(ok_status()));
    let service = service(&remote);
    let first = service.refresh().await.expect("first refresh");

    remote.set_status(Err(VrccError::new(
        ErrorLayer::Client,
        ErrorDetail::ThrottledLocally {
            reason: vrcx_0_contracts::vrcc::LocalThrottle::Gap,
            retry_after_secs: 30,
        },
    )));
    let second = service.refresh().await.expect("throttled refresh");

    assert_eq!(second.probe, first.probe, "结论不能被一次本机限流抹掉");
    assert_eq!(second.checked_at, first.checked_at, "没有新结论就不动时间");
    assert!(!second.refreshing);
}

/// 探活跳过全局退避之后会自己撞上 429：官方限流按**全部接口合计**算。
/// 那说明「问得太勤」，不是「服务挂了」。
#[tokio::test]
async fn a_429_keeps_the_previous_conclusion_instead_of_reporting_an_outage() {
    let remote = FakeRemote::new();
    remote.set_status(Ok(ok_status()));
    let service = service(&remote);
    let first = service.refresh().await.expect("first refresh");

    remote.set_status(Err(http_error(429)));
    let second = service.refresh().await.expect("rate limited refresh");

    assert_eq!(second.probe, first.probe);
    assert_eq!(second.checked_at, first.checked_at);
    assert_eq!(
        second.polling_interval_ms, first.polling_interval_ms,
        "被限流不代表服务有问题，不该切到异常间隔"
    );
}

/// 5xx 是站方那边的事，必须如实记下来 —— 玩家做不了什么，但该知道。
#[tokio::test]
async fn a_5xx_is_reported_as_a_failure() {
    let remote = FakeRemote::new();
    remote.set_status(Err(http_error(500)));

    let service = service(&remote);
    let _ = service.refresh().await;

    let snapshot = service.snapshot();
    assert!(matches!(
        snapshot.probe,
        VrccHealthProbe::Failed { ref layer, .. } if layer == "server"
    ));
}

#[tokio::test]
async fn concurrent_refreshes_share_one_request() {
    let remote = FakeRemote::new();
    remote.set_status(Ok(ok_status()));
    // 回答必须真的花时间，否则三个调用谁也碰不到谁的 in-flight。
    remote.set_status_delay(std::time::Duration::from_millis(20));
    let service = service(&remote);

    let (a, b, c) = tokio::join!(service.refresh(), service.refresh(), service.refresh());

    assert!(a.is_ok() && b.is_ok() && c.is_ok());
    assert_eq!(remote.status_calls(), 1, "并发调用只发一次请求");
}

#[tokio::test]
async fn a_recovered_service_returns_to_the_normal_interval() {
    let remote = FakeRemote::new();
    remote.set_status(Err(transport_error()));
    let service = service(&remote);
    let failed = service.refresh().await;
    assert!(failed.is_err());
    assert_eq!(service.snapshot().polling_interval_ms, ISSUE_POLL_MS);

    remote.set_status(Ok(ok_status()));
    let recovered = service.refresh().await.expect("recovered");

    assert_eq!(recovered.polling_interval_ms, OK_POLL_MS);
}
