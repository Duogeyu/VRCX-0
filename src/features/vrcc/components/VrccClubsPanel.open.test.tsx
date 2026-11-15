// SPDX-License-Identifier: MPL-2.0

// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import {
    cleanup,
    fireEvent,
    render,
    screen,
    waitFor
} from '@testing-library/react';
import type { ReactElement, ReactNode } from 'react';
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

const dialog = vi.hoisted(() => ({
    openGroupDialog: vi.fn()
}));

const shell = vi.hoisted(() => ({
    openExternalLink: vi.fn()
}));

vi.mock('@/services/dialogService', () => ({
    openGroupDialog: dialog.openGroupDialog
}));

vi.mock('@/services/shellIntegrationService', () => ({
    openExternalLink: shell.openExternalLink
}));

import { VrccClubsPanel } from './VrccClubsPanel';

function wrapper({ children }: { children: ReactNode }): ReactElement {
    const client = new QueryClient({
        defaultOptions: { queries: { retry: false, gcTime: 0 } }
    });
    return (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
}

const GROUP_URL =
    'https://vrchat.com/home/group/grp_fdd4cdf6-b3e0-4be3-a040-5b8abf2617f4';

describe('VrccClubsPanel 点卡片', () => {
    afterEach(cleanup);

    beforeEach(() => {
        vi.clearAllMocks();
    });

    it('打开对应的群组详情，标题用社团名', async () => {
        api.clubs.mockResolvedValue({
            total: 1,
            clubs: [
                {
                    code: 'KIPFEL.2104',
                    name: '中文kipfel厅',
                    group: GROUP_URL
                }
            ]
        });

        render(<VrccClubsPanel connection={null} />, { wrapper });

        fireEvent.click(
            await screen.findByRole('button', { name: '中文kipfel厅' })
        );

        await waitFor(() =>
            expect(dialog.openGroupDialog).toHaveBeenCalledWith({
                groupId: 'grp_fdd4cdf6-b3e0-4be3-a040-5b8abf2617f4',
                title: '中文kipfel厅'
            })
        );
        expect(shell.openExternalLink).not.toHaveBeenCalled();
    });

    it('没有 group 字段时退回打开社团页，不留死按钮', async () => {
        api.clubs.mockResolvedValue({
            total: 1,
            clubs: [
                {
                    code: 'KIPFEL.2104',
                    name: '中文kipfel厅',
                    page: 'https://vrcc.cc/c/KIPFEL.2104'
                }
            ]
        });

        render(<VrccClubsPanel connection={null} />, { wrapper });

        fireEvent.click(
            await screen.findByRole('button', { name: '中文kipfel厅' })
        );

        await waitFor(() =>
            expect(shell.openExternalLink).toHaveBeenCalledWith(
                'https://vrcc.cc/c/KIPFEL.2104'
            )
        );
        expect(dialog.openGroupDialog).not.toHaveBeenCalled();
    });

    it('卡片里不再有「社团页」外链', async () => {
        api.clubs.mockResolvedValue({
            total: 1,
            clubs: [
                { code: 'KIPFEL.2104', name: '中文kipfel厅', group: GROUP_URL }
            ]
        });

        render(<VrccClubsPanel connection={null} />, { wrapper });

        await screen.findByRole('button', { name: '中文kipfel厅' });
        expect(screen.queryByText('vrcc.clubs.open')).toBeNull();
    });
});
