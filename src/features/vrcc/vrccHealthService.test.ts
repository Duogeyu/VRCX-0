// SPDX-License-Identifier: MPL-2.0

/**
 * 前端这一侧只做投影：把后端推来的快照写进 store，再请后端探一次。
 *
 * 采集逻辑（单飞、自适应间隔、本机限流不覆盖）都在 Rust 的
 * `VrccHealthService` 里，那边的测试是 Rust 测试。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
    healthGet: vi.fn(),
    healthRefresh: vi.fn()
}));

vi.mock('@/platform/tauri/bindings', () => ({
    commands: {
        appVrccHealthGet: mocks.healthGet,
        appVrccHealthRefresh: mocks.healthRefresh
    }
}));

import type { VrccHealthSnapshot } from '@/platform/tauri/bindings';
import { useVrccHealthStore } from '@/state/vrccHealthStore';

import { hydrateVrccHealth, refreshVrccHealth } from './vrccHealthService';

const OK_SNAPSHOT: VrccHealthSnapshot = {
    probe: { outcome: 'ok', at: null },
    checkedAt: '2026-11-20T00:00:00.000Z',
    refreshing: false,
    pollingIntervalMs: 300_000
};

describe('vrccHealthService', () => {
    beforeEach(() => {
        mocks.healthGet.mockReset();
        mocks.healthRefresh.mockReset();
        useVrccHealthStore.getState().reset();
    });

    describe('快照 → store', () => {
        it('ok 连同时间一起落地', () => {
            useVrccHealthStore.getState().applySnapshot(OK_SNAPSHOT);

            const state = useVrccHealthStore.getState();
            expect(state.health).toEqual({ state: 'ok', at: null });
            expect(state.checkedAt).toBe(
                Date.parse('2026-11-20T00:00:00.000Z')
            );
            expect(state.refreshing).toBe(false);
        });

        it('传输层失败连 transportKind 一起带出来', () => {
            useVrccHealthStore.getState().applySnapshot({
                ...OK_SNAPSHOT,
                probe: {
                    outcome: 'failed',
                    layer: 'transport',
                    transportKind: 'dns',
                    needsReauth: false,
                    message: 'error sending request'
                }
            });

            expect(useVrccHealthStore.getState().health).toEqual({
                state: 'unreachable',
                reason: 'error sending request',
                transportKind: 'dns'
            });
        });

        it('refreshing 跟着快照走', () => {
            useVrccHealthStore
                .getState()
                .applySnapshot({ ...OK_SNAPSHOT, refreshing: true });

            expect(useVrccHealthStore.getState().refreshing).toBe(true);
        });

        // 首次加载时后端快照还是 idle。那不是结论，不能拿它抹掉已有的结论。
        it('idle 只跟进「正在探」，不动已有结论', () => {
            useVrccHealthStore.getState().applySnapshot(OK_SNAPSHOT);

            useVrccHealthStore.getState().applySnapshot({
                probe: { outcome: 'idle' },
                checkedAt: null,
                refreshing: true,
                pollingIntervalMs: 300_000
            });

            const state = useVrccHealthStore.getState();
            expect(state.health).toEqual({ state: 'ok', at: null });
            expect(state.checkedAt).toBe(
                Date.parse('2026-11-20T00:00:00.000Z')
            );
            expect(state.refreshing).toBe(true);
        });
    });

    describe('hydrate', () => {
        it('读一次命令，写进 store', async () => {
            mocks.healthGet.mockResolvedValue(OK_SNAPSHOT);

            await hydrateVrccHealth();

            expect(mocks.healthGet).toHaveBeenCalledTimes(1);
            expect(useVrccHealthStore.getState().health).toEqual({
                state: 'ok',
                at: null
            });
        });

        it('命令失败只留日志，不抛也不清空已有结论', async () => {
            useVrccHealthStore.getState().applySnapshot(OK_SNAPSHOT);
            mocks.healthGet.mockRejectedValue(new Error('no ipc'));

            await expect(hydrateVrccHealth()).resolves.toBeUndefined();
            expect(useVrccHealthStore.getState().health).toEqual({
                state: 'ok',
                at: null
            });
        });
    });

    describe('refresh', () => {
        it('请后端探一次，并把回答写进 store', async () => {
            mocks.healthRefresh.mockResolvedValue({
                ...OK_SNAPSHOT,
                probe: { outcome: 'stateNotOk', raw: 'degraded' }
            });

            await refreshVrccHealth();

            expect(mocks.healthRefresh).toHaveBeenCalledTimes(1);
            expect(useVrccHealthStore.getState().health).toEqual({
                state: 'failing',
                reason: 'degraded'
            });
        });

        it('命令失败只留日志，不抛', async () => {
            mocks.healthRefresh.mockRejectedValue(new Error('no ipc'));

            await expect(refreshVrccHealth()).resolves.toBeUndefined();
        });
    });
});
