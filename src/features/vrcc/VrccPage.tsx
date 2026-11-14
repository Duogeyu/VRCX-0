// SPDX-License-Identifier: MPL-2.0

import {
    AlertCircle,
    ExternalLink,
    Megaphone,
    RefreshCw,
    Star,
    Unplug,
    X
} from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useSearchParams } from 'react-router';

import {
    PageBody,
    PageHeader,
    PageScaffold,
    PageTitle,
    PageToolbar
} from '@/components/layout/PageScaffold';
import { formatCompactDateTime } from '@/lib/dateTime';
import { scopeGrants } from '@/lib/vrccScope';
import { openExternalLink } from '@/services/shellIntegrationService';
import { toast } from '@/services/toastService';
import { useVrccTourActiveStore } from '@/state/vrccTourActiveStore';
import { Badge } from '@/ui/shadcn/badge';
import { Button } from '@/ui/shadcn/button';
import { Spinner } from '@/ui/shadcn/spinner';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/ui/shadcn/tabs';

import { announcementSummary } from './components/VrccAnnouncementBody';
import { VrccAnnouncementDialog } from './components/VrccAnnouncementDialog';
import { VrccCrossingsPanel } from './components/VrccCrossingsPanel';
import { VrccCurrentMapCard } from './components/VrccCurrentMapCard';
import { VrccMapsPanel } from './components/VrccMapsPanel';
import { VrccMePanel } from './components/VrccMePanel';
import { VrccModelsPanel } from './components/VrccModelsPanel';
import { VrccMutualsPanel } from './components/VrccMutualsPanel';
import { VrccNotificationsPanel } from './components/VrccNotificationsPanel';
import { useOpenExternalLink } from './useOpenExternalLink';
import { useVrccController, type VrccTab } from './useVrccController';
import {
    markSeenIds,
    normalizeLevel,
    readBarCursor,
    readSeenIds,
    selectPendingAnnouncements,
    writeBarCursor
} from './vrccAnnouncementStore';
import type { VrccAnnouncement } from './vrccApi';
import { countVrccCrossingsUnread, countVrccUnread } from './vrccUnreadCount';

/**
 * 页面**可能**出现的全部页签，含「星轨相交」。
 *
 * `crossings` 必须在这里：`VrccTab` 一直包含它，而这份名单在两条分支合并时
 * 曾经只剩四个，于是 `?tab=crossings` 永远被 `isVrccTab` 挡掉，下面那段
 * 「服务端要求隐藏时不落过去」的判断成了死代码。要不要真的显示由
 * `crossingsHidden` 决定，不在识别阶段决定。
 */
const VRCC_TABS: readonly VrccTab[] = [
    'maps',
    'models',
    'notifications',
    'mutuals',
    'crossings',
    'me'
];

/** 页面认的页签名。未知值一律忽略，不让它把界面切到一个不存在的页签。 */
function isVrccTab(value: string | null): value is VrccTab {
    return value !== null && (VRCC_TABS as readonly string[]).includes(value);
}

export function VrccPage() {
    const { t } = useTranslation();
    const {
        tab,
        setTab,
        loading,
        error,
        connection,
        usage,
        boards,
        boardSort,
        boardSortSupported,
        models,
        modelSort,
        modelSortSupported,
        modelsAvailable,
        recent,
        unread,
        notificationsView,
        notes,
        notesSupported,
        announcements,
        myMap,
        mutuals,
        mutualsError,
        crossings,
        crossingCards,
        crossingsLoading,
        crossingsError,
        crossingsNeedsScope,
        crossingsChecked,
        refreshCrossings,
        service,
        deviceAuth,
        deviceAuthBusy,
        deviceAuthMessage,
        deviceAuthCooldown,
        reload,
        refreshBoards,
        setBoardSort,
        refreshModels,
        setModelSort,
        setNotificationsView,
        beginDeviceAuth,
        disconnect,
        openWorld,
        openUser,
        openGroup
    } = useVrccController();

    const connected = connection?.connected ?? false;
    const unreadTotal = connected ? countVrccUnread(unread, recent) : 0;
    // 星轨相交自己那一页的红点。它**也**含在 `unreadTotal` 里（那是侧边栏、
    // 腕表、托盘共用的合计，和 Rust 的 `VrccUnread::total()` 同口径），
    // 这里只是给这一页补一个入口角标——未读是这个页签自己的，玩家不该为了
    // 发现它去翻「提醒」。
    const crossingsUnread = connected ? countVrccCrossingsUnread(unread) : 0;
    // 服务端说这块不该显示时（16–17 岁星池，或玩家关了星轨相交），页签也
    // 一起藏起来，并顺手把停留在这页的玩家送回地图榜——否则会出现「所有
    // 页签都不选中、内容空白」的状态。
    //
    // 三个条件取「或」，任何一个说不行就不显示入口：
    //
    // 1. **令牌有没有 `crossings` 权限**——没给的话连请求都发不出去
    //    （`403 scope`），等响应再藏页签等于让玩家先看到入口、点进去才被告知
    //    不行。令牌的 scope 在 `connection` 里，是本地纯函数，不用等网络。
    // 2. **还没问过**（`!crossingsChecked`）——`hide` 只有问过才知道，没问过
    //    就当「不知道」，先不显示。探测失败也停在这里，不会因为一次网络抖动
    //    就把入口放出来；页面刷新会重试。
    // 3. **服务端说 hidden**。
    const crossingsHidden =
        !scopeGrants(connection?.scope, 'crossings') ||
        !crossingsChecked ||
        crossings?.hidden === true;

    // 从通知中心点进来时带的页签名。
    const [searchParams, setSearchParams] = useSearchParams();
    const requestedTab = searchParams.get('tab');
    // 用 ref 读隐藏状态：`crossingsHidden` 是每次渲染新算的布尔值，直接进
    // 依赖数组会让这个 effect 在星轨相交数据回来时白跑一遍（那时页面已经
    // 落好页签、参数也已经清掉，行为其实不变）。
    const crossingsHiddenRef = useRef(crossingsHidden);
    crossingsHiddenRef.current = crossingsHidden;
    useEffect(() => {
        if (!isVrccTab(requestedTab)) {
            return;
        }
        // **不需要连接就能看的页签要立刻落过去**：地图榜是开放数据，「我的」
        // 页正是连接入口本身。首连引导的「现在连接」就靠 `?tab=me` 把玩家
        //送到连接卡片前面——早先这里对未连接一律 return，引导点下去会停在
        // 默认页签、参数还留在地址栏里，等哪天连上了才突然跳走。
        //
        // 其余页签仍然等连接就绪：`setTab` 在未连接时会跳过「提醒」页的已读
        // 同步，先落过去就是落过去了而同步没发生。
        const reachableNow =
            connected || requestedTab === 'maps' || requestedTab === 'me';
        if (!reachableNow) {
            return;
        }
        // 「星轨相交」被服务端要求隐藏时不能落过去：那个页签根本不存在，
        // 落过去就是一个谁都没选中、内容空白的页面。首帧时数据还没回来，
        // 现在落到别处反而会误伤——那种情况由下面的 `activeTab` 兜底。
        const hidden =
            requestedTab === 'crossings' && crossingsHiddenRef.current;
        if (!hidden) {
            setTab(requestedTab);
        }
        // 参数用掉就清掉，而且是**无论落没落过去都清**。留着的后果有两个：
        // 玩家手动切页签时会被它拽回去；以及——`requestedTab` 一直是同一个
        // 值，后续同一条深链（再点一次同一条通知）就再也触发不了这个 effect。
        // 早先靠一个 `applied` 布尔去防重入，结果第二次深链被永久吞掉。
        const remaining = new URLSearchParams(searchParams);
        remaining.delete('tab');
        setSearchParams(remaining, { replace: true });
    }, [connected, requestedTab, searchParams, setSearchParams, setTab]);
    const activeTab = crossingsHidden && tab === 'crossings' ? 'maps' : tab;
    const stop = announcements?.stop ?? null;

    /**
     * 页内的公告：`bar` 走页顶提示、`pop` 走页面内弹窗。
     *
     * `force` 不在这里处理——它归 `VrccAnnouncementHost` 全局弹，同一份内容
     * 不该因为页面也打开而弹两次。`pop` 也不用全局：那属于「需知」（新功能、
     * 活动），全局弹就是打扰。
     *
     * 已读按 `level` 分开记：`bar` 是水位线（关掉即连同更早的一起不显示），
     * `pop` 是逐条 id（补发一条旧编号的更正不会因此被吞掉）。
     */
    const [barCursor, setBarCursor] = useState<number | null>(() =>
        readBarCursor()
    );
    const [seenIds, setSeenIds] = useState<ReadonlySet<number>>(() =>
        readSeenIds()
    );

    const pageAnnouncements = selectPendingAnnouncements(announcements?.posts, {
        barCursor,
        seenIds,
        scope: 'page'
    });
    // 页顶 tips 只放 `bar`：那是最轻的一档，一条提示就够。
    const barPost =
        pageAnnouncements.find(
            (post) => normalizeLevel(post.level) === 'bar'
        ) ?? null;
    // `pop` 进弹窗序列，可 `‹ 1/3 ›` 翻。
    const pendingPopPosts = pageAnnouncements.filter(
        (post) => normalizeLevel(post.level) === 'pop'
    );

    /**
     * **导览在跑时，`pop` 公告先挂起。**
     *
     * 优先级是一条队列，不是压栈：
     * ```
     * force 公告  →  挂起导览（见 shouldAutoOpenFirstRun）
     * 导览        →  挂起 pop 公告（这里）
     * ```
     * 同一时刻只让一样东西对玩家说话。实测过的故障：导览走到第 4/18 步
     * 「四个页签」时，页面挂载初期拉的 `pop` 公告盖了上来（「加群领见面礼」），
     * 一边是遮罩里指着页签条，一边是另一个带分页的弹窗，玩家不知道该看哪个。
     *
     * **挂起不是丢弃**：`posts` 传空数组给弹窗，它自然不渲染；等导览结束、
     * `active` 变回 false，下一帧照常弹。已读状态全程没动——挂起期间不记
     * 已读，所以不会把玩家还没看过的公告吞掉。
     */
    const tourActive = useVrccTourActiveStore((state) => state.active);
    const popPosts = tourActive ? [] : pendingPopPosts;

    const dismissBar = (id: number) => {
        writeBarCursor(id);
        setBarCursor(readBarCursor());
    };
    const dismissPop = (id: number) => {
        markSeenIds([id]);
        setSeenIds(readSeenIds());
    };
    const skipAllPop = (ids: readonly number[]) => {
        markSeenIds(ids);
        setSeenIds(readSeenIds());
    };
    // 页面级的「打开链接」：走共享 hook，保证点击立刻有状态、成功有回执。
    // 以前成功时静默，玩家只能等浏览器自己跳出来——期间会重复点。
    const { open: openLinkExternally } = useOpenExternalLink();
    const openLink = (url: string) => {
        // 走系统的 `app__open_link`，不要用 `window.open`：Tauri 的 WebView
        // 会拦掉 `window.open` 且什么都不做，按钮点了没反应。
        openLinkExternally(url);
    };
    // 只在这三种情况下才算「服务不可用」：没有状态对象、state 不是 ok、
    // 或后端明确报告了故障。
    //
    // 不能写成 `service?.state !== 'ok'`：`service` 初值是 `null`，
    // `null?.state` 是 `undefined`，`undefined !== 'ok'` 为真——于是请求
    // 还没发出去，徽章就先亮「服务不可用」，玩家看到的是一个凭空出现的故障。
    // 「还不知道」和「已经坏了」是两件事。
    const serviceDown = service !== null && service.state !== 'ok';

    return (
        <PageScaffold>
            <PageHeader>
                <div className="flex min-w-0 items-center gap-2">
                    <Star className="text-muted-foreground size-4 shrink-0" />
                    <PageTitle>{t('vrcc.title')}</PageTitle>
                    {connected ? (
                        <Badge variant="outline" className="shrink-0">
                            #{connection?.star ?? '—'}
                        </Badge>
                    ) : (
                        <Badge variant="secondary" className="shrink-0">
                            {t('vrcc.not_connected')}
                        </Badge>
                    )}
                    {serviceDown && (
                        <Badge variant="destructive" className="shrink-0">
                            {t('vrcc.service_down')}
                        </Badge>
                    )}
                    {unreadTotal > 0 && (
                        <Badge className="shrink-0">{unreadTotal}</Badge>
                    )}
                </div>
                <PageToolbar>
                    <Button
                        variant="ghost"
                        size="icon"
                        onClick={() => void reload()}
                        disabled={loading}
                        aria-label={t('common.actions.refresh')}
                    >
                        <RefreshCw
                            className={
                                loading ? 'size-4 animate-spin' : 'size-4'
                            }
                        />
                    </Button>
                </PageToolbar>
            </PageHeader>

            <PageBody>
                {/* 停站维护（`announcements` 的 `stop`）：个人接口这时回答
                    `503 stop`，先把原因和预计恢复时间摆出来。 */}
                {stop && (
                    <div className="bg-muted/50 mb-3 flex items-start gap-2 rounded-lg border p-3 text-sm">
                        <AlertCircle className="mt-0.5 size-4 shrink-0" />
                        <div className="min-w-0">
                            <div className="font-medium">
                                {t('vrcc.stop.title')}
                            </div>
                            {stop.say && (
                                <div className="text-muted-foreground text-xs break-words whitespace-pre-line">
                                    {stop.say}
                                </div>
                            )}
                            {stop.until != null && (
                                <div className="text-muted-foreground text-xs">
                                    {t('vrcc.stop.until', {
                                        time: formatCompactDateTime(stop.until)
                                    })}
                                </div>
                            )}
                        </div>
                    </div>
                )}

                {barPost && (
                    <VrccAnnouncementBar
                        post={barPost}
                        onOpen={openLink}
                        onDismiss={dismissBar}
                    />
                )}

                {/* `pop`：页面内弹窗，可 `‹ 1/3 ›` 翻。 */}
                <VrccAnnouncementDialog
                    posts={popPosts}
                    onAdvance={dismissPop}
                    onSkipAll={skipAllPop}
                />

                {error && (
                    <div className="border-destructive/40 bg-destructive/10 text-destructive mb-3 flex items-start gap-2 rounded-lg border p-3 text-sm">
                        <AlertCircle className="mt-0.5 size-4 shrink-0" />
                        <div className="min-w-0">
                            <div className="break-words">{error.message}</div>
                            {error.kind === 'needsReauth' && (
                                <Button
                                    variant="link"
                                    size="sm"
                                    className="h-auto p-0 text-xs"
                                    onClick={() => setTab('me')}
                                >
                                    {t('vrcc.reconnect_hint')}
                                </Button>
                            )}
                            {error.kind === 'missingScope' && error.scope && (
                                <div className="text-muted-foreground text-xs">
                                    {t('vrcc.missing_scope', {
                                        scope: error.scope
                                    })}
                                </div>
                            )}
                        </div>
                    </div>
                )}

                {loading && !boards ? (
                    <div className="text-muted-foreground flex h-40 items-center justify-center gap-2 text-sm">
                        <Spinner className="size-4" />
                        {t('common.loading', '加载中…')}
                    </div>
                ) : (
                    <Tabs
                        value={activeTab}
                        onValueChange={(value) => setTab(value as typeof tab)}
                        className="flex min-h-0 flex-1 flex-col"
                    >
                        <TabsList data-tour="vrcc-tabs" className="shrink-0">
                            <TabsTrigger value="maps">
                                {t('vrcc.tab.maps')}
                            </TabsTrigger>
                            {/* 模型榜排在「提醒」前面：它和地图榜同属开放数据、
                                不需要连接，两个浏览型页签放一起，个人内容往后排。 */}
                            <TabsTrigger value="models">
                                {t('vrcc.tab.models')}
                            </TabsTrigger>
                            <TabsTrigger value="notifications">
                                {t('vrcc.tab.notifications')}
                                {unreadTotal > 0 && (
                                    <span className="bg-primary/15 text-primary ml-1.5 rounded-full px-1.5 text-[10px] leading-4">
                                        {unreadTotal}
                                    </span>
                                )}
                            </TabsTrigger>
                            <TabsTrigger value="mutuals">
                                {t('vrcc.tab.mutuals')}
                            </TabsTrigger>
                            {/* `hidden` 时**连页签都不出现**：面板自己返回
                                null 只藏住了内容，标签还写着「星轨相交」，
                                等于照样确认了「这个功能存在、只是没给你」。
                                官方要求的是客户端不显示此模块。 */}
                            {!crossingsHidden && (
                                <TabsTrigger value="crossings">
                                    {t('vrcc.tab.crossings')}
                                    {/* 红点挂在**自己**的页签上：那几条未读
                                        就在这一页里，挂到「提醒」上玩家
                                        点过去会找不到。 */}
                                    {crossingsUnread > 0 && (
                                        <span className="bg-primary/15 text-primary ml-1.5 rounded-full px-1.5 text-[10px] leading-4">
                                            {crossingsUnread}
                                        </span>
                                    )}
                                </TabsTrigger>
                            )}
                            <TabsTrigger value="me">
                                {t('vrcc.tab.me')}
                            </TabsTrigger>
                        </TabsList>

                        <TabsContent value="maps" className="min-h-0 flex-1">
                            {/* 所在地图卡片放在榜单上方：玩家最关心的是
                                «我现在这张图怎么样»，而不是全站排行。
                                `off` 是硬阻断：服务层不再把它折成 null，
                                所以这里必须自己判断——官方要求此时隐藏
                                这张图的卡片。 */}
                            {myMap && myMap.off !== true && (
                                <VrccCurrentMapCard
                                    map={myMap}
                                    onOpenWorld={openWorld}
                                />
                            )}
                            <VrccMapsPanel
                                boards={boards}
                                sort={boardSort}
                                sortSupported={boardSortSupported}
                                onSortChange={setBoardSort}
                                onRefresh={() => void refreshBoards()}
                                onOpenWorld={openWorld}
                            />
                        </TabsContent>

                        <TabsContent value="models" className="min-h-0 flex-1">
                            <VrccModelsPanel
                                models={models}
                                sort={modelSort}
                                sortSupported={modelSortSupported}
                                available={modelsAvailable}
                                onSortChange={setModelSort}
                                onRefresh={() => void refreshModels()}
                            />
                        </TabsContent>

                        <TabsContent
                            value="notifications"
                            className="min-h-0 flex-1"
                        >
                            <VrccNotificationsPanel
                                connected={connected}
                                recent={recent}
                                unread={unread}
                                view={notificationsView}
                                notes={notes}
                                notesSupported={notesSupported}
                                crossingsHidden={crossingsHidden}
                                onViewChange={setNotificationsView}
                                onConnect={() => setTab('me')}
                                onOpenWorld={openWorld}
                            />
                        </TabsContent>

                        <TabsContent value="mutuals" className="min-h-0 flex-1">
                            <VrccMutualsPanel
                                connected={connected}
                                mutuals={mutuals}
                                mutualsError={mutualsError}
                                onOpenUser={openUser}
                                onOpenGroup={openGroup}
                            />
                        </TabsContent>
                        <TabsContent
                            value="crossings"
                            className="min-h-0 flex-1"
                        >
                            <VrccCrossingsPanel
                                connected={connected}
                                crossings={crossings}
                                cards={crossingCards}
                                loading={crossingsLoading}
                                crossingsError={crossingsError}
                                crossingsNeedsScope={crossingsNeedsScope}
                                onOpenLink={openLink}
                                onRefresh={() => {
                                    // 手动刷新不算「看过」：这一下可能只是
                                    // 想看看有没有新记录，不该顺手替玩家
                                    // 把红点清掉。`mark=true` 只留给「切到
                                    // 这一页」那一个动作（见 `setTab`）。
                                    void refreshCrossings(false).catch(
                                        (error: unknown) => {
                                            toast.add({
                                                type: 'error',
                                                title:
                                                    error instanceof Error
                                                        ? error.message
                                                        : t(
                                                              'vrcc.error.unknown'
                                                          )
                                            });
                                        }
                                    );
                                }}
                            />
                        </TabsContent>

                        <TabsContent value="me" className="min-h-0 flex-1">
                            <VrccMePanel
                                connection={connection}
                                usage={usage}
                                deviceAuth={deviceAuth}
                                deviceAuthBusy={deviceAuthBusy}
                                deviceAuthMessage={deviceAuthMessage}
                                deviceAuthCooldown={deviceAuthCooldown}
                                onBegin={beginDeviceAuth}
                                onDisconnect={disconnect}
                                onOpenLink={openLink}
                            />
                        </TabsContent>
                    </Tabs>
                )}
            </PageBody>
        </PageScaffold>
    );
}

/**
 * 页顶的 `bar` 公告：一行提示，正文取摘要。
 *
 * `bar` 是三种里最轻的一种，本来就只该占一条。完整正文留给 `pop`/`force`
 * 的弹窗——那条 765 字、8 个栏目的更新说明塞进一行横条既读不了也排不下。
 */
function VrccAnnouncementBar({
    post,
    onOpen,
    onDismiss
}: {
    post: VrccAnnouncement;
    onOpen: (url: string) => void;
    onDismiss: (id: number) => void;
}) {
    const { t } = useTranslation();
    const link = (post.link ?? '').trim();
    const summary = announcementSummary(post.body ?? null);
    return (
        <div className="mb-3 flex items-start gap-2 rounded-lg border px-3 py-2 text-sm">
            <Megaphone className="text-muted-foreground mt-0.5 size-4 shrink-0" />
            <div className="min-w-0 flex-1">
                <span className="font-medium">{post.title}</span>
                {summary !== '' && (
                    <span className="text-muted-foreground ml-2 text-xs">
                        {summary}
                    </span>
                )}
            </div>
            {link !== '' && (
                <Button
                    variant="link"
                    size="sm"
                    className="h-auto shrink-0 p-0 text-xs"
                    onClick={() => onOpen(link)}
                >
                    {t('vrcc.announcement.open')}
                </Button>
            )}
            <Button
                variant="ghost"
                size="icon"
                className="size-6 shrink-0"
                onClick={() => onDismiss(post.id ?? 0)}
                aria-label={t('vrcc.announcement.dismiss')}
            >
                <X className="size-3" />
            </Button>
        </div>
    );
}

/**
 * 页面里用来打开 vrcc.cc 的小按钮，多处复用。
 *
 * **不要用 `<a target="_blank">`**：Tauri 的 WebView 处理 `target=_blank`
 * 走的是和 `window.open` 同一条新窗口路径，那里什么都不做（或者弹出一个
 * 不受管理的窗口），同时还会绕过 `app__open_link` 的 scheme 校验。
 * 统一走 `openExternalLink`，失败时由调用方的 toast 兜住。
 */
export function VrccExternalLink({
    href,
    children,
    onError
}: {
    href: string;
    children: React.ReactNode;
    /** 打开失败时的回调（一般是弹 toast）。不传就只记日志。 */
    onError?: (error: unknown) => void;
}) {
    // 走共享 hook：即时 pending 状态 + 成功/失败反馈。
    // 以前成功时什么都不做——点了没反应，玩家就会重复点。
    const { open: openLink, pending } = useOpenExternalLink();
    return (
        <button
            type="button"
            disabled={pending}
            onClick={() => {
                if (onError) {
                    // 调用方自己管失败提示，这里只保证即时状态。
                    void openExternalLink(href).catch((error: unknown) => {
                        onError(error);
                    });
                    return;
                }
                openLink(href);
            }}
            className="text-primary inline-flex items-center gap-1 text-xs hover:underline"
        >
            {children}
            <ExternalLink className="size-3" />
        </button>
    );
}

/** 断开按钮的图标在「我的」页里复用。 */
export const VrccUnplugIcon = Unplug;
