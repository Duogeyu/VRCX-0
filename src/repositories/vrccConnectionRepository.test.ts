// SPDX-License-Identifier: MPL-2.0

import { QueryObserver } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';

const appVrccConnection = vi.hoisted(() => vi.fn());

vi.mock('@/platform/tauri/client', () => ({
    tauriClient: { events: { subscribe: vi.fn() } }
}));

vi.mock('@/platform/tauri/bindings', () => ({
    commands: { appVrccConnection }
}));

import { queryClient } from '@/lib/queryClient';
import { VrccError } from '@/lib/vrccError';

import {
    callVrcc,
    vrccAccountKey,
    vrccConnectionQueryOptions
} from './vrccConnectionRepository';

describe('vrccAccountKey', () => {
    it('未连接（或读不到连接）时是 offline', () => {
        expect(vrccAccountKey(null)).toBe('offline');
        expect(vrccAccountKey(undefined)).toBe('offline');
        expect(vrccAccountKey({ connected: false, star: 77 })).toBe('offline');
    });

    it('已连接时按星号区分', () => {
        expect(vrccAccountKey({ connected: true, star: 77 })).toBe('star:77');
        expect(vrccAccountKey({ connected: true, star: 88 })).toBe('star:88');
    });

    it('已连接但没有星号时，不和「未连接」共用', () => {
        // 官方文档：没有点亮编号的玩家 `star` 是 null。
        expect(vrccAccountKey({ connected: true, star: null })).toBe(
            'star:none'
        );
    });
});

describe('callVrcc', () => {
    afterEach(() => {
        queryClient.clear();
        appVrccConnection.mockReset();
    });

    /** 挂一个连接查询的观察者（相当于世界详情面板开着），等它读完第一次。 */
    async function watchConnection() {
        appVrccConnection.mockResolvedValue({ connected: true, star: 77 });
        const observer = new QueryObserver(
            queryClient,
            vrccConnectionQueryOptions()
        );
        const unsubscribe = observer.subscribe(() => {});
        await vi.waitFor(() =>
            expect(observer.getCurrentResult().isSuccess).toBe(true)
        );
        appVrccConnection.mockClear();
        return { observer, unsubscribe };
    }

    it('令牌失效时让连接查询重读，已经挂着的面板会换到未连接', async () => {
        const { observer, unsubscribe } = await watchConnection();
        appVrccConnection.mockResolvedValue({ connected: false, star: null });

        // 命令直接抛出的原始错误（带后端分好的类别），没有经过归一化。
        const failure = Object.assign(new Error('404 gone'), {
            vrcc: { kind: 'needsReauth', layer: 'session' }
        });
        await expect(callVrcc(() => Promise.reject(failure))).rejects.toBe(
            failure
        );

        await vi.waitFor(() =>
            expect(observer.getCurrentResult().data?.connected).toBe(false)
        );
        expect(appVrccConnection).toHaveBeenCalledTimes(1);
        unsubscribe();
    });

    it('缺权限或别的失败不动连接：令牌还在', async () => {
        const { unsubscribe } = await watchConnection();

        await expect(
            callVrcc(() =>
                Promise.reject(new VrccError('403 scope', 'missingScope'))
            )
        ).rejects.toBeInstanceOf(VrccError);
        await expect(
            callVrcc(() => Promise.reject(new Error('network down')))
        ).rejects.toThrow('network down');

        await Promise.resolve();
        expect(appVrccConnection).not.toHaveBeenCalled();
        unsubscribe();
    });
});
