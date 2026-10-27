import { commands } from '@/platform/tauri/bindings';

/**
 * 申请设备授权时显式请求的权限集合。
 *
 * 与 Rust 侧 `VRCC_DEVICE_AUTH_SCOPE` 保持一致：只列界面真正用到的
 * `profile` / `notifications` / `maps`。`mutuals`（互相连上）是敏感权限，
 * 需要单独审核，拿到正式 `app_` 标识后再加。
 */
const DEVICE_AUTH_SCOPE = 'profile notifications maps';

/**
 * 星遇同频（vrcc.cc）前端数据层。
 *
 * 所有调用都走生成的绑定，不手写命令名。这一层只做两件事：
 * 把后端的错误收成统一的 `VrccError`，以及把「是否需要重新授权」这类
 * 判断交给后端（前端不解析错误文案）。
 */

/** 后端已经分好类的错误类别。 */
export type VrccErrorKind =
    | 'needsReauth'
    | 'missingScope'
    | 'throttled'
    | 'transport'
    | 'other';

export class VrccError extends Error {
    readonly kind: VrccErrorKind;
    readonly scope: string | null;

    constructor(
        message: string,
        kind: VrccErrorKind,
        scope: string | null = null
    ) {
        super(message);
        this.name = 'VrccError';
        this.kind = kind;
        this.scope = scope;
    }
}

/**
 * 把命令抛出的错误归一化。
 *
 * 分类结论**随错误一起从后端过来**（`AppErrorPayload.vrcc`），前端不再解析
 * 文案。早先这里要再走一次 IPC 对中文错误串做子串匹配，结果 `401 token`
 * 落进 `other`、缺权限的 `need` 在转字符串时丢失、`missingScope` 提示
 * 永远不显示——都源于「让消费方去猜分类」。
 */
async function normalizeError(error: unknown): Promise<VrccError> {
    const message = error instanceof Error ? error.message : String(error);
    const detail = readVrccDetail(error);
    return new VrccError(
        message,
        normalizeKind(detail?.kind),
        detail?.need ?? null
    );
}

/** 后端给出的结构化细节；老后端或非 vrcc 错误时没有这一项。 */
function readVrccDetail(
    error: unknown
): { kind?: string; need?: string | null } | null {
    if (!error || typeof error !== 'object') {
        return null;
    }
    const detail = (error as { vrcc?: unknown }).vrcc;
    if (!detail || typeof detail !== 'object') {
        return null;
    }
    return detail as { kind?: string; need?: string | null };
}

/** 只接受已知的五个类别，别的一律当 `other`，免得后端加了新类别前端崩。 */
function normalizeKind(kind: string | undefined): VrccErrorKind {
    switch (kind) {
        case 'needsReauth':
        case 'missingScope':
        case 'throttled':
        case 'transport':
            return kind;
        default:
            return 'other';
    }
}

async function call<T>(run: () => Promise<T>): Promise<T> {
    try {
        return await run();
    } catch (error) {
        throw await normalizeError(error);
    }
}

export const vrccApi = {
    connection: () => call(() => commands.appVrccConnection()),
    usage: () => call(() => commands.appVrccUsage()),

    /**
     * 地图榜。不需要连接。
     *
     * `sort` 是本周榜或新上榜、回答里却没有 `sort` 时，说明服务器还不支持
     * （回的是总榜），界面要藏起切换，不能把总榜当本周榜显示。
     */
    boards: (limit = 20, sort: VrccBoardSort = 'top') =>
        call(() => commands.appVrccBoards(limit, sort)),

    status: () => call(() => commands.appVrccStatus()),

    /** 公告和停站。不需要连接。服务器还没有这个接口时为 null。 */
    announcements: () => call(() => commands.appVrccAnnouncements()),

    /** 第一步设备授权：拿码。`scope` 传 null 用官方默认。 */
    /**
     * 开始设备授权。
     *
     * **必须显式带 `scope`**：不写的话拿到哪些权限完全取决于客户端怎么登记
     * （自助登记只有 `profile`，开会话登记的又会带上用不到的敏感权限）。
     * 这里只要当前界面真正用到的两项；`mutuals` 等敏感权限拿到正式标识后再加。
     */
    beginDeviceAuth: (scope: string | null = DEVICE_AUTH_SCOPE) =>
        call(() => commands.appVrccDeviceAuthBegin(scope)),

    /** 第二步：问一次令牌。返回 pending / connected / terminal。 */
    pollDeviceAuth: (
        prompt: Awaited<ReturnType<typeof commands.appVrccDeviceAuthBegin>>
    ) => call(() => commands.appVrccDeviceAuthPoll(prompt)),

    disconnect: () => call(() => commands.appVrccDisconnect()),

    /**
     * 最近的提醒。
     *
     * `mark` 传 true 表示玩家正在看这个列表，服务器会把已读同步到账号；
     * 后台只为算数字时传 false，否则等于替玩家把提醒标成看过了。
     */
    recentNotifications: (mark = false) =>
        call(() => commands.appVrccRecentNotifications(mark)),

    /**
     * 几处未读数（连向我、星轨相交、私聊、通知）。读了不标已读。
     *
     * `refresh` 为 false 时只读后台轮询的缓存、不发请求；收到后端「有变化」
     * 的事件时用它。服务器还没有 `me/unread` 时为 null。
     */
    unread: (refresh = true) => call(() => commands.appVrccUnread(refresh)),

    /** 通知中心。读了不标已读。服务器还没有 `me/notes` 时为 null。 */
    notes: () => call(() => commands.appVrccNotes()),

    /** 增量轮询。游标由后端维护。 */
    pollNotifications: () => call(() => commands.appVrccPollNotifications()),

    /**
     * 当前所在地图在榜上的状态与自己的评分。
     *
     * 地图被下架或不对当前玩家开放时返回 null，此时**不要**显示打分入口。
     */
    myMap: () => call(() => commands.appVrccMyMap()),

    /** 今天这一轮。需要 today 敏感权限（审核后才可用）。 */
    today: () => call(() => commands.appVrccToday()),

    /**
     * 「互相连上」的星友列表。需要 mutuals 敏感权限（审核后才可用）。
     *
     * 条目带 `vrcUser` 时只能用来打开 VRCX-0 自己的玩家详情页——官方合规
     * 要求加好友由玩家在原生界面里自己点，禁止程序代发好友请求。
     */
    mutuals: () => call(() => commands.appVrccMutuals())
};

export type VrccBoard = Awaited<ReturnType<typeof commands.appVrccBoards>>;
/** 总榜 / 本周榜 / 新上榜（1.13.0 起）。 */
export type VrccBoardSort = NonNullable<
    Parameters<typeof commands.appVrccBoards>[1]
>;
/** 榜单里的一行。`maps` 在契约里是可选的，先把 undefined 排掉再取元素类型。 */
export type VrccBoardMap = NonNullable<VrccBoard['maps']>[number];
export type VrccConnection = Awaited<
    ReturnType<typeof commands.appVrccConnection>
>;
export type VrccUsage = Awaited<ReturnType<typeof commands.appVrccUsage>>;
export type VrccRecentNotifications = Awaited<
    ReturnType<typeof commands.appVrccRecentNotifications>
>;
export type VrccNotificationItem = NonNullable<
    VrccRecentNotifications['items']
>[number];
export type VrccDeviceAuthPrompt = Awaited<
    ReturnType<typeof commands.appVrccDeviceAuthBegin>
>;
export type VrccDeviceAuthStep = Awaited<
    ReturnType<typeof commands.appVrccDeviceAuthPoll>
>;
export type VrccStatus = Awaited<ReturnType<typeof commands.appVrccStatus>>;
export type VrccToday = Awaited<ReturnType<typeof commands.appVrccToday>>;
export type VrccMutuals = Awaited<ReturnType<typeof commands.appVrccMutuals>>;
export type VrccMutualStar = NonNullable<VrccMutuals['stars']>[number];
export type VrccMyMap = NonNullable<
    Awaited<ReturnType<typeof commands.appVrccMyMap>>
>;
export type VrccUnread = NonNullable<
    Awaited<ReturnType<typeof commands.appVrccUnread>>
>;
export type VrccNotes = NonNullable<
    Awaited<ReturnType<typeof commands.appVrccNotes>>
>;
export type VrccNote = NonNullable<VrccNotes['notes']>[number];
export type VrccAnnouncements = NonNullable<
    Awaited<ReturnType<typeof commands.appVrccAnnouncements>>
>;
export type VrccAnnouncement = NonNullable<VrccAnnouncements['posts']>[number];
