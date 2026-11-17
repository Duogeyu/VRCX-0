import type { QueryKey } from '@tanstack/react-query';

import { queryClient } from '@/lib/queryClient';
import type {
    CalendarListParams,
    ReleaseStatusFilter,
    VrchatGroupGalleryInput,
    VrchatGroupMembersInput,
    VrchatWorldListByUserInput
} from '@/platform/tauri/bindings';
import { MINUTE_MS, SECOND_MS } from '@/shared/constants/time';
import { normalizeVrchatEndpointKey } from '@/shared/vrchatEndpoint';

type EntityQueryPolicy = Readonly<{
    staleTime: number;
    gcTime: number;
    retry: number;
    refetchOnWindowFocus: boolean;
}>;

type FetchWithEntityPolicyOptions<TData = unknown> = {
    queryKey: QueryKey;
    policy: EntityQueryPolicy;
    queryFn: () => Promise<TData> | TData;
    force?: boolean;
};

export const entityQueryPolicies = Object.freeze({
    instance: Object.freeze({
        staleTime: 0,
        gcTime: 90 * SECOND_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    group: Object.freeze({
        staleTime: 5 * MINUTE_MS,
        gcTime: 30 * MINUTE_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    groupDialog: Object.freeze({
        staleTime: 120 * SECOND_MS,
        gcTime: 30 * MINUTE_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    // 地图在星遇同频榜上的公开信息。半小时是刻意的：判断的是「在不在榜、
    // 大概几分」这类变化很慢的事实，而且官方自己最多 5 分钟才更新一次。
    // 缓存短了，玩家翻世界列表就等于在刷接口。
    vrccMapPage: Object.freeze({
        staleTime: 30 * MINUTE_MS,
        gcTime: 120 * MINUTE_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    // 群组对应的星遇同频社团（`GET /clubs/{code}`）。官方写「最多 1 分钟更新
    // 一次」，所以半小时缓存完全够——图标、人数、一句话这些一天里几乎不动。
    //
    // **重试 0**：绝大多数群组没入驻星遇同频，`404` 才是常态。重试只会把
    // 一个已经确定的空结论多问两遍，白花额度也白等。
    vrccClub: Object.freeze({
        staleTime: 30 * MINUTE_MS,
        gcTime: 120 * MINUTE_MS,
        retry: 0,
        refetchOnWindowFocus: false
    }),
    // 社团列表（`GET /clubs` 公开 / `GET /me/clubs` 玩家视角）。
    //
    // 服务器最多 5 分钟重新生成一次，问再勤也是同一份——所以 5 分钟，和官网
    // 那张网页的刷新节奏对齐。**重试 1**：这是公开数据，失败一次多半是网络抖，
    // 和「群组没入驻社团」那种确定结论不同。
    vrccClubs: Object.freeze({
        staleTime: 5 * MINUTE_MS,
        gcTime: 30 * MINUTE_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    groupCollection: Object.freeze({
        staleTime: 60 * SECOND_MS,
        gcTime: 300 * SECOND_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    economy: Object.freeze({
        staleTime: 10 * MINUTE_MS,
        gcTime: 30 * MINUTE_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    economyBalance: Object.freeze({
        staleTime: 120 * SECOND_MS,
        gcTime: 600 * SECOND_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    groupCalendarEvent: Object.freeze({
        staleTime: 120 * SECOND_MS,
        gcTime: 600 * SECOND_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    worldCollection: Object.freeze({
        staleTime: 60 * SECOND_MS,
        gcTime: 300 * SECOND_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    avatarGallery: Object.freeze({
        staleTime: 30 * SECOND_MS,
        gcTime: 120 * SECOND_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    inventoryCollection: Object.freeze({
        staleTime: 20 * SECOND_MS,
        gcTime: 120 * SECOND_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    inventoryTemplate: Object.freeze({
        staleTime: 60 * MINUTE_MS,
        gcTime: 240 * MINUTE_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    cosmeticsIndex: Object.freeze({
        staleTime: 240 * MINUTE_MS,
        gcTime: 480 * MINUTE_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    fileAnalysis: Object.freeze({
        staleTime: 120 * MINUTE_MS,
        gcTime: 240 * MINUTE_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    avatarStyles: Object.freeze({
        staleTime: 60 * MINUTE_MS,
        gcTime: 240 * MINUTE_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    representedGroup: Object.freeze({
        staleTime: 60 * SECOND_MS,
        gcTime: 300 * SECOND_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    userDialogTabCounts: Object.freeze({
        staleTime: 0,
        gcTime: 10 * MINUTE_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    worldPersistData: Object.freeze({
        staleTime: 30 * MINUTE_MS,
        gcTime: 120 * MINUTE_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    userAppearanceProfile: Object.freeze({
        staleTime: 60 * SECOND_MS,
        gcTime: 300 * SECOND_MS,
        retry: 1,
        refetchOnWindowFocus: false
    }),
    userAvatarLookup: Object.freeze({
        staleTime: 30 * MINUTE_MS,
        gcTime: 5 * MINUTE_MS,
        retry: 0,
        refetchOnWindowFocus: false
    }),
    boopEmojiLookup: Object.freeze({
        staleTime: 30 * MINUTE_MS,
        gcTime: 30 * MINUTE_MS,
        retry: 0,
        refetchOnWindowFocus: false
    })
});

type UserDialogTabCountQueryParams = {
    userId: string;
    currentUserId: string;
    avatarReleaseStatus: ReleaseStatusFilter;
    includeMutualFriends: boolean;
};

type AggregateCalendarQueryParams = {
    date: string;
    includeFeatured: boolean;
    userId: string | null;
};

function withEndpoint(queryKey: QueryKey, endpoint: string = ''): QueryKey {
    const normalizedEndpoint = normalizeVrchatEndpointKey(endpoint);
    return normalizedEndpoint
        ? [...queryKey, { endpoint: normalizedEndpoint }]
        : queryKey;
}

function stableParams(params: object = {}): Record<string, unknown> {
    return Object.fromEntries(
        Object.entries(params)
            .filter(([, value]) => value !== undefined)
            .sort(([left], [right]) => left.localeCompare(right))
    );
}

export const queryKeys = Object.freeze({
    user: (userId: string, endpoint: string = '') =>
        withEndpoint(['user', userId], endpoint),
    userAppearanceProfile: (userId: string, endpoint: string = '') =>
        withEndpoint(['user', userId, 'appearanceProfile'], endpoint),
    userGroups: (userId: string, endpoint: string = '') =>
        withEndpoint(['user', userId, 'groups'], endpoint),
    userGroupsOverview: (userId: string, endpoint: string = '') =>
        withEndpoint(['user', userId, 'groupsOverview'], endpoint),
    instance: (worldId: string, instanceId: string, endpoint: string = '') =>
        withEndpoint(['instance', worldId, instanceId], endpoint),
    instanceShortName: (
        worldId: string,
        instanceId: string,
        endpoint: string = ''
    ) => withEndpoint(['instance', worldId, instanceId, 'shortName'], endpoint),
    group: (
        groupId: string,
        includeRoles: boolean = false,
        endpoint: string = ''
    ) => withEndpoint(['group', groupId, includeRoles], endpoint),
    worldsByUser: (params: VrchatWorldListByUserInput, endpoint: string = '') =>
        withEndpoint(
            ['worlds', 'user', params.userId, stableParams(params)],
            endpoint
        ),
    // 这张地图在星遇同频地图榜上的公开信息。`endpoint` 这里恒为空串，但
    // **不能省掉这一段**：`withEndpoint` 只在非空时才追加，所以传空串的
    // 结果本身是干净的；而一旦有人手拼键、漏掉这一段，键的长度就和这里
    // 不一致，缓存永远命中不了——A2 那个 bug 就是这么来的。
    worldVrcc: (worldId: string, endpoint: string = '') =>
        withEndpoint(['world', worldId, 'vrcc'], endpoint),
    // 带令牌的地图页。**必须与 `worldVrcc` 分开**：两条路的缓存时长、失效
    // 条件、以及「未连接时会失败」这个性质都不同，共用一个键会让公开数据
    // 被登录数据顶掉，反之亦然。`sort` 也进键：`hot` 和 `new` 是两份不同的
    // 列表，玩家切了排序不该看到上一份。
    //
    // **星遇同频的个人数据，键里都带账号**（这里、`worldVrccMine`、
    // `groupVrccClubDetail`、玩家视角的 `vrccClubs`）：换 VRChat 账号时整份缓存
    // 会被清掉，但同一个 VRChat 账号里断开、令牌失效或换连另一个星遇同频账号
    // 不会；不带账号的话，staleTime 内会先显示上一个账号的评分和关注。账号标识
    // 取自 `vrccAccountKey`（`star:<编号>` / `star:none` / `offline`）。
    worldVrccSignedIn: (
        worldId: string,
        sort: 'hot' | 'new' = 'hot',
        account: string = 'offline',
        endpoint: string = ''
    ) =>
        withEndpoint(
            ['world', worldId, 'vrcc', 'signedIn', sort, account],
            endpoint
        ),
    // 当前的星遇同频连接（本地会话快照），见 `useVrccConnection`。
    vrccConnection: () => ['vrcc', 'connection'] as const,
    // 群组对应的星遇同频社团（`GET /clubs/{code}`）。
    //
    // 按**短码**分键而不是群组 ID：短码本身就是 `shortCode.discriminator`，
    // 同一个社团从别处打开也命中同一份缓存，不会问两遍。
    groupVrccClub: (code: string) => ['group', 'vrcc-club', code] as const,
    // 同一个社团的**玩家视角**（`GET /me/clubs/{code}`）：公告、房间、活动、点赞、
    // 关注全在这一份回答里。
    //
    // **卡片和房间列表共用这一个键**：两个组件各自建键的话，打开一次群组详情会
    // 把同一个接口问两遍，而它在客户端侧带 30 秒 `min_gap`、还吃每日额度。
    groupVrccClubDetail: (code: string, account: string = 'offline') =>
        ['group', 'vrcc-club-detail', code, account] as const,
    // 社团列表。**键里带来源**：公开列表按页取、玩家视角是全量，两者形状虽然
    // 归一过，缓存却不能混——混了以后切回来会拿到另一份的行（有人有 `likes`、
    // 有人没有）。
    vrccClubs: (
        source: 'public' | 'mine',
        limit = 0,
        offset = 0,
        account: string = 'offline'
    ) => ['vrcc', 'clubs', source, limit, offset, account] as const,
    // 一张地图在榜上的状态（board / pending / off）。只有带令牌的接口能区分
    // 「未收录」和「待审核」，所以独立一个键，不与公开页共用。
    worldVrccMine: (worldId: string, account: string = 'offline') =>
        ['world', worldId, 'vrcc', 'mine', account] as const,
    groupMembers: (params: VrchatGroupMembersInput, endpoint: string = '') =>
        withEndpoint(
            ['group', params.groupId, 'members', stableParams(params)],
            endpoint
        ),
    groupGallery: (params: VrchatGroupGalleryInput, endpoint: string = '') =>
        withEndpoint(
            [
                'group',
                params.groupId,
                'gallery',
                params.galleryId,
                stableParams(params)
            ],
            endpoint
        ),
    groupCalendarList: (
        kind: 'aggregate' | 'group' | 'following',
        params:
            | AggregateCalendarQueryParams
            | CalendarListParams
            | { groupId: string },
        endpoint: string = ''
    ) => withEndpoint(['calendar', kind, stableParams(params)], endpoint),
    groupCalendarEvent: (
        {
            groupId = '',
            eventId = ''
        }: { eventId?: string; groupId?: string } = {},
        endpoint: string = ''
    ) => withEndpoint(['calendar', groupId, eventId], endpoint),
    economyPurchases: (userId: string, endpoint: string = '') =>
        withEndpoint(['economy', 'purchases', userId], endpoint),
    economyTransactions: (userId: string, endpoint: string = '') =>
        withEndpoint(['economy', 'transactions', userId], endpoint),
    economyBalance: (userId: string, endpoint: string = '') =>
        withEndpoint(['economy', 'balance', userId], endpoint),
    economyStore: (storeId: string, endpoint: string = '') =>
        withEndpoint(['economy', 'store', storeId], endpoint),
    licensedAvatars: (userId: string, endpoint: string = '') =>
        withEndpoint(['avatars', 'licensed', userId], endpoint),
    worldRooms: (worldId: string, endpoint: string = '') =>
        withEndpoint(['world', worldId, 'rooms'], endpoint),
    avatarGallery: (avatarId: string, endpoint: string = '') =>
        withEndpoint(['avatar', avatarId, 'gallery'], endpoint),
    userInventoryItem: (
        {
            inventoryId = '',
            userId = ''
        }: { inventoryId?: string; userId?: string } = {},
        endpoint: string = ''
    ) => withEndpoint(['inventory', 'item', userId, inventoryId], endpoint),
    inventoryTemplate: (inventoryTemplateId: string, endpoint: string = '') =>
        withEndpoint(['inventory', 'template', inventoryTemplateId], endpoint),
    cosmeticsIndex: (itemType: string, endpoint: string = '') =>
        withEndpoint(['cosmetics', 'index', itemType], endpoint),
    boopEmoji: (userId: string, inventoryId: string, endpoint: string = '') =>
        withEndpoint(['boopEmoji', userId, inventoryId], endpoint),
    fileAnalysis: (
        {
            fileId = '',
            version = 0,
            variant = ''
        }: { fileId?: string; variant?: string; version?: number } = {},
        endpoint: string = ''
    ) => withEndpoint(['analysis', fileId, version, variant], endpoint),
    avatarStyles: (endpoint: string = '') =>
        withEndpoint(['avatar', 'styles'], endpoint),
    representedGroup: (userId: string, endpoint: string = '') =>
        withEndpoint(['user', userId, 'representedGroup'], endpoint),
    userMutualFriends: (userId: string, endpoint: string = '') =>
        withEndpoint(['user', userId, 'mutualFriends'], endpoint),
    userDialogTabCounts: (
        params: UserDialogTabCountQueryParams,
        endpoint: string = ''
    ) =>
        withEndpoint(
            ['user', params.userId, 'dialogTabCounts', stableParams(params)],
            endpoint
        ),
    worldPersistData: (
        {
            userId = '',
            worldId = ''
        }: { userId?: string; worldId?: string } = {},
        endpoint: string = ''
    ) => withEndpoint(['world', worldId, 'persistData', userId], endpoint)
});

function toQueryOptions(
    policy: EntityQueryPolicy,
    overrides: Record<string, unknown> = {}
) {
    return {
        staleTime: policy.staleTime,
        gcTime: policy.gcTime,
        retry: policy.retry,
        refetchOnWindowFocus: policy.refetchOnWindowFocus,
        ...overrides
    };
}

async function fetchWithEntityPolicy<TData = unknown>({
    queryKey,
    policy,
    queryFn,
    force = false
}: FetchWithEntityPolicyOptions<TData>): Promise<{
    data: TData;
    cache: boolean;
}> {
    const staleTime = force ? 0 : policy.staleTime;
    const queryState = queryClient.getQueryState(queryKey);
    const cache =
        !force &&
        Boolean(queryState?.dataUpdatedAt) &&
        staleTime > 0 &&
        Date.now() - (queryState?.dataUpdatedAt ?? 0) < staleTime;

    const data = await queryClient.fetchQuery<TData>({
        queryKey,
        queryFn,
        ...toQueryOptions(policy, { staleTime })
    });

    return {
        data,
        cache
    };
}

export async function fetchCachedData<TData = unknown>(
    options: FetchWithEntityPolicyOptions<TData>
): Promise<TData> {
    const { data } = await fetchWithEntityPolicy(options);
    return data;
}

export function setCachedQueryData<TData = unknown>(
    queryKey: QueryKey,
    data: TData
) {
    queryClient.setQueryData(queryKey, data);
}

export function getCachedQueryData<TData = unknown>(queryKey: QueryKey) {
    return queryClient.getQueryData<TData>(queryKey);
}

export function invalidateEntityQueries(queryKey: QueryKey) {
    return queryClient.invalidateQueries({
        queryKey,
        refetchType: 'active'
    });
}

export async function clearEntityQueryCache() {
    await queryClient.cancelQueries();
    queryClient.clear();
}
