// SPDX-License-Identifier: MPL-2.0

import { create } from 'zustand';

import type { VrccHealth } from '@/lib/vrccHealth';

/**
 * 星遇同频（vrcc.cc）的服务健康，供任何界面读。
 *
 * ## 为什么是一份常驻状态
 *
 * 同样的位置以前住在 `useVrccController` 里，而那个 hook 只在星遇同频页面挂载
 * 时存在。于是：
 *
 * - 登录页想知道"vrcc.cc 现在能不能连"，拿不到，因为登录页没有那个 controller；
 * - 页面关掉之后没人再探，徽章停在最后一次的结论上。
 *
 * 服务可达性和玩家账号无关（`GET /api/v1/status` 不要令牌；`X-Client` 官方不要求，
 * 但我们的客户端照旧会带，见 `vrcc-client` 的 `send_reserved`），
 * 所以它该活在整个应用的生命周期里，而不是某个页面的生命周期里。
 *
 * ## 为什么不存原始响应
 *
 * 和 `vrccUnreadStore` 一样只存**结论**。`VrccStatus` 的形状会随服务端版本变，
 * 界面不该跟着它改；判定规则住在 `@/lib/vrccHealth`，改那里就够了。
 */
type VrccHealthStore = {
    health: VrccHealth;
    /**
     * 最近一次拿到**结论**的时刻（`Date.now()` 毫秒）。
     *
     * `null` 表示还没有过结论 —— 和 `health.state === 'unknown'` 是两件事：
     * 探测被本机限流挡下时也不产生结论，那时这个值同样不变。
     */
    checkedAt: number | null;
    setHealth(health: VrccHealth, checkedAt: number): void;
    reset(): void;
};

export const useVrccHealthStore = create<VrccHealthStore>((set) => ({
    health: { state: 'unknown' },
    checkedAt: null,
    setHealth: (health, checkedAt) => set({ health, checkedAt }),
    reset: () => set({ health: { state: 'unknown' }, checkedAt: null })
}));
