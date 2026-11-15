// SPDX-License-Identifier: MPL-2.0

// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('react-i18next', async (importOriginal) => {
    const actual = await importOriginal<typeof import('react-i18next')>();
    return {
        ...actual,
        useTranslation: () => ({
            t: (key: string) => key,
            i18n: { language: 'zh-CN' }
        })
    };
});

const api = vi.hoisted(() => ({
    clubs: vi.fn(),
    myClubs: vi.fn()
}));

vi.mock('../vrccApi', async (importOriginal) => ({
    ...(await importOriginal<typeof import('../vrccApi')>()),
    vrccApi: { clubs: api.clubs, myClubs: api.myClubs }
}));

vi.mock('@/services/dialogService', () => ({ openGroupDialog: vi.fn() }));
vi.mock('@/services/shellIntegrationService', () => ({
    openExternalLink: vi.fn()
}));

import { VrccClubsPanel } from './VrccClubsPanel';

const CLUBS = {
    total: 2,
    clubs: [
        { code: 'A', name: '甲', grade: 2, members: 900 },
        { code: 'B', name: '乙', grade: 4, members: 100 }
    ]
};

describe('VrccClubsPanel 刷新失败', () => {
    afterEach(cleanup);

    beforeEach(() => {
        vi.clearAllMocks();
    });

    it('手上有列表时刷新失败，留着列表并提示，不整页报错', async () => {
        const client = new QueryClient({
            defaultOptions: { queries: { retry: false } }
        });
        api.clubs.mockResolvedValueOnce(CLUBS);
        render(
            <QueryClientProvider client={client}>
                <VrccClubsPanel connection={null} />
            </QueryClientProvider>
        );
        expect(await screen.findByRole('button', { name: '甲' })).toBeTruthy();

        // 过了 staleTime 再回来、刷新又失败（接口自带一次重试，所以两次都失败）。
        api.clubs.mockRejectedValue(new Error('network down'));
        await act(async () => {
            await client.refetchQueries();
        });
        await waitFor(
            () =>
                expect(screen.getByText('vrcc.clubs.stale_hint')).toBeTruthy(),
            { timeout: 5000 }
        );

        expect(screen.getByRole('button', { name: '甲' })).toBeTruthy();
        expect(screen.queryByText('vrcc.clubs.error')).toBeNull();
    }, 10000);

    it('什么都没拿到时才整页报错', async () => {
        const client = new QueryClient({
            defaultOptions: { queries: { retry: false } }
        });
        api.clubs.mockRejectedValue(new Error('network down'));
        render(
            <QueryClientProvider client={client}>
                <VrccClubsPanel connection={null} />
            </QueryClientProvider>
        );
        expect(
            await screen.findByText('vrcc.clubs.error', undefined, {
                timeout: 5000
            })
        ).toBeTruthy();
        expect(screen.queryByText('vrcc.clubs.stale_hint')).toBeNull();
    }, 10000);
});
