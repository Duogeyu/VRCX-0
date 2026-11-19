// SPDX-License-Identifier: MPL-2.0

/**
 * 星遇同频（vrcc.cc）服务可达性：前端这一侧只负责**读投影**。
 *
 * ## 采集不在这里
 *
 * 探测住在 Rust 侧的后台循环（`VrccHealthService`）：单飞、自适应间隔（正常
 * 5 分钟 / 异常 2 分钟）、每次变化都推 `vrccHealth` 事件。前端自己 `setInterval`
 * 有个硬伤 —— 窗口最小化到托盘后 WebView 会节流定时器，探测就停摆了，而
 * 「切出去很久、回来想知道 vrcc 还能不能用」恰恰是它最该可靠的场景。
 *
 * ## 它探的是什么
 *
 * `GET /api/v1/status`：官方探活接口，不要令牌、不计入任何个人额度，服务端自己
 * 缓存一分钟。**采集不看登录态**，所以登录页也读这份结论。
 *
 * ## 前端还剩什么
 *
 * - `applyVrccHealthSnapshot`：事件桥把推来的快照交给 store；
 * - `hydrateVrccHealth`：启动时读一次已有快照（纯内存，不发请求）——
 *   WebView 里存着的那份可能比后端旧；
 * - `refreshVrccHealth`：请后端立刻探一次（应用层单飞，重复调用共用一次请求）。
 */

import { commands } from '@/platform/tauri/bindings';
import type { VrccHealthSnapshot } from '@/platform/tauri/bindings';
import { useVrccHealthStore } from '@/state/vrccHealthStore';

/**
 * 把后端推来的快照写进 store。
 *
 * 「没有结论就不写」的规则在 store 里（`applySnapshot` 对 `idle` 的处理），
 * 本层不做判断。
 */
function applySnapshot(snapshot: VrccHealthSnapshot): void {
    useVrccHealthStore.getState().applySnapshot(snapshot);
}

/** 读一次当前快照。纯内存读取，不发请求。 */
export async function hydrateVrccHealth(): Promise<void> {
    try {
        applySnapshot(await commands.appVrccHealthGet());
    } catch (error: unknown) {
        console.warn('[vrcc] failed to read health snapshot:', error);
    }
}

/**
 * 请后端立刻探一次。
 *
 * 失败不抛：这是一条后台探活，界面上由消费点自己决定要不要说话。真正「这次探测
 * 得到什么结论」是通过 `vrccHealth` 事件回来的，不是这个 Promise 的返回值。
 */
export async function refreshVrccHealth(): Promise<void> {
    try {
        applySnapshot(await commands.appVrccHealthRefresh());
    } catch (error: unknown) {
        console.warn('[vrcc] health probe failed:', error);
    }
}
