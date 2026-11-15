// SPDX-License-Identifier: MPL-2.0

/**
 * 社团列表两个数据源的归一化。
 *
 * ## 为什么需要这一层
 *
 * 和地图榜同一个套路（见 `boardRow.ts`），面板不该知道这次数据是哪个接口来的。
 * 两套接口的差集：
 *
 * | | `/clubs`（公开） | `/me/clubs`（玩家） |
 * |---|---|---|
 * | 令牌 | 不需要 | 需要 `clubs` |
 * | 条数 | `limit` 1–100 + `offset` | **全量，没有分页参数** |
 * | 总数 | 有 `total` / `next` | **没有** |
 * | 额外 | —— | `likes` / `followed` / `member` / `room` / `event` |
 * | 可见范围 | 只公开社团 | 含只给成员看的 |
 *
 * ## `total` 是这一层最要紧的字段
 *
 * 地图榜是「玩家视角更少」（固定最多 60 条），降级到公开反而更多；社团榜**反过来**
 * ——玩家视角更全，但拿不到总数。所以：
 *
 * - `total: null` 一律表示「这个接口不给」，**绝不能用 `rows.length` 顶上**；
 *   那只是「这次拿到几条」，冒充总数就是撒谎。
 * - 面板看到 `null` 就把「共 N 个」和分页器整条隐藏。
 */

import type { VrccClub, VrccClubs, VrccMyClub, VrccMyClubs } from './vrccApi';

/** 这次列表是哪来的。 */
export type VrccClubSource = 'public' | 'mine';

/**
 * 归一化之后的一行。
 *
 * **`null` 一律表示「这个接口不给这个字段」**，不表示「值为空」：公开列表里
 * `followed` 永远是 `null`（它不知道你是谁），而玩家视角里真的没关注时是
 * `false`。面板要靠这个区分「显示未关注」和「这一列不显示」。
 */
export type VrccClubRow = {
    code: string;
    name: string | null;
    lede: string | null;
    tags: string[];
    /** VRChat 群组人数。 */
    members: number | null;
    /** 挂着这个社团的星有几颗（本站用户），不是群组人数。 */
    here: number;
    since: number | null;
    grade: number | null;
    gradeName: string | null;
    icon: string | null;
    page: string | null;
    group: string | null;
    /** 点赞数。只有玩家视角给——而榜就是按它排的。 */
    likes: number | null;
    followed: boolean | null;
    member: boolean | null;
    room: boolean | null;
    event: boolean | null;
};

/** 面板真正消费的形状。 */
export type VrccClubListView = {
    source: VrccClubSource;
    rows: VrccClubRow[];
    /** 一共有几个公开社团；`null` = 这个接口不给。 */
    total: number | null;
    /** 下一页的 `offset`；`null` = 没有下一页，或这个接口不支持翻页。 */
    next: number | null;
};

function fromClub(club: VrccClub): VrccClubRow {
    return {
        code: typeof club.code === 'string' ? club.code : '',
        name: club.name ?? null,
        lede: club.lede ?? null,
        tags: Array.isArray(club.tags) ? club.tags.filter(Boolean) : [],
        members: typeof club.members === 'number' ? club.members : null,
        here: typeof club.here === 'number' ? club.here : 0,
        since: typeof club.since === 'number' ? club.since : null,
        grade: typeof club.grade === 'number' ? club.grade : null,
        gradeName: club.gradeName ?? null,
        icon: club.icon ?? null,
        page: club.page ?? null,
        group: club.group ?? null,
        // 公开接口不给这几项，全部落 `null`——不是 `false`、不是 `0`。
        likes: null,
        followed: null,
        member: null,
        room: null,
        event: null
    };
}

/** `GET /clubs` → 面板形状。有 `total` / `next`，能翻页。 */
export function fromPublicClubs(
    response: VrccClubs | null | undefined
): VrccClubListView {
    const clubs = Array.isArray(response?.clubs) ? response.clubs : [];
    return {
        source: 'public',
        rows: clubs.filter((club) => Boolean(club)).map(fromClub),
        // 老服务器没有 `total`：保持 `null`，让面板别编数字。
        total: typeof response?.total === 'number' ? response.total : null,
        next: typeof response?.next === 'number' ? response.next : null
    };
}

/** `GET /me/clubs` → 面板形状。**没有 `total` / `next`**，这两个字段只能是 `null`。 */
export function fromMyClubs(
    response: VrccMyClubs | null | undefined
): VrccClubListView {
    const clubs = Array.isArray(response?.clubs) ? response.clubs : [];
    return {
        source: 'mine',
        rows: clubs.filter((club) => Boolean(club)).map(fromMyClub),
        total: null,
        next: null
    };
}

function fromMyClub(club: VrccMyClub): VrccClubRow {
    return {
        // `VrccMyClub` 是扁平化之后的一份 `VrccClub` 加玩家视角字段，
        // 所以公共字段直接复用同一个适配器，只在后面覆盖差集。
        ...fromClub(club),
        likes: typeof club.likes === 'number' ? club.likes : null,
        followed: typeof club.followed === 'boolean' ? club.followed : null,
        member: typeof club.member === 'boolean' ? club.member : null,
        room: typeof club.room === 'boolean' ? club.room : null,
        event: typeof club.event === 'boolean' ? club.event : null
    };
}
