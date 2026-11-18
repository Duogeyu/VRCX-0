// SPDX-License-Identifier: MPL-2.0

import { networkCopyKey } from '@/lib/vrccError';
import type { VrccHealth } from '@/lib/vrccHealth';

/**
 * 状态栏里「星遇状态」那一段长什么样。
 *
 * 和 `statusBarVrc.ts`、`statusBarProxy.ts` 同一个套路：把判定结论翻成"圆点参数 +
 * 提示内容"，组件只负责画。这样这段能脱离渲染被测试。
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
 * | 服务端回了 5xx | 橙 | `--status-askme` | Ask Me |
 *
 * 和 VRChat 那段保持一致（那边多一档红，给 statuspage 定级明确的 major/critical）。
 *
 * 不闪：vrcc 的故障会持续到站方修好，期间每 5 分钟才重探一次，一直闪等于噪音，
 * 反而让人学会无视它。VRChat 那段保留闪烁，是因为它的重大故障由 statuspage 定级，
 * 出现频率低得多。
 *
 * ## 提示的形状
 *
 * 和 VRChat 那段同构：首行结论，之后是灰色次要行（服务端原文 / 原始错误 / 时间）。
 */

export type VrccStatusIndicator = {
    /** 绿点。 */
    active: boolean;
    /** 需要在 `dotClassName` 里覆盖的颜色类；用默认色时为 `undefined`。 */
    dotClassName: string | undefined;
    /** 首行结论，已经翻译过。 */
    headline: string;
    /** 灰色次要行，已经翻译过，按显示顺序排列。 */
    details: string[];
};

/** 灰：Offline。用在"够不着"和"还没探到" —— 都是"不知道"，不是一种健康状态。 */
const OFFLINE_DOT = 'bg-[var(--status-offline)]';
/** 橙：Ask Me。服务端回了 5xx。和 VRChat 那段"服务异常"同一个色。 */
const WARN_DOT = 'bg-[var(--status-askme)]';

/**
 * @param checkedAt 最近一次拿到结论的时刻（毫秒）。`null` 表示还没有过结论。
 * @param formatCheckedAt 把毫秒数写成人话。沿用状态栏里其他地方的时间格式。
 */
export function resolveVrccStatusIndicator(
    health: VrccHealth,
    checkedAt: number | null,
    t: (key: string) => string,
    formatCheckedAt: (ms: number) => string
): VrccStatusIndicator {
    const timeRow = checkedAt === null ? [] : [formatCheckedAt(checkedAt)];

    switch (health.state) {
        case 'ok':
            return {
                active: true,
                dotClassName: undefined,
                headline: t('status_bar.vrcc_ok'),
                details: timeRow
            };

        // "还没探到"不能显示成健康，但也不该染色 —— 灰（Offline）点，
        // 和「出了事」区分开。
        case 'unknown':
            return {
                active: false,
                dotClassName: OFFLINE_DOT,
                headline: t('status_bar.vrcc_unknown'),
                details: []
            };

        // 网络层：玩家自己能修，所以首行按卡在哪一步（DNS / 建连 / 超时）说。
        // 映射和登录页、`describeVrccError` 共用 `networkCopyKey`。
        // 灰（Offline）点：够不着不等于服务坏了。
        case 'unreachable':
            return {
                active: false,
                dotClassName: OFFLINE_DOT,
                headline: t(networkCopyKey(health.transportKind)),
                details: [health.reason, ...timeRow]
            };

        // 服务端 5xx：站方那边的事。橙色，首行短，服务端原文（带 HTTP 状态码）在次要行。
        case 'failing':
            return {
                active: false,
                dotClassName: WARN_DOT,
                headline: t('status_bar.vrcc_issue'),
                details: [health.reason, ...timeRow]
            };
    }
}
