// SPDX-License-Identifier: MPL-2.0

import { create } from 'zustand';

import type { VrccHealth } from '@/lib/vrccHealth';
import { healthFromSnapshot, probeFromSnapshot } from '@/lib/vrccHealth';
import type { VrccHealthSnapshot } from '@/platform/tauri/bindings';

/**
 * 星遇同频（vrcc.cc）的服务健康，供任何界面读。
 *
 * ## 只读投影
 *
 * 采集在 Rust 侧的后台循环里（`VrccHealthService`），这个 store 只接收推来的
 * 快照。**前端不再自己轮询**：窗口最小化到托盘后 WebView 会节流定时器，而
 * 「切出去很久、回来想知道 vrcc 还能不能用」恰恰是最该可靠的场景。
 *
 * ## 为什么只有一处 `unknown` 不覆盖
 *
 * 应用层已经挡掉了「本机限流」那一类（请求根本没发出去，服务状态没变）。这里
 * 再挡一次的是**首次加载**：快照的 `probe` 还是 `idle`，那不是结论，不能拿它
 * 抹掉已经拿到的 `ok` / `failing`。
 *
 * ## 为什么存结论而不存原始响应
 *
 * `VrccStatus` 的形状会随服务端版本变，界面不该跟着它改；判定规则住在
 * `@/lib/vrccHealth`，改那里就够了。
 */
type VrccHealthStore = {
    health: VrccHealth;
    /**
     * 最近一次拿到**结论**的时刻（毫秒）。
     *
     * `null` 表示还没有过结论 —— 和 `health.state === 'unknown'` 是两件事：
     * 探测被本机限流挡下时也不产生结论，那时这个值同样不变。
     */
    checkedAt: number | null;
    /** 后台正在探。界面拿它做脉冲，和 VRChat 那段一致。 */
    refreshing: boolean;
    applySnapshot(snapshot: VrccHealthSnapshot): void;
    setHealth(health: VrccHealth, checkedAt: number): void;
    reset(): void;
};

export const useVrccHealthStore = create<VrccHealthStore>((set) => ({
    health: { state: 'unknown' },
    checkedAt: null,
    refreshing: false,

    applySnapshot: (snapshot) =>
        set((current) => {
            const checkedAt = snapshot.checkedAt
                ? Date.parse(snapshot.checkedAt)
                : null;
            // 还没探过（`idle`）：只跟进「正在探」，不动已有结论。
            if (probeFromSnapshot(snapshot.probe) === null) {
                return { ...current, refreshing: snapshot.refreshing };
            }
            return {
                health: healthFromSnapshot(snapshot),
                checkedAt,
                refreshing: snapshot.refreshing
            };
        }),

    setHealth: (health, checkedAt) => set({ health, checkedAt }),
    reset: () =>
        set({
            health: { state: 'unknown' },
            checkedAt: null,
            refreshing: false
        })
}));
