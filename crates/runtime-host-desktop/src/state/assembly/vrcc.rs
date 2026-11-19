// SPDX-License-Identifier: MPL-2.0

//! 星遇同频运行时的装配。
//!
//! 单独一个文件，是为了让上游改 `state.rs` / `assembly/*` 时不和 vrcc 的装配
//! 撞在一起：这里只属于本下游仓库，`state.rs` 里只剩一次调用和一个字段。

use std::sync::Arc;

use vrcx_0_composition::RuntimeHostState;

use super::super::DesktopRuntimeBundle;
use crate::vrcc::DesktopVrccRuntime;

/// 建好星遇同频运行时，并挂到腕表上。
pub(in crate::state) fn build_vrcc_runtime(
    runtime: &RuntimeHostState,
    desktop: &DesktopRuntimeBundle,
) -> Arc<DesktopVrccRuntime> {
    // `X-Client` 在构建期定死（见 `resolve_client_id`），这里没有配置入口：
    // 标识属于「这个包是哪个客户端」，不该由用户侧的配置文件改写。
    // 令牌按账号隔离：闭包每次读写时现取当前登录用户，切号自然生效。
    let vrcc_owner_scope = runtime.desktop_assembly().auth_scope().clone();
    // 标成 trait object，好让同一个闭包既能给令牌存储、也能给运行时用。
    let vrcc_owner: Arc<dyn Fn() -> Option<vrcx_0_core::OwnerId> + Send + Sync> =
        Arc::new(move || {
            let snapshot = vrcc_owner_scope.snapshot();
            snapshot
                .active
                .then(|| vrcx_0_core::OwnerId::new(snapshot.current_user_id))
        });
    let vrcc = Arc::new(DesktopVrccRuntime::new(
        vrcx_0_outbound_adapters::LocalVrccTokenStore::new(
            Arc::clone(runtime.database()),
            Arc::clone(&vrcc_owner),
        ),
        // 用宿主应用统一的代理设置：本机实测 vrcc.cc 直连 DNS 超时，
        // 不接代理时所有接口都会表现为「暂时不可用」。
        runtime.web_client().proxy_url(),
        // 和令牌存储同一个来源：通知中心的行要落到当前账号的表里。
        vrcc_owner,
        // 服务名要按界面语言取官方译名（`vrcc.title`），所以给它配置仓库。
        runtime.desktop_assembly().config_store(),
        // 健康探测要推事件给前端，和 VRChat 那条用同一个总线。
        runtime.desktop_assembly().event_bus().clone(),
    ));
    // 腕表要显示星遇同频摘要，把运行时挂上；没挂时腕表不显示这一块。
    desktop.services.attach_vrcc_runtime(Arc::clone(&vrcc));
    // 令牌按账号存，而这里还没登录：登录、换号、登出时都要按新账号重新恢复
    // 会话。作用域只存弱引用，`vrcc` 由宿主状态持有。要先于后台轮询挂上（见
    // `start_vrcc_background_poll`）：观察者按注册顺序回调，轮询起任务时会话
    // 应当已经恢复好了。
    runtime.desktop_assembly().auth_scope().add_observer(
        Arc::clone(&vrcc) as Arc<dyn vrcx_0_application_core::RuntimeAuthScopeObserver>
    );
    vrcc
}
