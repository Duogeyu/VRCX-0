// SPDX-License-Identifier: MPL-2.0

// @vitest-environment jsdom

import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
    connection: vi.fn(),
    usage: vi.fn(),
    boards: vi.fn(),
    models: vi.fn(),
    model: vi.fn(),
    status: vi.fn(),
    announcements: vi.fn(),
    unread: vi.fn(),
    notes: vi.fn(),
    recentNotifications: vi.fn(),
    myMap: vi.fn(),
    mutuals: vi.fn(),
    crossings: vi.fn(),
    crossingsVisibility: vi.fn(),
    crossingCards: vi.fn(),
    today: vi.fn(),
    beginDeviceAuth: vi.fn(),
    deviceAuthPoll: vi.fn(),
    disconnect: vi.fn(),
    // A 批新增的读接口。`vi.fn()` 默认返回 `undefined`，调用方拿到的是
    // 「没有数据」，和真实的「旧服务器回 404」等价，不会在初始化时炸掉。
    maps: vi.fn(),
    stickers: vi.fn(),
    myMaps: vi.fn(),
    mapPage: vi.fn(),
    myBoard: vi.fn(),
    toastAdd: vi.fn(),
    openWorldDialog: vi.fn(),
    openUserDialog: vi.fn(),
    openGroupDialog: vi.fn(),
    subscribeNotices: vi.fn()
}));

vi.mock('@/services/toastService', () => ({
    toast: { add: mocks.toastAdd }
}));

vi.mock('@/services/dialogService', () => ({
    openWorldDialog: mocks.openWorldDialog,
    openUserDialog: mocks.openUserDialog,
    openGroupDialog: mocks.openGroupDialog
}));

// 提醒改由后端轮询、前端订阅事件。测试里给一个能立刻退订的假订阅：
// 不关心事件何时到，只关心订阅/退订的成对行为。
vi.mock('@/platform/tauri/client', () => ({
    tauriClient: {
        events: {
            subscribe: mocks.subscribeNotices
        }
    }
}));

vi.mock('@/platform/tauri/bindings', () => ({
    commands: {
        appVrccConnection: mocks.connection,
        appVrccUsage: mocks.usage,
        appVrccBoards: mocks.boards,
        appVrccModels: mocks.models,
        appVrccModel: mocks.model,
        appVrccStatus: mocks.status,
        appVrccAnnouncements: mocks.announcements,
        appVrccUnread: mocks.unread,
        appVrccNotes: mocks.notes,
        appVrccRecentNotifications: mocks.recentNotifications,
        appVrccMyMap: mocks.myMap,
        appVrccMutuals: mocks.mutuals,
        appVrccCrossings: mocks.crossings,
        appVrccCrossingsVisibility: mocks.crossingsVisibility,
        appVrccCrossingCards: mocks.crossingCards,
        appVrccToday: mocks.today,
        appVrccDeviceAuthBegin: mocks.beginDeviceAuth,
        appVrccDeviceAuthPoll: mocks.deviceAuthPoll,
        appVrccDisconnect: mocks.disconnect,
        appVrccMaps: mocks.maps,
        appVrccStickers: mocks.stickers,
        appVrccMyMaps: mocks.myMaps,
        appVrccMapPage: mocks.mapPage,
        appVrccMyBoard: mocks.myBoard
    }
}));

// 必须部分 mock：`i18nService` 在模块初始化时要用到真实的
// `initReactI18next`，整体替换会让它在 import 阶段就失败。
vi.mock('react-i18next', async (importOriginal) => {
    const actual = await importOriginal<typeof import('react-i18next')>();
    return {
        ...actual,
        useTranslation: () => ({ t: (key: string) => key })
    };
});

import { queryKeys } from '@/lib/entityQueryCache';
import { queryClient } from '@/lib/queryClient';

import { cachedModelDetail, rememberModelDetail } from './useModelDetail';
import { useVrccController } from './useVrccController';
import { useVrccAnnouncementCache } from './vrccAnnouncementCache';

const connected = {
    connected: true,
    expired: false,
    clientIsDev: true,
    star: 77,
    vrcName: 'VRC',
    scope: 'profile maps',
    client: 'dev-test',
    lastPollAt: null,
    nextPollAt: null
};

/**
 * 令牌里没有 `maps`：`/me/board` 一定 403，地图榜只能走公开榜。
 *
 * 这不是异常状态——`dev-` 标识下的敏感权限被拒、玩家自己取消授权，都会
 * 落到这里，界面要做的是静默降级而不是报错。
 */
const connectedWithoutMaps = {
    ...connected,
    scope: 'profile notifications'
};

const usage = {
    personalUsedToday: 7,
    personalDailyLimit: 1000,
    unreadUsedToday: 40,
    unreadDailyLimit: 1500,
    windowUsed: 2,
    windowLimit: 120,
    backoffRemainingMs: 0,
    resetSecs: 3600
};

beforeEach(() => {
    vi.clearAllMocks();
    // 公告走了模块级共享缓存（`vrccAnnouncementCache`），不清的话上一条
    // 测试写进去的数据会在 1 分钟内被这条测试读到——表现为「明明 mock 成
    // null，取到的却是上一轮的 { stop: null, posts: [] }」。
    useVrccAnnouncementCache.getState().invalidate();
    useVrccAnnouncementCache.getState().setInFlight(null);
    mocks.connection.mockResolvedValue(connected);
    mocks.usage.mockResolvedValue(usage);
    mocks.boards.mockImplementation((_limit: number, sort: string | null) =>
        Promise.resolve({
            board: 'maps',
            sort: sort ?? 'top',
            min: 1,
            maps: [],
            at: 1
        })
    );
    // 默认连接就带 `maps` 权限，所以地图榜默认走玩家视角 `/me/board`。
    // 要测公开榜那条路，把 `connection` 换成 `connectedWithoutMaps`。
    mocks.myBoard.mockImplementation((sort: string | null) =>
        Promise.resolve({ sort: sort ?? 'top', min: 1, maps: [] })
    );
    mocks.status.mockResolvedValue({ at: 1, state: 'ok' });
    // 模型榜：默认给一个空榜。它和地图榜是**两条独立调用**，只设 `boards`
    // 会让这条返回 `undefined`，而那和「旧服务器回 null」在断言上看起来一样。
    mocks.models.mockImplementation((_limit: number, sort: string | null) =>
        Promise.resolve({
            board: 'models',
            sort: sort ?? 'top',
            models: [],
            at: 1
        })
    );
    mocks.announcements.mockResolvedValue({ stop: null, posts: [] });
    mocks.unread.mockResolvedValue({ inbox: 1, cross: 0, chats: 2, notes: 0 });
    mocks.notes.mockResolvedValue({ notes: [] });
    mocks.recentNotifications.mockResolvedValue({ items: [], read: 0 });
    mocks.myMap.mockResolvedValue(null);
    mocks.mutuals.mockResolvedValue({ stars: [], n: 0 });
    mocks.crossingsVisibility.mockResolvedValue({
        stars: [],
        n: 0,
        why: null,
        hidden: false,
        me: null
    });
    // 探测和正式读取是两条调用，各自要有一个默认回答；只设一条会让另一条
    // 返回 `undefined`，测试里看起来像「拿到了空数据」。
    mocks.crossings.mockResolvedValue({
        stars: [],
        n: 0,
        why: null,
        hidden: false,
        me: null
    });
    mocks.crossingCards.mockResolvedValue({
        cards: [],
        seen: null,
        left: null,
        max: null
    });
    // 订阅立即返回一个退订函数，模拟 Tauri 正常挂上监听。
    mocks.subscribeNotices.mockResolvedValue(() => {});
});

afterEach(() => {
    cleanup();
});

describe('useVrccController', () => {
    it('加载后填入连接状态与真实用量', async () => {
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));

        expect(result.current.connection?.connected).toBe(true);
        // 用量必须是后端给的真值：写死的 0 会被这条抓住。
        expect(result.current.usage?.personalUsedToday).toBe(7);
        expect(result.current.usage?.personalDailyLimit).toBe(1000);
    });

    it('申请连接码后进入 5 秒冷却，冷却期内不再发请求', async () => {
        vi.useFakeTimers({ shouldAdvanceTime: true });
        mocks.beginDeviceAuth.mockResolvedValue({
            userCode: 'ABCD-2345',
            deviceCode: 'dc',
            verificationUri: 'https://vrcc.cc/device',
            verificationUriComplete: 'https://vrcc.cc/device?code=ABCD-2345',
            interval: 5,
            expiresIn: 600
        });
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));

        expect(result.current.deviceAuthCooldown).toBe(0);

        await act(async () => {
            await result.current.beginDeviceAuth();
        });
        expect(mocks.beginDeviceAuth).toHaveBeenCalledTimes(1);
        // 请求一发出就开始计时，不等它回来。
        expect(result.current.deviceAuthCooldown).toBeGreaterThan(0);

        // 冷却期内再点：连请求都不该发出去。
        await act(async () => {
            await result.current.beginDeviceAuth();
        });
        expect(mocks.beginDeviceAuth).toHaveBeenCalledTimes(1);

        // 走完 5 秒后按钮恢复可用。
        await act(async () => {
            vi.advanceTimersByTime(5_000);
        });
        await waitFor(() => expect(result.current.deviceAuthCooldown).toBe(0));
        vi.useRealTimers();
    });

    it('打开「提醒」页签时用 mark=true 同步账号已读', async () => {
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));
        mocks.recentNotifications.mockClear();

        act(() => result.current.setTab('notifications'));

        await waitFor(() =>
            expect(mocks.recentNotifications).toHaveBeenCalledWith(true)
        );
        // 只有这一次是 true——后台轮询不会传 true。
        const trues = mocks.recentNotifications.mock.calls.filter(
            (call) => call[0] === true
        );
        expect(trues).toHaveLength(1);
    });

    it('初次加载的提醒拉取不带 mark', async () => {
        renderHook(() => useVrccController());
        await waitFor(() =>
            expect(mocks.recentNotifications).toHaveBeenCalled()
        );
        // 加载时只是为了拿数据，不该替玩家标记已读。
        expect(mocks.recentNotifications).toHaveBeenCalledWith(false);
    });

    it('已连接时订阅后端提醒事件，卸载时退订', async () => {
        const unsubscribe = vi.fn();
        mocks.subscribeNotices.mockResolvedValue(unsubscribe);

        const { unmount } = renderHook(() => useVrccController());

        await waitFor(() =>
            expect(mocks.subscribeNotices).toHaveBeenCalledWith(
                'vrccNoticesChanged',
                expect.any(Function)
            )
        );
        expect(unsubscribe).not.toHaveBeenCalled();

        unmount();
        // 必须退订：否则页面切走后监听还挂着，事件到达会打到已卸戴的组件上。
        await waitFor(() => expect(unsubscribe).toHaveBeenCalled());
    });

    it('后端推来「提醒有变化」时刷新列表，且不标记已读', async () => {
        const handlers: Record<string, () => void> = {};
        mocks.subscribeNotices.mockImplementation(
            (event: string, handler: () => void) => {
                handlers[event] = handler;
                return Promise.resolve(() => {});
            }
        );

        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));
        await waitFor(() => expect(handlers.vrccNoticesChanged).toBeDefined());

        mocks.recentNotifications.mockClear();
        mocks.unread.mockClear();
        act(() => handlers.vrccNoticesChanged());

        // 后台发现的提醒不能替玩家标记已读。
        await waitFor(() =>
            expect(mocks.recentNotifications).toHaveBeenCalledWith(false)
        );
        // 未读数读后台刚问过的缓存，不再花一次 me/unread。
        expect(mocks.unread).toHaveBeenCalledWith(false);
    });

    it('后端说连接失效时重读连接状态', async () => {
        const handlers: Record<string, () => void> = {};
        mocks.subscribeNotices.mockImplementation(
            (event: string, handler: () => void) => {
                handlers[event] = handler;
                return Promise.resolve(() => {});
            }
        );

        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));
        await waitFor(() =>
            expect(handlers.vrccConnectionChanged).toBeDefined()
        );

        mocks.connection.mockResolvedValue({
            ...connected,
            connected: false,
            expired: true
        });
        act(() => handlers.vrccConnectionChanged());

        await waitFor(() =>
            expect(result.current.connection?.connected).toBe(false)
        );
        expect(result.current.connection?.expired).toBe(true);
    });

    it('换账号（star 变了）就清空模型的两层会话缓存', async () => {
        // 前端这两层缓存只活一个会话，所以「按账号分区」的等价做法是**换人就
        // 整表清空**。改坏了界面上完全看不出来——只会把 A 的详情给 B 看。
        const handlers: Record<string, () => void> = {};
        mocks.subscribeNotices.mockImplementation(
            (event: string, handler: () => void) => {
                handlers[event] = handler;
                return Promise.resolve(() => {});
            }
        );

        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));
        await waitFor(() =>
            expect(handlers.vrccConnectionChanged).toBeDefined()
        );

        // 挂载时那个 effect 已经清过一次，这里先塞一份进去。
        rememberModelDetail(1, { model: null, hots: [], at: 1 });
        expect(cachedModelDetail(1)).not.toBeNull();

        // 同一台机器上换了账号：77 → 88。
        mocks.connection.mockResolvedValue({ ...connected, star: 88 });
        act(() => handlers.vrccConnectionChanged());

        await waitFor(() => expect(result.current.connection?.star).toBe(88));
        expect(cachedModelDetail(1)).toBeNull();
    });

    it('请求报令牌失效时也重读连接状态，界面不再挂着「已连接」', async () => {
        const consoleSpy = vi
            .spyOn(console, 'error')
            .mockImplementation(() => {});
        mocks.myMap.mockRejectedValue(
            Object.assign(new Error('星遇同频接口返回 HTTP 404（gone）'), {
                vrcc: { kind: 'needsReauth', need: null }
            })
        );
        mocks.connection.mockResolvedValueOnce(connected).mockResolvedValue({
            ...connected,
            connected: false,
            expired: true
        });

        const { result } = renderHook(() => useVrccController());

        await waitFor(() =>
            expect(result.current.connection?.expired).toBe(true)
        );
        expect(result.current.error?.kind).toBe('needsReauth');
        consoleSpy.mockRestore();
    });

    it('crossings 缺权限时只记在页签里，不弹 toast、不拖垮整页', async () => {
        // 与 mutuals 同一个道理：`dev-` 标识拿不到 `crossings` 敏感权限，
        // 线上实返 `403 scope`。这是预期结果，不是故障——只在「星轨相交」
        // 页签里说明，不该渲染成整页红色横幅。
        mocks.crossings.mockRejectedValue(
            Object.assign(new Error('HTTP 403 scope'), {
                vrcc: { kind: 'missingScope', need: 'crossings' }
            })
        );
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));
        act(() => result.current.setTab('crossings'));
        await waitFor(() =>
            expect(result.current.crossingsError).not.toBeNull()
        );

        expect(result.current.crossingsError).toContain('403');
        // **不清空已有数据**：失败只加一条原因，不能把手上这份列表抹掉
        // 变成「一条都没有」。
        expect(result.current.crossingsNeedsScope).toBe(true);
        expect(mocks.toastAdd).not.toHaveBeenCalled();
        // 地图榜与连接状态照旧可用。
        expect(result.current.boards).not.toBeNull();
        expect(result.current.connection?.connected).toBe(true);
    });

    it('crossings 把 hidden 带出来，交给界面隐藏整个模块', async () => {
        // `hide` 意味着玩家不该看到这个模块（16-17 岁星池，或玩家在隐私
        // 设置里关了星轨相交）。官方要求客户端不显示，所以控制器必须把
        // 后端算好的 `hidden` 留在 state 里，不能吞掉——吞掉就会变成一个
        // 普通空列表，等于向不该看到的人确认「这里有星轨相交」这个事实。
        mocks.crossingsVisibility.mockResolvedValue({
            stars: [],
            n: 0,
            why: 'hide',
            hidden: true,
            me: null
        });
        const { result } = renderHook(() => useVrccController());
        // **不用点进去**：探测本身就是那条回答，`hidden` 在开页时就该到手，
        // 页签因此在玩家点之前就不存在。
        await waitFor(() =>
            expect(result.current.crossings?.hidden).toBe(true)
        );
        expect(result.current.crossings?.why).toBe('hide');
        expect(result.current.crossings?.stars).toHaveLength(0);
        expect(result.current.crossingsChecked).toBe(true);
    });

    it('首屏只探可见性、不拉星卡；切到这一页才带 mark=true 拉全', async () => {
        // 三件事一起钉住：
        // 1. 首屏探一次**列表**（不带 mark），只为了知道 `hidden`——
        //    `hide` 不探测就会「页签先出现、点进去才消失」，那等于已经
        //    确认了功能存在，正是官方要求避免的。
        // 2. 星卡首屏**一定不拉**：它是第二份读的额度，而可见性在列表里。
        // 3. 玩家真切到这一页 = 看过了，要带 mark=true 把红点消掉。
        //    这条链路错了，红点永远消不掉。
        mocks.crossingsVisibility.mockResolvedValue({
            stars: [],
            n: 0,
            why: null,
            hidden: false,
            me: null
        });
        mocks.crossingCards.mockResolvedValue({
            cards: [],
            seen: null,
            left: 3,
            max: 5
        });
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));

        // 首屏：**探测**问过（走独立限流槽的那条），列表读取一次都没问过；
        // 星卡也没问过。
        await waitFor(() =>
            expect(mocks.crossingsVisibility).toHaveBeenCalledTimes(1)
        );
        expect(mocks.crossings).not.toHaveBeenCalled();
        expect(mocks.crossingCards).not.toHaveBeenCalled();
        // 探测成功 → 视为「已确认」，界面可以显示入口。
        await waitFor(() => expect(result.current.crossingsChecked).toBe(true));

        // 玩家切到「星轨相交」：列表带 mark=true 拉。
        act(() => result.current.setTab('crossings'));
        await waitFor(() => expect(mocks.crossings).toHaveBeenCalledWith(true));
        // 星卡**不带** mark：已读只有列表那一条接口能消，星卡再标一次是
        // 重复动作，而它算第二份读的额度。它只是为了拿 left / max。
        expect(mocks.crossingCards).toHaveBeenCalledWith(false);
        // 星卡的额度读数被记下来了。
        await waitFor(() => expect(result.current.crossingCards?.left).toBe(3));
    });

    /**
     * 探测和玩家那次 `mark=1` 必须是**两条不同的请求**，不能被本地限流
     * 合成一次。后端用独立的限流槽（`crossings/probe`）保证这一点，这里
     * 从调用侧钉住：探测走 `crossingsVisibility`，切页签走 `crossings(true)`。
     */
    it('探测与切页签是两次不同的调用，互不占用限流槽', async () => {
        mocks.crossingsVisibility.mockResolvedValue({
            stars: [],
            n: 0,
            why: null,
            hidden: false,
            me: null
        });
        mocks.crossings.mockResolvedValue({
            stars: [],
            n: 0,
            why: null,
            hidden: false,
            me: null
        });
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.crossingsChecked).toBe(true));

        act(() => result.current.setTab('crossings'));
        await waitFor(() => expect(mocks.crossings).toHaveBeenCalledWith(true));
        // 探测那条没被复用成带 mark 的调用。
        expect(mocks.crossingsVisibility).not.toHaveBeenCalledWith(true);
    });

    /** 探测拿到的 `hidden` 要让页签在玩家点进去之前就消失。 */
    it('探测到 hidden 后 crossings.hidden 为真，界面据此藏入口', async () => {
        mocks.crossingsVisibility.mockResolvedValue({
            stars: [],
            n: 0,
            why: 'hide',
            hidden: true,
            me: null
        });
        const { result } = renderHook(() => useVrccController());
        await waitFor(() =>
            expect(result.current.crossings?.hidden).toBe(true)
        );
        // 可见性不需要星卡，首屏不该问它。
        expect(mocks.crossingCards).not.toHaveBeenCalled();
    });

    /** 探测失败不打扰玩家：页签照常，等点进去那次报真实原因。 */
    it('探测失败是静默的，不写 crossingsError', async () => {
        mocks.crossingsVisibility.mockRejectedValueOnce(
            new Error('network down')
        );
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));
        expect(result.current.crossingsError).toBeNull();
    });

    it('手动刷新星轨相交不替玩家把记录标成已读', async () => {
        // 刷新按钮只是「再看看有没有新的」，不是「我看过了」。顺手传
        // mark=true 会把玩家还没看完的记录标成已读，而且不可撤销、
        // 不会报错。
        mocks.crossingsVisibility.mockResolvedValue({
            stars: [],
            n: 0,
            why: null,
            hidden: false,
            me: null
        });
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));

        mocks.crossings.mockClear();
        await act(async () => {
            await result.current.refreshCrossings(false);
        });
        expect(mocks.crossings).toHaveBeenCalledWith(false);
    });

    it('断开连接后清掉星轨相交，不把上个账号的记录留在界面上', async () => {
        // 擦肩记录是**个人数据**。不清的话，断开后页签里还留着别人的
        // 记录和额度，看起来像是连着的。
        mocks.crossingsVisibility.mockResolvedValue({
            stars: [{ n: 77, name: '拾光旅人', linked: false, mutual: false }],
            n: 1,
            why: null,
            hidden: false,
            me: null
        });
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));

        // 首屏不拉，先切到那一页把数据填上。
        act(() => result.current.setTab('crossings'));
        await waitFor(() =>
            expect(result.current.crossings?.stars).toHaveLength(1)
        );

        mocks.disconnect.mockResolvedValue(undefined);
        await act(async () => {
            await result.current.disconnect();
        });

        expect(result.current.crossings).toBeNull();
        expect(result.current.crossingCards).toBeNull();
    });

    it('mutuals 缺权限时只记在页签里，不弹 toast、不拖垮整页', async () => {
        // `dev-` 标识拿不到 `mutuals` 敏感权限，线上实返 `403 scope`。
        // 这是**预期内**的结果，不是故障：界面只在「连上」页签里说明，
        // 不该走全局错误路径（那会渲染成整页红色横幅，看着像连接坏了）。
        mocks.mutuals.mockRejectedValue(
            Object.assign(new Error('HTTP 403 scope'), {
                vrcc: { kind: 'missingScope', need: 'mutuals' }
            })
        );
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));
        await waitFor(() => expect(result.current.mutualsError).not.toBeNull());

        // 失败原因被记下来了，供页签自己显示。
        expect(result.current.mutualsError).toContain('403');
        expect(result.current.mutuals).toBeNull();
        // 关键：**没有**弹 toast。这条断言以前不存在，用例名与行为不符。
        expect(mocks.toastAdd).not.toHaveBeenCalled();
        // 地图榜和连接状态照旧可用。
        expect(result.current.boards).not.toBeNull();
        expect(result.current.connection?.connected).toBe(true);
    });

    it('mutuals 遇到真故障时仍然弹 toast', async () => {
        // 与上一条对照：非缺权限的失败（网络、5xx）是真故障，要有反馈。
        const consoleSpy = vi
            .spyOn(console, 'error')
            .mockImplementation(() => {});
        mocks.mutuals.mockRejectedValue(
            Object.assign(new Error('network unreachable'), {
                vrcc: { kind: 'transport', need: null }
            })
        );
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));
        await waitFor(() => expect(mocks.toastAdd).toHaveBeenCalled());

        expect(result.current.mutualsError).toBeNull();
        consoleSpy.mockRestore();
    });

    it('连上后拉一次未读数（发请求）', async () => {
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.unread).not.toBeNull());
        expect(mocks.unread).toHaveBeenCalledWith(true);
        expect(result.current.unread?.chats).toBe(2);
    });

    it('服务器还没有新接口时安静地不显示', async () => {
        mocks.unread.mockResolvedValue(null);
        mocks.announcements.mockResolvedValue(null);
        mocks.notes.mockResolvedValue(null);

        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));
        await waitFor(() => expect(mocks.unread).toHaveBeenCalled());

        act(() => result.current.setNotificationsView('notes'));
        await waitFor(() => expect(result.current.notesSupported).toBe(false));
        expect(result.current.unread).toBeNull();
        expect(result.current.announcements).toBeNull();
        expect(result.current.error).toBeNull();
        expect(mocks.toastAdd).not.toHaveBeenCalled();
    });

    it('切到「通知」才拉通知中心', async () => {
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));
        expect(mocks.notes).not.toHaveBeenCalled();

        act(() => result.current.setNotificationsView('notes'));
        await waitFor(() => expect(mocks.notes).toHaveBeenCalledTimes(1));
        expect(result.current.notificationsView).toBe('notes');
        await waitFor(() => expect(result.current.notesSupported).toBe(true));
    });

    it('连上且带 maps 权限时，地图榜走玩家视角', async () => {
        // 默认连接（`scope: 'profile maps'`）就该切到 `/me/board`。
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.boards).not.toBeNull());

        expect(mocks.myBoard).toHaveBeenCalledWith('top', null);
        expect(mocks.boards).not.toHaveBeenCalled();
        expect(result.current.boards?.source).toBe('mine');
    });

    it('切换地图榜排序（玩家视角）', async () => {
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.boards).not.toBeNull());
        expect(mocks.myBoard).toHaveBeenCalledWith('top', null);

        act(() => result.current.setBoardSort('week'));
        await waitFor(() => expect(result.current.boardSort).toBe('week'));
        expect(mocks.myBoard).toHaveBeenCalledWith('week', null);
        expect(result.current.boardSortSupported).toBe(true);

        // 一分钟内切回总榜，直接用手上的，不再请求。
        mocks.myBoard.mockClear();
        act(() => result.current.setBoardSort('top'));
        await waitFor(() => expect(result.current.boardSort).toBe('top'));
        expect(mocks.myBoard).not.toHaveBeenCalled();
    });

    it('令牌缺 maps 时静默走公开榜，不弹错误', async () => {
        mocks.connection.mockResolvedValue(connectedWithoutMaps);

        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.boards).not.toBeNull());

        // 缺权限是预期状态（`dev-` 标识、玩家取消授权都会落到这里），
        // 所以**连试都不试** `/me/board`，也不把 403 摆到界面上。
        expect(mocks.myBoard).not.toHaveBeenCalled();
        expect(mocks.boards).toHaveBeenCalledWith(100, 'top');
        expect(result.current.boards?.source).toBe('public');
        expect(result.current.error).toBeNull();
    });

    it('玩家视角请求失败时退回公开榜，且同一次连接内不再重试', async () => {
        mocks.myBoard.mockRejectedValue(new Error('403 scope'));

        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.boards).not.toBeNull());
        expect(result.current.boards?.source).toBe('public');

        // 已经知道这次连接下用不了，再刷新不应该又去撞一次 403。
        const attemptsAfterFirstLoad = mocks.myBoard.mock.calls.length;
        const boardsBefore = mocks.boards.mock.calls.length;
        await act(async () => {
            result.current.refreshBoards();
        });
        await waitFor(() =>
            expect(mocks.boards.mock.calls.length).toBeGreaterThan(boardsBefore)
        );
        expect(mocks.myBoard.mock.calls.length).toBe(attemptsAfterFirstLoad);
        expect(result.current.error).toBeNull();
    });

    it('「我去过的」在没有玩家视角时退回总榜，不把 went 发到公开榜', async () => {
        mocks.connection.mockResolvedValue(connectedWithoutMaps);

        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.boards).not.toBeNull());

        act(() => result.current.setBoardSort('went'));
        await waitFor(() => expect(result.current.boardSort).toBe('top'));
        // `/boards/maps` 没有 `went`，发出去只会拿到总榜。
        expect(mocks.boards).toHaveBeenCalledWith(100, 'top');
    });

    it('搜索只在玩家视角下发出 q，公开榜不带', async () => {
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.boards).not.toBeNull());

        act(() => result.current.setBoardQuery('  孤 '));
        await waitFor(() => expect(result.current.boardQuery).toBe('  孤 '));
        // 前后空白要清掉，否则服务器会拿带空格的关键词去匹配。
        await waitFor(() =>
            expect(mocks.myBoard).toHaveBeenCalledWith('top', '孤')
        );
    });

    it('服务器不认 sort 时退回总榜并藏起切换', async () => {
        // 旧服务器 + 令牌没有 maps：走公开榜那条路。
        mocks.connection.mockResolvedValue(connectedWithoutMaps);
        // 不管要什么都回总榜，回答里没有 sort。
        mocks.boards.mockResolvedValue({
            board: 'maps',
            min: 1,
            maps: [],
            at: 1
        });
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.boards).not.toBeNull());

        act(() => result.current.setBoardSort('week'));
        await waitFor(() =>
            expect(result.current.boardSortSupported).toBe(false)
        );
        expect(result.current.boardSort).toBe('top');
    });

    it('地图被隐藏时后端返回 null，页面不显示卡片', async () => {
        mocks.myMap.mockResolvedValue(null);
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));
        expect(result.current.myMap).toBeNull();
    });

    it('地图可见时填充卡片数据', async () => {
        mocks.myMap.mockResolvedValue({
            world: 'wrld_x',
            name: '茶屋',
            board: true,
            pending: false,
            off: false,
            avg: 9.2,
            raters: 18,
            mine: 9,
            words: null,
            wordsOk: null,
            img: null,
            imgUrl: null
        });
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));
        expect(result.current.myMap?.avg).toBe(9.2);
    });

    it('openUser 只接受 usr_ 前缀', () => {
        const { result } = renderHook(() => useVrccController());

        act(() => result.current.openUser('not-a-user-id'));
        expect(mocks.openUserDialog).not.toHaveBeenCalled();

        act(() => result.current.openUser('usr_0123456789abcdef'));
        expect(mocks.openUserDialog).toHaveBeenCalledWith({
            userId: 'usr_0123456789abcdef'
        });
    });

    it('openWorld 空 id 不打开对话框', () => {
        const { result } = renderHook(() => useVrccController());

        act(() => result.current.openWorld(''));
        expect(mocks.openWorldDialog).not.toHaveBeenCalled();

        act(() => result.current.openWorld('wrld_abc'));
        expect(mocks.openWorldDialog).toHaveBeenCalledWith({
            worldId: 'wrld_abc'
        });
    });

    it('加载失败时同时写 console 与 toast', async () => {
        const consoleSpy = vi
            .spyOn(console, 'error')
            .mockImplementation(() => {});
        // 两条路都挂掉才算「加载失败」：只有玩家视角失败时会静默退回公开榜，
        // 那是降级不是错误（那条有单独的用例）。
        mocks.myBoard.mockRejectedValue(new Error('boom'));
        mocks.boards.mockRejectedValue(new Error('boom'));

        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));

        expect(consoleSpy).toHaveBeenCalled();
        expect(mocks.toastAdd).toHaveBeenCalled();
        const toastCall = mocks.toastAdd.mock.calls[0][0];
        expect(toastCall.type).toBe('error');
        // 错误也留在页面上，切页签回来还能看到。
        expect(result.current.error).not.toBeNull();

        consoleSpy.mockRestore();
    });

    it('切回模型榜复用缓存，不重新请求', async () => {
        // 离开页签几秒钟再回来，是很常见的动作。每次都重新拉整个榜，
        // 玩家看到的就是一屏加载中。60 秒内必须直接用手上的那份。
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));

        // 首屏拉了一次。
        await waitFor(() => expect(mocks.models).toHaveBeenCalledTimes(1));

        act(() => result.current.setTab('maps'));
        act(() => result.current.setTab('models'));

        // 切走再切回：仍然只有那一次请求。
        expect(mocks.models).toHaveBeenCalledTimes(1);
    });

    it('未连接时切到模型榜也走缓存', async () => {
        // 模型榜是开放数据，不需要连接。缓存复用那一行必须放在
        // `connected` 检查**之前**，否则未连接的玩家每次切页签都会重新请求。
        mocks.connection.mockResolvedValue({
            ...connected,
            connected: false
        });

        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.loading).toBe(false));
        await waitFor(() => expect(mocks.models).toHaveBeenCalledTimes(1));

        act(() => result.current.setTab('me'));
        act(() => result.current.setTab('models'));

        expect(mocks.models).toHaveBeenCalledTimes(1);
    });

    it('隔了一分钟以上再切回模型榜，必须重新请求', async () => {
        // 上一轮 review 留下的缺口：60 秒内复用有测试，**过期那一侧没有**。
        // 只测「不重新请求」的话，把复用做成永久缓存也能全绿——那样模型榜会
        // 永远停在打开页面的那一刻，而且没有一条测试看得出来。
        //
        // 只把 `Date` 换掉，不动 `setTimeout`：`waitFor` 靠真定时器轮询。
        vi.useFakeTimers({ toFake: ['Date'] });
        try {
            vi.setSystemTime(new Date('2026-01-01T00:00:00Z'));

            const { result } = renderHook(() => useVrccController());
            await waitFor(() => expect(result.current.loading).toBe(false));
            await waitFor(() => expect(mocks.models).toHaveBeenCalledTimes(1));

            // 61 秒之后：越过 60 秒的复用窗口。
            vi.setSystemTime(new Date('2026-01-01T00:01:01Z'));

            act(() => result.current.setTab('maps'));
            act(() => result.current.setTab('models'));

            await waitFor(() => expect(mocks.models).toHaveBeenCalledTimes(2));
        } finally {
            vi.useRealTimers();
        }
    });

    it('先点的排序后回来，不会盖掉后点的那个', async () => {
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.boards).not.toBeNull());

        // 本周榜卡在后端（例如在排队等限流），新上榜立刻就回。
        let deliverWeek: (board: unknown) => void = () => {};
        mocks.myBoard.mockImplementation((sort: string | null) =>
            sort === 'week'
                ? new Promise((resolve) => {
                      deliverWeek = resolve;
                  })
                : Promise.resolve({ sort: sort ?? 'top', min: 1, maps: [] })
        );
        act(() => result.current.setBoardSort('week'));
        act(() => result.current.setBoardSort('new'));
        await waitFor(() => expect(result.current.boardSort).toBe('new'));

        await act(async () => {
            deliverWeek({ sort: 'week', min: 1, maps: [] });
        });
        expect(result.current.boardSort).toBe('new');
    });

    it('断开后地图榜换回公开榜，不留上个账号的玩家视角', async () => {
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.boards?.source).toBe('mine'));

        mocks.disconnect.mockResolvedValue(undefined);
        mocks.connection.mockResolvedValue({
            ...connected,
            connected: false,
            star: null,
            vrcName: null,
            scope: null
        });
        await act(async () => {
            await result.current.disconnect();
        });

        await waitFor(() =>
            expect(result.current.boards?.source).toBe('public')
        );
        expect(mocks.boards).toHaveBeenCalled();
    });

    it('已经被取代的旧请求失败了，不把玩家视角判成用不了', async () => {
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.boards?.source).toBe('mine'));

        let failWeek: (error: unknown) => void = () => {};
        mocks.myBoard.mockImplementation((sort: string | null) =>
            sort === 'week'
                ? new Promise((_resolve, reject) => {
                      failWeek = reject;
                  })
                : Promise.resolve({ sort: sort ?? 'top', min: 1, maps: [] })
        );
        act(() => result.current.setBoardSort('week'));
        act(() => result.current.setBoardSort('new'));
        await waitFor(() => expect(result.current.boardSort).toBe('new'));

        mocks.boards.mockClear();
        await act(async () => {
            failWeek(new Error('network down'));
        });

        // 之后换排序仍走玩家视角，不降级到公开榜。
        act(() => result.current.setBoardSort('top'));
        await waitFor(() => expect(result.current.boardSort).toBe('top'));
        expect(result.current.boards?.source).toBe('mine');
        expect(mocks.boards).not.toHaveBeenCalled();
    });

    it('断开调用报错、但其实已经断开时，也不留着上个账号的玩家视角', async () => {
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.boards?.source).toBe('mine'));

        // 服务器错误时后端会先清本地会话、再把错误抛上来：调用失败了，
        // 重读到的连接却是「已断开」。公开榜也拉不下来。
        mocks.disconnect.mockRejectedValue(new Error('503 busy'));
        mocks.connection.mockResolvedValue({
            ...connected,
            connected: false,
            star: null,
            vrcName: null,
            scope: null
        });
        mocks.boards.mockRejectedValue(new Error('network down'));
        await act(async () => {
            await result.current.disconnect();
        });

        expect(result.current.connection?.connected).toBe(false);
        expect(result.current.boards?.source).not.toBe('mine');
        queryClient.clear();
    });

    it('断开调用报错、后端刻意留着令牌时，界面照旧显示还连着', async () => {
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.boards?.source).toBe('mine'));

        // 可重试的失败（网络不通）：后端留着令牌，重读到的仍是「已连接」。
        mocks.disconnect.mockRejectedValue(new Error('network down'));
        await act(async () => {
            await result.current.disconnect();
        });

        expect(result.current.connection?.connected).toBe(true);
        expect(result.current.boards?.source).toBe('mine');
        queryClient.clear();
    });

    it('已经被取代的公开榜请求失败了，不弹错误', async () => {
        mocks.connection.mockResolvedValue(connectedWithoutMaps);
        const { result } = renderHook(() => useVrccController());
        await waitFor(() =>
            expect(result.current.boards?.source).toBe('public')
        );

        let failWeek: (error: unknown) => void = () => {};
        mocks.boards.mockImplementation((_limit: number, sort: string | null) =>
            sort === 'week'
                ? new Promise((_resolve, reject) => {
                      failWeek = reject;
                  })
                : Promise.resolve({
                      board: 'maps',
                      sort: sort ?? 'top',
                      min: 1,
                      maps: [],
                      at: 1
                  })
        );
        act(() => result.current.setBoardSort('week'));
        act(() => result.current.setBoardSort('new'));
        await waitFor(() => expect(result.current.boardSort).toBe('new'));

        mocks.toastAdd.mockClear();
        await act(async () => {
            failWeek(new Error('network down'));
        });

        expect(mocks.toastAdd).not.toHaveBeenCalled();
        expect(result.current.error).toBeNull();
        expect(result.current.boardSort).toBe('new');
    });

    it('断开时公开榜拉不下来，也不留着上个账号的玩家视角', async () => {
        const { result } = renderHook(() => useVrccController());
        await waitFor(() => expect(result.current.boards?.source).toBe('mine'));

        mocks.disconnect.mockResolvedValue(undefined);
        mocks.connection.mockResolvedValue({
            ...connected,
            connected: false,
            star: null,
            vrcName: null,
            scope: null
        });
        mocks.boards.mockRejectedValue(new Error('network down'));
        await act(async () => {
            await result.current.disconnect();
        });

        expect(result.current.boards?.source).not.toBe('mine');
        // 世界 / 群组详情读的那份连接缓存也换成了断开后的。
        expect(
            queryClient.getQueryData<{ connected: boolean }>(
                queryKeys.vrccConnection()
            )?.connected
        ).toBe(false);
        queryClient.clear();
    });
});
