// SPDX-License-Identifier: MPL-2.0

/**
 * `resolveServerHealth` 的判定表。
 *
 * 这些用例是从一次真实验证里反推出来的：把 `vrc_status.rs` 的快照生成逻辑复刻成
 * 脚本，枚举 statuspage.io 各种取值，再看三处前端判据会给出什么结论。
 * 当时的结论是"三处判据等价、但共享同一个盲区"—— 盲区那几条在这里逐条钉住。
 */

import { describe, expect, it } from 'vitest';

import {
    resolveServerHealth,
    serverHealthNeedsAttention,
    type ServerHealthInput
} from './serverHealth';

/** 后端刚启动、一次都没取过的快照（`runtimeStore.ts` 的初始值）。 */
const NEVER_FETCHED: ServerHealthInput = {
    indicator: '',
    status: '',
    summary: '',
    error: '',
    lastFetchedAt: null
};

/** 后端取到了、服务说一切正常。 */
const HEALTHY: ServerHealthInput = {
    indicator: '',
    status: '',
    summary: '',
    error: '',
    lastFetchedAt: '2026-11-20T00:00:00.000Z'
};

/** 后端取到了、服务说 major，而且当前这次也成功了。 */
const MAJOR: ServerHealthInput = {
    indicator: 'major',
    status: 'Major Service Outage',
    summary: 'API',
    error: '',
    lastFetchedAt: '2026-11-20T00:00:00.000Z'
};

function withPatch(
    base: ServerHealthInput,
    patch: Partial<ServerHealthInput>
): ServerHealthInput {
    return { ...base, ...patch };
}

describe('resolveServerHealth', () => {
    describe('四个状态各自成立', () => {
        it('从没取过是 unknown，不是 healthy', () => {
            expect(resolveServerHealth(NEVER_FETCHED)).toEqual({
                state: 'unknown'
            });
        });

        it('取到过且服务说没事才是 healthy', () => {
            expect(resolveServerHealth(HEALTHY)).toEqual({
                state: 'healthy',
                checkedAt: '2026-11-20T00:00:00.000Z'
            });
        });

        it('服务说有事就是 issue，带严重度和细节', () => {
            expect(resolveServerHealth(MAJOR)).toEqual({
                state: 'issue',
                severity: 'major',
                detail: 'API',
                stale: false,
                checkedAt: '2026-11-20T00:00:00.000Z'
            });
        });

        it('取不到、又没有任何"有事"证据，是 unreachable', () => {
            expect(
                resolveServerHealth(
                    withPatch(HEALTHY, { error: 'error sending request' })
                )
            ).toEqual({
                state: 'unreachable',
                reason: 'error sending request',
                checkedAt: '2026-11-20T00:00:00.000Z'
            });
        });
    });

    // 这一组是整个模块存在的理由。
    describe('盲区：失败不能被读成健康', () => {
        it('首次启动就失败（lastFetchedAt 被写成非空）仍不是 healthy', () => {
            const health = resolveServerHealth(
                withPatch(NEVER_FETCHED, {
                    error: 'error sending request',
                    // Rust 的失败分支也会写 last_fetched_at（vrc_status.rs:108），
                    // 所以这一份和"曾经健康过"在时间戳上分不开。
                    lastFetchedAt: '2026-11-20T00:00:00.000Z'
                })
            );

            expect(health.state).toBe('unreachable');
            expect(serverHealthNeedsAttention(health)).toBe(true);
        });

        it('曾经健康、之后一直失败，读成 unreachable 而不是 healthy', () => {
            expect(
                resolveServerHealth(
                    withPatch(HEALTHY, { error: 'error sending request' })
                ).state
            ).toBe('unreachable');
        });

        it('unknown 不打扰玩家，unreachable 打扰', () => {
            expect(
                serverHealthNeedsAttention(resolveServerHealth(NEVER_FETCHED))
            ).toBe(false);
            expect(
                serverHealthNeedsAttention(
                    resolveServerHealth(
                        withPatch(NEVER_FETCHED, { error: 'boom' })
                    )
                )
            ).toBe(true);
        });
    });

    describe('陈旧结论：有结论但刷新失败', () => {
        it('结论保留，同时标成 stale', () => {
            expect(
                resolveServerHealth(
                    withPatch(MAJOR, { error: 'error sending request' })
                )
            ).toEqual({
                state: 'issue',
                severity: 'major',
                detail: 'API',
                stale: true,
                checkedAt: '2026-11-20T00:00:00.000Z'
            });
        });

        it('陈旧结论仍然要玩家注意', () => {
            expect(
                serverHealthNeedsAttention(
                    resolveServerHealth(withPatch(MAJOR, { error: 'boom' }))
                )
            ).toBe(true);
        });
    });

    // 后端不变量缺口（vrc_status.rs:162 判、:174 写）：
    // 结论来自 description 时，indicator 会是 none 而 status 非空。
    describe('后端不变量缺口：indicator 说没事但 status 有事', () => {
        it('只信 indicator 会漏掉，这里不能漏', () => {
            const health = resolveServerHealth({
                indicator: 'none',
                status: 'Partial System Outage',
                summary: '',
                error: '',
                lastFetchedAt: '2026-11-20T00:00:00.000Z'
            });

            expect(health.state).toBe('issue');
            expect(serverHealthNeedsAttention(health)).toBe(true);
        });

        it('indicator 是空串、只有 status 非空，同样算 issue', () => {
            expect(
                resolveServerHealth({
                    indicator: '',
                    status: 'Scheduled Maintenance',
                    summary: '',
                    error: '',
                    lastFetchedAt: '2026-11-20T00:00:00.000Z'
                }).state
            ).toBe('issue');
        });

        it('这种结论来自 description、没有可用 indicator，严重度落到最轻一档', () => {
            const health = resolveServerHealth({
                indicator: 'none',
                status: 'Partial System Outage',
                summary: '',
                error: '',
                lastFetchedAt: '2026-11-20T00:00:00.000Z'
            });

            expect(health.state === 'issue' && health.severity).toBe('minor');
        });
    });

    describe('严重度只认 indicator 的枚举值', () => {
        it.each([
            ['minor', 'minor'],
            ['major', 'major'],
            ['critical', 'critical'],
            ['maintenance', 'maintenance']
        ] as const)('%s 映射到 %s', (indicator, expected) => {
            const health = resolveServerHealth(withPatch(MAJOR, { indicator }));

            expect(health.state === 'issue' && health.severity).toBe(expected);
        });

        it('认不出的取值落到最轻一档，不冒充重大故障', () => {
            const health = resolveServerHealth(
                withPatch(MAJOR, { indicator: 'weird' })
            );

            expect(health.state === 'issue' && health.severity).toBe('minor');
        });

        it('大小写变体不当作 none', () => {
            // 后端写的是小写常量。`None` 这种取值只会来自别处，
            // 那时应当进入 issue 分支由 fallback 兜住，而不是静默当成没问题。
            expect(
                resolveServerHealth(withPatch(HEALTHY, { indicator: 'None' }))
                    .state
            ).toBe('issue');
        });
    });

    describe('细节优先取组件名', () => {
        it('summary 非空时用 summary', () => {
            const health = resolveServerHealth(
                withPatch(MAJOR, { summary: 'API, Website' })
            );

            expect(health.state === 'issue' && health.detail).toBe(
                'API, Website'
            );
        });

        it('summary 为空时退回 status', () => {
            const health = resolveServerHealth(
                withPatch(MAJOR, { summary: '' })
            );

            expect(health.state === 'issue' && health.detail).toBe(
                'Major Service Outage'
            );
        });
    });

    describe('空白字符不会改变结论', () => {
        it('全是空格的字段等于空串', () => {
            expect(
                resolveServerHealth({
                    indicator: '   ',
                    status: '  ',
                    summary: ' ',
                    error: '   ',
                    lastFetchedAt: null
                })
            ).toEqual({ state: 'unknown' });
        });
    });
});
