// SPDX-License-Identifier: MPL-2.0

import { useCallback, useEffect, useRef, useState } from 'react';

import { tauriClient } from '@/platform/tauri/client';
import { copyTextToClipboard } from '@/services/clipboardService';
import {
    openGroupDialog,
    openUserDialog,
    openWorldDialog
} from '@/services/dialogService';
import i18n from '@/services/i18nService';
import { openExternalLink } from '@/services/shellIntegrationService';
import { toast } from '@/services/toastService';
import { useRuntimeStore } from '@/state/runtimeStore';
import { useVrccUnreadStore } from '@/state/vrccUnreadStore';
import { useVrcNotificationStore } from '@/state/vrcNotificationStore';

import { resetModelCoverCache } from './useModelCoverCache';
import { resetModelDetailCache } from './useModelDetail';
import { loadAnnouncements } from './vrccAnnouncementLoader';
import {
    vrccApi,
    VrccError,
    type VrccAnnouncements,
    type VrccBoard,
    type VrccBoardSort,
    type VrccConnection,
    type VrccCrossingCards,
    type VrccCrossings,
    type VrccDeviceAuthPrompt,
    type VrccModelBoard,
    type VrccMutuals,
    type VrccMyMap,
    type VrccNotes,
    type VrccRecentNotifications,
    type VrccStatus,
    type VrccUnread,
    type VrccUsage
} from './vrccApi';
import { countVrccUnread } from './vrccUnreadCount';

/**
 * 后端提醒轮询发出的事件名，与 Rust 侧
 * `runtime-host-desktop::vrcc::VRCC_NOTICES_CHANGED_EVENT` 必须一致。
 */
const VRCC_NOTICES_CHANGED_EVENT = 'vrccNoticesChanged';

/**
 * 令牌被服务器判了失效（`404 gone`）、后端已经清掉它时发出的事件名，
 * 与 Rust 侧 `VRCC_CONNECTION_CHANGED_EVENT` 必须一致。
 */
const VRCC_CONNECTION_CHANGED_EVENT = 'vrccConnectionChanged';

/**
 * 同一种排序的地图榜多久内不重新请求。
 *
 * 服务器最多 5 分钟更新一次；后端对同一种排序还有 60 秒的最小间隔（不到
 * 60 秒再问会等着）。切回刚看过的排序直接用手上的，不让玩家干等。
 */
const BOARD_REUSE_MS = 60_000;

/**
 * 两次「申请连接码」之间至少隔多久（秒）。
 *
 * 与后端 `device_code` 的 `min_gap` 一致。后端那一层是**发请求前等 5 秒**，
 * 而这里挡的是**界面上的连点**：那份额度按网络出口 IP 每天只有 30 个，
 * 同一 NAT 后所有人共用，点一次就真的消耗服务端一次计数。让按钮在冷却期间
 * 不可点，玩家就不会用连点去赌「也许这次能过」。
 */
const DEVICE_AUTH_COOLDOWN_SECS = 5;

/**
 * 把结构化错误翻译成一句玩家看得懂的话。
 *
 * **为什么不直接用 `error.message`**：那是后端给的原文，传输层失败时里面是
 * `error sending request for url (…)` 这种英文技术串，玩家既看不懂，也判断
 * 不了该查网络还是该重试。
 *
 * 现在按 `layer` + `transportKind` 出文案：
 * - `input`：我们自己的 bug，只说「操作失败」，细节留给日志
 * - `transport`：告诉玩家卡在哪一步（DNS / 连不上 / 超时 / 代理）
 * - `session` / `server`：沿用后端的描述，那些本来就是给人看的
 *
 * **`server` 层里有一处例外**：`429 too_many` 打在 `device_code` 上时，额度
 * 是按**网络出口 IP** 算的（每天 30 个，同一路由下所有人共用），和别人账号的
 * 额度不是一回事。这句话早先写在 Rust 里，重构后只保留了错误码，所以这里按
 * `op` 补回来——`op` 一路从 `HttpDetail` 带过来，正是为了让这种「同一个码在
 * 不同接口下含义不同」的地方有地方判。
 *
 * **不假装能分辨「被封」**：RST、端口拒绝、运营商拦截在系统层面一模一样，
 * 都是建连失败。文案写「可能被拦截」，不写「已被拦截」。
 */
export function describeVrccError(error: VrccError): string {
    if (error.layer === 'input') {
        // 不该发生。真发生了说明我们构造请求出了错，玩家帮不上忙。
        return i18n.t('vrcc.error.unknown');
    }

    if (error.layer === 'transport') {
        const kind = error.transportKind;
        if (kind === 'timeout') {
            return i18n.t('vrcc.error.network.timeout');
        }
        // DNS 与建连对玩家的意义不同：前者换 DNS/换网络可能好，后者多半要开代理。
        // 注意走 SOCKS5 代理时 DNS 由代理解析，这里只会看到 `connect`。
        if (kind === 'connect') {
            return i18n.t('vrcc.error.network.connect');
        }
        if (kind === 'dns') {
            return i18n.t('vrcc.error.network.dns');
        }
        return i18n.t('vrcc.error.network.generic');
    }

    // 出口 IP 维度的额度：说成「你自己的次数用完了」会让玩家去查账号。
    if (error.kind === 'throttled' && error.op === 'device_code') {
        return i18n.t('vrcc.error.device_code_quota');
    }

    return error.message || i18n.t('vrcc.error.unknown');
}

export type VrccTab =
    | 'maps'
    | 'models'
    | 'notifications'
    | 'mutuals'
    | 'crossings'
    | 'me';

/** 「提醒」页签里看哪一个：最近的提醒，还是通知中心。 */
export type VrccNotificationsView = 'recent' | 'notes';

type State = {
    tab: VrccTab;
    connection: VrccConnection | null;
    usage: VrccUsage | null;
    boards: VrccBoard | null;
    /** 正在看的地图榜排序。 */
    boardSort: VrccBoardSort;
    /**
     * 服务器认不认 `sort`（1.13.0 起）。要了本周榜却拿回没有 `sort` 的总榜时
     * 为 false，界面藏起切换。
     */
    boardSortSupported: boolean;
    /** 模型榜（1.17.0 起）。不需要连接。 */
    models: VrccModelBoard | null;
    /** 正在看的模型榜排序。 */
    modelSort: VrccBoardSort;
    /** 服务器认不认模型榜的 `sort`；语义同 `boardSortSupported`。 */
    modelSortSupported: boolean;
    /**
     * 模型榜这台服务器上到底有没有（1.17.0 才有的接口）。
     *
     * 旧服务器回 `404 not_found`、后端收敛成 `null`，那是**「还没有这个功能」**
     * 而不是故障：界面把这一页显示成「暂时没有」，不弹错误横幅。
     */
    modelsAvailable: boolean;
    recent: VrccRecentNotifications | null;
    /** 几处未读数；null 表示还没拿到，或服务器还没有 `me/unread`。 */
    unread: VrccUnread | null;
    notificationsView: VrccNotificationsView;
    /** 通知中心；null 表示还没拉过，或服务器还没有 `me/notes`。 */
    notes: VrccNotes | null;
    /** 服务器有没有通知中心；还不知道时为 null。 */
    notesSupported: boolean | null;
    /** 公告和停站；null 表示没拉到，或服务器还没有这个接口。 */
    announcements: VrccAnnouncements | null;
    /** 当前所在地图在榜上的状态；null 表示不显示（未连接、无地图、或被隐藏）。 */
    myMap: VrccMyMap | null;
    /** 「互相连上」的星友；null 表示还没拉过或没权限。 */
    mutuals: VrccMutuals | null;
    /**
     * 「互相连上」这一项失败的原因。
     *
     * 单独放一个字段而不是共用全局 error：它以 dev- 标识必然失败（敏感
     * 权限要审核），那是**某个页签的功能没开通**，不是整个页面坏了。塞进
     * 全局错误会让玩家以为连接出了问题。
     */
    mutualsError: string | null;
    /**
     * 「星轨相交」列表；null 表示还没拉过或没权限。
     *
     * 与 mutuals 的区别：mutuals 是**双向已连线**的星友，crossings 只是
     * **同房间擦肩**的人，大多数还没连线。
     */
    crossings: VrccCrossings | null;
    /** 星轨相交的完整星卡（1.13.0 起）；null 表示没拉过或不支持。 */
    crossingCards: VrccCrossingCards | null;
    /**
     * 正在请求星轨相交。
     *
     * 和 `crossings === null` 分开：那个只说明「手上没有数据」，既可能是
     * 还没拉，也可能是拉失败了。合成一个状态的话，面板分不清「加载中」和
     * 「确实为空」，会先闪一下「还没有擦肩记录」。
     */
    crossingsLoading: boolean;
    /**
     * 上一次拉取星轨相交失败的原因（人话）；`null` 表示这次成功了。
     *
     * **失败时保留旧的 `crossings`**：清空会让有记录的玩家看到「还没有
     * 擦肩」，把「这次没拿到」说成了「一条都没有」。界面据此决定是
     * 「显示旧数据 + 一条提示」还是「空态」。
     */
    crossingsError: string | null;
    /**
     * 失败是因为令牌缺 `crossings` 权限（永久性，要重连才能好）。
     *
     * 和 `crossingsError` 分开：那一栏是给玩家看的原文，这个是给界面选
     * 文案用的——缺权限要引导重新授权，网络失败要引导再试一次，两者
     * 不能用同一句话。
     */
    crossingsNeedsScope: boolean;
    /**
     * 已经拿到过至少一次「可见吗」的回答。
     *
     * 为 false 表示**还不知道**——探测没回来、或探测失败。界面据此先不
     * 显示入口：`hide` 的玩家不该因为一次网络抖动就看见这个页签。
     */
    crossingsChecked: boolean;
    service: VrccStatus | null;
    loading: boolean;
    error: VrccError | null;
    /** 设备授权进行中。 */
    deviceAuth: VrccDeviceAuthPrompt | null;
    deviceAuthBusy: boolean;
    deviceAuthMessage: string | null;
    /**
     * 距离下一次允许点「申请连接码」还有多少秒；`0` 表示现在就能点。
     *
     * 这个额度是**按网络出口 IP** 算的（每天 30 个，同一 NAT 后所有人共用），
     * 点一次就真的消耗服务端一次计数，所以不能靠连续点击去碰运气。
     */
    deviceAuthCooldown: number;
};

const initialState: State = {
    tab: 'maps',
    connection: null,
    usage: null,
    boards: null,
    boardSort: 'top',
    boardSortSupported: true,
    models: null,
    modelSort: 'top',
    modelSortSupported: true,
    modelsAvailable: true,
    recent: null,
    unread: null,
    notificationsView: 'recent',
    notes: null,
    notesSupported: null,
    announcements: null,
    myMap: null,
    mutuals: null,
    mutualsError: null,
    crossings: null,
    crossingCards: null,
    crossingsLoading: false,
    crossingsError: null,
    crossingsNeedsScope: false,
    crossingsChecked: false,
    service: null,
    loading: true,
    error: null,
    deviceAuth: null,
    deviceAuthBusy: false,
    deviceAuthMessage: null,
    deviceAuthCooldown: 0
};

/**
 * 星遇同频页面的控制器。
 *
 * 轮询在未连接时也跑，因为地图榜是开放数据、不需要令牌。个人接口只在
 * 已连接时轮询，且间隔带抖动。
 */
export function useVrccController() {
    const [state, setState] = useState<State>(initialState);
    const authTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
    const mounted = useRef(true);
    /** 设备码冷却的倒计时句柄。 */
    const cooldownTimer = useRef<ReturnType<typeof setInterval> | null>(null);
    /** 各种排序最近拿到的地图榜，切回来时直接用。 */
    const boardCache = useRef<
        Partial<Record<VrccBoardSort, { board: VrccBoard; at: number }>>
    >({});
    /** 正在看的排序；重新加载时照它拉，而不是跳回总榜。 */
    const boardSortRef = useRef<VrccBoardSort>('top');
    /** 各种排序最近拿到的模型榜，切回来时直接用。 */
    const modelCache = useRef<
        Partial<Record<VrccBoardSort, { board: VrccModelBoard; at: number }>>
    >({});
    /** 正在看的模型榜排序。 */
    const modelSortRef = useRef<VrccBoardSort>('top');
    /**
     * 个人数据（所在地图、互相连上）的请求代次。
     *
     * 断开连接时把它推进一位，让**断开前已经发出、还没回来**的请求作废——
     * 否则它们晚到一步，会把刚清掉的状态又写回去，界面就显示上一个账号的
     * 评分了。同一个接口的并发刷新也靠它区分先后。
     */
    const personalEpoch = useRef(0);

    const patch = useCallback((next: Partial<State>) => {
        if (mounted.current) {
            setState((current) => ({ ...current, ...next }));
        }
    }, []);

    /**
     * 统一上报一个错误。
     *
     * 三条路一起走，缺一不可：
     * - `console.error` 留原始对象，便于在 DevTools 里看堆栈和错误码
     * - 应用内 toast 给即时反馈（是窗口内的 toast，不是系统级 Windows 通知）
     * - 页内 `error` 横幅留住它，玩家切页签回来还能看到
     *
     * 只做其中一两样会出现「操作失败了但没有任何提示」，这是最难受的体验。
     *
     * `vrccApi` 保证抛出的都是 `VrccError`（内部已把后端错误归一化并分类），
     * 所以这里直接用；万一有非 `VrccError` 漏出来，按 `other` 兜住。
     */
    const reportError = useCallback(
        (error: unknown, where: string) => {
            console.error(`[vrcc] ${where} failed:`, error);
            const normalized =
                error instanceof VrccError
                    ? error
                    : new VrccError(
                          error instanceof Error
                              ? error.message
                              : String(error),
                          'other'
                      );
            toast.add({
                type: 'error',
                title: describeVrccError(normalized)
            });
            patch({ error: normalized });
            if (normalized.kind === 'needsReauth') {
                // 令牌失效（`404 gone`）时后端已经把它清掉了：重读连接状态，
                // 界面改显示「连接已失效，请重新连接」，而不是一直挂着「已连接」。
                void vrccApi
                    .connection()
                    .then((connection) => patch({ connection }))
                    .catch(() => {});
            }
        },
        // 这里刻意只依赖 `patch`（空依赖的 useCallback，永远稳定）。
        //
        // 翻译不通过 `useTranslation()` 拿，而是直接用全局 `i18n` 实例——
        // 和 `toastService` 的做法一致。原因：`useTranslation()` 返回的 `t`
        // 每次渲染可能是新引用，放进依赖会让 `reportError` 换引用 →
        // 依赖它的 `load` 换引用 → 调 `load` 的 effect 重跑 → `load` 里
        // `patch` → 再渲染 → 无限循环。实测在 jsdom 里几秒就把堆打爆。
        //
        // 而且 `t` 的引用变化并不代表文案变了：`i18n.t` 读全局实例，
        // 切语言时自己会返回新文案。
        [patch]
    );

    /** 刷新连接状态与本地用量。 */
    const refreshConnection = useCallback(async () => {
        const [connection, usage] = await Promise.all([
            vrccApi.connection(),
            vrccApi.usage()
        ]);
        patch({ connection, usage });
        return connection;
    }, [patch]);

    /**
     * 地图榜：官方说明服务器最多 5 分钟更新一次，问再勤也是同一份。
     *
     * `sort` 是本周榜或新上榜、回来的却没有 `sort` 时，服务器还不认这个参数，
     * 回的是总榜：照总榜显示，并藏起切换。
     */
    const refreshBoards = useCallback(
        async (sort: VrccBoardSort = 'top', reuse = false) => {
            const cached = boardCache.current[sort];
            if (reuse && cached && Date.now() - cached.at < BOARD_REUSE_MS) {
                boardSortRef.current = sort;
                patch({ boards: cached.board, boardSort: sort });
                return;
            }
            // 100 是接口允许的上限。一次取满，面板在本地分页——翻页不再发请求。
            const boards = await vrccApi.boards(100, sort);
            if (sort !== 'top' && boards.sort == null) {
                boardCache.current.top = { board: boards, at: Date.now() };
                boardSortRef.current = 'top';
                patch({ boards, boardSort: 'top', boardSortSupported: false });
                return;
            }
            boardCache.current[sort] = { board: boards, at: Date.now() };
            boardSortRef.current = sort;
            patch({ boards, boardSort: sort });
        },
        [patch]
    );

    /**
     * 模型榜。开放数据，不需要连接。
     *
     * 和 [`refreshBoards`] 同一套降级，但有**一处不同**：旧服务器上这个接口
     * 根本不存在。后端已经把 `404 not_found` 收敛成 `null`，这里记成
     * `modelsAvailable: false`，让那一页显示成空的，而不是弹错误横幅。
     *
     * 错误自己吞掉，不往上抛：模型榜是可选区块，失败不该连累地图榜，也不该在
     * 页面顶上挂一条红横幅——保留旧数据，玩家再点一次刷新即可。
     */
    const refreshModels = useCallback(
        async (sort: VrccBoardSort = 'top', reuse = false) => {
            const cached = modelCache.current[sort];
            if (reuse && cached && Date.now() - cached.at < BOARD_REUSE_MS) {
                modelSortRef.current = sort;
                patch({ models: cached.board, modelSort: sort });
                return;
            }
            try {
                // 和地图榜一样取满 100 条，面板在本地分页。
                const models = await vrccApi.models(100, sort);
                // `null`：旧服务器上没有这个接口。那是「这台服务器还没有
                // 模型榜」，不是故障——显示成空的，不弹错误。
                if (models == null) {
                    patch({ models: null, modelsAvailable: false });
                    return;
                }
                if (sort !== 'top' && models.sort == null) {
                    modelCache.current.top = { board: models, at: Date.now() };
                    modelSortRef.current = 'top';
                    patch({
                        models,
                        modelSort: 'top',
                        modelSortSupported: false,
                        modelsAvailable: true
                    });
                    return;
                }
                modelCache.current[sort] = { board: models, at: Date.now() };
                modelSortRef.current = sort;
                patch({
                    models,
                    modelSort: sort,
                    modelSortSupported: true,
                    modelsAvailable: true
                });
            } catch (error) {
                // 网络、限流之类的失败：保留旧数据，只把原因留给日志。
                // 清空会把「这次没拿到」说成「一个都没有」。
                patch({ modelsAvailable: true });
                console.warn('[vrcc] model board refresh failed:', error);
            }
        },
        [patch]
    );

    /**
     * 最近的提醒。
     *
     * `mark` 只在玩家真正打开「提醒」页时为 true —— 那会把已读同步到账号，
     * 网页和手机 App 上的红点跟着消。后台轮询绝不传 true。
     */
    const refreshRecent = useCallback(
        async (mark = false) => {
            const recent = await vrccApi.recentNotifications(mark);
            patch({ recent });
            if (mark) {
                // 后端借这次调用把通知中心里那几行按服务端游标标成了已读。
                // 通知中心是另一条查询、另一份内存快照，不重读就还挂着红点。
                useVrcNotificationStore
                    .getState()
                    .loadForCurrentUser()
                    .catch((error: unknown) => {
                        console.warn(
                            '[vrcc] notification centre reload failed:',
                            error
                        );
                    });
            }
        },
        [patch]
    );

    /**
     * 当前所在地图在榜上的状态。
     *
     * 位置由后端自己取，前端不参与——地图 ID 只有后端知道。
     * 官方要求：地图被下架或不对当前玩家开放时客户端不显示打分面板；
     * 后端已经把那种情况转成 null 返回。
     */
    const refreshMyMap = useCallback(async () => {
        // 记下发起时的代次：断开连接会推进它，这样晚到的响应就不会把
        // 已经清空的 `myMap` 又写回去（那会让界面显示上一个账号的评分）。
        const epoch = personalEpoch.current;
        const myMap = await vrccApi.myMap();
        if (epoch !== personalEpoch.current) return;
        patch({ myMap });
    }, [patch]);

    /**
     * 「互相连上」的星友列表。
     *
     * 需要 `mutuals` 敏感权限。`dev-` 标识拿到的是 `403 scope`——这是
     * **预期内**的结果（敏感权限要审核），不是故障。
     *
     * 所以失败时只把原因记到 `mutualsError` 供「连上」页签自己显示，
     * 不走全局 `reportError`：那会把一条「某个可选功能没开通」渲染成
     * 整页红色横幅，让玩家以为连接坏了。
     */
    const refreshMutuals = useCallback(async () => {
        const epoch = personalEpoch.current;
        try {
            const mutuals = await vrccApi.mutuals();
            if (epoch !== personalEpoch.current) return;
            patch({ mutuals, mutualsError: null });
        } catch (error) {
            if (epoch !== personalEpoch.current) return;
            if (error instanceof VrccError && error.kind === 'missingScope') {
                // 权限没开通：静默记录，界面在自己的页签里说明。
                patch({ mutuals: null, mutualsError: error.message });
                return;
            }
            patch({ mutuals: null });
            throw error;
        }
    }, [patch]);

    /**
     * 几处未读数（`me/unread`）。
     *
     * `refresh` 为 false 时只读后台轮询的缓存：收到「有变化」的事件时后台刚问过，
     * 不必再花一次（每天 1500 次）。
     */
    const refreshUnread = useCallback(
        async (refresh = true) => {
            const unread = await vrccApi.unread(refresh);
            patch({ unread });
        },
        [patch]
    );

    /** 通知中心（`me/notes`）。只在玩家切到「通知」时拉，读了不标已读。 */
    const refreshNotes = useCallback(async () => {
        const notes = await vrccApi.notes();
        patch({ notes, notesSupported: notes !== null });
    }, [patch]);

    /**
     * 公告和停站。拿不到不算页面错误：它只是附加信息，停站时个人接口
     * 自己也会报「停站维护中」。
     *
     * 走共享加载器而不是直接 `vrccApi.announcements()`：登录后的全局宿主
     * 已经拉过一次，1 分钟内这里直接复用缓存，两个消费点不再各发一次请求。
     * 这也顺带躲开 `min_gap: 30s` 的本地限流——登录后 30 秒内进页面时，
     * 以前第二次请求会得到 `ThrottledLocally`，`pop` 直接拉不到。
     */
    const refreshAnnouncements = useCallback(async () => {
        try {
            const announcements = await loadAnnouncements();
            patch({ announcements });
        } catch (error) {
            console.error('[vrcc] refreshAnnouncements failed:', error);
        }
    }, [patch]);

    /**
     * 「星轨相交」列表与完整星卡。
     *
     * 需要 `crossings` 敏感权限。`dev-` 标识拿到的是 `403 scope`——预期内
     * 的结果（敏感权限要审核），不是故障，所以只记到 `crossingsError`
     * 供该页签自己显示，不走全局 `reportError`。
     *
     * `mark` 只在玩家真的打开这一页时为 true：它会把已读同步到账号，
     * 网页和手机上的红点跟着消。别的场景一律 false。
     *
     * **星卡永远不带 `mark`**：已读同步只有列表那一条接口能消红点，星卡再标
     * 一次是重复动作，而它算**第二份读**的额度。星卡只是为了拿 `left` /
     * `max` 两个整数，没必要连带一次写。
     */
    const refreshCrossings = useCallback(
        async (mark = false) => {
            const epoch = personalEpoch.current;
            patch({ crossingsLoading: true });
            try {
                // 星卡是 1.13.0 才有的接口，后端已经把 `404 not_found` 转成
                // `Ok(None)`（旧服务器降级），所以这里**只剩真正的失败**。
                //
                // 但星卡只是「今天还能连几颗」那一行的可选信息：它挂了不该
                // 让整个列表跟着失败。所以一律降级成 `null`（那一行不显示），
                // 同时把原错误打进日志——**不谎称只有额度问题**：5xx、传输
                // 失败、403 都会走到这里，日志里如实写是什么。
                const [crossings, cards] = await Promise.all([
                    vrccApi.crossings(mark),
                    vrccApi.crossingCards(false).catch((error: unknown) => {
                        console.error(
                            '[vrcc] crossing cards unavailable, hiding the quota row:',
                            error
                        );
                        return null;
                    })
                ]);
                // 断开连接会推进代次：断开前发出、断开后才回来的响应
                // 不能再把状态写回去，否则界面会显示上一个账号的擦肩记录。
                if (epoch !== personalEpoch.current) return null;
                patch({
                    crossings,
                    crossingCards: cards,
                    crossingsLoading: false,
                    crossingsError: null,
                    crossingsNeedsScope: false,
                    crossingsChecked: true
                });
                return crossings;
            } catch (error) {
                if (epoch !== personalEpoch.current) return null;
                const isMissingScope =
                    error instanceof VrccError && error.kind === 'missingScope';
                // **保留上一次的数据**。失败时清空会让「有记录」的玩家看到
                // 「还没有擦肩」，把「这次没拿到」说成了「一条都没有」。
                // 空态只在确实拿到过响应且 `stars` 为空时出现。
                patch({
                    crossingsLoading: false,
                    crossingsError: describeVrccError(
                        error instanceof VrccError
                            ? error
                            : new VrccError(String(error), 'other')
                    ),
                    crossingsNeedsScope: isMissingScope
                });
                // 缺权限是**预期内**的结果（`dev-` 标识必然要不到敏感权限，
                // 玩家也可能没开这个功能）。它在自己的页签里说明就够了，
                // 不该弹 toast、也不该被当成整页的错误。
                if (isMissingScope) return null;
                throw error;
            }
        },
        [patch]
    );

    /**
     * 只问一次「星轨相交对这个玩家可见吗」，不标记已读。
     *
     * ## 为什么要探测
     *
     * `hide`（16–17 岁星池、或玩家自己在隐私设置里关掉）只有**问过才知道**，
     * 而官方要求「客户端不应显示此模块」。不探测的话，玩家会先看到页签、
     * 点进去、两个请求回来后才消失——那等于已经确认了「这功能存在、只是
     * 没给你」，正是要避免的那件事，只是换了个时间窗口。
     *
     * ## 代价
     *
     * 每次打开星遇同频页面多 1 次第一份读（1000/天）。这是有意的交换：
     * 合规要求优先于一次读。只问列表、不连带星卡（星卡是第二份读的额度，
     * 而可见性在列表的 `hidden` 字段里）。
     *
     * ## 失败时**不显示**入口
     *
     * 探测没回来、或者失败时按「不知道」处理：页签先不显示，页面刷新时
     * 重试。失败就放行等于把这条又还回去了——`hide` 的玩家会在探测失败
     * 时看到入口，而那正是要避免的那件事。
     *
     * 所以这里要区分「确认可见」和「不知道」：`crossingsChecked` 只在真的
     * 拿到一次回答后才为 true。
     */
    const probeCrossings = useCallback(async () => {
        const epoch = personalEpoch.current;
        try {
            const crossings = await vrccApi.crossingsVisibility();
            if (epoch !== personalEpoch.current) return;
            patch({
                crossings,
                crossingsChecked: true,
                crossingsLoading: false
            });
        } catch (error) {
            if (epoch !== personalEpoch.current) return;
            // 静默：不弹 toast。但**不把 `crossingsChecked` 置真**——界面
            // 据此继续把入口藏着，页面刷新（`reload`）会再试一次。
            console.info('[vrcc] crossings visibility probe failed:', error);
        }
    }, [patch]);

    const refreshService = useCallback(async () => {
        try {
            const service = await vrccApi.status();
            patch({ service });
        } catch (error) {
            // 探活失败本身不是页面错误，静默处理。
            //
            // 但**不能一律标 down**：`status()` 也要过 `acquire()`，别的接口
            // 刚吃过一次 429 时，在全局退避期间这个请求根本不会发出去，
            // 本地直接返回 `throttled`。那时服务是好的，标成「服务不可用」
            // 是在冤枉它。只有真的网络失败或拿到了非 ok 的回答才算 down。
            if (error instanceof VrccError && error.kind === 'throttled') {
                return;
            }
            patch({ service: { at: null, state: 'down' } });
        }
    }, [patch]);

    /**
     * 初次加载。
     *
     * 分两段：本地状态（连接、用量、所在地图）先出来，页面立刻可用；
     * 网络内容（地图榜、提醒、连上）各自异步加载。
     *
     * 原来是一路 `await` 串行，其中 `refreshBoards` 在没网时要等 12 秒超时，
     * 期间 `loading` 一直是 true，整页卡在转圈上——一个可选区块拖住了整页。
     * 网络请求失败该表现为那一块空着 + 一条错误提示，而不是「页面没加载完」。
     */
    const load = useCallback(async () => {
        patch({ loading: true, error: null });
        let connected = false;
        try {
            // 只读本地状态：这两个不会因为网络卡住。
            const connection = await refreshConnection();
            connected = connection.connected;
        } catch (error) {
            reportError(error, 'load');
        } finally {
            // 骨架先落地：即使后面的请求全都失败，页面也是可用的。
            patch({ loading: false });
        }

        // 以下全都可能发网络请求，一律不阻塞首屏。
        void refreshService().catch((error) => {
            reportError(error, 'refreshService');
        });
        void refreshAnnouncements();
        void refreshBoards(boardSortRef.current).catch((error) => {
            reportError(error, 'refreshBoards');
        });
        // 模型榜和地图榜并列：都是开放数据，一起拉。旧服务器上它回 `null`，
        // 那一页显示成空的，不弹错误；`refreshModels` 自己兜住异常。
        void refreshModels(modelSortRef.current);
        if (connected) {
            // 所在地图要已连接才有意义：未连接时后端直接报 TokenMissing，
            // 会被判成 needsReauth，于是每开一次页面就弹一次「去重新连接」，
            // 而其实玩家只是还没连。
            void refreshMyMap().catch((error) => {
                reportError(error, 'refreshMyMap');
            });
            void refreshRecent(false).catch((error) => {
                reportError(error, 'refreshRecent');
            });
            // 「互相连上」要敏感权限，`dev-` 一定拿不到；单独兜住，不影响别的。
            void refreshMutuals().catch((error) => {
                reportError(error, 'refreshMutuals');
            });
            // 「星轨相交」这里只**探一次可见性**，不拉星卡、不标已读。
            //
            // 为什么不索性不拉：`hide`（16–17 岁星池、或玩家自己关了）只有
            // 问过才知道，而官方要求「客户端不应显示此模块」。不探测的话
            // 页签会先出现、点进去才消失——那等于已经确认了「这功能存在、
            // 只是没给你」。多花 1 次第一份读换掉这个时间窗口，值得。
            //
            // 星卡（第二份读）仍然等玩家真的切过去才拉：可见性在列表的
            // `hidden` 字段里，不需要它。
            void probeCrossings();
            void refreshUnread(true).catch((error) => {
                reportError(error, 'refreshUnread');
            });
        }
    }, [
        patch,
        probeCrossings,
        refreshAnnouncements,
        refreshBoards,
        refreshConnection,
        refreshModels,
        refreshMutuals,
        refreshMyMap,
        refreshRecent,
        refreshService,
        refreshUnread,
        reportError
    ]);

    /**
     * 初次加载。
     *
     * 按 React 的生命周期写：每次挂载都正常注册清理函数，依赖用 `[load]`。
     * 早期用 `didLoad` ref + 空依赖挡重复执行，但 `if (didLoad.current) return`
     * 挡在 cleanup 注册之前，严格模式第二次挂载就没有 cleanup 了，
     * `mounted.current` 会永久停在 false，`patch` 里的 setState 全部静默丢掉。
     *
     * `load` 只依赖稳定的 `patch`（空依赖 useCallback），它依赖的各个
     * `refresh*` 也一样，所以 `[load]` 不会重跑。仓库里
     * `useActivityPageResource` 用的是同一种写法。
     */
    useEffect(() => {
        mounted.current = true;
        void load();
        return () => {
            mounted.current = false;
            if (authTimer.current) clearTimeout(authTimer.current);
            if (cooldownTimer.current) clearInterval(cooldownTimer.current);
        };
    }, [load]);

    /**
     * 玩家换世界时刷新「所在地图」卡片。
     *
     * 官方 README「场景三」要求在「打开面板或切地图时」请求所在地图。
     * 原来只在 `load()` 里拉一次，玩家在页面开着时换图，卡片会一直显示旧地图。
     *
     * 用 ref 记住上一次的世界：首次只是记下来，不重复请求——`load()` 已经
     * 拉过一次，而后端对 `me/maps/{world}` 有 30 秒最小间隔，多问一次会白等。
     */
    const currentWorldId = useRuntimeStore(
        (store) => store.gameState.currentWorldId ?? null
    );
    const lastWorldRef = useRef<string | null>(null);
    useEffect(() => {
        const previous = lastWorldRef.current;
        lastWorldRef.current = currentWorldId;
        // 首次（或没换图）不请求。
        if (previous === null || previous === currentWorldId) return;
        if (!state.connection?.connected) return;
        void refreshMyMap().catch((error) => {
            reportError(error, 'refreshMyMap');
        });
    }, [
        currentWorldId,
        state.connection?.connected,
        refreshMyMap,
        reportError
    ]);

    /**
     * 订阅后端推来的「提醒有变化」事件。
     *
     * **轮询在 Rust 侧**（`DesktopVrccRuntime::spawn_background_poll`）。
     * 原来这里自己 `setTimeout` 轮询，有两个问题：
     *
     * 1. 窗口隐藏到托盘后 Chromium 会节流隐藏页面的定时器，60 秒可能被拉到
     *    几分钟——而"后台也能收到提醒"正是这个功能的主要场景。
     * 2. 和 Rust 侧轮询并存会每个周期发两次请求，`mark` 语义也会打架。
     *
     * 现在后端发事件、前端只负责刷新状态。事件本身不带提醒正文（不把玩家
     * 内容经手额外的事件层），所以收到后要自己拉一次列表。
     *
     * 另一个事件是「连接失效」：后端收到 `404 gone` 后已经清掉令牌，这里
     * 重读连接状态，界面改显示「请重新连接」。连接一断，这个 effect 就退订。
     */
    useEffect(() => {
        if (!state.connection?.connected) {
            return;
        }
        let disposed = false;
        const unsubscribers: Array<() => void> = [];

        const subscribe = (event: string, handler: () => void) => {
            tauriClient.events
                .subscribe(event, () => {
                    if (!disposed) handler();
                })
                .then((off) => {
                    if (disposed) {
                        // 订阅回来时组件已经卸载：立刻退订，别留下悬空监听。
                        off();
                        return;
                    }
                    unsubscribers.push(off);
                    console.info(`[vrcc] ${event} event subscribed`);
                })
                .catch((error) => {
                    console.error(
                        `[vrcc] subscribe ${event} event failed:`,
                        error
                    );
                });
        };

        subscribe(VRCC_NOTICES_CHANGED_EVENT, () => {
            // 收到后端信号说明有新提醒或未读数变了，这里拉一次列表。
            // 这行日志用于区分「事件没到」和「事件到了但刷新失败」，
            // 是排查「后台弹了通知但界面没变」的第一现场。
            console.info('[vrcc] notices changed event received');
            // 后台发现的提醒不标记已读：玩家还没看到，替他标记是错的。
            refreshRecent(false).catch((error) => {
                if (disposed) return;
                if (
                    error instanceof VrccError &&
                    error.kind === 'needsReauth'
                ) {
                    reportError(error, 'poll');
                }
            });
            // 后台刚问过未读数，读它留下的就行。
            refreshUnread(false).catch((error) => {
                console.error('[vrcc] refreshUnread failed:', error);
            });
        });
        subscribe(VRCC_CONNECTION_CHANGED_EVENT, () => {
            console.info('[vrcc] connection changed event received');
            refreshConnection().catch((error) => {
                if (!disposed) reportError(error, 'refreshConnection');
            });
        });

        return () => {
            disposed = true;
            for (const off of unsubscribers) off();
            console.info('[vrcc] events unsubscribed');
        };
    }, [
        state.connection?.connected,
        refreshConnection,
        refreshRecent,
        refreshUnread,
        reportError
    ]);

    /**
     * 把未读合计同步给侧边栏。
     *
     * 侧边栏是常驻组件，而 controller 只在星遇同频页面挂载时跑，所以数字得
     * 从这里推出去。算的口径和页面标题旁那个角标一致（同一个
     * `countVrccUnread`），否则两处会显示不同的数字。
     *
     * 没连上时推 0：连接断开后留一个旧数字，比不显示更容易误导。
     */
    useEffect(() => {
        const connected = state.connection?.connected ?? false;
        useVrccUnreadStore.getState().setConnected(connected);
        useVrccUnreadStore
            .getState()
            .setUnreadTotal(
                connected ? countVrccUnread(state.unread, state.recent) : 0
            );
    }, [state.connection?.connected, state.unread, state.recent]);

    /**
     * 登录态一变就把模型相关的会话缓存整表清掉。
     *
     * **不能假设「模型详情永远与登录无关」。** 今天 `/models/{id}` 是公开数据，
     * 但权限可以再申请、接口也可能开始返回「你投过票没有」这类绑定登录态的
     * 字段。那天到来时，一个跨账号共享的前端缓存会把 A 的详情给 B 看。
     *
     * 后端那边是把账号分区写进缓存键（`OwnedModelKey` / `OwnedBoardKey` /
     * `OwnedModelBoardKey`）；前端这两层只活一个会话，所以用更省事的等价做法：
     * **换了账号就整表清空**。挂在 `star` 上而不只是 `connected`——同一个人
     * 断开重连不该白清，换人才该。
     */
    useEffect(() => {
        resetModelDetailCache();
        resetModelCoverCache();
    }, [state.connection?.star]);

    /**
     * 卸载时清掉侧边栏的角标。
     *
     * 只清数字不清连接态：玩家可能只是切到别的页面，连接仍然有效，角标在
     * 下次进页面时会重新拉。真正断开连接时上面的 effect 已经推了 0。
     */
    useEffect(
        () => () => {
            useVrccUnreadStore.getState().reset();
        },
        []
    );

    /**
     * 切页签。
     *
     * 切到「提醒」时用 `mark=true` 重拉一次：这是玩家真正在看这个列表的时刻，
     * 服务器据此把已读同步到账号，网页和手机 App 上的红点会跟着消。
     * 官方的做法也是「打开最近的提醒」时才 mark。
     *
     * 「星轨相交」同理，而且这里**只有**这一条路会传 `mark=true`：
     * `load()` 与刷新按钮都传 false。漏掉这一条，玩家永远消不掉红点；
     * 反过来在别处误传 true，等于替玩家把没看过的记录标成看过。
     */
    const setTab = useCallback(
        (tab: VrccTab) => {
            patch({ tab });
            // 模型榜是开放数据，不需要连接，所以这一句**必须在下面那道
            // `connected` 检查之前**：未连接的玩家也会切到模型榜，那时同样
            // 该走缓存，而不是又发一次请求。
            //
            // 只有「切回这一页」时才复用：60 秒内看过的直接用，超过就当没有。
            // 手动刷新和切排序各自有入口，不受这里影响。
            if (tab === 'models') {
                void refreshModels(modelSortRef.current, true);
            }
            if (!state.connection?.connected) return;
            if (tab === 'notifications') {
                refreshRecent(true).catch(() => {
                    // 标记已读失败不该打断看列表；列表内容已经在了。
                });
            }
            if (tab === 'mutuals') {
                // 打开时拉一次：列表可能在上次加载后变了。
                refreshMutuals().catch((error) => {
                    reportError(error, 'refreshMutuals');
                });
            }
            if (tab === 'crossings') {
                // 打开这一页 = 看过了。权限没开通时 `refreshCrossings`
                // 自己把原因记进 `crossingsError`，不会抛出来。
                refreshCrossings(true).then(
                    () => {
                        // `mark=1` 已经把服务端的已读清掉了，但界面上的数字
                        // 是上一次 `me/unread` 的结果，不重读就会一直亮着，
                        // 玩家要重开页面才看到它消失。失败不报：红点晚一轮
                        // 消失比弹一条错误更合适。
                        refreshUnread(false).catch((error) => {
                            console.error(
                                '[vrcc] refreshUnread after mark failed:',
                                error
                            );
                        });
                    },
                    (error) => {
                        reportError(error, 'refreshCrossings');
                    }
                );
            }
        },
        [
            patch,
            refreshCrossings,
            refreshModels,
            refreshMutuals,
            refreshRecent,
            refreshUnread,
            reportError,
            state.connection?.connected
        ]
    );

    /** 切换地图榜的排序。刚看过的直接用手上的，不再请求。 */
    const setBoardSort = useCallback(
        (sort: VrccBoardSort) => {
            refreshBoards(sort, true).catch((error) => {
                reportError(error, 'refreshBoards');
            });
        },
        [refreshBoards, reportError]
    );

    /** 切换模型榜的排序。同一套复用策略：60 秒内看过的那一份直接用。 */
    const setModelSort = useCallback(
        (sort: VrccBoardSort) => {
            // 失败由 `refreshModels` 自己吞掉并记日志：模型榜是可选区块，
            // 服务器上还没有它时不该在页面顶上挂一条红横幅。
            void refreshModels(sort, true);
        },
        [refreshModels]
    );

    /**
     * 「提醒」页签里切换「最近的提醒 / 通知中心」。
     *
     * 通知中心每次切过来都拉一次：它不标已读、玩家在网页上看过会变，而它算
     * 每天 1000 次的第二份读，只在玩家真的要看时才花。
     */
    const setNotificationsView = useCallback(
        (view: VrccNotificationsView) => {
            patch({ notificationsView: view });
            if (view === 'notes' && state.connection?.connected) {
                refreshNotes().catch((error) => {
                    reportError(error, 'refreshNotes');
                });
            }
        },
        [patch, refreshNotes, reportError, state.connection?.connected]
    );

    /**
     * 启动（或重启）设备码冷却倒计时。
     *
     * 用 `setInterval` 而不是一次性 `setTimeout`：界面要显示「还有几秒」，
     * 每秒都要动一次。到 0 就自己停掉，不留下空转的定时器。
     */
    const startDeviceAuthCooldown = useCallback(() => {
        if (cooldownTimer.current != null) {
            clearInterval(cooldownTimer.current);
            cooldownTimer.current = null;
        }
        let left = DEVICE_AUTH_COOLDOWN_SECS;
        patch({ deviceAuthCooldown: left });
        cooldownTimer.current = setInterval(() => {
            left -= 1;
            if (!mounted.current) {
                if (cooldownTimer.current != null) {
                    clearInterval(cooldownTimer.current);
                    cooldownTimer.current = null;
                }
                return;
            }
            patch({ deviceAuthCooldown: Math.max(0, left) });
            if (left <= 0 && cooldownTimer.current != null) {
                clearInterval(cooldownTimer.current);
                cooldownTimer.current = null;
            }
        }, 1000);
    }, [patch]);

    /**
     * 开始设备授权。
     *
     * 冷却期内直接返回，**连请求都不发**：那份额度是按网络出口 IP 算的，
     * 每次调用都真实消耗服务端一个计数，不能用连点去试探。
     */
    const beginDeviceAuth = useCallback(async () => {
        if (state.deviceAuthCooldown > 0) {
            return;
        }
        // 请求一发出就开始冷却，不等它回来：`too_many` 时没有响应，若等结果
        // 再计时，失败的那一次反而让玩家立刻又能点。
        startDeviceAuthCooldown();
        patch({ deviceAuthBusy: true, deviceAuthMessage: null, error: null });
        try {
            const prompt = await vrccApi.beginDeviceAuth();
            patch({ deviceAuth: prompt, deviceAuthBusy: false });

            // 先把授权码放进剪贴板：玩家的下一步要么在网页上粘贴，
            // 要么在手机上输——两种都要这个码，先给到手最省事。
            // 很短的码（`ABCD-2345`）手工抄写极易错位。
            void copyTextToClipboard(prompt.userCode, {
                successMessage: i18n.t('vrcc.me.code_copied', {
                    code: prompt.userCode
                })
            });

            // 再用系统浏览器打开同意页。`window.open` 在 Tauri 里会被拦掉，
            // 必须走 `app__open_link`。
            const target =
                prompt.verificationUriComplete ?? prompt.verificationUri;
            if (target) {
                void openExternalLink(target).catch((error: unknown) => {
                    // 打不开浏览器不该让整个授权流程失败：码已经复制好了，
                    // 玩家还能自己打开网页。给一条提示即可。
                    console.error('[vrcc] openExternalLink failed:', error);
                    toast.add({
                        type: 'error',
                        title: i18n.t('vrcc.me.open_browser_failed')
                    });
                });
            }
        } catch (error) {
            patch({ deviceAuthBusy: false });
            reportError(error, 'beginDeviceAuth');
        }
    }, [patch, reportError, startDeviceAuthCooldown, state.deviceAuthCooldown]);

    /**
     * 轮询设备授权。按后端给的 interval 反复问，直到成功、终态或超时。
     */
    useEffect(() => {
        const prompt = state.deviceAuth;
        if (!prompt) return;
        let cancelled = false;

        const tick = async () => {
            try {
                const step = await vrccApi.pollDeviceAuth(prompt);
                if (cancelled) return;
                if (step.state === 'connected') {
                    patch({
                        deviceAuth: null,
                        deviceAuthMessage: null
                    });
                    await load();
                    return;
                }
                if (step.state === 'terminal') {
                    patch({
                        deviceAuth: null,
                        deviceAuthMessage: step.message
                    });
                    return;
                }
                if (step.state === 'pending') {
                    authTimer.current = setTimeout(
                        tick,
                        Math.max(5, step.interval_secs) * 1000
                    );
                }
            } catch (error) {
                if (!cancelled) {
                    patch({ deviceAuth: null });
                    reportError(error, 'pollDeviceAuth');
                }
            }
        };

        authTimer.current = setTimeout(
            tick,
            Math.max(5, prompt.intervalSecs) * 1000
        );

        return () => {
            cancelled = true;
            if (authTimer.current) clearTimeout(authTimer.current);
        };
    }, [state.deviceAuth, load, patch, reportError]);

    const disconnect = useCallback(async () => {
        patch({ error: null });
        // 先推进代次：断开前发出的个人数据请求从这里开始就作废了，
        // 它们的响应不会再写回状态。
        personalEpoch.current += 1;
        try {
            await vrccApi.disconnect();
            patch({
                deviceAuth: null,
                recent: null,
                unread: null,
                notes: null,
                // 「所在地图」「互相连上」「星轨相交」都是**上一个账号**的
                // 个人数据。不清的话，`VrccPage` 只看 `myMap` 是否存在就渲染
                // 卡片，断开后还会继续显示别人的评分和擦肩记录。
                myMap: null,
                mutuals: null,
                mutualsError: null,
                crossings: null,
                crossingCards: null,
                crossingsLoading: false,
                crossingsError: null,
                crossingsNeedsScope: false,
                crossingsChecked: false
            });
        } catch (error) {
            // 可重试的失败（网络不通、正在退避）**后端会刻意保留令牌**，
            // 等玩家再点一次。这时界面必须如实显示「还连着」，不能骗人。
            reportError(error, 'disconnect');
        } finally {
            // 无论成败都要重读连接状态。
            //
            // 原来这句在 `try` 里 `await` 之后：一旦 `disconnect` 抛错就跳过，
            // 于是后端已经断开、页面却还挂着「已连接」；反过来后端刻意保留了
            // 令牌、页面又可能显示成已断开。放 `finally` 保证界面跟后端一致。
            await refreshConnection();
        }
    }, [patch, refreshConnection, reportError]);

    /**
     * 打开 VRCX-0 自己的世界详情窗。
     *
     * 对齐官方插件「加好友/看地图是打开 VRCX 自己的窗口，由你自己点」的做法：
     * 我们只负责把窗口叫出来，真正的操作交给 VRCX 原生界面。
     */
    const openWorld = useCallback((worldId: string) => {
        if (!worldId) return;
        openWorldDialog({ worldId });
    }, []);

    /**
     * 打开 VRCX-0 自己的玩家详情窗。
     *
     * 官方合规要求的落点就在这里：`me/mutuals` 里对方的 `vrcUser` 只用来打开
     * 我们自己的玩家页，**加好友由玩家在那个界面里自己点**。
     * 官方原文两处都写明「严禁由程序自动代替玩家向 VRChat 发送好友请求」，
     * 所以这里只到"打开详情页"为止，没有、也不会有自动发送好友请求的分支。
     */
    const openUser = useCallback((userId: string) => {
        if (!userId.startsWith('usr_')) return;
        openUserDialog({ userId });
    }, []);

    /**
     * 打开 VRCX-0 自己的群组详情窗。
     *
     * 和 `openUser` 同一条合规边界：社区群组的入口只负责把玩家带到我们
     * 自己的群组页，**加入由玩家在那个界面里自己点**，程序不代发请求。
     */
    const openGroup = useCallback((groupId: string) => {
        if (!groupId.startsWith('grp_')) return;
        openGroupDialog({ groupId });
    }, []);

    return {
        ...state,
        setTab,
        reload: load,
        refreshBoards: () => refreshBoards(boardSortRef.current),
        setBoardSort,
        refreshModels: () => refreshModels(modelSortRef.current),
        setModelSort,
        setNotificationsView,
        refreshMyMap,
        refreshMutuals,
        refreshCrossings,
        beginDeviceAuth,
        disconnect,
        openWorld,
        openUser,
        openGroup
    };
}
