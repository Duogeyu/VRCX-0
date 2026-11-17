// SPDX-License-Identifier: MPL-2.0

//! 把 vrcc 后台轮询绑定到「VRCX 账号登录状态」上的装配。
//!
//! **调度逻辑本身在 `vrcx_0_vrcc_client::poll`**：什么时候起一代任务、什么时候
//! 停，是纯判断加两个动作，跟宿主无关。这里只做一件事——把宿主的调度器、通知
//! 分发器、事件出口和登录状态包装成那三个端口。
//!
//! 为什么要这么分：轮询「怎么跑」属于宿主（要碰数据库、通知中心、多语言包），
//! 而「什么时候跑」不属于。混在一起时，整段调度都被钉死在 GPL-only 的宿主类型
//! 上，MPL 授权对想复用的外部消费者等于没有实际意义。
//!
//! 它跟的是「谁登录了 VRCX」，不是「VRChat 游戏进程在不在」：令牌属于账号，
//! 与游戏状态无关，所以只有登出（或换号）才停任务。理由见
//! `DesktopVrccRuntime::spawn_background_poll` 的「为什么不看游戏是否在运行」。
//!
//! 登录就绪时起一个属于该账号的任务；账号变化或登出时把上一代的 stop 置位，
//! 让它在下一次醒来时自行退出。旧任务不立即中止而是「下次醒来退出」，是因为
//! `TaskSupervisor` 不对外暴露单个任务的句柄——只能靠任务自己看信号。

use std::sync::{Arc, Mutex};

use vrcx_0_application_core::{
    RuntimeAuthScope, RuntimeAuthScopeObserver, RuntimeAuthScopeSnapshot, RuntimeEventSink,
    TaskSpawnOutcome, TaskSupervisor,
};
use vrcx_0_core::OwnerId;
use vrcx_0_vrcc_client::{
    VrccAuthIdentity, VrccAuthObserver, VrccPollRuntime, VrccPollSpawn, VrccPollStop,
};

use crate::notification::NotificationDispatcher;
use crate::vrcc::DesktopVrccRuntime;

/// 端口实现：把「起一代轮询」转成宿主的那一次 `spawn_background_poll`。
struct HostPollRuntime {
    runtime: Arc<DesktopVrccRuntime>,
    /// 站外提醒走通知分发器，和 VRChat 活动同一条投递链。
    ///
    /// 早先这里放的是 `DesktopNotifier`，直接调 `show()`——那等于绕过了免打扰、
    /// 隐私锁、通道偏好、图标解析和有序投递。现在没有第二条路径。
    dispatcher: Arc<NotificationDispatcher>,
    event_sink: Arc<dyn RuntimeEventSink>,
    supervisor: TaskSupervisor,
}

impl VrccPollRuntime for HostPollRuntime {
    fn spawn_poll(&self, owner: &str, stop: VrccPollStop) -> VrccPollSpawn {
        let outcome = self.runtime.spawn_background_poll(
            &self.supervisor,
            Arc::clone(&self.dispatcher),
            Arc::clone(&self.event_sink),
            OwnerId::new(owner),
            stop,
        );
        match outcome {
            TaskSpawnOutcome::Scheduled => VrccPollSpawn::Scheduled,
            // 调度层对 `Rejected` 与 `Failed` 的反应完全一样：记日志、不记 active。
            _ => VrccPollSpawn::NotScheduled,
        }
    }
}

/// 端口实现：把宿主的登录作用域翻译成「当前账号 + 变化通知」。
struct HostAuthIdentity {
    scope: RuntimeAuthScope,
    /// 注册出去的观察者，**强引用由这里持有**。
    ///
    /// `RuntimeAuthScope::add_observer` 只存 `Weak`，观察者活多久归注册方管。
    /// 早先这里直接把一个临时 `Arc` 交出去：注册时那一次初始回调一结束它就被
    /// 释放了，之后的登录、登出、换号一个都通知不到。应用启动时还没登录，那次
    /// 初始回调什么也不做，于是后台轮询从来没有起来过。
    ///
    /// 这会形成 `binding → identity → adapter → observer → binding` 的环，是有意
    /// 的：订阅与宿主同寿，装配只在启动时发生一次。
    subscriptions: Mutex<Vec<Arc<AuthObserverAdapter>>>,
}

impl HostAuthIdentity {
    fn new(scope: RuntimeAuthScope) -> Self {
        Self {
            scope,
            subscriptions: Mutex::new(Vec::new()),
        }
    }
}

impl VrccAuthIdentity for HostAuthIdentity {
    fn current_owner(&self) -> Option<String> {
        let snapshot = self.scope.snapshot();
        snapshot.active.then_some(snapshot.current_user_id)
    }

    fn subscribe(&self, observer: Arc<dyn VrccAuthObserver>) {
        let adapter = Arc::new(AuthObserverAdapter { observer });
        // 先留住再注册：`add_observer` 会立刻回调一次，回调里可能已经起了任务。
        self.subscriptions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(Arc::clone(&adapter));
        self.scope.add_observer(adapter);
    }
}

/// 把宿主的观察者回调转给调度层。
///
/// `add_observer` 会立刻按当前状态回调一次，转达即可——已登录的玩家不该等到
/// 下一次登录才起任务。
struct AuthObserverAdapter {
    observer: Arc<dyn VrccAuthObserver>,
}

impl RuntimeAuthScopeObserver for AuthObserverAdapter {
    fn runtime_auth_scope_changed(&self, snapshot: &RuntimeAuthScopeSnapshot) {
        let owner = snapshot.active.then_some(snapshot.current_user_id.as_str());
        self.observer.vrc_auth_changed(owner);
    }
}

/// 装配：建立两个端口实现，交给调度层挂到登录状态上。
///
/// `add_observer` 会立刻按当前状态回调一次：已登录时任务这时就起来了。
pub(crate) fn install<E>(
    runtime: Arc<DesktopVrccRuntime>,
    dispatcher: Arc<NotificationDispatcher>,
    supervisor: TaskSupervisor,
    auth_scope: RuntimeAuthScope,
    event_sink: Arc<E>,
) where
    E: RuntimeEventSink + 'static,
{
    let runtime_port: Arc<dyn VrccPollRuntime> = Arc::new(HostPollRuntime {
        runtime,
        dispatcher,
        event_sink: event_sink as Arc<dyn RuntimeEventSink>,
        supervisor,
    });
    let identity_port: Arc<dyn VrccAuthIdentity> = Arc::new(HostAuthIdentity::new(auth_scope));
    vrcx_0_vrcc_client::install(runtime_port, identity_port);
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use vrcx_0_application_core::RuntimeAuthScope;
    use vrcx_0_vrcc_client::{VrccAuthIdentity, VrccAuthObserver};

    use super::HostAuthIdentity;

    #[derive(Default)]
    struct Recorder {
        seen: Mutex<Vec<Option<String>>>,
    }

    impl VrccAuthObserver for Recorder {
        fn vrc_auth_changed(&self, owner: Option<&str>) {
            self.seen.lock().unwrap().push(owner.map(str::to_string));
        }
    }

    #[test]
    fn subscription_keeps_hearing_about_logins_after_registration() {
        // 应用启动时还没登录：初始回调是 None，真正有用的是之后那次登录。
        let scope = RuntimeAuthScope::new();
        let identity = HostAuthIdentity::new(scope.clone());
        let recorder = Arc::new(Recorder::default());
        identity.subscribe(Arc::clone(&recorder) as Arc<dyn VrccAuthObserver>);

        scope.set("usr_a", "https://api.vrchat.cloud/api/1");
        scope.set("usr_b", "https://api.vrchat.cloud/api/1");
        scope.set("", "");

        assert_eq!(
            *recorder.seen.lock().unwrap(),
            vec![
                None,
                Some("usr_a".to_string()),
                Some("usr_b".to_string()),
                None
            ]
        );
    }
}
