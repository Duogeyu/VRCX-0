// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { ServerHealth } from '@/lib/serverHealth';

import { resolveVrcStatusIndicator } from './statusBarVrc';

const t = (key: string) => key;
const formatFetchedAt = (value: string) => `at:${value}`;

function resolve(
    health: ServerHealth,
    refreshing = false
): ReturnType<typeof resolveVrcStatusIndicator> {
    return resolveVrcStatusIndicator(
        health,
        { refreshing },
        t,
        formatFetchedAt
    );
}

describe('statusBarVrc', () => {
    describe('正常', () => {
        it('绿点，没有覆盖色也没有警报', () => {
            expect(
                resolve({ state: 'healthy', checkedAt: '2026-11-20T00:00Z' })
            ).toMatchObject({
                active: true,
                dotClassName: undefined,
                alert: null,
                headline: 'status_bar.servers_ok'
            });
        });

        it('次要行是上次获取时间', () => {
            expect(
                resolve({ state: 'healthy', checkedAt: '2026-11-20T00:00Z' })
                    .details
            ).toEqual(['at:2026-11-20T00:00Z']);
        });
    });

    // ★ 这一档以前是绿的：刷新失败、indicator 保持原值或空串，判据读成"健康"。
    describe('网络层够不着', () => {
        it('灰（Offline）点，也不闪', () => {
            const indicator = resolve({
                state: 'unreachable',
                reason: 'error sending request',
                checkedAt: null
            });

            expect(indicator.active).toBe(false);
            expect(indicator.dotClassName).toBe('bg-[var(--status-offline)]');
            expect(indicator.alert).toBeNull();
        });

        it('首行不再声称"一切正常"，而是"无法确认"', () => {
            expect(
                resolve({
                    state: 'unreachable',
                    reason: 'error sending request',
                    checkedAt: null
                }).headline
            ).toBe('status_bar.servers_unreachable');
        });

        it('次要行先摆原始错误', () => {
            expect(
                resolve({
                    state: 'unreachable',
                    reason: 'error sending request',
                    checkedAt: null
                }).details[0]
            ).toBe('error sending request');
        });
    });

    describe('还没探到', () => {
        it('也是灰（Offline）点，但和"够不着"用不同文案', () => {
            const indicator = resolve({ state: 'unknown' });

            expect(indicator.active).toBe(false);
            expect(indicator.dotClassName).toBe('bg-[var(--status-offline)]');
            expect(indicator.headline).toBe('status_bar.servers_unknown');
        });
    });

    describe('服务端报异常', () => {
        // 橙色：站方确实出了一件事，但没到 statuspage 的 major/critical。
        it('minor 用橙点，并轻微闪烁', () => {
            const indicator = resolve({
                state: 'issue',
                severity: 'minor',
                detail: 'API',
                stale: false,
                checkedAt: '2026-11-20T00:00Z'
            });

            expect(indicator.active).toBe(false);
            expect(indicator.dotClassName).toMatch(/status-askme/);
            expect(indicator.alert).toBe('warn');
        });

        it('maintenance 也走橙色', () => {
            expect(
                resolve({
                    state: 'issue',
                    severity: 'maintenance',
                    detail: 'Scheduled Maintenance',
                    stale: false,
                    checkedAt: null
                }).dotClassName
            ).toMatch(/status-askme/);
        });

        // 红色保留给 statuspage 定级明确的重大故障，警报强度和原来一致。
        it.each(['major', 'critical'] as const)(
            '%s 用红点 + danger 闪烁',
            (severity) => {
                const indicator = resolve({
                    state: 'issue',
                    severity,
                    detail: 'Major Service Outage',
                    stale: false,
                    checkedAt: null
                });

                expect(indicator.dotClassName).toMatch(/status-busy/);
                expect(indicator.alert).toBe('danger');
            }
        );

        it('首行用服务器给的具体细节，没有就退回通用一句', () => {
            expect(
                resolve({
                    state: 'issue',
                    severity: 'minor',
                    detail: 'API, Website',
                    stale: false,
                    checkedAt: null
                }).headline
            ).toBe('API, Website');

            expect(
                resolve({
                    state: 'issue',
                    severity: 'minor',
                    detail: '',
                    stale: false,
                    checkedAt: null
                }).headline
            ).toBe('status_bar.servers_issue');
        });

        // 结论来自更早的一次成功检查：照旧显示，但要说明它不是此刻的。
        it('陈旧结论多一行说明', () => {
            expect(
                resolve({
                    state: 'issue',
                    severity: 'major',
                    detail: 'Major Service Outage',
                    stale: true,
                    checkedAt: '2026-11-20T00:00Z'
                }).details
            ).toEqual(['status_bar.servers_stale', 'at:2026-11-20T00:00Z']);
        });

        it('不陈旧时没有那一行', () => {
            expect(
                resolve({
                    state: 'issue',
                    severity: 'major',
                    detail: 'Major Service Outage',
                    stale: false,
                    checkedAt: '2026-11-20T00:00Z'
                }).details
            ).toEqual(['at:2026-11-20T00:00Z']);
        });
    });

    describe('加载中的那一行', () => {
        it('不管什么状态，正在问就告诉玩家', () => {
            expect(
                resolve({ state: 'healthy', checkedAt: null }, true).details[0]
            ).toBe('common.loading');
            expect(resolve({ state: 'unknown' }, true).details[0]).toBe(
                'common.loading'
            );
        });
    });
});
