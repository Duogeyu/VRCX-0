// SPDX-License-Identifier: MPL-2.0
// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import type { ReactElement } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
    clubCodeForGroup,
    isClubCodeShaped
} from '@/repositories/vrccClubRepository';

import { GroupVrccClub } from './GroupVrccClub';

const mocks = vi.hoisted(() => ({
    appVrccClub: vi.fn(),
    appVrccConnection: vi.fn(),
    appVrccMyClub: vi.fn()
}));

vi.mock('@/platform/tauri/bindings', () => ({
    commands: {
        appVrccClub: mocks.appVrccClub,
        appVrccConnection: mocks.appVrccConnection,
        appVrccMyClub: mocks.appVrccMyClub
    }
}));

vi.mock('@/services/shellIntegrationService', () => ({
    openExternalLink: vi.fn()
}));

// `dateTime` 会顺着 i18nService 把真正的 i18next 拉起来；这条用例只关心时间有
// 没有落到行里，格式是它自己的单测。固定成可断言的形状。
vi.mock('@/lib/dateTime', () => ({
    formatCompactDateTime: (value: unknown) => `t:${String(value)}`
}));

vi.mock('react-i18next', () => ({
    useTranslation: () => ({
        t: (key: string, options?: { count?: number }) =>
            options?.count === undefined ? key : `${key}:${options.count}`
    })
}));

function renderWithQuery(ui: ReactElement) {
    const client = new QueryClient({
        defaultOptions: { queries: { retry: false, gcTime: 0 } }
    });
    return render(
        <QueryClientProvider client={client}>{ui}</QueryClientProvider>
    );
}

/** 一份和实测 `GET /clubs/KIPFEL.2104` 一样形状的回答。 */
const clubResponse = {
    club: {
        code: 'KIPFEL.2104',
        name: '中文kipfel厅',
        lede: '喜欢/使用猫猫模？想要一个风气干净友好的大环境？来我们这喵！',
        tags: ['聊天', '新人友好'],
        members: 6871,
        here: 13,
        grade: 4,
        gradeName: '北极星',
        icon: 'https://vrcc.cc/c/KIPFEL.2104.icon.jpg',
        page: 'https://vrcc.cc/c/KIPFEL.2104'
    }
};

describe('clubCodeForGroup', () => {
    it('拼的就是 shortCode.discriminator', () => {
        expect(
            clubCodeForGroup({ shortCode: 'KIPFEL', discriminator: '2104' })
        ).toBe('KIPFEL.2104');
    });

    it('缺任何一个都返回空串，不拼出半截短码', () => {
        expect(clubCodeForGroup({ shortCode: 'KIPFEL' })).toBe('');
        expect(clubCodeForGroup({ discriminator: '2104' })).toBe('');
        expect(clubCodeForGroup({ shortCode: '', discriminator: '2104' })).toBe(
            ''
        );
        expect(clubCodeForGroup({})).toBe('');
    });

    it('短码形状与官方 pattern 一致', () => {
        expect(isClubCodeShaped('KIPFEL.2104')).toBe(true);
        expect(isClubCodeShaped('kipfel.2104')).toBe(true);
        expect(isClubCodeShaped('520.6088')).toBe(true);
        expect(isClubCodeShaped('KIPFEL.')).toBe(false);
        expect(isClubCodeShaped('KIPFEL')).toBe(false);
        expect(isClubCodeShaped('KIPFEL.21045')).toBe(false);
    });
});

describe('GroupVrccClub', () => {
    afterEach(cleanup);

    beforeEach(() => {
        vi.clearAllMocks();
        // 默认：已连接、令牌有 `clubs`，带令牌那条能拿到点赞与关注。
        mocks.appVrccConnection.mockResolvedValue({
            scope: 'profile maps clubs'
        });
        mocks.appVrccMyClub.mockResolvedValue({
            club: { likes: 6, followed: true }
        });
    });

    it('把社团渲染成左栏一块卡片', async () => {
        mocks.appVrccClub.mockResolvedValue(clubResponse);

        renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'KIPFEL', discriminator: '2104' }}
            />
        );

        // **不渲染群组名**：它就贴在群组详情左栏，正上方已经写着一遍。
        expect(await screen.findByText('dialog.group.vrcc.club')).toBeTruthy();
        expect(screen.queryByText('中文kipfel厅')).toBeNull();
        // 评级、一句话、挂着几颗星、标签、来源声明都在。
        expect(screen.getByText('北极星')).toBeTruthy();
        expect(screen.getByText(clubResponse.club.lede)).toBeTruthy();
        expect(screen.getByText('dialog.group.vrcc.here:13')).toBeTruthy();
        expect(screen.getByText('聊天')).toBeTruthy();
        expect(screen.getByText('dialog.group.vrcc.source')).toBeTruthy();
        // 点赞与关注：只读展示，数据来自带令牌那条。
        expect(
            await screen.findByText('dialog.group.vrcc.likes:6')
        ).toBeTruthy();
        expect(screen.getByText('dialog.group.vrcc.followed')).toBeTruthy();
        // 群组人数不显示——VRChat 群组详情已经有权威值。
        expect(screen.queryByText('dialog.group.vrcc.members:6871')).toBeNull();
        expect(mocks.appVrccClub).toHaveBeenCalledWith('KIPFEL.2104');
    });

    it('没有 clubs 权限时不问带令牌那一条，点赞与关注都不显示', async () => {
        mocks.appVrccClub.mockResolvedValue(clubResponse);
        mocks.appVrccConnection.mockResolvedValue({ scope: 'profile maps' });

        renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'KIPFEL', discriminator: '2104' }}
            />
        );

        expect(await screen.findByText('北极星')).toBeTruthy();
        await Promise.resolve();
        expect(mocks.appVrccMyClub).not.toHaveBeenCalled();
        expect(screen.queryByText('dialog.group.vrcc.likes:6')).toBeNull();
        expect(screen.queryByText('dialog.group.vrcc.followed')).toBeNull();
        // **也不该显示「未关注」**：拿不到数据不等于没关注，那是编造信息。
        expect(screen.queryByText('dialog.group.vrcc.not_followed')).toBeNull();
    });

    it('明确未关注时才显示「未关注」，拿不到数据时不显示', async () => {
        mocks.appVrccClub.mockResolvedValue(clubResponse);

        // ① 接口明确说没关注。
        mocks.appVrccMyClub.mockResolvedValue({
            club: { likes: 6, followed: false }
        });
        renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'KIPFEL', discriminator: '2104' }}
            />
        );
        expect(
            await screen.findByText('dialog.group.vrcc.not_followed')
        ).toBeTruthy();
        expect(screen.queryByText('dialog.group.vrcc.followed')).toBeNull();
        cleanup();

        // ② 接口没给这个字段（拿不到）→ 两态都不显示。
        mocks.appVrccConnection.mockResolvedValue({
            scope: 'profile maps clubs'
        });
        mocks.appVrccMyClub.mockResolvedValue({ club: { likes: 6 } });
        renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'KIPFEL', discriminator: '2104' }}
            />
        );
        expect(
            await screen.findByText('dialog.group.vrcc.likes:6')
        ).toBeTruthy();
        expect(screen.queryByText('dialog.group.vrcc.not_followed')).toBeNull();
        expect(screen.queryByText('dialog.group.vrcc.followed')).toBeNull();
    });

    it('不渲染横幅：横幅在 19rem 的左栏里只挤占空间', async () => {
        mocks.appVrccClub.mockResolvedValue({
            club: {
                ...clubResponse.club,
                banner: 'https://vrcc.cc/c/KIPFEL.2104.banner.jpg'
            }
        });

        const { container } = renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'KIPFEL', discriminator: '2104' }}
            />
        );

        await screen.findByText('dialog.group.vrcc.club');
        const images = Array.from(container.querySelectorAll('img'));
        // 只保留图标那一张。
        expect(images).toHaveLength(1);
        expect(images[0]?.getAttribute('src')).toBe(clubResponse.club.icon);
    });

    it('404（后端转成 null）时整块不渲染', async () => {
        mocks.appVrccClub.mockResolvedValue(null);

        const { container } = renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'VRCN', discriminator: '0483' }}
            />
        );

        // 绝大多数群组走这条：**不留占位、不显示「没有社团」的结论**。
        await vi.waitFor(() => {
            expect(mocks.appVrccClub).toHaveBeenCalled();
        });
        expect(
            container.querySelector('[data-tour="group-vrcc-club-card"]')
        ).toBeNull();
        expect(screen.queryByText('dialog.group.vrcc.club')).toBeNull();
    });

    it('群组缺短码时一个请求都不发', async () => {
        renderWithQuery(<GroupVrccClub group={{ shortCode: 'KIPFEL' }} />);

        // 给 react-query 一个调度机会再断言。
        await Promise.resolve();
        expect(mocks.appVrccClub).not.toHaveBeenCalled();
    });

    it('请求失败时不把网络问题说成「这个群组没有社团」', async () => {
        mocks.appVrccClub.mockRejectedValue(new Error('network down'));

        const { container } = renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'VRCN', discriminator: '0483' }}
            />
        );

        await vi.waitFor(() => {
            expect(mocks.appVrccClub).toHaveBeenCalled();
        });
        expect(
            container.querySelector('[data-tour="group-vrcc-club-card"]')
        ).toBeNull();
    });

    it('没有评级（grade 为 null）时不显示评级行，其它照常', async () => {
        mocks.appVrccClub.mockResolvedValue({
            club: { ...clubResponse.club, grade: null, gradeName: null }
        });

        renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'KIPFEL', discriminator: '2104' }}
            />
        );

        expect(await screen.findByText('dialog.group.vrcc.club')).toBeTruthy();
        // 没有评级时显示「未分级」，而不是留空或退到最低档。
        expect(
            screen.getByText('dialog.group.vrcc.grade_unknown')
        ).toBeTruthy();
        expect(screen.queryByText('北极星')).toBeNull();
    });

    it('背面用详情里已经拿到的简介、房间和活动，不额外发请求', async () => {
        mocks.appVrccClub.mockResolvedValue(clubResponse);
        mocks.appVrccMyClub.mockResolvedValue({
            club: {
                likes: 6,
                followed: true,
                intro: [
                    { text: '我们是', hidden: false },
                    { text: '安静的社团', hidden: false }
                ],
                rooms: [{ map: '孤', users: 8, cap: 16 }],
                events: [{ title: '周会', at: 1700000000000 }]
            }
        });

        renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'KIPFEL', discriminator: '2104' }}
            />
        );

        const flip = await screen.findByLabelText(
            'dialog.group.vrcc.flip_back'
        );
        await vi.waitFor(() => expect(mocks.appVrccMyClub).toHaveBeenCalled());
        fireEvent.click(flip);

        expect(await screen.findByText('我们是安静的社团')).toBeTruthy();
        expect(screen.getByText('孤')).toBeTruthy();
        expect(screen.getByText('8/16')).toBeTruthy();
        expect(screen.getByText('周会')).toBeTruthy();
        expect(screen.getByText('t:1700000000000')).toBeTruthy();
        // 简介、房间、活动都来自第一次查详情的那份回答。
        expect(mocks.appVrccMyClub).toHaveBeenCalledTimes(1);
    });

    it('没有 clubs 权限时背面说「连接后才看得到」，不说成社团什么都没写', async () => {
        mocks.appVrccClub.mockResolvedValue(clubResponse);
        mocks.appVrccConnection.mockResolvedValue({ scope: 'profile maps' });

        renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'KIPFEL', discriminator: '2104' }}
            />
        );

        fireEvent.click(
            await screen.findByLabelText('dialog.group.vrcc.flip_back')
        );

        expect(
            await screen.findByText('dialog.group.vrcc.back_locked')
        ).toBeTruthy();
        expect(screen.queryByText('dialog.group.vrcc.back_empty')).toBeNull();
        expect(mocks.appVrccMyClub).not.toHaveBeenCalled();
    });

    it('背对镜头的那一面从可交互树上摘掉', async () => {
        mocks.appVrccClub.mockResolvedValue(clubResponse);

        renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'KIPFEL', discriminator: '2104' }}
            />
        );

        const flip = await screen.findByLabelText(
            'dialog.group.vrcc.flip_back'
        );
        const front = document.querySelector('.vrcc-club-card__face');
        const back = document.querySelector('.vrcc-club-card__face--back');
        expect(front?.hasAttribute('inert')).toBe(false);
        expect(back?.hasAttribute('inert')).toBe(true);

        fireEvent.click(flip);

        expect(front?.hasAttribute('inert')).toBe(true);
        expect(back?.hasAttribute('inert')).toBe(false);
    });

    /** 翻了没有，只看那片壳上的标记——比找文案稳。 */
    function flippedNow(): boolean {
        const flipper = document.querySelector('.vrcc-club-card__flipper');
        return Boolean(flipper?.hasAttribute('data-flipped'));
    }

    it('整张卡都能点着翻面，不只标题那一行', async () => {
        mocks.appVrccClub.mockResolvedValue(clubResponse);

        renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'KIPFEL', discriminator: '2104' }}
            />
        );

        const lede = await screen.findByText(clubResponse.club.lede);
        expect(flippedNow()).toBe(false);

        fireEvent.click(lede);

        expect(flippedNow()).toBe(true);
    });

    it('点在卡里的按钮上不翻面', async () => {
        mocks.appVrccClub.mockResolvedValue(clubResponse);

        renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'KIPFEL', discriminator: '2104' }}
            />
        );

        fireEvent.click(await screen.findByText('dialog.group.vrcc.open_page'));

        expect(flippedNow()).toBe(false);
    });

    it('正在选字时不翻面——玩家是在挑文字，不是在翻卡', async () => {
        mocks.appVrccClub.mockResolvedValue(clubResponse);

        renderWithQuery(
            <GroupVrccClub
                group={{ shortCode: 'KIPFEL', discriminator: '2104' }}
            />
        );

        const lede = await screen.findByText(clubResponse.club.lede);
        const range = document.createRange();
        range.selectNodeContents(lede);
        const selection = window.getSelection();
        selection?.removeAllRanges();
        selection?.addRange(range);

        fireEvent.click(lede);

        expect(flippedNow()).toBe(false);

        selection?.removeAllRanges();
    });
});
