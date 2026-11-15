// SPDX-License-Identifier: MPL-2.0
// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, renderHook, waitFor } from '@testing-library/react';
import type { ReactElement, ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { VrccError } from '@/lib/vrccError';

import { useVrccClubs } from './useVrccClubs';

const mocks = vi.hoisted(() => ({
    clubs: vi.fn(),
    myClubs: vi.fn()
}));

vi.mock('./vrccApi', async (importOriginal) => ({
    ...(await importOriginal<typeof import('./vrccApi')>()),
    vrccApi: { clubs: mocks.clubs, myClubs: mocks.myClubs }
}));

function wrapper({ children }: { children: ReactNode }): ReactElement {
    const client = new QueryClient({
        defaultOptions: { queries: { retry: false, gcTime: 0 } }
    });
    return (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
}

const PUBLIC_RESPONSE = {
    total: 16,
    next: 24,
    clubs: [{ code: 'KIPFEL.2104', name: '中文kipfel厅', members: 6911 }]
};

const MY_RESPONSE = {
    clubs: [{ code: 'KIPFEL.2104', name: '中文kipfel厅', likes: 42 }],
    mine: ['KIPFEL.2104']
};

function connectedWith(clubs: boolean) {
    return {
        connected: true,
        scope: clubs ? 'profile clubs' : 'profile',
        client: 'app_test',
        pollEverySecs: 60
    } as never;
}

describe('useVrccClubs 的双源切换', () => {
    afterEach(cleanup);

    beforeEach(() => {
        vi.clearAllMocks();
        mocks.clubs.mockResolvedValue(PUBLIC_RESPONSE);
        mocks.myClubs.mockResolvedValue(MY_RESPONSE);
    });

    it('没连接时走公开列表，并且有 total', async () => {
        const { result } = renderHook(
            () => useVrccClubs({ connection: null, limit: 24, offset: 0 }),
            { wrapper }
        );

        await waitFor(() => expect(result.current.view).not.toBeNull());
        expect(result.current.source).toBe('public');
        expect(result.current.view?.total).toBe(16);
        expect(mocks.myClubs).not.toHaveBeenCalled();
    });

    it('连上但令牌没有 clubs 权限时，仍然走公开列表', async () => {
        const { result } = renderHook(
            () =>
                useVrccClubs({
                    connection: connectedWith(false),
                    limit: 24,
                    offset: 0
                }),
            { wrapper }
        );

        await waitFor(() => expect(result.current.view).not.toBeNull());
        expect(result.current.source).toBe('public');
        expect(mocks.myClubs).not.toHaveBeenCalled();
    });

    it('连上且有 clubs 时走玩家视角，total 保持 null', async () => {
        const { result } = renderHook(
            () =>
                useVrccClubs({
                    connection: connectedWith(true),
                    limit: 24,
                    offset: 0
                }),
            { wrapper }
        );

        await waitFor(() => expect(result.current.view).not.toBeNull());
        expect(result.current.source).toBe('mine');
        // 玩家视角没有 total——不能拿条数顶上。
        expect(result.current.view?.total).toBeNull();
        expect(result.current.view?.rows[0]?.likes).toBe(42);
    });

    it('玩家视角报缺权限时，一次性降级到公开列表', async () => {
        mocks.myClubs.mockRejectedValue(new VrccError('scope', 'missingScope'));

        const { result } = renderHook(
            () =>
                useVrccClubs({
                    connection: connectedWith(true),
                    limit: 24,
                    offset: 0
                }),
            { wrapper }
        );

        await waitFor(() => expect(result.current.source).toBe('public'), {
            timeout: 4000
        });
        await waitFor(() => expect(result.current.view?.total).toBe(16));
        // 降级之后不该再把错误摆在界面上——玩家要看的是列表。
        expect(result.current.error).toBeNull();
    });

    it('限流不算「用不了」：不降级，错误照实抛给界面', async () => {
        mocks.myClubs.mockRejectedValue(new VrccError('too soon', 'throttled'));

        const { result } = renderHook(
            () =>
                useVrccClubs({
                    connection: connectedWith(true),
                    limit: 24,
                    offset: 0
                }),
            { wrapper }
        );

        await waitFor(() => expect(result.current.error).not.toBeNull(), {
            timeout: 4000
        });
        // 还是玩家视角：降级会让列表来源被静默换掉，玩家看到的和刚才不一样。
        expect(result.current.source).toBe('mine');
        expect(mocks.clubs).not.toHaveBeenCalled();
    });
});
