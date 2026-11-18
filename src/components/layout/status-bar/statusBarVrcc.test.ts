// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { VrccHealth } from '@/lib/vrccHealth';

import { resolveVrccStatusIndicator } from './statusBarVrcc';

/** 测试里 `t` 直接返回 key，断言的是「走了哪个 key」。 */
const t = (key: string) => key;

/** 时间在这个假格式化里写成固定串，断言时不用关心区域格式。 */
const formatCheckedAt = (ms: number) => `at:${ms}`;

const CHECKED_AT = 1_794_453_729_283;

function resolve(health: VrccHealth, checkedAt: number | null = CHECKED_AT) {
    return resolveVrccStatusIndicator(health, checkedAt, t, formatCheckedAt);
}

describe('statusBarVrcc', () => {
    it('正常时是绿点', () => {
        expect(resolve({ state: 'ok', at: 1 })).toMatchObject({
            active: true,
            dotClassName: undefined,
            headline: 'status_bar.vrcc_ok'
        });
    });

    // 不能显示成健康（那是这次改动要修的），但也不该染色：那是"还没问到"，不是故障。
    it('还没探到时是灰（Offline）点，且不冒充健康', () => {
        expect(resolve({ state: 'unknown' })).toMatchObject({
            active: false,
            dotClassName: 'bg-[var(--status-offline)]',
            headline: 'status_bar.vrcc_unknown'
        });
    });

    // 和旁边 VRChat 那段同构：首行结论，之后是灰色次要行。
    describe('提示是多行结构', () => {
        it('拿到结论时带上"上次拿到结论的时间"', () => {
            expect(resolve({ state: 'ok', at: 1 }).details).toEqual([
                `at:${CHECKED_AT}`
            ]);
        });

        it('还没有过结论时不显示时间行', () => {
            expect(resolve({ state: 'unknown' }, null).details).toEqual([]);
        });
    });

    describe('网络层失败', () => {
        // 够不着不等于服务坏了：灰（Offline），不染色。
        it('灰（Offline）点，不染色', () => {
            const indicator = resolve({
                state: 'unreachable',
                reason: 'boom',
                transportKind: 'connect'
            });

            expect(indicator.active).toBe(false);
            expect(indicator.dotClassName).toBe('bg-[var(--status-offline)]');
        });

        // 和登录页、describeVrccError 共用同一份映射：同一个 DNS 失败不该有多个名字。
        it.each([
            ['dns', 'vrcc.error.network.dns'],
            ['connect', 'vrcc.error.network.connect'],
            ['timeout', 'vrcc.error.network.timeout'],
            ['body', 'vrcc.error.network.generic'],
            ['other', 'vrcc.error.network.generic'],
            [null, 'vrcc.error.network.generic']
        ] as const)('transportKind=%s 时首行用 %s', (transportKind, key) => {
            expect(
                resolve({
                    state: 'unreachable',
                    reason: 'boom',
                    transportKind
                }).headline
            ).toBe(key);
        });

        it('原始错误摆在次要行，再跟时间', () => {
            expect(
                resolve({
                    state: 'unreachable',
                    reason: 'error sending request',
                    transportKind: 'dns'
                }).details
            ).toEqual(['error sending request', `at:${CHECKED_AT}`]);
        });
    });

    describe('服务端 5xx', () => {
        it('橙点：这是站方那边的事，和"网络不通"分开', () => {
            const indicator = resolve({
                state: 'failing',
                reason: '星遇同频接口返回 HTTP 500（http_500）'
            });

            expect(indicator.active).toBe(false);
            expect(indicator.dotClassName).toMatch(/status-askme/);
        });

        it('首行一句话，服务端原文（带状态码）在次要行', () => {
            const indicator = resolve({
                state: 'failing',
                reason: '星遇同频接口返回 HTTP 500（http_500）'
            });

            expect(indicator.headline).toBe('status_bar.vrcc_issue');
            expect(indicator.details).toEqual([
                '星遇同频接口返回 HTTP 500（http_500）',
                `at:${CHECKED_AT}`
            ]);
        });
    });
});
