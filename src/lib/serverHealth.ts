// SPDX-License-Identifier: MPL-2.0

/**
 * VRChat 服务端状态的共享判定。
 *
 * ## 为什么单独一个模块
 *
 * 同一份 `vrcStatus` 快照有三处消费点：
 *
 * | 消费点 | 位置 | 需要什么 |
 * |---|---|---|
 * | 状态栏圆点 | `components/layout/status-bar/StatusBarFooter.tsx` | 三档颜色 |
 * | 登录页告警 | `features/auth/components/LoginServerStatusAlert.tsx` | 有/无 + 严重度 |
 * | 隐私锁圆点 | `features/privacy-lock/PrivacyLockOverlay.tsx` | 布尔 |
 *
 * 三处原先各写一遍 `indicator && indicator !== 'none'`。判据本身等价（已穷举验证），
 * 但它们**共有一个盲区**：后端刷新失败时 `indicator` 保持原值，首次启动则是空串，
 * 于是"查不到"和"一切正常"在数据上完全一样 —— 三处会一起显示绿色。
 *
 * `StatusBarFooter` 是唯一读 `error` 的消费者，而且只在代理已启用时生效
 * （`statusBarProxy.ts` 的 `!enabled` 分支排在前面）。所以彻底断网时，整个应用
 * 没有任何一处说得出"我连不上状态页"。
 *
 * 把判定收到这里，是为了让"未知"成为一个**能被表达的状态**，而不是被折叠成"健康"。
 * 放在 `lib` 而不是 `features`：`components/**` 按架构约束不能引用 `@/features/**`，
 * 而状态栏属于 `components`。`vrccError.ts` 当初下沉到 `lib` 是同一个理由。
 *
 * ## 输入从哪来
 *
 * 字段全部来自 Rust 侧 `VrcStatusSnapshot`（`crates/application-core/src/event_bus.rs`）。
 * 它的编码方式决定了这里怎么判：
 *
 * - 后端认定"有事"时，`status` **一定**非空（描述原文，或 `VRChat Server Issues` 兜底）；
 * - 后端认定"没事"时，`status` / `indicator` / `summary` 三个字段一起写成空串；
 * - `lastFetchedAt` 在**成功和失败两条路径上都会写**（见 `vrc_status.rs` 的
 *   `perform_refresh`），所以它只说明"试过"，不说明"拿到过"。
 *
 * 因此 `status !== ''` 是比 `indicator` 更可靠的"有事"信号：它不受
 * `has_status_issue()` 与 `indicator` 编码不一致的影响（那个不一致见下）。
 *
 * 本模块**不解析文案**：`status` 里是 `"Partial System Outage"` 还是别的英文一律不管，
 * 严重度只认 `indicator` 的枚举值。仓库里已有的 `vrccError.ts` 也是同一条纪律。
 */

/**
 * 后端能给出的严重度档位。
 *
 * 取值来自 statuspage.io 的 `indicator`，经 `vrc_status.rs` 的
 * `component_indicator()` 归一（`major_outage` → `major`，
 * `partial_outage` / `degraded_performance` / `under_maintenance` → `minor`）。
 */
export type ServerHealthSeverity =
    | 'minor'
    | 'major'
    | 'critical'
    | 'maintenance';

/**
 * 一份快照能支持的所有结论。
 *
 * 四个状态互斥，`state` 是判别字段。**没有"健康"以外可以显示成健康的兜底**：
 * 只有真的拿到过一次成功回答、且那次说有没事，才是 `healthy`。
 */
export type ServerHealth =
    /** 还从没成功取到过，也没失败过：启动初期、或状态服务还没起来。 */
    | { state: 'unknown' }
    /** 最近一次取到了，且服务说没事。 */
    | { state: 'healthy'; checkedAt: string | null }
    /**
     * 服务说有事。
     *
     * `stale` 为真表示这条结论来自**更早**的一次成功检查，最近一次刷新失败了 ——
     * 结论本身仍然有效（服务确实报过这个），只是不能保证此刻还是这样。
     */
    | {
          state: 'issue';
          severity: ServerHealthSeverity;
          detail: string;
          stale: boolean;
          checkedAt: string | null;
      }
    /**
     * 取不到，而且手上没有任何"有事"的证据。
     *
     * 这和 `unknown` 不是一回事：`unknown` 是还没问过，这里是**问了但没答上来**。
     * 界面必须能说出这个区别，否则玩家分不清"VRChat 挂了"和"我的网络有问题"。
     */
    | { state: 'unreachable'; reason: string; checkedAt: string | null };

/** `resolveServerHealth` 的输入。字段名与 `VrcStatusSnapshot` 一致，便于调用方直传。 */
export type ServerHealthInput = {
    indicator: string;
    status: string;
    summary: string;
    error: string;
    lastFetchedAt: string | null;
};

/**
 * `indicator` 认不出来时的严重度。
 *
 * 取最轻的一档而不是 `major`：未知的取值不该把界面染成红色，但**也不能显示成健康**
 * —— 调用方拿到 `issue` 就一定会显示点什么。反过来把未知当 `major`，会让一次
 * statuspage 改字段名变成一场假警报。
 */
const FALLBACK_SEVERITY: ServerHealthSeverity = 'minor';

/** `indicator` → 严重度。只认枚举值，大小写敏感（后端写的是小写常量）。 */
function severityOf(indicator: string): ServerHealthSeverity {
    switch (indicator) {
        case 'critical':
            return 'critical';
        case 'major':
            return 'major';
        case 'maintenance':
            return 'maintenance';
        case 'minor':
            return 'minor';
        default:
            return FALLBACK_SEVERITY;
    }
}

/**
 * 把一份 `vrcStatus` 快照判成一个结论。
 *
 * 判定顺序是有意义的，三条注释说明了每一步为什么在这。
 */
export function resolveServerHealth(input: ServerHealthInput): ServerHealth {
    const error = input.error.trim();
    const indicator = input.indicator.trim();
    const status = input.status.trim();
    const checkedAt = input.lastFetchedAt;

    // 1. 有没有"有事"的证据。
    //
    // 两个信号取或：
    // - `status !== ''`：后端只有认定有事时才写它（见文件头的编码说明）；
    // - `indicator` 是个非 `none` 的值。
    //
    // 取或而不是只看 `indicator`，是为了接住后端的一个不变量缺口：
    // `vrc_status.rs:162` 判结论时同时看 `indicator` 和 `description`，但写回快照时
    // 只写 `effective_indicator`（`:174`）。当结论来自 description 时（例如
    // `indicator="none"` + `description="Partial System Outage"`），快照会是
    // "indicator 说没事、status 说有事"。只看 indicator 会把这一份读成健康。
    const hasIssue =
        status !== '' || (indicator !== '' && indicator !== 'none');
    if (hasIssue) {
        return {
            state: 'issue',
            severity: severityOf(indicator),
            // 组件名比总描述具体（"API, Website" 比 "Partial System Outage" 有用）。
            detail: input.summary.trim() || status,
            // 有结论、但最近一次刷新失败：结论照旧显示，同时标成陈旧。
            stale: error !== '',
            checkedAt
        };
    }

    // 2. 没有"有事"的证据，但刷新失败过。
    //
    // 这一步必须排在 `unknown` 之前：**失败会把 lastFetchedAt 写上一个值**，
    // 所以"从没成功过"和"成功过、现在连不上"在时间戳上分不开。两者都只能说
    // "我不确定"，不能说"没问题"。
    if (error !== '') {
        return { state: 'unreachable', reason: error, checkedAt };
    }

    // 3. 没证据、没报错、也没试过：启动初期。
    if (checkedAt === null) {
        return { state: 'unknown' };
    }

    // 4. 取到过、没报错、服务说没事。
    return { state: 'healthy', checkedAt };
}

/** 需不需要对玩家说点什么。`unknown` 不算（别在启动时闪一下告警）。 */
export function serverHealthNeedsAttention(health: ServerHealth): boolean {
    return health.state === 'issue' || health.state === 'unreachable';
}
