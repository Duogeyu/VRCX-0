// SPDX-License-Identifier: MPL-2.0

/**
 * 社团列表两个数据源的归一化。面板只认 `VrccClubRow`。
 * 做法与 `boardRow.ts` 相同。
 *
 * 两个数据源的差别：
 *
 * | | `/clubs` 公开 | `/me/clubs` 玩家 |
 * |---|---|---|
 * | 令牌 | 不需要 | 需要 `clubs` 权限 |
 * | 条数 | `limit` 1 至 100，加 `offset` | 全量，无分页参数 |
 * | 总数 | 有 `total` 与 `next` | 没有 |
 * | 额外字段 | 无 | `likes`、`followed`、`member`、`room`、`event` |
 * | 可见范围 | 公开社团 | 含成员专属社团 |
 *
 * `total` 为 `null` 表示该数据源不提供总数。
 * 禁止用 `rows.length` 代替。行数只是本次取回的数量。
 * 面板收到 `null` 时隐藏总数与分页器。
 */

import type { VrccClub, VrccClubs, VrccMyClub, VrccMyClubs } from './vrccApi';

/** 这次列表是哪来的。 */
export type VrccClubSource = 'public' | 'mine';

/**
 * 归一化之后的一行。
 *
 * `null` 表示该数据源不提供这个字段，不表示字段为空。
 * 公开列表的 `followed` 恒为 `null`。玩家视角中未关注时为 `false`。
 * 面板据此决定显示「未关注」，还是隐藏该列。
 */
export type VrccClubRow = {
    code: string;
    name: string | null;
    lede: string | null;
    tags: string[];
    /**
     * 服务器顺序里的位置，从 1 开始。这就是热度名次：
     * 点赞多 → 群组人数多 → 短码。
     *
     * 排序是客户端做的。按别的键重排后，这个值**不变**。
     * 所以换排序时名次仍然是真实名次，不会变成显示位置。
     */
    rank: number;
    /** VRChat 群组人数。 */
    members: number | null;
    /** 挂着这个社团的星有几颗（本站用户），不是群组人数。 */
    here: number;
    since: number | null;
    grade: number | null;
    gradeName: string | null;
    icon: string | null;
    /**
     * 群组横幅。比例 16:9。来源是公开页那张图。成员专属社团没有横幅。
     *
     * 与 `icon` 一样带 `?v=` 版本串。图换后版本串会变，缓存时一并保存。
     */
    banner: string | null;
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
    /** 一共几个公开社团；`null` = 这个接口不给。 */
    total: number | null;
};

function fromClub(club: VrccClub, rank: number): VrccClubRow {
    return {
        code: typeof club.code === 'string' ? club.code : '',
        name: club.name ?? null,
        lede: club.lede ?? null,
        tags: Array.isArray(club.tags) ? club.tags.filter(Boolean) : [],
        rank,
        members: typeof club.members === 'number' ? club.members : null,
        here: typeof club.here === 'number' ? club.here : 0,
        since: typeof club.since === 'number' ? club.since : null,
        grade: typeof club.grade === 'number' ? club.grade : null,
        gradeName: club.gradeName ?? null,
        icon: club.icon ?? null,
        banner: club.banner ?? null,
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

/**
 * `GET /clubs` → 面板形状。
 *
 * 名次按回答里的顺序编号。服务器顺序就是热度顺序，所以这个编号就是真实名次。
 */
export function fromPublicClubs(
    response: VrccClubs | null | undefined
): VrccClubListView {
    const clubs = Array.isArray(response?.clubs) ? response.clubs : [];
    return {
        source: 'public',
        rows: clubs
            .filter((club) => Boolean(club))
            .map((club, index) => fromClub(club, index + 1)),
        // 老服务器没有 `total`：保持 `null`，让面板别编数字。
        total: typeof response?.total === 'number' ? response.total : null
    };
}

/** `GET /me/clubs` → 面板形状。这个接口不提供 `total`，所以它恒为 `null`。 */
export function fromMyClubs(
    response: VrccMyClubs | null | undefined
): VrccClubListView {
    const clubs = Array.isArray(response?.clubs) ? response.clubs : [];
    return {
        source: 'mine',
        rows: clubs
            .filter((club) => Boolean(club))
            .map((club, index) => fromMyClub(club, index + 1)),
        total: null
    };
}

function fromMyClub(club: VrccMyClub, rank: number): VrccClubRow {
    return {
        // `VrccMyClub` 由两部分组成：扁平的 `VrccClub` 字段，加上玩家视角字段。
        // 公共字段复用同一个适配器。下面只覆盖新增字段。
        ...fromClub(club, rank),
        likes: typeof club.likes === 'number' ? club.likes : null,
        followed: typeof club.followed === 'boolean' ? club.followed : null,
        member: typeof club.member === 'boolean' ? club.member : null,
        room: typeof club.room === 'boolean' ? club.room : null,
        event: typeof club.event === 'boolean' ? club.event : null
    };
}
