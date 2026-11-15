// SPDX-License-Identifier: MPL-2.0

//! 后台轮询的调度：什么时候起一代任务、什么时候停。
//!
//! **这里只做调度，不做轮询本体。** 「怎么发请求、发哪些、通知怎么弹」是宿主的
//! 知识；「登录就绪时起、换账号/登出时停」这一层判断与宿主无关，所以放在这里。
//!
//! 三个端口（[`VrccPollRuntime`]、[`VrccAuthIdentity`]、[`VrccAuthObserver`]）
//! 的存在是为了让这个 crate 不出现任何宿主类型——没有 `TaskSupervisor`，没有
//! `NotificationDispatcher`，也没有 `OwnerId`。宿主在自己的 crate 里实现它们。
//!
//! # 为什么不直接收 `Arc<dyn Fn()>`
//!
//! 起任务要传的东西（调度器、通知分发器、事件出口）全是宿主类型，塞进闭包会让
//! 调用方每次自己拼一遍，而那正是「宿主细节渗进调度逻辑」的老路。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 一代轮询任务的停止开关。
///
/// 置位后任务在**下一次醒来时**自行退出，不做强制中断：调度层拿不到任务句柄，
/// 也不想拿到。
pub type VrccPollStop = Arc<AtomicBool>;

/// 「当前登录状态 × 是否已有任务」应当触发的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrccPollAction {
    /// 已登录且已有任务：重起一次。
    Restart,
    /// 已登录但没有任务：起一个。
    Start,
    /// 未登录但有任务：停掉。
    Stop,
    /// 两边都空：不动。
    Idle,
}

/// 纯函数：把登录状态与「当前是否有任务」映射成要执行的动作。
///
/// 抽成纯函数只为可单测：四个分支互斥且无副作用，[`VrccPollBinding::reconcile`]
/// 只负责执行。空字符串与纯空白都按「未登录」处理，与调用方传 `None` 等价。
pub fn decide(signed_in_as: Option<&str>, has_active: bool) -> VrccPollAction {
    let signed_in = matches!(signed_in_as.map(str::trim), Some(id) if !id.is_empty());
    match (signed_in, has_active) {
        (true, true) => VrccPollAction::Restart,
        (true, false) => VrccPollAction::Start,
        (false, true) => VrccPollAction::Stop,
        (false, false) => VrccPollAction::Idle,
    }
}

/// 一代任务的启动结果。
///
/// 只区分「排上了」和「没排上」，不像宿主的 `TaskSpawnOutcome` 那样细分
/// `Rejected` / `Failed`——调度层对这两者的反应完全一样（记一条日志、不记 active），
/// 细分只会让这里多一个需要映射的枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrccPollSpawn {
    /// 任务已排上。
    Scheduled,
    /// 没有排上（宿主侧拒绝或失败）。
    NotScheduled,
}

/// 起一代轮询任务。宿主实现，负责真正的轮询循环。
pub trait VrccPollRuntime: Send + Sync {
    /// 起一个属于 `owner` 的后台轮询任务。
    ///
    /// `stop` 置位时任务应在下次醒来时退出。实现方**不应**在返回前阻塞。
    fn spawn_poll(&self, owner: &str, stop: VrccPollStop) -> VrccPollSpawn;
}

/// 当前登录身份，以及它的变化通知。
pub trait VrccAuthIdentity: Send + Sync {
    /// 当前登录的账号 ID；未登录时 `None`。
    ///
    /// 返回空白串与 `None` 等价——调用方统一走 [`decide`] 判定。
    fn current_owner(&self) -> Option<String>;

    /// 订阅登录状态变化。
    ///
    /// **注册时应立即按当前状态回调一次**：已登录的玩家不该等到下一次登录才起
    /// 任务。宿主的观察者注册语义本来就带这次初始回调，实现方如实转达即可。
    ///
    /// **实现方必须一直持有 `observer` 的强引用**（通常与宿主同寿）。[`install`]
    /// 自己不留任何引用，调度状态只靠这个观察者活着；宿主若只存弱引用，就得
    /// 自己另外留一份，否则初始回调一结束订阅就没了，之后的登录、换号都听不到。
    fn subscribe(&self, observer: Arc<dyn VrccAuthObserver>);
}

/// 登录状态变化的接收方。
pub trait VrccAuthObserver: Send + Sync {
    /// 登录账号变化；登出时传 `None`。
    fn vrc_auth_changed(&self, owner: Option<&str>);
}

/// 按登录状态维护「当前这一代」轮询任务。
pub struct VrccPollBinding {
    runtime: Arc<dyn VrccPollRuntime>,
    identity: Arc<dyn VrccAuthIdentity>,
    /// 当前那代任务的停止开关；`None` 表示没有任务在跑。
    active: Mutex<Option<VrccPollStop>>,
}

impl VrccPollBinding {
    pub fn new(runtime: Arc<dyn VrccPollRuntime>, identity: Arc<dyn VrccAuthIdentity>) -> Self {
        Self {
            runtime,
            identity,
            active: Mutex::new(None),
        }
    }

    /// 按当前登录状态对齐：该起就起，该停就停。
    ///
    /// **整个过程都持着 `active` 的锁**，包括起任务那一步。早先在 `start` 之前
    /// 放了锁：两次登录事件挨得很近时，两边都看到「没有任务」、各起一代，后写
    /// 的那个覆盖 `active`，先起的那代就再也没人能叫停，同一条提醒会弹两遍。
    /// 持锁起任务是安全的：[`VrccPollRuntime::spawn_poll`] 约定不阻塞，
    /// [`VrccAuthIdentity::current_owner`] 也只是读一下宿主的登录状态。
    fn reconcile(&self, signed_in_as: Option<&str>) {
        let mut active = match self.active.lock() {
            Ok(active) => active,
            Err(error) => error.into_inner(),
        };

        match decide(signed_in_as, active.is_some()) {
            // 已登录且已有任务：重起一次。任务本身很轻，而跨代复用会让
            // owner 与令牌的一致性难以推理。
            VrccPollAction::Restart => {
                self.stop(&mut active);
                self.start(&mut active);
            }
            VrccPollAction::Start => {
                self.start(&mut active);
            }
            VrccPollAction::Stop => {
                self.stop(&mut active);
            }
            VrccPollAction::Idle => {}
        }
    }

    fn start(&self, active: &mut Option<VrccPollStop>) {
        let Some(owner) = self.identity.current_owner() else {
            return;
        };
        let owner = owner.trim();
        if owner.is_empty() {
            // `current_owner` 与 `reconcile` 之间账号可能刚好失效；这里再挡一次，
            // 免得把一个空 owner 交给宿主。
            return;
        }
        let stop: VrccPollStop = Arc::new(AtomicBool::new(false));
        match self.runtime.spawn_poll(owner, Arc::clone(&stop)) {
            VrccPollSpawn::Scheduled => {
                tracing::info!(owner, "vrcc: background poll scheduled");
                *active = Some(stop);
            }
            other => {
                tracing::warn!(?other, owner, "vrcc: background poll not scheduled");
            }
        }
    }

    fn stop(&self, active: &mut Option<VrccPollStop>) {
        if let Some(stop) = active.take() {
            stop.store(true, Ordering::Release);
            tracing::info!("vrcc: background poll asked to stop (account change)");
        }
    }
}

/// 登录状态变化的观察者：转给 [`VrccPollBinding`] 对齐任务。
struct VrccPollObserver {
    binding: Arc<VrccPollBinding>,
}

impl VrccAuthObserver for VrccPollObserver {
    fn vrc_auth_changed(&self, owner: Option<&str>) {
        self.binding.reconcile(owner);
    }
}

/// 装配：建立绑定并挂到登录状态上。
///
/// 宿主的 `subscribe` 会立刻按当前状态回调一次：已登录时任务这时就起来了。
pub fn install(runtime: Arc<dyn VrccPollRuntime>, identity: Arc<dyn VrccAuthIdentity>) {
    let binding = Arc::new(VrccPollBinding::new(runtime, Arc::clone(&identity)));
    identity.subscribe(Arc::new(VrccPollObserver { binding }));
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{
        decide, install, VrccAuthIdentity, VrccAuthObserver, VrccPollAction, VrccPollRuntime,
        VrccPollSpawn, VrccPollStop,
    };

    #[test]
    fn signed_in_with_existing_task_restarts() {
        assert_eq!(decide(Some("usr_1"), true), VrccPollAction::Restart);
    }

    #[test]
    fn signed_in_without_task_starts() {
        assert_eq!(decide(Some("usr_1"), false), VrccPollAction::Start);
    }

    #[test]
    fn signed_out_with_task_stops() {
        assert_eq!(decide(None, true), VrccPollAction::Stop);
    }

    #[test]
    fn signed_out_without_task_is_idle() {
        assert_eq!(decide(None, false), VrccPollAction::Idle);
    }

    #[test]
    fn blank_identity_is_treated_as_signed_out() {
        assert_eq!(decide(Some("   "), true), VrccPollAction::Stop);
        assert_eq!(decide(Some(""), false), VrccPollAction::Idle);
        assert_eq!(decide(Some("  "), false), VrccPollAction::Idle);
    }

    #[test]
    fn surrounding_whitespace_still_counts_as_signed_in() {
        assert_eq!(decide(Some(" usr_1 "), false), VrccPollAction::Start);
    }

    /// 记下每一代任务的 owner 和停止开关。
    #[derive(Default)]
    struct FakeRuntime {
        spawned: Mutex<Vec<(String, VrccPollStop)>>,
        /// 起任务前睡一会儿，把「检查 → 起任务」之间的窗口拉大。
        spawn_delay: Duration,
    }

    impl FakeRuntime {
        fn running(&self) -> Vec<String> {
            self.spawned
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, stop)| !stop.load(Ordering::Acquire))
                .map(|(owner, _)| owner.clone())
                .collect()
        }
    }

    impl VrccPollRuntime for FakeRuntime {
        fn spawn_poll(&self, owner: &str, stop: VrccPollStop) -> VrccPollSpawn {
            std::thread::sleep(self.spawn_delay);
            self.spawned.lock().unwrap().push((owner.to_string(), stop));
            VrccPollSpawn::Scheduled
        }
    }

    /// 像宿主那样持有观察者的强引用，并能手动「换号」。
    #[derive(Default)]
    struct FakeIdentity {
        owner: Mutex<Option<String>>,
        observers: Mutex<Vec<Arc<dyn VrccAuthObserver>>>,
    }

    impl FakeIdentity {
        fn sign_in_as(&self, owner: Option<&str>) {
            *self.owner.lock().unwrap() = owner.map(str::to_string);
            let observers = self.observers.lock().unwrap().clone();
            for observer in observers {
                observer.vrc_auth_changed(owner);
            }
        }
    }

    impl VrccAuthIdentity for FakeIdentity {
        fn current_owner(&self) -> Option<String> {
            self.owner.lock().unwrap().clone()
        }

        fn subscribe(&self, observer: Arc<dyn VrccAuthObserver>) {
            self.observers.lock().unwrap().push(Arc::clone(&observer));
            observer.vrc_auth_changed(self.current_owner().as_deref());
        }
    }

    #[test]
    fn follows_login_switch_and_logout_with_one_generation_at_a_time() {
        let runtime = Arc::new(FakeRuntime::default());
        let identity = Arc::new(FakeIdentity::default());
        install(runtime.clone(), identity.clone());
        assert!(runtime.spawned.lock().unwrap().is_empty());

        identity.sign_in_as(Some("usr_a"));
        assert_eq!(runtime.running(), vec!["usr_a"]);

        identity.sign_in_as(Some("usr_b"));
        assert_eq!(runtime.running(), vec!["usr_b"]);

        identity.sign_in_as(None);
        assert!(runtime.running().is_empty());
        assert_eq!(runtime.spawned.lock().unwrap().len(), 2);
    }

    #[test]
    fn concurrent_auth_events_never_leave_an_orphaned_generation() {
        let runtime = Arc::new(FakeRuntime {
            spawn_delay: Duration::from_millis(5),
            ..FakeRuntime::default()
        });
        let identity = Arc::new(FakeIdentity::default());
        install(runtime.clone(), identity.clone());
        *identity.owner.lock().unwrap() = Some("usr_a".to_string());
        let observer = Arc::clone(&identity.observers.lock().unwrap()[0]);

        let threads: Vec<_> = (0..8)
            .map(|_| {
                let observer = Arc::clone(&observer);
                std::thread::spawn(move || observer.vrc_auth_changed(Some("usr_a")))
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }

        assert_eq!(runtime.running(), vec!["usr_a"]);
        assert_eq!(runtime.spawned.lock().unwrap().len(), 8);
    }
}
