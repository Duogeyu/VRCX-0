// SPDX-License-Identifier: MPL-2.0

/**
 * 常驻探针的行为。
 *
 * 重点钉两条不变量：
 *
 * 1. **没有结论就不写状态** —— 一次无关的本机限流不该抹掉一条真实结论；
 * 2. 并发调用不发两次请求。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
    status: vi.fn()
}));

vi.mock('./vrccApi', () => ({
    vrccApi: { status: mocks.status }
}));

import { VrccError } from '@/lib/vrccError';
import { useVrccHealthStore } from '@/state/vrccHealthStore';

import { refreshVrccHealth } from './vrccHealthService';

function healthyStore() {
    useVrccHealthStore.setState({
        health: { state: 'ok', at: 1 },
        checkedAt: 1_000
    });
}

describe('vrccHealthService', () => {
    beforeEach(() => {
        mocks.status.mockReset();
        useVrccHealthStore.setState({
            health: { state: 'unknown' },
            checkedAt: null
        });
    });

    describe('拿到 200', () => {
        it('state=ok 写成 ok，并记下时间', async () => {
            mocks.status.mockResolvedValue({ at: 1794453729283, state: 'ok' });

            await refreshVrccHealth();

            expect(useVrccHealthStore.getState().health).toEqual({
                state: 'ok',
                at: 1794453729283
            });
            expect(useVrccHealthStore.getState().checkedAt).not.toBeNull();
        });

        it('state 不是 ok 时读成 failing', async () => {
            mocks.status.mockResolvedValue({ at: null, state: 'degraded' });

            await refreshVrccHealth();

            expect(useVrccHealthStore.getState().health).toEqual({
                state: 'failing',
                reason: 'degraded'
            });
        });
    });

    describe('请求失败', () => {
        it('网络层失败 → unreachable', async () => {
            mocks.status.mockRejectedValue(
                new VrccError(
                    'error sending request',
                    'transport',
                    null,
                    'transport',
                    'dns'
                )
            );

            await refreshVrccHealth();

            expect(useVrccHealthStore.getState().health).toEqual({
                state: 'unreachable',
                reason: 'error sending request',
                transportKind: 'dns'
            });
        });

        it('服务端 5xx → failing', async () => {
            mocks.status.mockRejectedValue(
                new VrccError(
                    '星遇同频接口返回 HTTP 500（http_500）',
                    'other',
                    null,
                    'server'
                )
            );

            await refreshVrccHealth();

            expect(useVrccHealthStore.getState().health).toMatchObject({
                state: 'failing'
            });
        });
    });

    // ★ 这条是本次改动的关键不变量。
    describe('没有结论时不覆盖已有结论', () => {
        it('本机限流不抹掉上一次的 ok', async () => {
            healthyStore();
            mocks.status.mockRejectedValue(
                new VrccError(
                    '星遇同频请求被本机限流拦下',
                    'throttled',
                    null,
                    'client'
                )
            );

            await refreshVrccHealth();

            expect(useVrccHealthStore.getState().health).toEqual({
                state: 'ok',
                at: 1
            });
            // 时间也不该被刷新：这次根本没拿到结论。
            expect(useVrccHealthStore.getState().checkedAt).toBe(1_000);
        });

        it('不是星遇同频的错误（IPC 挂了）也不覆盖', async () => {
            healthyStore();
            mocks.status.mockRejectedValue(new Error('command not found'));

            await refreshVrccHealth();

            expect(useVrccHealthStore.getState().health).toEqual({
                state: 'ok',
                at: 1
            });
        });
    });

    it('并发调用只发一次请求', async () => {
        mocks.status.mockImplementation(
            () =>
                new Promise((resolve) => {
                    setTimeout(() => resolve({ at: 1, state: 'ok' }), 10);
                })
        );

        await Promise.all([refreshVrccHealth(), refreshVrccHealth()]);

        expect(mocks.status).toHaveBeenCalledTimes(1);
    });
});
