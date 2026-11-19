// SPDX-License-Identifier: MPL-2.0

/**
 * `resolveVrccHealth` 的判定表。
 *
 * 重点钉的是"谁的锅"：网络失败和服务端 5xx 处置方向相反，被揉成一团是这次要修的东西。
 */

import { describe, expect, it } from 'vitest';

import type { VrccTransportKind } from './vrccError';
import {
    resolveVrccHealth,
    vrccHealthNeedsAttention,
    type VrccProbe
} from './vrccHealth';

/** 网络层失败（Rust 的 `transport_error()` 产出的形状）。 */
function networkFailure(transportKind: VrccTransportKind = 'dns'): VrccProbe {
    return {
        outcome: 'failed',
        layer: 'transport',
        kind: 'transport',
        transportKind,
        message: 'error sending request for url (https://vrcc.cc/api/v1/status)'
    };
}

/** 服务端拒绝：拿到了响应，但状态码不是 2xx。 */
function serverFailure(
    message = '星遇同频接口返回 HTTP 500（http_500）'
): VrccProbe {
    return {
        outcome: 'failed',
        layer: 'server',
        kind: 'other',
        transportKind: null,
        message
    };
}

describe('resolveVrccHealth', () => {
    describe('还没探过', () => {
        it('null 是 unknown', () => {
            expect(resolveVrccHealth(null)).toEqual({ state: 'unknown' });
        });

        it('unknown 不打扰玩家', () => {
            expect(vrccHealthNeedsAttention(resolveVrccHealth(null))).toBe(
                false
            );
        });
    });

    describe('服务器说正常', () => {
        it('200 + state=ok 是 ok，并带上服务端时间', () => {
            expect(
                resolveVrccHealth({ outcome: 'ok', at: 1794453729283 })
            ).toEqual({ state: 'ok', at: 1794453729283 });
        });

        it('ok 不打扰玩家', () => {
            expect(
                vrccHealthNeedsAttention(
                    resolveVrccHealth({ outcome: 'ok', at: null })
                )
            ).toBe(false);
        });
    });

    // 这一组是这次改动的核心：两个方向的处置完全不同。
    describe('区分"谁的锅"', () => {
        it('网络层失败读成 unreachable，并保留卡在哪一步', () => {
            expect(resolveVrccHealth(networkFailure('timeout'))).toEqual({
                state: 'unreachable',
                reason: 'error sending request for url (https://vrcc.cc/api/v1/status)',
                transportKind: 'timeout'
            });
        });

        it('服务端 5xx 读成 failing，不是 unreachable', () => {
            const health = resolveVrccHealth(serverFailure());

            expect(health.state).toBe('failing');
        });

        it('两者都要让玩家看见，但说法不同', () => {
            expect(
                vrccHealthNeedsAttention(resolveVrccHealth(networkFailure()))
            ).toBe(true);
            expect(
                vrccHealthNeedsAttention(resolveVrccHealth(serverFailure()))
            ).toBe(true);
        });

        it('DNS / 建连 / 超时 / 响应中断都归 unreachable', () => {
            for (const kind of [
                'dns',
                'connect',
                'timeout',
                'body',
                'other'
            ] as const) {
                const health = resolveVrccHealth(networkFailure(kind));

                expect(health.state).toBe('unreachable');
                expect(
                    health.state === 'unreachable' && health.transportKind
                ).toBe(kind);
            }
        });

        it('服务端的原文原样带出来：里面本来就有 HTTP 状态码', () => {
            const health = resolveVrccHealth(
                serverFailure('星遇同频接口返回 HTTP 502（http_502）')
            );

            expect(health.state === 'failing' && health.reason).toContain(
                '502'
            );
        });
    });

    describe('本机限流不能升级成故障', () => {
        // `ErrorLayer::Client` 只在 `acquire()` 里产生 —— 请求根本没发出去。
        // 服务可能是完全好的，这时报"vrcc 挂了"是在冤枉它。
        it('读成 unknown 而不是 unreachable', () => {
            const health = resolveVrccHealth({
                outcome: 'failed',
                layer: 'client',
                kind: 'throttled',
                transportKind: null,
                message: '星遇同频请求被本机限流拦下，30 秒后再问'
            });

            expect(health).toEqual({ state: 'unknown' });
        });

        it('限流时不打扰玩家', () => {
            expect(
                vrccHealthNeedsAttention(
                    resolveVrccHealth({
                        outcome: 'failed',
                        layer: 'client',
                        kind: 'throttled',
                        transportKind: null,
                        message: 'throttled'
                    })
                )
            ).toBe(false);
        });
    });

    describe('200 但 state 不是 ok', () => {
        // 契约里 `state` 的 enum 只有 `"ok"`。出现别的值是服务端加了新语义，
        // 读不懂就不能说它正常。
        it('读成 failing，并带出原始取值', () => {
            expect(
                resolveVrccHealth({ outcome: 'state-not-ok', raw: 'degraded' })
            ).toEqual({ state: 'failing', reason: 'degraded' });
        });

        it('state 为空也算，不能静默当成正常', () => {
            expect(
                resolveVrccHealth({ outcome: 'state-not-ok', raw: '' }).state
            ).toBe('failing');
        });
    });

    describe('不该出现、但出现时按"不知道"处理', () => {
        // `/status` 不需要令牌，参数也是写死的。`input` / `session` 走到这里
        // 说明我们有 bug。拿一个本机错误去说"vrcc 挂了"是冤枉站方。
        it.each(['input', 'session'] as const)(
            'layer=%s 落到 unknown',
            (layer) => {
                const health = resolveVrccHealth({
                    outcome: 'failed',
                    layer,
                    kind: 'other',
                    transportKind: null,
                    message: 'boom'
                });

                expect(health).toEqual({ state: 'unknown' });
            }
        );

        // `/status` 是 `authed: false`，令牌失效那样的问题走不到这条路径。
        // 这里的 `needsReauth` 只可能是 `403 client_unknown` 之类 —— 我们的标识坏了。
        it('服务端 403 但属于 client 标识问题，不说成站方挂了', () => {
            const health = resolveVrccHealth({
                outcome: 'failed',
                layer: 'server',
                kind: 'needsReauth',
                transportKind: null,
                message: '星遇同频接口返回 HTTP 403（client_unknown）'
            });

            expect(health).toEqual({ state: 'unknown' });
        });

        it('同样 403，但不是标识问题（比如别的码），照旧算站方拒绝', () => {
            const health = resolveVrccHealth({
                outcome: 'failed',
                layer: 'server',
                kind: 'other',
                transportKind: null,
                message: '星遇同频接口返回 HTTP 403（forbidden）'
            });

            expect(health.state).toBe('failing');
        });
    });
});
