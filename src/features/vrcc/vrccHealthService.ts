// SPDX-License-Identifier: MPL-2.0

/**
 * 星遇同频（vrcc.cc）服务可达性的常驻探测。
 *
 * ## 它探的是什么
 *
 * `GET /api/v1/status`：官方的探活接口。`security: []` —— 不要求令牌，也不要求
 * `X-Client`（我们的客户端照旧会带，见 `vrcc-client` 的 `send_reserved`）；
 * 不计入任何个人额度；服务端自己缓存一分钟。
 *
 * 也就是说，**它和玩家登没登录、连没连星遇同频完全无关**。这是它能挂在
 * `GlobalHosts` 里、在登录页也有结论前提。
 *
 * ## 为什么不是每次调用都真发请求
 *
 * 客户端那一层已经管了：`status` 有 30 秒最小间隔，间隔内的同一个读直接复用
 * 上一份回答（不发请求、不占额度）；而且探活**不受全局退避限制**（见
 * `Call::ignore_backoff`）—— 别的接口吃 429 之后进程会整体退避，探活不该被
 * 自家闸门挡回去，那正是玩家最想知道"服务到底还活着吗"的时候。
 *
 * 所以这里只管两件事：**并发去重**，以及**别让"没有结论"覆盖掉已有的结论**。
 */

import { isVrccError, type VrccError } from '@/lib/vrccError';
import { resolveVrccHealth, type VrccProbe } from '@/lib/vrccHealth';
import { useVrccHealthStore } from '@/state/vrccHealthStore';

import { vrccApi } from './vrccApi';
import type { VrccStatus } from './vrccApi';

/** 正在路上的那一次。并发调用共用它，不重复发请求。 */
let inFlight: Promise<void> | null = null;

function probeFromStatus(status: VrccStatus): VrccProbe {
    if (status.state === 'ok') {
        return { outcome: 'ok', at: status.at ?? null };
    }
    // 契约里 `state` 的 enum 只有 `"ok"`。别的值说明服务端加了新语义，
    // 我们读不懂 —— 读不懂就不能说它正常。
    return { outcome: 'state-not-ok', raw: status.state ?? '' };
}

function probeFromError(error: VrccError): VrccProbe {
    return {
        outcome: 'failed',
        layer: error.layer,
        kind: error.kind,
        transportKind: error.transportKind,
        message: error.message
    };
}

/**
 * 探一次，把结论写进 store。
 *
 * 失败不抛：这是一条后台探活，界面上由 `LoginVrccStatusAlert` 之类的消费点
 * 自己决定要不要说话；把异常抛给调用方只会让每个调用点重复写 try/catch。
 */
export function refreshVrccHealth(): Promise<void> {
    if (inFlight) {
        return inFlight;
    }

    inFlight = (async () => {
        let probe: VrccProbe;
        try {
            probe = probeFromStatus(await vrccApi.status());
        } catch (error) {
            if (!isVrccError(error)) {
                // IPC 本身失败、命令名对不上之类：拿不到任何关于服务的信息，
                // 不能据此说 vrcc 挂了。留个日志，状态保持原样。
                console.warn('[vrcc] health probe failed:', error);
                return;
            }
            probe = probeFromError(error);
            // 我们自己标识坏了（`403 client_unknown` 之类）会被判成 unknown，
            // 界面上不显示是**对的**，但它需要被修，所以这里得响一声。
            if (error.kind === 'needsReauth') {
                console.warn(
                    '[vrcc] health probe rejected by the server; this looks like a client-id problem, not an outage:',
                    error.message
                );
            }
        } finally {
            inFlight = null;
        }

        const health = resolveVrccHealth(probe);
        // **没有结论就不写**。被本机限流挡下、或者遇到不该出现在这条路径上的
        // 错误时，`resolveVrccHealth` 给的是 `unknown`；用它覆盖掉上一次的
        // `ok` / `unreachable`，等于让一次无关的限流抹掉一条真实结论。
        if (health.state === 'unknown') {
            return;
        }
        useVrccHealthStore.getState().setHealth(health, Date.now());
    })();

    return inFlight;
}
