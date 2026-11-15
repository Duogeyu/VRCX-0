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

vi.mock('@/services/dialogService', () => ({ openGroupDialog: vi.fn() }));
vi.mock('@/services/shellIntegrationService', () => ({
    openExternalLink: vi.fn()
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

/** 服务器顺序（也就是热度名次）：甲 → 乙 → 丙。 */
const CLUBS = {
    total: 3,
    clubs: [
        { code: 'A', name: '甲', grade: 2, members: 900 },
        { code: 'B', name: '乙', grade: 4, members: 100 },
        { code: 'C', name: '丙', grade: null, members: null }
    ]
};

/** 卡片是带 `aria-label` 的按钮，按出现顺序读出来。 */
function cardOrder(): string[] {
    return screen
        .getAllByRole('button')
        .map((element) => element.getAttribute('aria-label') ?? '')
        .filter((label) => ['甲', '乙', '丙'].includes(label));
}

describe('VrccClubsPanel 排序', () => {
    afterEach(cleanup);

    beforeEach(() => {
        vi.clearAllMocks();
        api.clubs.mockResolvedValue(CLUBS);
        api.myClubs.mockResolvedValue({ clubs: [], mine: [] });
    });

    it('默认按服务器顺序', async () => {
        render(<VrccClubsPanel connection={null} />, { wrapper });

        await waitFor(() => expect(cardOrder()).toEqual(['甲', '乙', '丙']));

        // 名次就是服务器顺序：甲 1、乙 2、丙 3。
        const jia = screen.getByRole('button', { name: '甲' });
        expect(jia.textContent).toContain('1');
    });

    it('按星级排序：高在前，未评级排最后，名次不变', async () => {
        render(<VrccClubsPanel connection={null} />, { wrapper });
        await waitFor(() => expect(cardOrder()).toHaveLength(3));

        fireEvent.click(screen.getByText('vrcc.clubs.sort.grade'));

        await waitFor(() =>
            // 乙(4) → 甲(2) → 丙(null)
            expect(cardOrder()).toEqual(['乙', '甲', '丙'])
        );
        // 名次仍是热度名次：甲第 1、乙第 2。排序只换显示位置。
        expect(
            screen.getByRole('button', { name: '甲' }).textContent
        ).toContain('1');
        expect(
            screen.getByRole('button', { name: '乙' }).textContent
        ).toContain('2');
    });

    it('按人数排序：多在前，缺值排最后', async () => {
        render(<VrccClubsPanel connection={null} />, { wrapper });
        await waitFor(() => expect(cardOrder()).toHaveLength(3));

        fireEvent.click(screen.getByText('vrcc.clubs.sort.members'));

        await waitFor(() =>
            // 甲(900) → 乙(100) → 丙(null)
            expect(cardOrder()).toEqual(['甲', '乙', '丙'])
        );
    });

    it('按名称排序：走语言排序，不是码位排序', async () => {
        render(<VrccClubsPanel connection={null} />, { wrapper });
        await waitFor(() => expect(cardOrder()).toHaveLength(3));

        fireEvent.click(screen.getByText('vrcc.clubs.sort.name'));

        await waitFor(() => expect(cardOrder()).toHaveLength(3));
        // 中文按拼音：丙(bing) → 甲(jia) → 乙(yi)。
        // 按码位排会得到另一套顺序，所以这条能区分「有没有用 Collator」。
        expect(cardOrder()).toEqual(['丙', '甲', '乙']);
    });
});
