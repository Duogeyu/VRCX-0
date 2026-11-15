// SPDX-License-Identifier: MPL-2.0

// @vitest-environment jsdom

import { act, cleanup, render } from '@testing-library/react';
import type { PropsWithChildren } from 'react';
import { MemoryRouter, useNavigate, useSearchParams } from 'react-router';
import { afterEach, describe, expect, it, vi } from 'vitest';

/**
 * `?tab=` 深链的解析规则。
 *
 * 这一组用例守的是**合并缝**上的两个缺陷：
 *
 * 1. 页签名单里曾经漏掉 `crossings`，于是 `?tab=crossings` 被当成未知值，
 *    而「服务端要求隐藏时不要落过去」那条判断因此永远不会被执行——
 *    看着像是有防护，其实是死代码。
 * 2. 应用过之后用一个 `applied` 布尔挡住重入，而它**永不复位**：页面在
 *    `/vrcc` 上本来就保持挂载，第二次深链（再点一条通知）就被永久吞掉。
 */

const controller = vi.hoisted(() => ({
    tab: 'maps',
    setTab: vi.fn(),
    connected: true,
    hidden: false,
    checked: true,
    // 默认给足权限；要测「没开这个功能」的用例自己改。
    scope: 'profile notifications maps mutuals crossings' as string | null
}));

vi.mock('./useVrccController', () => ({
    useVrccController: () => ({
        tab: controller.tab,
        setTab: controller.setTab,
        loading: false,
        error: null,
        connection: {
            connected: controller.connected,
            scope: controller.scope
        },
        usage: null,
        boards: null,
        boardSort: 'top',
        boardQuery: '',
        boardSortSupported: true,
        recent: null,
        unread: null,
        notificationsView: 'recent',
        notes: null,
        notesSupported: null,
        announcements: null,
        stickers: null,
        myMap: null,
        mutuals: null,
        mutualsError: null,
        crossings: controller.hidden ? { stars: [], hidden: true } : null,
        crossingCards: null,
        crossingsError: null,
        crossingsChecked: controller.checked,
        refreshCrossings: vi.fn(),
        service: null,
        deviceAuth: null,
        deviceAuthBusy: false,
        deviceAuthMessage: null,
        deviceAuthCooldown: 0,
        reload: vi.fn(),
        refreshBoards: vi.fn(),
        setBoardSort: vi.fn(),
        setBoardQuery: vi.fn(),
        setNotificationsView: vi.fn(),
        beginDeviceAuth: vi.fn(),
        disconnect: vi.fn(),
        openWorld: vi.fn(),
        openUser: vi.fn(),
        openGroup: vi.fn()
    })
}));

/**
 * 页签容器只当一层透明外壳。
 *
 * 真的 `Tabs` 是 base-ui 组件，卸载时不会调用 `onValueChange`；这里替掉是
 * 为了把断言集中在「页面把哪个值交给了 `setTab`」，不被组件内部状态干扰。
 */
vi.mock('@/ui/shadcn/tabs', () => ({
    Tabs: ({ children }: PropsWithChildren) => <div>{children}</div>,
    TabsList: ({ children }: PropsWithChildren) => <div>{children}</div>,
    TabsTrigger: ({ children }: PropsWithChildren) => <div>{children}</div>,
    TabsContent: ({ children }: PropsWithChildren) => <div>{children}</div>
}));

// 各面板只关心「页面把页签落在哪」，不关心内容，全部替成空。
vi.mock('./components/VrccMapsPanel', () => ({
    VrccMapsPanel: () => null
}));
vi.mock('./components/VrccNotificationsPanel', () => ({
    VrccNotificationsPanel: () => null
}));
vi.mock('./components/VrccMutualsPanel', () => ({
    VrccMutualsPanel: () => null
}));
vi.mock('./components/VrccCrossingsPanel', () => ({
    VrccCrossingsPanel: () => null
}));
vi.mock('./components/VrccMePanel', () => ({
    VrccMePanel: () => null
}));
vi.mock('./components/VrccCurrentMapCard', () => ({
    VrccCurrentMapCard: () => null
}));

import { VrccPage } from './VrccPage';

afterEach(() => {
    cleanup();
    controller.tab = 'maps';
    controller.connected = true;
    controller.hidden = false;
    controller.checked = true;
    controller.scope = 'profile notifications maps mutuals crossings';
    controller.setTab.mockReset();
});

/** 地址栏当前的样子：查询串 + 一个供用例调用的 `navigate`。 */
const view = {
    search: '',
    navigate: (_to: string) => {}
};

function RouteSpy() {
    const [params] = useSearchParams();
    view.search = params.toString();
    view.navigate = useNavigate();
    return null;
}

function renderPage(initialEntry: string) {
    return render(
        <MemoryRouter initialEntries={[initialEntry]}>
            <VrccPage />
            <RouteSpy />
        </MemoryRouter>
    );
}

/** 页面已经挂载的情况下，从别处（通知中心）再跳一次深链。 */
async function deepLink(to: string) {
    await act(async () => {
        view.navigate(to);
    });
}

describe('VrccPage 的 ?tab= 深链', () => {
    it('落到请求的页签，并把参数清掉', () => {
        renderPage('/vrcc?tab=notifications');

        expect(controller.setTab).toHaveBeenCalledWith('notifications');
        expect(view.search).not.toContain('tab=');
    });

    it('同一次挂载里，第二条深链仍然生效', async () => {
        // 页面在 `/vrcc` 上不卸载，玩家看完「提醒」又点了一条「互相连上」的
        // 通知。早先用一个永不复位的 `applied` 布尔挡重入，这一次会被吞掉。
        renderPage('/vrcc');
        await deepLink('/vrcc?tab=notifications');
        expect(controller.setTab).toHaveBeenLastCalledWith('notifications');

        await deepLink('/vrcc?tab=mutuals');
        expect(controller.setTab).toHaveBeenLastCalledWith('mutuals');
    });

    it('认得出 crossings 这个页签名', () => {
        // 名单里漏掉它时这条会失败：`isVrccTab` 把值挡掉，一个 setTab 都不发。
        renderPage('/vrcc?tab=crossings');

        expect(controller.setTab).toHaveBeenCalledWith('crossings');
    });

    it('服务端要求隐藏 crossings 时不落过去，但参数照样清掉', () => {
        controller.hidden = true;
        renderPage('/vrcc?tab=crossings');

        expect(controller.setTab).not.toHaveBeenCalled();
        // 留着参数会被下一次 effect 反复消费，也会拽住玩家手动的页签切换。
        expect(view.search).not.toContain('tab=');
    });

    /**
     * **还没问过**（探测没回来、或探测失败）时，入口也不显示。
     *
     * `hide` 只有问过才知道，所以「不知道」必须当成「不行」。失败就放行
     * 等于把这条还回去：`hide` 的玩家会在网络抖动时看到入口，而那正是官方
     * 要求避免的那件事。页面刷新会重试探测。
     */
    it('可见性还没确认时，深链也不落到这一页', () => {
        controller.checked = false;
        renderPage('/vrcc?tab=crossings');

        expect(controller.setTab).not.toHaveBeenCalled();
    });

    /**
     * 令牌里没有 `crossings` 时，深链也不该落过去。
     *
     * 这是**本地就能判**的事：没给权限的话请求根本发不出去（`403 scope`），
     * 等响应回来再藏入口等于让玩家先看到、点进去才被告知不行。
     */
    it('令牌缺 crossings 权限时，深链不落到这一页', () => {
        controller.scope = 'profile notifications maps mutuals';
        renderPage('/vrcc?tab=crossings');

        expect(controller.setTab).not.toHaveBeenCalled();
    });

    it('未连接时先不落页签，参数也不消费', () => {
        // `setTab` 在未连接时自己会返回，那时「提醒」页的已读同步也一并跳过，
        // 页签落过去而同步没发生。等连上再落。
        controller.connected = false;
        renderPage('/vrcc?tab=notifications');

        expect(controller.setTab).not.toHaveBeenCalled();
        expect(view.search).toContain('tab=notifications');
    });

    /**
     * 两个页签**不需要连接**，未连接时也要立刻落过去。
     *
     * 「我的」是连接入口本身：首连引导的「现在连接」就靠 `?tab=me` 把玩家
     * 送到连接卡片前面。早先这里对未连接一律 return，引导点下去会停在默认
     * 页签、参数还留在地址栏，等哪天连上了再突然跳走。地图榜是开放数据，
     * 同理。
     */
    it('未连接时，我的页和地图榜仍然立刻落过去', () => {
        controller.connected = false;
        for (const tab of ['me', 'maps'] as const) {
            vi.clearAllMocks();
            renderPage(`/vrcc?tab=${tab}`);

            expect(controller.setTab).toHaveBeenCalledWith(tab);
            expect(view.search).not.toContain('tab=');
        }
    });

    it('不认识的页签名一律忽略（也不清掉参数）', () => {
        renderPage('/vrcc?tab=nope');

        expect(controller.setTab).not.toHaveBeenCalled();
        expect(view.search).toContain('tab=nope');
    });
});
