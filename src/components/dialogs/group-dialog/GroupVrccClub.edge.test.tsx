// SPDX-License-Identifier: MPL-2.0
// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import type { ReactElement } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { GroupVrccClub } from './GroupVrccClub';

/**
 * 和已有那份 `GroupVrccClub.test.tsx` 的分工：
 *
 * 那份把 `t` mock 成 `(key) => key`，**插值不参与**，所以「把 `{year}` 写成
 * `{years}`」「把 `when` 传成 `at`」这类错误它抓不到——界面上只是显示一个
 * `{year}` 占位符，不报错、不进日志。这里换成一个**真会插值**的 `t`（照本仓库
 * 的单花括号约定，见 `i18nService`），并把模板抄成和 `zh-CN.json` 一致的形状。
 */

const TEMPLATES: Record<string, string> = {
    'dialog.group.vrcc.club': '星遇同频社团',
    'dialog.group.vrcc.source': '数据来源：星遇同频（非 VRChat 官方）',
    'dialog.group.vrcc.open_page': '社团页',
    'dialog.group.vrcc.since': '{year} 年成立',
    'dialog.group.vrcc.here': '{count} 颗星挂着',
    'dialog.group.vrcc.likes': '{count} 个赞',
    'dialog.group.vrcc.followed': '已关注',
    'dialog.group.vrcc.not_followed': '未关注',
    'dialog.group.vrcc.room_open': '有房间在开',
    'dialog.group.vrcc.event_open': '有活动',
    'dialog.group.vrcc.back_title': '社团详情',
    'dialog.group.vrcc.back_rooms': '正在开房',
    'dialog.group.vrcc.back_events': '下次活动',
    'dialog.group.vrcc.back_room_unlabeled': '未命名的房间',
    'dialog.group.vrcc.back_event_untitled': '未命名的活动',
    'dialog.group.vrcc.back_empty':
        '这个社团还没有公告，也没有在开的房间或活动',
    'dialog.group.vrcc.back_locked': '连接星遇同频之后可以看到公告、房间和活动',
    'dialog.group.vrcc.back_partial': '这个社团现在有房间或活动，但明细拿不到',
    'dialog.group.vrcc.grade_1': '三等星',
    'dialog.group.vrcc.grade_2': '二等星',
    'dialog.group.vrcc.grade_3': '一等星',
    'dialog.group.vrcc.grade_4': '北极星',
    'dialog.group.vrcc.grade_unknown': '未分级',
    'dialog.group.vrcc.grade_hint_1': '三等星说明',
    'dialog.group.vrcc.grade_hint_2': '二等星说明',
    'dialog.group.vrcc.grade_hint_3': '一等星说明',
    'dialog.group.vrcc.grade_hint_4': '北极星说明',
    'dialog.group.vrcc.grade_hint_unknown': '未分级说明',
    'dialog.group.vrcc.notice.title': '星遇社团公告',
    'dialog.group.vrcc.notice.expired': '已失效',
    'dialog.group.vrcc.notice.until': '{when} 到期',
    'dialog.group.vrcc.flip_back': '翻到背面',
    'dialog.group.vrcc.flip_front': '翻回正面'
};

function interpolate(template: string, options?: Record<string, unknown>) {
    return template.replace(
        /(?<!\{)\{(\w+)\}(?!\})/g,
        (whole, name: string) => {
            const value = options?.[name];
            // 参数名对不上就原样留着，好让断言看见占位符本身。
            return value === undefined ? whole : String(value);
        }
    );
}

vi.mock('react-i18next', () => ({
    useTranslation: () => ({
        t: (key: string, options?: Record<string, unknown>) =>
            interpolate(TEMPLATES[key] ?? key, options),
        i18n: { language: 'zh-CN', changeLanguage: () => Promise.resolve() },
        ready: true
    })
}));

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

vi.mock('@/lib/dateTime', () => ({
    formatCompactDateTime: (value: unknown) => `t:${String(value)}`
}));

function renderCard() {
    const client = new QueryClient({
        defaultOptions: { queries: { retry: false, gcTime: 0 } }
    });
    const ui: ReactElement = (
        <QueryClientProvider client={client}>
            <GroupVrccClub
                group={{ shortCode: 'KIPFEL', discriminator: '2104' }}
            />
        </QueryClientProvider>
    );
    return render(ui);
}

const CLUB = {
    club: {
        code: 'KIPFEL.2104',
        name: '中文kipfel厅',
        lede: '来我们这喵',
        tags: ['聊天'],
        here: 13,
        grade: 4,
        gradeName: '北极星',
        icon: 'https://vrcc.cc/c/KIPFEL.2104.icon.jpg',
        page: 'https://vrcc.cc/c/KIPFEL.2104'
    }
};

function flippedNow(): boolean {
    const flipper = document.querySelector('.vrcc-club-card__flipper');
    return Boolean(flipper?.hasAttribute('data-flipped'));
}

async function flipOpen() {
    fireEvent.click(await screen.findByLabelText('翻到背面'));
}

beforeEach(() => {
    vi.clearAllMocks();
    mocks.appVrccConnection.mockResolvedValue({
        scope: 'profile maps clubs'
    });
    mocks.appVrccMyClub.mockResolvedValue({ club: {} });
});

afterEach(cleanup);

describe('GroupVrccClub · 插值文案', () => {
    it('成立年份落在 {year} 里，不是留着占位符', async () => {
        mocks.appVrccClub.mockResolvedValue({
            club: { ...CLUB.club, since: 2024 }
        });
        renderCard();

        expect(await screen.findByText('2024 年成立')).toBeTruthy();
        expect(screen.queryByText('{year} 年成立')).toBeNull();
    });

    it('到期时间落在 {when} 里，且传的就是那个时间戳', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        mocks.appVrccMyClub.mockResolvedValue({
            club: {
                notice: {
                    segs: [{ text: '国庆合照', hidden: false }],
                    until: 1799251199999
                }
            }
        });
        renderCard();
        await flipOpen();

        // 参数名传错（先传 `value`）时这里会看到 `{when} 到期`。
        expect(await screen.findByText('t:1799251199999 到期')).toBeTruthy();
    });

    it('挂着几颗星与点赞数走 {@count}', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        mocks.appVrccMyClub.mockResolvedValue({
            club: { likes: 0, followed: true }
        });
        renderCard();

        expect(await screen.findByText('13 颗星挂着')).toBeTruthy();
        // 0 个赞是有意义的 0，不能被 `||` 吞掉。
        expect(await screen.findByText('0 个赞')).toBeTruthy();
        expect(screen.getByText('已关注')).toBeTruthy();
    });
});

describe('GroupVrccClub · 评级与成立年份的范围保护', () => {
    it('评级只认 1..4：0 / 5 / 字符串都落到「未分级」', async () => {
        for (const grade of [0, 5, -1, '3', null, undefined]) {
            mocks.appVrccClub.mockResolvedValue({
                club: { ...CLUB.club, grade, gradeName: '北极星' }
            });
            renderCard();

            expect(
                await screen.findByText('未分级'),
                `grade=${String(grade)}`
            ).toBeTruthy();
            // `gradeName` 给了也不认——档位本身就是坏的。
            expect(screen.queryByText('北极星'), String(grade)).toBeNull();
            cleanup();
        }
    });

    it('gradeName 是空白时退回档位名', async () => {
        mocks.appVrccClub.mockResolvedValue({
            club: { ...CLUB.club, grade: 3, gradeName: '   ' }
        });
        renderCard();

        expect(await screen.findByText('一等星')).toBeTruthy();
    });

    it('1989 / 2101 / 字符串都不算年份', async () => {
        for (const since of [1989, 2101, 0, '2024', null]) {
            mocks.appVrccClub.mockResolvedValue({
                club: { ...CLUB.club, since }
            });
            renderCard();

            await screen.findByText('星遇同频社团');
            expect(screen.queryByText(/\d+ 年成立/), String(since)).toBeNull();
            cleanup();
        }
    });

    it('两端都在「像年份」的范围里', async () => {
        for (const since of [1990, 2100]) {
            mocks.appVrccClub.mockResolvedValue({
                club: { ...CLUB.club, since }
            });
            renderCard();

            expect(await screen.findByText(`${since} 年成立`)).toBeTruthy();
            cleanup();
        }
    });
});

describe('GroupVrccClub · 公告与背面', () => {
    it('公告只有纯换行段时当作没有公告', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        mocks.appVrccMyClub.mockResolvedValue({
            club: { notice: { segs: [{ text: '\n', hidden: false }] } }
        });
        renderCard();

        // `joinSegments` 会把它 trim 成空串 → 没有公告正文 → 走「什么都没有」。
        expect(
            await screen.findByText(
                '这个社团还没有公告，也没有在开的房间或活动'
            )
        ).toBeTruthy();
        expect(screen.queryByText('星遇社团公告')).toBeNull();
        expect(document.querySelector('[data-notice-live]')).toBeNull();
    });

    it('没有 until 的公告算长期：不写到期，也亮小点', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        mocks.appVrccMyClub.mockResolvedValue({
            club: { notice: { segs: [{ text: '常驻开放', hidden: false }] } }
        });
        renderCard();
        await flipOpen();

        expect(await screen.findByText('常驻开放')).toBeTruthy();
        expect(screen.queryByText('已失效')).toBeNull();
        expect(screen.queryByText(/到期/)).toBeNull();
        expect(document.querySelector('[data-notice-live]')).toBeTruthy();
    });

    it('until 是字符串时不当时间用', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        mocks.appVrccMyClub.mockResolvedValue({
            club: {
                notice: {
                    segs: [{ text: '常驻开放', hidden: false }],
                    until: '1799251199999'
                }
            }
        });
        renderCard();
        await flipOpen();

        await screen.findByText('常驻开放');
        // `typeof notice.until === 'number'` 的判定不能被字符串骗过去。
        expect(screen.queryByText('已失效')).toBeNull();
        expect(screen.queryByText(/到期/)).toBeNull();
        expect(document.querySelector('[data-notice-live]')).toBeTruthy();
    });

    it('有权限、背面确实什么都没有时才说「什么都没有」', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        mocks.appVrccMyClub.mockResolvedValue({
            club: { likes: 2, followed: false }
        });
        renderCard();
        await flipOpen();

        expect(
            await screen.findByText(
                '这个社团还没有公告，也没有在开的房间或活动'
            )
        ).toBeTruthy();
        expect(
            screen.queryByText('连接星遇同频之后可以看到公告、房间和活动')
        ).toBeNull();
    });

    /**
     * `room` / `event` 两个布尔位**也参与**「背面有没有东西」的判定。实测有社团
     * `event: true` 但 `events: []`；只看两个数组的话，正面写着「有房间在开 /
     * 有活动」，背面却说「什么都没有」，同一张卡自相矛盾。
     */
    it('只有 room/event 布尔位时，背面说「明细拿不到」而不是「没有」', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        mocks.appVrccMyClub.mockResolvedValue({
            club: { room: true, event: true, events: [] }
        });
        renderCard();

        expect(await screen.findByText('有房间在开')).toBeTruthy();
        expect(screen.getByText('有活动')).toBeTruthy();
        await flipOpen();
        // 背面不再宣称「什么都没有」，而是说明明细拿不到。
        expect(
            await screen.findByText('这个社团现在有房间或活动，但明细拿不到')
        ).toBeTruthy();
        expect(
            screen.queryByText('这个社团还没有公告，也没有在开的房间或活动')
        ).toBeNull();
    });

    it('房间：有 users 没 cap 就只写人数；cap 是字符串不信', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        mocks.appVrccMyClub.mockResolvedValue({
            club: {
                rooms: [
                    { map: '孤', users: 8 },
                    { map: '海', users: 3, cap: '16' },
                    { map: '森', users: '5', cap: 16 },
                    { map: '  ', users: 1, cap: 2 }
                ]
            }
        });
        renderCard();
        await flipOpen();

        await screen.findByText('孤');
        expect(screen.getByText('8')).toBeTruthy();
        // cap 不是数字 → 只写「3」，不能拼出 3/16 那种看起来精确的值。
        expect(screen.getByText('3')).toBeTruthy();
        expect(screen.queryByText('5/16')).toBeNull();
        expect(screen.getByText('1/2')).toBeTruthy();
        // 地图名是空白 → 用「未命名的房间」占位，不留空行。
        expect(screen.getByText('未命名的房间')).toBeTruthy();
    });

    it('活动：at 不是数字就不写时间，标题空白用占位文案', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        mocks.appVrccMyClub.mockResolvedValue({
            club: {
                events: [
                    { title: '周会', at: '1700000000000' },
                    { title: '   ', at: 1700000000000 }
                ]
            }
        });
        renderCard();
        await flipOpen();

        await screen.findByText('周会');
        // 两条活动里只有第二条给了数字时间戳 → 时间只出现一次。
        expect(screen.getAllByText('t:1700000000000')).toHaveLength(1);
        expect(screen.getByText('未命名的活动')).toBeTruthy();
    });
});

describe('GroupVrccClub · 正面细节', () => {
    it('没有图标时用名字首字兜底，名字也空时用短码首字', async () => {
        mocks.appVrccClub.mockResolvedValue({
            club: { ...CLUB.club, icon: undefined, name: '中文kipfel厅' }
        });
        const first = renderCard();
        expect(await screen.findByText('中')).toBeTruthy();
        first.unmount();

        mocks.appVrccClub.mockResolvedValue({
            club: { ...CLUB.club, icon: undefined, name: '' }
        });
        renderCard();
        // 名字退到短码 `KIPFEL.2104`，首字是 K。
        expect(await screen.findByText('K')).toBeTruthy();
    });

    it('没有 page 就不出现「社团页」按钮', async () => {
        mocks.appVrccClub.mockResolvedValue({
            club: { ...CLUB.club, page: '' }
        });
        renderCard();

        await screen.findByText('星遇同频社团');
        expect(screen.queryByText('社团页')).toBeNull();
        // 来源文案那一行仍然在：有没有外链不影响「这是别家数据」这句话。
        expect(
            screen.getByText('数据来源：星遇同频（非 VRChat 官方）')
        ).toBeTruthy();
    });

    it('标签为空项被剔掉，最多画 4 个', async () => {
        mocks.appVrccClub.mockResolvedValue({
            club: {
                ...CLUB.club,
                tags: ['a', null, '', 'b', 'c', 'd', 'e']
            }
        });
        renderCard();

        await screen.findByText('a');
        for (const tag of ['a', 'b', 'c', 'd']) {
            expect(screen.getByText(tag), tag).toBeTruthy();
        }
        // 第 5 个真标签被 slice 掉，空项不占位置。
        expect(screen.queryByText('e')).toBeNull();
    });

    it('拿不到点赞/关注时整行不渲染，也不写「未关注」', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        mocks.appVrccMyClub.mockResolvedValue({ club: {} });
        renderCard();

        await screen.findByText('星遇同频社团');
        await vi.waitFor(() => expect(mocks.appVrccMyClub).toHaveBeenCalled());
        expect(screen.queryByText('未关注')).toBeNull();
        expect(screen.queryByText('已关注')).toBeNull();
        expect(screen.queryByText(/\d+ 个赞/)).toBeNull();
    });
});

describe('GroupVrccClub · 翻面状态机', () => {
    it('正面按钮只会翻到背面，点背面空白处再翻回来', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        renderCard();

        const flipButton = await screen.findByLabelText('翻到背面');
        expect(flippedNow()).toBe(false);
        fireEvent.click(flipButton);
        expect(flippedNow()).toBe(true);

        // 背面那一面自己的标题栏不是按钮，点它就是「翻回正面」。
        fireEvent.click(screen.getByText('社团详情'));
        expect(flippedNow()).toBe(false);
    });

    it('翻面只切 inert，不摘 DOM：两面始终都在', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        renderCard();

        await screen.findByText('星遇同频社团');
        expect(document.querySelectorAll('.vrcc-club-card__face')).toHaveLength(
            2
        );

        await flipOpen();
        expect(document.querySelectorAll('.vrcc-club-card__face')).toHaveLength(
            2
        );
        // 背面的「翻回正面」按钮在翻过去之后才够得着。
        fireEvent.click(screen.getByLabelText('翻回正面'));
        expect(flippedNow()).toBe(false);
    });
});
