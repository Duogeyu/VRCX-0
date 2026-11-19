// SPDX-License-Identifier: MPL-2.0

import { useEffect } from 'react';

import { useVrccHealthStore } from '@/state/vrccHealthStore';

import { hydrateVrccHealth, refreshVrccHealth } from './vrccHealthService';

/** 回到窗口时，距上次结论超过这么久才请后端补探一次。 */
const FOCUS_REFRESH_MS = 60 * 1_000;

/**
 * 星遇同频服务可达性：把后端那份快照接进前端 store。
 *
 * ## 这个组件不再轮询
 *
 * 探测住在 Rust 侧的后台循环（`VrccHealthService::start_loop`），由
 * `DesktopVrccRuntime` 在装配阶段起。前端守着一个定时器的问题是：窗口最小化到
 * 托盘后 WebView 会节流它，探测跟着停摆 —— 而"切出去很久、回来想知道还能不能
 * 用"恰恰是最该可靠的场景。
 *
 * ## 它只做两件事
 *
 * 1. 启动时读一次已有快照（无论 WebView 什么时候起来、是不是重启过，都能立刻
 *    显示上一次的结论）；
 * 2. 回到窗口时请后端补探一次 —— 后端循环的间隔最长 5 分钟，而玩家切回来的
 *    那一刻他最想知道的正是"现在"。一分钟内的重复聚焦不重复请。
 *
 * 刻意不看登录态：`GET /api/v1/status` 不要令牌，登录页也该有结论。
 */
export function VrccHealthProbeHost(): null {
    useEffect(() => {
        void hydrateVrccHealth();

        const onFocus = () => {
            const checkedAt = useVrccHealthStore.getState().checkedAt;
            if (
                checkedAt !== null &&
                Date.now() - checkedAt < FOCUS_REFRESH_MS
            ) {
                return;
            }
            void refreshVrccHealth();
        };
        window.addEventListener('focus', onFocus);

        return () => {
            window.removeEventListener('focus', onFocus);
        };
    }, []);

    return null;
}
