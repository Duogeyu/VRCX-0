// SPDX-License-Identifier: MPL-2.0

/**
 * 地图榜两个数据源的归一化。
 *
 * ## 为什么需要这一层
 *
 * 榜单有两套接口，字段**几乎不重叠**：
 *
 * | | `/boards/maps`（公开） | `/me/board`（玩家） |
 * |---|---|---|
 * | 令牌 | 不需要 | 需要 `maps` |
 * | 一次条数 | `limit` 1–100 | **固定最多 60，没有分页参数** |
 * | 排序 | `top` `week` `new` | `top` `week` `new` **`went`** |
 * | 搜索 | 没有 | `q=`（地图名或作者） |
 *
 * 面板不应该知道这次数据是哪个接口来的——它只认 [`VrccBoardRow`]。
 * 所以两个适配器把差集吃在自己这一层：公开榜没有的字段填 `null`，
 * 而不是让面板去猜「这个字段是没值，还是这个接口不给」。
 *
 * ## 行字段刻意沿用公开榜的名字
 *
 * `world` / `name` / `avg` / `rank` / `week` / `img` / `page` 这些名字和
 * `VrccBoardMap` 一样，面板的渲染逻辑因此几乎不用改。真正换名的只有热评：
 * 两个接口的 `hot` **同名不同形**（见 {@link VrccBoardRow.hotText}）。
 */

import type {
    VrccBoard,
    VrccBoardMap,
    VrccMyBoard,
    VrccMyBoardMap
} from './vrccApi';

/** 这次的地图榜是哪来的。 */
export type VrccBoardSource = 'public' | 'mine';

/** 地图榜的排序。`went` 只有玩家视角支持。 */
export type VrccBoardViewSort = 'top' | 'week' | 'new' | 'went';

/**
 * 归一化之后的一行。
 *
 * **`null` 一律表示「这个接口不给这个字段」，不表示「值为空」**。举例：
 * 公开榜的 `went` 永远是 `null`（它不知道你是谁），而 `/me/board` 里
 * 真的没去过时是 `0`。面板要区分这两种，才能决定「显示 0 次」还是
 * 「这一列不显示」。
 */
export type VrccBoardRow = {
    world: string;
    name: string | null;
    author: string | null;
    avg: number | null;
    raters: number | null;
    /**
     * 总榜名次。
     *
     * **`/me/board` 的本周榜没有这个字段**（那时名次就是列表位置），
     * 所以调用方不能假设它一定有值。
     */
    rank: number | null;
    /** 本周几人打分，只有本周榜有。 */
    week: number | null;
    /** 轻重：`light` / `mid` / `heavy`；没测过是 `null`（实测多数图都没测过）。 */
    weight: string | null;
    img: string | null;
    imgUrl: string | null;
    /** vrcc.cc 上的地图页。只有公开榜给；玩家视角要用 `world` 自己拼。 */
    page: string | null;
    /**
     * 可展示的短评候选。
     *
     * 公开榜最多 1 条（`hot`），玩家视角最多 3 条（`hots`）——**多条时随机挑
     * 一条显示**，让每次刷新看到的评价不一样。空数组表示这张图没有短评。
     */
    hotTexts: string[];
    /**
     * 热评是谁写的（「某位派对点灯人」这种）。
     *
     * **只有公开榜有**。玩家视角那个 `hot` 是另一个形状，根本没有出处字段，
     * 所以那边恒为 `null`——这不是解析失败。
     */
    hotBy: string | null;
    /** 赞最多的那段介绍的第一行。只有玩家视角有。 */
    intro: string | null;
    /** 我去过几次。只有玩家视角有。 */
    went: number | null;
    /** 此刻有人在开房。只有玩家视角有。 */
    room: boolean | null;
    /** 房间人数上限。只有玩家视角有。 */
    cap: number | null;
    /**
     * 青少年可进。只有玩家视角有。
     *
     * **会自己消失**：官方弃用表里写明 `teen` 从 **2026-12-26** 起不再返回
     * （「网页不显示，是内部的标记」）。到期后服务器不给这个键，这里变成
     * `null`，面板那个绿色小苗就不渲染了——不需要改代码，也不会崩。
     *
     * 所以**不要**为「拿不到 teen」写兜底或占位：那会让一个本来就要下线的
     * 字段在界面上留下痕迹。到那天直接删掉这个字段和它的渲染即可。
     */
    teen: boolean | null;
    /**
     * 排序真正用的加权分（按评分人数向均值收缩）。
     *
     * 实测同一张图 `avg` 9.6、`adj` 9.47——**服务器是按 `adj` 排名次的**。
     * 只有玩家视角给这个数。
     *
     * 已弃用：和 `teen` 一起在 **2026-12-26** 到期。界面从来没读过它（名次
     * 用服务器给的 `rank`），所以到期后只是这个字段变 `null`，没有可见影响。
     */
    adj: number | null;
    /** 我给这张图打的分。只有 `sort=went` 有。 */
    mine: number | null;
};

/** 一份地图榜。`maps` 是归一化后的行，其余是面板要用的元信息。 */
export type VrccBoardView = {
    maps: VrccBoardRow[];
    /** 进榜需要几人评分（当前为 1）。 */
    min: number | null;
    /** 这一份是哪来的，用于决定要不要显示「连接后可看」的提示。 */
    source: VrccBoardSource;
};

/** 公开榜的一行 → 归一化行。玩家视角独有的字段一律 `null`。 */
export function fromPublicBoardRow(map: VrccBoardMap): VrccBoardRow {
    return {
        world: map.world,
        name: map.name ?? null,
        author: map.author ?? null,
        avg: map.avg ?? null,
        raters: map.raters ?? null,
        rank: map.rank ?? null,
        week: map.week ?? null,
        weight: map.weight ?? null,
        img: map.img ?? null,
        imgUrl: map.imgUrl ?? null,
        page: map.page ?? null,
        hotTexts: map.hot?.text ? [map.hot.text] : [],
        hotBy: map.hot?.by ?? null,
        intro: null,
        went: null,
        room: null,
        cap: null,
        teen: null,
        adj: null,
        mine: null
    };
}

/** 玩家视角的一行 → 归一化行。只有公开榜给的字段填 `null`。 */
export function fromMyBoardRow(map: VrccMyBoardMap): VrccBoardRow {
    // 优先用完整列表；`hots` 为空时才退回单条 `hot`（老服务器只给这一条）。
    const hotTexts = (map.hots ?? [])
        .map((hot) => hot.text)
        .filter((text): text is string => Boolean(text));
    if (hotTexts.length === 0 && map.hot?.text) {
        hotTexts.push(map.hot.text);
    }
    return {
        // 1.11.0 起 `id` 和 `world` 同值；老一点的服务器只给 `id`，
        // 所以两个都试，都拿不到就跳过这一行（见 `fromMyBoard`）。
        world: map.world ?? map.id ?? '',
        name: map.name ?? null,
        author: map.author ?? null,
        avg: map.avg ?? null,
        raters: map.raters ?? null,
        rank: map.rank ?? null,
        week: map.week ?? null,
        weight: map.weight ?? null,
        img: map.img ?? null,
        imgUrl: map.imgUrl ?? null,
        // 玩家视角不给 `page`，交给面板用 `${SITE}/maps/${world}` 兜底。
        page: null,
        hotTexts,
        // 玩家视角的 `hot` 里**没有出处**，这不是解析失败。
        hotBy: null,
        intro: map.intro ?? null,
        went: map.went ?? null,
        room: map.room ?? null,
        cap: map.cap ?? null,
        teen: map.teen ?? null,
        adj: map.adj ?? null,
        mine: map.mine ?? null
    };
}

/**
 * 这张图该显示哪一条短评。
 *
 * ## 为什么按世界 ID 算，而不是 `Math.random()`
 *
 * 玩家视角一次最多给 3 条短评，随机挑一条能让榜单不至于永远是同一句话。
 * 但「随机」在渲染里有个代价：滚动、悬停、翻页、切语言都会重渲染，
 * 每次重掷的话玩家会看到文字在自己眼前跳。
 *
 * 用世界 ID 算一个**稳定**的下标就没有这个问题——各张图看到的短评不一样
 * （看起来是随机的），而同一张图永远落到同一条，重开应用也一样。
 * 顺带还让这个函数是纯的：组件里那个 `useMemo` 不会再去调 `Math.random()`
 * （React Compiler 的 purity 规则会拒绝那种写法）。
 */
export function hotTextFor(row: VrccBoardRow): string | null {
    const count = row.hotTexts.length;
    if (count === 0) return null;
    if (count === 1) return row.hotTexts[0];
    let hash = 0;
    for (let index = 0; index < row.world.length; index += 1) {
        hash = (hash * 31 + row.world.charCodeAt(index)) >>> 0;
    }
    return row.hotTexts[hash % count];
}

export function fromPublicBoard(board: VrccBoard): VrccBoardView {
    return {
        maps: (board.maps ?? []).map(fromPublicBoardRow),
        min: board.min ?? null,
        source: 'public'
    };
}

export function fromMyBoard(board: VrccMyBoard): VrccBoardView {
    return {
        // 拿不到 `world` / `id` 的行没法开详情窗，也没法当 key，直接丢掉
        // 比让 React 收到空 key 好。
        maps: (board.maps ?? [])
            .map(fromMyBoardRow)
            .filter((row) => row.world !== ''),
        min: board.min ?? null,
        source: 'mine'
    };
}
