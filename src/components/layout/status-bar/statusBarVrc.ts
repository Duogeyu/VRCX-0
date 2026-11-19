// SPDX-License-Identifier: MPL-2.0

import type { ServerHealth } from '@/lib/serverHealth';

/**
 * 状态栏里「服务器状态」（VRChat）那一段长什么样。
 *
 * 和 `statusBarVrcc.ts`、`statusBarProxy.ts` 同一个套路：把判定结论翻成"圆点参数 +
 * 提示内容"，组件只负责画。这样这段能脱离渲染被测试。
 *
 * ## 这里修的是什么
 *
 * 判据以前是组件里那一行 `indicator && indicator !== 'none'`。它有个盲区：
 * 后端刷新失败时 `indicator` 保持原值（首次启动则是空串），于是"查不到"和
 * "一切正常"在数据上完全一样 —— 状态栏照样是绿点，工具提示里却同时写着
 * 「VRChat 服务目前一切正常」和 `error sending request`。
 *
 * 现在改用 `@/lib/serverHealth` 的四态判定，**"未知"是一个能表达的状态**。
 *
 * ## 配色
 *
 * 四种颜色**全部取自仓库既有的状态色板**（`globals.css` 的 `--status-*`，
 * 也就是好友列表里那四档的同一套色），没有自己调色：
 *
 * | 情况 | 色 | 变量 | 沿用哪个语义 |
 * | --- | --- | --- | --- |
 * | 正常 | 绿 | `--status-online` | Online |
 * | 网络层够不着 / 还没探到 | 灰 | `--status-offline` | Offline |
 * | 服务端报了异常（minor / maintenance） | 橙 | `--status-askme` | Ask Me |
 * | statuspage 定级 major / critical | 红 | `--status-busy` | Busy |
 *
 * 灰不染色会被读成"健康"，所以必须显式给 `--status-offline`，不能靠
 * `StatusDot` 那个中性的 `bg-muted-foreground/40`（那是"未启用"，不是一种状态）。
 */

export type VrcStatusIndicator = {
    /** 绿点。 */
    active: boolean;
    /** 需要在 `dotClassName` 里覆盖的颜色类；用默认色时为 `undefined`。 */
    dotClassName: string | undefined;
    /** 是否触发闪烁。只有定级明确的重大故障才闪。 */
    alert: 'warn' | 'danger' | null;
    /** 首行结论，已经翻译过。 */
    headline: string;
    /** 灰色次要行，已经翻译过，按显示顺序排列。 */
    details: string[];
};

/** 灰：Offline。用在"够不着"和"还没探到" —— 都是"不知道"，不是一种健康状态。 */
const OFFLINE_DOT = 'bg-[var(--status-offline)]';
/** 橙：Ask Me。服务端报了异常（minor / maintenance）。 */
const WARN_DOT = 'bg-[var(--status-askme)]';
/** 红：Busy。statuspage 定级 major / critical。 */
const DANGER_DOT = 'bg-[var(--status-busy)]';

export function resolveVrcStatusIndicator(
    health: ServerHealth,
    source: { refreshing: boolean },
    t: (key: string) => string,
    formatFetchedAt: (value: string) => string
): VrcStatusIndicator {
    // 加载中那一行和原来的行为一致：不管什么状态，正在问就告诉玩家。
    const refreshingRow = source.refreshing ? [t('common.loading')] : [];

    switch (health.state) {
        // 取到过，服务说没事。
        case 'healthy':
            return {
                active: true,
                dotClassName: undefined,
                alert: null,
                headline: t('status_bar.servers_ok'),
                details: [
                    ...refreshingRow,
                    ...(health.checkedAt
                        ? [formatFetchedAt(health.checkedAt)]
                        : [])
                ]
            };

        // 还从没取到过。灰（Offline）点：既不是健康，也不该染成故障 ——
        // 启动瞬间红一下比什么都不说更糟。
        case 'unknown':
            return {
                active: false,
                dotClassName: OFFLINE_DOT,
                alert: null,
                headline: t('status_bar.servers_unknown'),
                details: refreshingRow
            };

        // 取不到，而且手上没有任何"有事"的证据。**这一档以前是绿的。**
        // 灰（Offline）、不闪：这是玩家那一侧的网络问题，不是服务在报警。
        case 'unreachable':
            return {
                active: false,
                dotClassName: OFFLINE_DOT,
                alert: null,
                headline: t('status_bar.servers_unreachable'),
                details: [health.reason, ...refreshingRow]
            };

        // 服务说有事，按严重度分档。陈旧结论额外说明它不是此刻的。
        case 'issue': {
            const major =
                health.severity === 'major' || health.severity === 'critical';
            return {
                active: false,
                dotClassName: major ? DANGER_DOT : WARN_DOT,
                alert: major ? 'danger' : 'warn',
                headline: health.detail || t('status_bar.servers_issue'),
                details: [
                    ...refreshingRow,
                    ...(health.stale ? [t('status_bar.servers_stale')] : []),
                    ...(health.checkedAt
                        ? [formatFetchedAt(health.checkedAt)]
                        : [])
                ]
            };
        }
    }
}
