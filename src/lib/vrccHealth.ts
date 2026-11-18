// SPDX-License-Identifier: MPL-2.0

/**
 * 星遇同频（vrcc.cc）服务状态的共享判定。
 *
 * ## 和 `serverHealth.ts` 的关系
 *
 * 两个模块形态相似、结论不同，**刻意没有合并**：
 *
 * - VRChat 那份来自 statuspage.io，能给出 `minor` / `major` / `critical` 这样的
 *   严重度枚举，所以 `ServerHealth` 有 `severity`；
 * - 星遇同频的 `GET /api/v1/status` 契约只有 `{at, state}`，`state` 的枚举值只有
 *   `"ok"` 一个。它给不出严重度，只给"能用/不能用"。
 *
 * 硬套成一个类型会逼着其中一方造假字段。等两边的真相都稳定了再谈抽象。
 *
 * ## 这里真正要区分的东西：谁的锅
 *
 * 官方文档说得很粗：「连不上或回答不是 `200`，就是接口出问题了」。对客户端这句话
 * 是**错的** —— 它把两件处置方向完全相反的事并成了一件事：
 *
 * | 情况 | 玩家的动作 | 我们的依据 |
 * |---|---|---|
 * | DNS 失败、连不上、超时 | 查代理、查网络 —— **他能修** | `ErrorLayer::Transport` |
 * | 拿到了响应，服务器回 5xx | 什么也做不了，只能等 —— **他修不了** | `ErrorLayer::Server` |
 * | 本机限流把请求拦下 | 等 30 秒，**服务可能是好的** | `ErrorLayer::Client` |
 *
 * 这三种的分类结论早就有了（`VrccError.layer`，由 Rust 的 `vrcc-client` 在
 * `transport_error()` 和 HTTP 分支里定），只是一直没人拿它做健康判定 ——
 * `useVrccController` 里是 `service.state !== 'ok'` 一刀切，于是三种情况在界面上
 * 长得一模一样。
 *
 * `ErrorLayer` 是从 Rust 一路带到前端的结构化字段，不是对文案做字符串匹配
 * （`vrccError.ts` 的文件头专门讲过为什么）。所以这里可以放心地按它分支。
 *
 * ## 停站不在这里
 *
 * `503 stop`（站方停站维护）**不归这个模块管**。它在 Rust 侧被归进
 * `ErrorLayer::Server`，因为对调用方来说"等一会儿就好"；但它的真相是"站方自己
 * 在维护、什么时候回来写在公告里"，属于公告层（`GET /announcements` 的 `stop`）。
 * 两件事走两套机制，不要在一个容器的文案里混起来。
 */

import type {
    VrccErrorKind,
    VrccErrorLayer,
    VrccTransportKind
} from './vrccError';

/**
 * 一份探测结果能支持的结论。
 *
 * 和 `ServerHealth` 一样，**没有"健康以外可以显示成健康"的兜底**。
 */
export type VrccHealth =
    /** 还没探过，或者唯一一次探测被本机限流挡下了（那时服务可能是好的）。 */
    | { state: 'unknown' }
    /** 服务器说它正常。 */
    | { state: 'ok'; at: number | null }
    /**
     * 请求没能拿到响应：DNS、建连、超时、响应中断。
     *
     * 这是**玩家能自己处理**的那一类，所以把 `transportKind` 带出去 —— 界面据此
     * 说"可能被拦截"还是"开个代理试试"，而不是笼统地报一句网络错误。
     */
    | {
          state: 'unreachable';
          reason: string;
          transportKind: VrccTransportKind | null;
      }
    /**
     * 拿到了响应，但服务器说它有问题（5xx），或回了个 `state` 不是 `ok` 的回答。
     *
     * 这一类**玩家做不了什么**，文案该说"等一会"而不是"检查你的网络"。
     */
    | { state: 'failing'; reason: string };

/**
 * 一次探测的结果。由加载器（`features/vrcc/vrccHealthService.ts`）产生。
 *
 * 刻意用扁平的纯数据而不是直接传 `VrccError` 实例：这个模块要能在测试里
 * 逐条造各种输入，包括真实网络很难复现的组合。
 */
export type VrccProbe =
    /** 拿到了 200，且 `state === "ok"`。 */
    | { outcome: 'ok'; at: number | null }
    /** 拿到了 200，但 `state` 不是 `ok`。契约里 enum 只有 `"ok"`，所以这是未知情况。 */
    | { outcome: 'state-not-ok'; raw: string }
    /** 请求失败。字段来自 `VrccError`。 */
    | {
          outcome: 'failed';
          layer: VrccErrorLayer;
          kind: VrccErrorKind;
          transportKind: VrccTransportKind | null;
          message: string;
      };

/**
 * 把一次探测判成结论。传 `null` 表示还没探过。
 */
export function resolveVrccHealth(probe: VrccProbe | null): VrccHealth {
    if (!probe) {
        return { state: 'unknown' };
    }

    if (probe.outcome === 'ok') {
        return { state: 'ok', at: probe.at };
    }

    if (probe.outcome === 'state-not-ok') {
        // 契约里 `state` 只有 `"ok"`。出现别的值说明服务端加了新语义，
        // 我们读不懂 —— 读不懂就不能说它正常。
        return { state: 'failing', reason: probe.raw };
    }

    // 本机限流：请求**根本没发出去**（`ErrorLayer::Client` 只在 `acquire()` 里产生），
    // 服务本身可能是好的。这种情况必须保持"不知道"，不能升级成故障。
    // 这和 `useVrccController` 里 `kind === 'throttled'` 那段是同一个判断，只是
    // 那里是为了不误报页面错误，这里是为了不误报站点状态。
    if (probe.layer === 'client') {
        return { state: 'unknown' };
    }

    if (probe.layer === 'transport') {
        return {
            state: 'unreachable',
            reason: probe.message,
            transportKind: probe.transportKind
        };
    }

    if (probe.layer === 'server') {
        // `needsReauth` 出现在这条路径上，只可能是**我们自己的标识有问题**。
        //
        // 理由：`/status` 是 `authed: false`，令牌失效那类（`404 gone`、`401 token`）
        // 根本走不到这里；剩下的 `403 client_mismatch` / `client_unknown` /
        // `client_off` / `client_missing` 说的是「这个 `X-Client` 不可用」——
        // 那是构建期的客户端标识坏了，不是站方挂了。
        //
        // 报成 `failing`（「接口暂时不可用」）会让玩家白白等一个永远等不好的东西。
        // 归到 `unknown`：界面上不打扰，但调用方应当把这条打进日志 —— 它需要被修，
        // 只是不该出现在玩家的故障提示里。
        if (probe.kind === 'needsReauth') {
            return { state: 'unknown' };
        }
        return { state: 'failing', reason: probe.message };
    }

    // `input` 是我们自己构造请求出了错，`session` 是令牌问题 —— `/status`
    // 既不需要令牌，参数也是写死的，两者都不该出现在这条路径上。
    // 真出现了属于我们的 bug（日志里会响），界面上按"不知道"处理：
    // 拿一个本机错误去告诉玩家"vrcc 挂了"是冤枉站方。
    return { state: 'unknown' };
}

/** 需不需要对玩家说点什么。`unknown` 不算（别在启动时闪一下告警）。 */
export function vrccHealthNeedsAttention(health: VrccHealth): boolean {
    return health.state === 'unreachable' || health.state === 'failing';
}
