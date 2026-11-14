// SPDX-License-Identifier: MPL-2.0
// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, render } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { GroupInstanceRecord } from '@/domain/entities/group';

import { useGroupClubInstanceRecords } from './useGroupClubInstanceRecords';

/**
 * 这个钩子有两件「错了也不报错」的事：
 *
 * 1. 空状态**必须**返回同一个数组引用。每次返回新数组会让调用方的 `useMemo`
 *    永远失效，把「没有社团」变成不停重算、不停重渲染——界面看起来只是有点卡。
 * 2. 投影出去的记录必须带 `friendCount: 0`。漏了会被 `mergeGroupInstances`
 *    的 `friendCount || userCount` 兜底成「总人数当好人数」，而界面照常显示。
 */

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

const CLUB = { club: { code: 'KIPFEL.2104', name: '中文kipfel厅' } };

const ROOM_LINK =
    'https://vrchat.com/home/launch?worldId=wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5&instanceId=05026~group(grp_x)~region(jp)';

type Group = {
    id?: string;
    shortCode?: string | null;
    discriminator?: string | null;
} | null;

let lastRecords: GroupInstanceRecord[] = [];

function Harness({ group }: { group: Group }) {
    lastRecords = useGroupClubInstanceRecords(group);
    return <div data-testid="count">{lastRecords.length}</div>;
}

function renderHook(group: Group) {
    const client = new QueryClient({
        defaultOptions: { queries: { retry: false, gcTime: 0 } }
    });
    return render(
        <QueryClientProvider client={client}>
            <Harness group={group} />
        </QueryClientProvider>
    );
}

beforeEach(() => {
    vi.clearAllMocks();
    lastRecords = [];
    mocks.appVrccConnection.mockResolvedValue({ scope: 'profile maps clubs' });
});

afterEach(cleanup);

describe('useGroupClubInstanceRecords · 空状态', () => {
    it('缺短码：一个请求都不发，返回空数组', async () => {
        renderHook({ id: 'grp_x', shortCode: 'KIPFEL' });
        await Promise.resolve();

        expect(lastRecords).toEqual([]);
        expect(mocks.appVrccClub).not.toHaveBeenCalled();
        expect(mocks.appVrccMyClub).not.toHaveBeenCalled();
    });

    it('group 为 null 也不炸', async () => {
        renderHook(null);
        await Promise.resolve();
        expect(lastRecords).toEqual([]);
    });

    it('公开页 404：不再问带令牌的那一条', async () => {
        mocks.appVrccClub.mockResolvedValue(null);
        renderHook({ id: 'grp_x', shortCode: 'KIPFEL', discriminator: '2104' });

        await vi.waitFor(() => expect(mocks.appVrccClub).toHaveBeenCalled());
        // 给 detailQuery 一个调度机会。
        await new Promise((resolve) => setTimeout(resolve, 0));

        expect(mocks.appVrccMyClub).not.toHaveBeenCalled();
        expect(lastRecords).toEqual([]);
    });

    it('详情里没有 rooms / 全拆不出来：都回到空数组', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        mocks.appVrccMyClub.mockResolvedValue({ club: { rooms: [] } });

        const { rerender } = renderHook({
            id: 'grp_x',
            shortCode: 'KIPFEL',
            discriminator: '2104'
        });
        await vi.waitFor(() => expect(mocks.appVrccMyClub).toHaveBeenCalled());
        await new Promise((resolve) => setTimeout(resolve, 0));
        expect(lastRecords).toEqual([]);

        // 脏数据的房：拆不出 location 就整条丢掉，不造点不开的空行。
        mocks.appVrccMyClub.mockResolvedValue({
            club: { rooms: [{ link: 'https://example.com/x' }, { link: null }] }
        });
        rerender(
            <QueryClientProvider
                client={
                    new QueryClient({
                        defaultOptions: { queries: { retry: false, gcTime: 0 } }
                    })
                }
            >
                <Harness
                    group={{
                        id: 'grp_x',
                        shortCode: 'KIPFEL',
                        discriminator: '2104'
                    }}
                />
            </QueryClientProvider>
        );
        await new Promise((resolve) => setTimeout(resolve, 0));
        expect(lastRecords.every((record) => record.location)).toBe(true);
    });

    it('空状态返回同一个引用：不是每次渲染新建一个 []', async () => {
        // 每次新建会让调用方的 `useMemo` 依赖每帧都变，把「没有社团」变成
        // 永不停歇的重算。这一条数的是引用，不是内容。
        mocks.appVrccClub.mockResolvedValue(null);
        const { rerender } = renderHook({
            id: 'grp_x',
            shortCode: 'KIPFEL',
            discriminator: '2104'
        });
        await new Promise((resolve) => setTimeout(resolve, 0));
        const first = lastRecords;

        rerender(
            <QueryClientProvider
                client={
                    new QueryClient({
                        defaultOptions: { queries: { retry: false, gcTime: 0 } }
                    })
                }
            >
                <Harness
                    group={{
                        id: 'grp_x',
                        shortCode: 'KIPFEL',
                        discriminator: '2104'
                    }}
                />
            </QueryClientProvider>
        );
        await new Promise((resolve) => setTimeout(resolve, 0));

        expect(lastRecords).toBe(first);
    });
});

describe('useGroupClubInstanceRecords · 投影', () => {
    it('房间投影成实例记录，friendCount 明确是 0', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        mocks.appVrccMyClub.mockResolvedValue({
            club: {
                rooms: [
                    {
                        link: ROOM_LINK,
                        map: '  孤  ',
                        users: 37,
                        cap: 40
                    },
                    // 上限缺失：`capacity` 不该出现，而不是 0。
                    {
                        link: 'https://vrchat.com/home/launch?worldId=wrld_b&instanceId=2',
                        map: null,
                        users: 2
                    }
                ]
            }
        });

        renderHook({ id: 'grp_x', shortCode: 'KIPFEL', discriminator: '2104' });
        await vi.waitFor(() => expect(lastRecords.length).toBe(2));

        const [first] = lastRecords;
        const firstLocation = String(first.location ?? '');
        expect(first.origin).toBe('vrcc');
        // **必须是 0**：漏了就变成「37 个好友」。
        expect(first.friendCount).toBe(0);
        expect(first.userCount).toBe(37);
        expect(first.capacity).toBe(40);
        expect(first.worldName).toBe('孤');
        expect(first.groupId).toBe('grp_x');
        expect(firstLocation).toBe(first.tag);
        expect(firstLocation.startsWith('wrld_410b9d63')).toBe(true);
        expect(first.ref).toMatchObject({ location: firstLocation });
        // 上下两行的 location 必须不同，否则会被 merge 合成一条。
        expect(lastRecords[0].location).not.toBe(lastRecords[1].location);
    });

    it('投影出来的记录能被 mergeGroupInstances 吃掉，且权威仍是 VRChat', async () => {
        mocks.appVrccClub.mockResolvedValue(CLUB);
        mocks.appVrccMyClub.mockResolvedValue({
            club: { rooms: [{ link: ROOM_LINK, users: 37, cap: 40 }] }
        });

        renderHook({ id: 'grp_x', shortCode: 'KIPFEL', discriminator: '2104' });
        await vi.waitFor(() => expect(lastRecords.length).toBe(1));

        const { mergeGroupInstances } = await import('./groupInstances');
        const rows = mergeGroupInstances(
            [
                ...lastRecords,
                // VRChat 的同一局：实时 39 人。
                {
                    id: lastRecords[0].instanceId,
                    location: lastRecords[0].location,
                    worldId: lastRecords[0].worldId,
                    userCount: 39,
                    origin: 'vrchat'
                }
            ],
            {
                groupId: 'grp_x',
                currentLocation: '',
                currentUserSnapshot: null,
                friendsById: {}
            }
        );

        expect(rows).toHaveLength(1);
        expect(rows[0].userCount).toBe(39);
        expect(rows[0].origin).toBe('vrchat');
        // 星遇那份的 friendCount 0 没有把 VRChat 的实况顶掉。
        expect(rows[0].friendCount).toBe(0);
    });
});
