// SPDX-License-Identifier: MPL-2.0

import {
    ChevronLeft,
    ChevronRight,
    ChevronsLeft,
    ChevronsRight,
    ExternalLink,
    RefreshCw,
    Search,
    Sprout,
    X
} from 'lucide-react';
import { Fragment, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { VrccStars } from '@/components/vrcc/VrccStars';
import { Button } from '@/ui/shadcn/button';
import { Input } from '@/ui/shadcn/input';
import { Spinner } from '@/ui/shadcn/spinner';
import {
    ToggleGroup,
    ToggleGroupItem,
    ToggleGroupSeparator
} from '@/ui/shadcn/toggle-group';

import { buildPageItems } from '../boardPages';
import {
    hotTextFor,
    type VrccBoardView,
    type VrccBoardViewSort
} from '../boardRow';
import { useOpenExternalLink } from '../useOpenExternalLink';
import { useVrccRowVisibility } from '../useVrccRowVisibility';
import { useVrccWorldCovers } from '../useVrccWorldCovers';
import { vrccCoverUrl } from '../vrccCover';
import { VrccCover } from './VrccCover';

/** 公开榜能用的三种排序。 */
const PUBLIC_SORTS: readonly VrccBoardViewSort[] = ['top', 'week', 'new'];

/** 一页多少行。公开榜一次取 100 条，玩家视角一次最多 60 条，本地按这个数切页。 */
const PAGE_SIZE = 20;

function isBoardSort(value: unknown): value is VrccBoardViewSort {
    return (
        value === 'top' ||
        value === 'week' ||
        value === 'new' ||
        value === 'went'
    );
}

const SITE = 'https://vrcc.cc';

/**
 * 地图榜。
 *
 * 数据可以来自两个接口，面板**不关心是哪个**——`boards` 已经是归一化后的
 * [`VrccBoardView`]。唯一的例外是排序按钮与搜索框：`went`（我去过的）和
 * 搜索都是玩家视角独有的，公开榜下不显示，而不是显示了点了必然失败。
 *
 * 密度取向：一行一张图，名次 + 名字 + 作者 + 均分 + 评分人数，全部单行，
 * 靠右对齐数字；悬停才出现外链，静止时不留视觉噪声。
 */
export function VrccMapsPanel({
    boards,
    sort,
    sortSupported,
    query,
    onSortChange,
    onQueryChange,
    onRefresh,
    onOpenWorld
}: {
    boards: VrccBoardView | null;
    sort: VrccBoardViewSort;
    sortSupported: boolean;
    /** 已生效的搜索词，空串表示不搜索。 */
    query: string;
    onSortChange: (sort: VrccBoardViewSort) => void;
    onQueryChange: (query: string) => void;
    onRefresh: () => void;
    /** 点一行时打开 VRCX-0 自己的世界详情窗，不跳网页。 */
    onOpenWorld: (worldId: string) => void;
}) {
    const { t } = useTranslation();
    // 榜单每行右侧那个「去网页看」：点下去要有即时反馈，否则玩家会重复点。
    const { open: openLink, pending: openLinkPending } = useOpenExternalLink();
    const maps = boards?.maps ?? [];
    const isPlayerView = boards?.source === 'mine';

    // 「我去过的」只有玩家视角有。公开榜下不显示这个按钮——摆一个点了必然
    // 退回总榜的按键，比没有它更让人困惑。
    const sorts = useMemo(
        () =>
            isPlayerView
                ? ([...PUBLIC_SORTS, 'went'] as VrccBoardViewSort[])
                : PUBLIC_SORTS,
        [isPlayerView]
    );

    /**
     * 搜索框里的**草稿**。和已生效的 `query` 分开：搜索走 `/me/board?q=`，
     * 每个新词在限流器里都是新槽，边敲边搜等于把请求全放出去。所以草稿只在
     * 玩家按下回车时才提交。
     */
    const [draft, setDraft] = useState(query);
    useEffect(() => {
        setDraft(query);
    }, [query]);

    /**
     * 搜索框展开着没有（聚焦，或里面有字）。
     *
     * 展开时排序胶囊整组收起：搜索走 `?q=`，服务器此时**忽略 `sort`**，
     * 留着那一排按钮既点不出效果、又占地方。
     */
    const [searchFocused, setSearchFocused] = useState(false);
    const searchExpanded =
        isPlayerView && (searchFocused || draft.trim().length > 0);

    /**
     * 一次取满、本地分页。
     *
     * 公开榜 `limit` 上限 100；玩家视角没有 limit（服务器固定最多 60）。
     * 两种都是「一次拿完」，翻页不再发请求。
     */
    const [page, setPage] = useState(0);
    const pageCount = Math.max(1, Math.ceil(maps.length / PAGE_SIZE));
    // 换排序/换搜索词后整个列表换掉了，页码可能越界；夹一次比在每个回调里
    // 各自重置更省心，也覆盖「这次回的条数比上次少」这种情况。
    const safePage = Math.min(page, pageCount - 1);

    // 换排序或换搜索词就是换了一份列表，页码要回到第 1 页——夹紧只覆盖
    // 「页码越界」，覆盖不了「本周榜也有 5 页、于是停在第 3 页」这种跨列表沿用。
    useEffect(() => {
        setPage(0);
    }, [sort, query]);
    const pageRows = maps.slice(
        safePage * PAGE_SIZE,
        (safePage + 1) * PAGE_SIZE
    );
    const min = boards?.min;

    // 封面：VRChat 的记录（清楚）优先，没有就退到星遇的小图；
    // 不可视的行连星遇的图也不挂，省下那次请求。
    // 依赖 `boards` 而不是上面那个 `?? []` 出来的 `maps`：后者为空时每次
    // 渲染都是新数组，会让 useMemo 形同虚设。
    const worldIds = useMemo(
        () => (boards?.maps ?? []).map((map) => map.world),
        [boards]
    );
    const { visible, observe } = useVrccRowVisibility();
    const covers = useVrccWorldCovers(worldIds, visible);

    const submitSearch = () => {
        const next = draft.trim();
        if (next === query) return;
        onQueryChange(next);
    };

    return (
        <div className="flex h-full min-h-0 flex-col">
            <div className="text-muted-foreground flex shrink-0 items-center gap-2 px-1 pb-2 text-xs">
                {/*
                 * 排序胶囊。搜索展开时整组收起（宽度和透明度一起走，看起来像被
                 * 吸进左边），因为搜索时服务器**忽略 `sort`**：留着那一排按钮
                 * 点下去只有高亮会变，列表纹丝不动。
                 */}
                <div
                    data-tour="vrcc-maps-sorts"
                    className={`flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ${
                        searchExpanded
                            ? 'pointer-events-none max-w-0 opacity-0'
                            : 'max-w-[26rem] opacity-100'
                    }`}
                >
                    {sortSupported && (
                        <ToggleGroup
                            variant="outline"
                            size="sm"
                            value={[sort]}
                            onValueChange={(next) => {
                                const value = next[0];
                                if (isBoardSort(value)) onSortChange(value);
                            }}
                        >
                            {sorts.map((value, index) => (
                                <Fragment key={value}>
                                    {index > 0 ? (
                                        <ToggleGroupSeparator />
                                    ) : null}
                                    <ToggleGroupItem
                                        value={value}
                                        className="text-xs"
                                    >
                                        {t(`vrcc.maps.sort.${value}`)}
                                    </ToggleGroupItem>
                                </Fragment>
                            ))}
                        </ToggleGroup>
                    )}
                </div>
                {/*
                 * `min` 只在榜单拉到之后才有值。加载中、拉失败、或旧服务器没给
                 * 这个字段时都不该渲染——否则会显示成「满  人评分才进总榜」。
                 * 搜索时也不显示：那时候列表已经不是榜单了。
                 *
                 * 这一格是 `flex-1`：它的存在把右边那组（搜索框 + 刷新）推到
                 * 行尾，也让搜索框的百分比宽度是相对**整行**算的。
                 */}
                <span className="min-w-0 flex-1 truncate">
                    {min != null && !searchExpanded
                        ? t('vrcc.maps.hint', { min })
                        : ''}
                </span>
                {/*
                 * 搜索只有玩家视角有（`/boards/maps` 没有 `q` 参数）。
                 *
                 * 平时收成一窄条待在右边，聚焦或有内容时向左展开。展开宽度
                 * 封在 45%：窗口再窄也不会越过中线去压左边的文字。
                 */}
                {isPlayerView && (
                    <div
                        className={`shrink-0 transition-[width] duration-200 ${
                            searchExpanded
                                ? 'w-[36rem] max-w-[45%]'
                                : 'w-64 max-w-[45%]'
                        }`}
                    >
                        <div className="relative">
                            <Search className="pointer-events-none absolute top-1/2 left-2 size-3 -translate-y-1/2" />
                            <Input
                                value={draft}
                                onChange={(event) =>
                                    setDraft(event.target.value)
                                }
                                onFocus={() => setSearchFocused(true)}
                                onBlur={() => setSearchFocused(false)}
                                onKeyDown={(event) => {
                                    if (event.key === 'Enter') submitSearch();
                                    if (event.key === 'Escape') {
                                        setDraft('');
                                        onQueryChange('');
                                    }
                                }}
                                placeholder={t('vrcc.maps.search')}
                                aria-label={t('vrcc.maps.search')}
                                className="h-7 pr-7 pl-7 text-xs"
                            />
                            {draft.length > 0 && (
                                <Button
                                    variant="ghost"
                                    size="icon"
                                    className="absolute top-1/2 right-0.5 size-6 -translate-y-1/2"
                                    aria-label={t('vrcc.maps.search_clear')}
                                    onClick={() => {
                                        setDraft('');
                                        onQueryChange('');
                                    }}
                                >
                                    <X className="size-3" />
                                </Button>
                            )}
                        </div>
                    </div>
                )}
                <Button
                    variant="ghost"
                    size="icon"
                    className="size-6 shrink-0"
                    onClick={onRefresh}
                    aria-label={t('common.actions.refresh')}
                >
                    <RefreshCw className="size-3" />
                </Button>
            </div>

            {/* 公开榜时说明一下玩家视角能多看到什么。已经连上却缺权限时这句
                也成立——那时点了重新授权就能拿到。 */}
            {boards != null && !isPlayerView && (
                <p className="text-muted-foreground shrink-0 px-1 pb-2 text-xs">
                    {t('vrcc.maps.player_hint')}
                </p>
            )}

            {maps.length === 0 ? (
                <div className="text-muted-foreground flex h-32 items-center justify-center gap-2 text-sm">
                    <Spinner className="size-3" />
                    {query
                        ? t('vrcc.maps.search_empty', { query })
                        : t('vrcc.maps.empty')}
                </div>
            ) : (
                <div
                    data-tour="vrcc-maps-list"
                    className="relative min-h-0 flex-1 overflow-y-auto rounded-xl border"
                >
                    {/*
                     * 「换个排法」那一步只点亮榜单**前几行**，而不是整个列表。
                     *
                     * 整块都亮的话，气泡只能落在列表下缘、压住最后几行；而这一步
                     * 只想让玩家看到「切换排序后名次跟着重排」，看前几名就够。
                     * 前几行的并集正好在可视区顶部，气泡落在它下方不挡关键内容。
                     *
                     * 用一个绝对定位的覆盖层来标这个范围，而不是逐行加属性：
                     * 覆盖层不参与布局（`pointer-events: none`），布局完全不变，
                     * 而多元素高亮要的是**一个可测量的矩形**。
                     *
                     * 高度按行高估算即可——它只是给导览画框用的，不必精确到像素。
                     * 悬停列表时淡出，避免那块半透明区域看起来像加载遮罩。
                     */}
                    <div
                        data-tour="vrcc-maps-head"
                        aria-hidden="true"
                        className="pointer-events-none absolute inset-x-0 top-0 z-10 h-[268px]"
                    />
                    <ul className="divide-y">
                        {pageRows.map((map, index) => {
                            // 本周榜只按本周的打分排，`rank` 却还是总榜名次：
                            // 本周的名次就是在列表里的位置。玩家视角的本周榜
                            // **根本没有 `rank`**，所以两条路都走这个兜底。
                            const place =
                                sort === 'week'
                                    ? safePage * PAGE_SIZE + index + 1
                                    : map.rank;
                            // 副标题下面的小标记，按有用程度排序，最多三个——
                            // 再多会把这一行挤满，而它们都是可选信息。
                            const badges: string[] = [];
                            if (map.room === true) {
                                badges.push(t('vrcc.maps.room'));
                            }
                            if (map.mine != null) {
                                badges.push(
                                    t('vrcc.maps.mine', { score: map.mine })
                                );
                            }
                            if (map.went != null) {
                                badges.push(
                                    t('vrcc.maps.went_count', {
                                        count: map.went
                                    })
                                );
                            }
                            if (map.cap != null) {
                                badges.push(
                                    t('vrcc.maps.cap', { count: map.cap })
                                );
                            }
                            // 多条短评时按世界 ID 挑一条（见 `hotTextFor`）：
                            // 各张图不一样，但同一张图永远同一条，不会跳。
                            const hotText = hotTextFor(map);
                            return (
                                <li
                                    key={map.world}
                                    // 导览靠它拿到「玩家此刻看得见的第一行」是哪个
                                    // 世界，再打开那张图的详情。从 DOM 取而不是再
                                    // 请求一次榜单：数据已经在界面上了。
                                    data-world={map.world}
                                    // 导览第 6 步要单独高亮「第一行」，让玩家看清
                                    // 「接下来要打开的是这一行」，而不是让详情凭空
                                    // 弹出来。只标第一行：多行都标就没有「具体某一
                                    // 行」这个语义了。
                                    {...(index === 0
                                        ? { 'data-tour': 'vrcc-maps-first-row' }
                                        : {})}
                                    // 用整个 `<li>` 观察，而不是那个 32px 的图：
                                    // 判断的是「这一行有没有露在视口里」。
                                    ref={(element) =>
                                        observe(element, map.world)
                                    }
                                    className="hover:bg-muted/40 group flex items-center gap-3 px-3 py-2"
                                >
                                    {/* 点整行打开 VRCX-0 自己的世界详情窗：这是
                                    官方插件的做法，玩家熟悉，也不需要联网。 */}
                                    <button
                                        type="button"
                                        className="flex min-w-0 flex-1 items-center gap-3 text-left"
                                        onClick={() => onOpenWorld(map.world)}
                                    >
                                        <span
                                            className={
                                                (place ?? 99) <= 3
                                                    ? 'text-primary w-6 shrink-0 text-right text-sm font-bold tabular-nums'
                                                    : 'text-muted-foreground w-6 shrink-0 text-right text-sm tabular-nums'
                                            }
                                        >
                                            {place ?? '—'}
                                        </span>
                                        {/* 封面。两级来源：
                                        1. VRChat 的世界图（清楚，来自本地缓存）；
                                        2. 缓存没命中且**这一行可见**时，用星遇
                                           的小图顶上，保证不空窗。
                                        不可视的行两者都不挂——星遇那次请求省下来，
                                        VRChat 的记录交给后台预载，滚下去时就有了。
                                        加载失败时组件整体返回 null，行退回纯文字。 */}
                                        <VrccCover
                                            src={
                                                covers[map.world] ??
                                                (visible.has(map.world)
                                                    ? vrccCoverUrl(
                                                          map.imgUrl,
                                                          map.img
                                                      )
                                                    : null)
                                            }
                                            className="size-8 shrink-0 rounded object-cover"
                                        />
                                        <div className="min-w-0 flex-1">
                                            {/* 第一行：地图名 +（青少年可进标记紧随其后）
                                            + 定宽的介绍首行，靠右贴着星级。
                                            介绍比作者名有信息量，又不该占掉整行，
                                            所以给它一个固定长度，放不下自己省略。 */}
                                            <div className="flex min-w-0 items-center gap-2">
                                                <span className="flex min-w-0 flex-1 items-center gap-1">
                                                    <span className="min-w-0 truncate text-sm font-medium">
                                                        {map.name ?? map.world}
                                                    </span>
                                                    {/*
                                                     * 「青少年可进」的绿色小苗。
                                                     *
                                                     * `teen` 已被官方弃用，**2026-12-26 起
                                                     * 服务器不再返回**。到期后 `map.teen`
                                                     * 变成 `null`，这里就不再渲染——不需要
                                                     * 改代码。
                                                     *
                                                     * 所以**别给它加兜底**：拿不到就是拿不到，
                                                     * 补一个占位或默认值只会让一个本来就要
                                                     * 下线的标记在界面上留得更久。
                                                     */}
                                                    {map.teen === true && (
                                                        <Sprout
                                                            className="size-3 shrink-0 text-emerald-500 opacity-40"
                                                            aria-label={t(
                                                                'vrcc.maps.teen'
                                                            )}
                                                        />
                                                    )}
                                                </span>
                                                {map.intro != null && (
                                                    <span className="text-muted-foreground w-80 max-w-[40%] shrink-0 truncate text-right text-xs">
                                                        {map.intro}
                                                    </span>
                                                )}
                                            </div>
                                            {/* 第二行：短评 / 作者 + 可选标记。
                                            `hotBy`（出处）只有公开榜有：玩家视角
                                            那个 `hot` 是另一个形状，没有出处字段。

                                            多条短评时随机挑一条，但同一份榜单内固定，
                                            不会边滚边跳。 */}
                                            <div className="text-muted-foreground flex min-w-0 items-center gap-1.5 text-xs">
                                                <span className="min-w-0 truncate">
                                                    {hotText
                                                        ? `「${hotText}」${
                                                              map.hotBy
                                                                  ? ` · ${map.hotBy}`
                                                                  : ''
                                                          }`
                                                        : (map.author ?? '—')}
                                                </span>
                                                {badges
                                                    .slice(0, 3)
                                                    .map(
                                                        (badge, badgeIndex) => (
                                                            <span
                                                                key={badge}
                                                                className={
                                                                    badgeIndex ===
                                                                        0 &&
                                                                    map.room ===
                                                                        true
                                                                        ? 'text-primary shrink-0'
                                                                        : 'shrink-0'
                                                                }
                                                            >
                                                                · {badge}
                                                            </span>
                                                        )
                                                    )}
                                            </div>
                                        </div>
                                        {/* 星级在上、分数在下，右侧一列。
                                        星负责扫视排序，数字负责精确比较——
                                        五档星分不出 8.4 和 8.2，两者缺一不可。
                                        星单独占一行会把行高抬高约 14px，这是
                                        有意的：当前布局下 20 行里约 18 行都在
                                        视口内，行高不敏感，可读性更重要。 */}
                                        <div className="flex shrink-0 flex-col items-end gap-0.5">
                                            <VrccStars
                                                score={map.avg}
                                                size="sm"
                                            />
                                            <div className="flex items-baseline gap-1">
                                                <span className="text-sm font-semibold tabular-nums">
                                                    {map.avg != null
                                                        ? map.avg.toFixed(1)
                                                        : '—'}
                                                </span>
                                                <span className="text-muted-foreground text-[10px] tabular-nums">
                                                    {/* 本周榜显示本周几人打分，和网页上的「本周 N」一样。 */}
                                                    {sort === 'week' &&
                                                    map.week != null
                                                        ? t(
                                                              'vrcc.maps.week_count',
                                                              {
                                                                  count: map.week
                                                              }
                                                          )
                                                        : (map.raters ?? 0)}
                                                </span>
                                            </div>
                                        </div>
                                    </button>
                                    {/* 外链单独留一个：想去网页看详情时用，
                                    和「在 VRCX 里看」是两件事。玩家视角不给
                                    `page`，用 world 自己拼——和公开榜给的地址
                                    是同一条路径。 */}
                                    <button
                                        type="button"
                                        disabled={openLinkPending}
                                        onClick={() =>
                                            openLink(
                                                map.page ??
                                                    `${SITE}/maps/${map.world}`
                                            )
                                        }
                                        className="text-muted-foreground hover:text-primary shrink-0 opacity-0 transition-opacity group-hover:opacity-100"
                                        aria-label={t('vrcc.maps.open')}
                                    >
                                        <ExternalLink className="size-3.5" />
                                    </button>
                                </li>
                            );
                        })}
                    </ul>
                </div>
            )}
            {/* 本地翻页：一次已取满，点这些按钮不再发请求。 */}
            {pageCount > 1 && (
                <nav className="mt-2 flex shrink-0 items-center justify-end gap-0.5">
                    <Button
                        variant="ghost"
                        size="sm"
                        className="size-6 p-0"
                        disabled={safePage === 0}
                        aria-label={t('table.pagination.page_number', {
                            page: 1
                        })}
                        onClick={() => setPage(0)}
                    >
                        <ChevronsLeft className="size-3.5" />
                    </Button>
                    <Button
                        variant="ghost"
                        size="sm"
                        className="size-6 p-0"
                        disabled={safePage === 0}
                        aria-label={t('table.pagination.previous')}
                        onClick={() => setPage(safePage - 1)}
                    >
                        <ChevronLeft className="size-3.5" />
                    </Button>
                    {buildPageItems(safePage + 1, pageCount).map((item) =>
                        item.kind === 'gap' ? (
                            <span
                                key={item.key}
                                aria-hidden="true"
                                className="text-muted-foreground w-4 text-center text-xs"
                            >
                                …
                            </span>
                        ) : (
                            <Button
                                key={item.page}
                                variant={
                                    item.page === safePage + 1
                                        ? 'secondary'
                                        : 'ghost'
                                }
                                size="sm"
                                className="h-6 min-w-6 px-1 text-xs tabular-nums"
                                aria-current={
                                    item.page === safePage + 1
                                        ? 'page'
                                        : undefined
                                }
                                aria-label={t('table.pagination.page_number', {
                                    page: item.page
                                })}
                                onClick={() => setPage(item.page - 1)}
                            >
                                {item.page}
                            </Button>
                        )
                    )}
                    <Button
                        variant="ghost"
                        size="sm"
                        className="size-6 p-0"
                        disabled={safePage >= pageCount - 1}
                        aria-label={t('table.pagination.next')}
                        onClick={() => setPage(safePage + 1)}
                    >
                        <ChevronRight className="size-3.5" />
                    </Button>
                    <Button
                        variant="ghost"
                        size="sm"
                        className="size-6 p-0"
                        disabled={safePage >= pageCount - 1}
                        aria-label={t('table.pagination.page_number', {
                            page: pageCount
                        })}
                        onClick={() => setPage(pageCount - 1)}
                    >
                        <ChevronsRight className="size-3.5" />
                    </Button>
                </nav>
            )}
        </div>
    );
}
