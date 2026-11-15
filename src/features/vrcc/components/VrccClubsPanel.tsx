// SPDX-License-Identifier: MPL-2.0

import {
    ChevronLeft,
    ChevronRight,
    ChevronsLeft,
    ChevronsRight
} from 'lucide-react';
import { Fragment, useCallback, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useTiltGrid } from '@/lib/useTiltGrid';
import { openGroupDialog } from '@/services/dialogService';
import { openExternalLink } from '@/services/shellIntegrationService';
import { Button } from '@/ui/shadcn/button';
import { Spinner } from '@/ui/shadcn/spinner';
import {
    ToggleGroup,
    ToggleGroupItem,
    ToggleGroupSeparator
} from '@/ui/shadcn/toggle-group';

import { buildPageItems } from '../boardPages';
import type { VrccClubRow } from '../clubRow';
import { useVrccClubs } from '../useVrccClubs';
import type { VrccConnection } from '../vrccApi';

/**
 * 从群组链接取出群组 ID：`https://vrchat.com/home/group/grp_xxx` → `grp_xxx`。
 *
 * 社团就是 VRChat 群组。`code` 等于群组的 `shortCode.discriminator`。
 * 点卡片打开群组详情，不新建详情页。群组详情已有公告、房间、活动、成员、相册、原始 JSON。
 */
function groupIdOf(url: string | null): string | null {
    const match = /\b(grp_[0-9a-fA-F-]{36})\b/.exec(url ?? '');
    return match ? match[1] : null;
}

/** 一页 24 张。左栏到全宽都能排成网格。 */
const PAGE_SIZE = 24;

/** 一次取回多少条。接口上限 100，现在线上不到 20 个社团。 */
const ALL_CLUBS_LIMIT = 100;

/**
 * 排序键。默认是服务器顺序，也就是热度顺序：点赞 → 群组人数 → 短码。
 *
 * 其余三种在客户端排。`/clubs` 没有 `sort` 参数，所以才要一次取全量：
 * 只排当前一页会让第 2 页里的高值社团错位。
 */
export type ClubSort = 'default' | 'grade' | 'members' | 'name';

const SORTS: readonly ClubSort[] = ['default', 'grade', 'members', 'name'];

/** 控件回传的值来自运行时，可能不是这四种之一。 */
function isClubSort(value: unknown): value is ClubSort {
    return (
        typeof value === 'string' &&
        (SORTS as readonly string[]).includes(value)
    );
}

/**
 * 排序。名次 `rank` 不参与计算，只用来定平手时的先后。
 *
 * `grade` 与 `members` 都可能为 `null`。缺值固定排最后，不当作 0。
 * 当成 0 会让「没评级」混在「三等星」后面，看起来像是最低一档。
 */
function sortClubs(
    rows: VrccClubRow[],
    sort: ClubSort,
    collator: Intl.Collator
): VrccClubRow[] {
    if (sort === 'default') {
        // 服务器已经排好，原样返回。名次也就是这个顺序。
        return rows;
    }
    const copy = [...rows];
    copy.sort((left, right) => {
        if (sort === 'name') {
            const diff = collator.compare(
                left.name?.trim() || left.code,
                right.name?.trim() || right.code
            );
            return diff !== 0 ? diff : left.rank - right.rank;
        }
        const a = sort === 'grade' ? left.grade : left.members;
        const b = sort === 'grade' ? right.grade : right.members;
        if (a === null && b === null) {
            return left.rank - right.rank;
        }
        if (a === null) {
            return 1;
        }
        if (b === null) {
            return -1;
        }
        return b - a || left.rank - right.rank;
    });
    return copy;
}

/** 评级配色。与群组详情那张卡一致：金(4)、紫(3)、蓝(2)、绿(1)。 */
const GRADE_STYLE: Record<number, string> = {
    4: 'border-amber-500/50 text-amber-600 dark:text-amber-300',
    3: 'border-violet-500/45 text-violet-600 dark:text-violet-300',
    2: 'border-sky-500/45 text-sky-600 dark:text-sky-300',
    1: 'border-emerald-500/45 text-emerald-600 dark:text-emerald-300'
};

/**
 * 社团卡。上方是横幅图。下方是纸片。纸片压住横幅下沿。
 *
 * 横幅不画圆角。外层 `overflow-hidden` 把横幅两个上角裁成卡片圆角。
 * 纸片有上圆角。纸片上移 36px。纸片上圆角外侧露出横幅。
 *
 * 横幅固定高 96px，用 `object-cover` 取中段。横幅原图比例 16:9。
 * 字段 `banner` 为 `null` 时不渲染横幅。此时纸片贴顶，不加负边距。
 */
function ClubCard({
    club,
    rank,
    onOpen
}: {
    club: VrccClubRow;
    rank: number;
    onOpen: (club: VrccClubRow) => void;
}) {
    const { t } = useTranslation();
    const grade =
        typeof club.grade === 'number' && club.grade >= 1 && club.grade <= 4
            ? club.grade
            : null;
    const gradeLabel =
        grade === null
            ? t('vrcc.clubs.grade_unknown')
            : club.gradeName?.trim() || t(`vrcc.clubs.grade_${grade}`);
    const initial = Array.from(club.name ?? club.code)[0] ?? '?';
    const hasBanner = Boolean(club.banner);

    return (
        // 高度由内容决定，不让网格拉满。`auto-rows-fr` 会把整块高度平均分给每排，
        // 卡片就会被拉成一张空纸。所以这里不加 `h-full`。
        <li className="vrcc-club-card relative flex min-w-0 flex-col" data-tilt>
            {/* 整张卡是一个按钮。点击打开该社团对应的群组详情。卡内不放外链。 */}
            <button
                type="button"
                onClick={() => onOpen(club)}
                aria-label={club.name?.trim() || club.code}
                className="vrcc-club-card__rotor focus-visible:ring-ring/50 flex min-w-0 flex-1 cursor-pointer flex-col rounded-lg text-left focus-visible:ring-2 focus-visible:outline-none"
            >
                {hasBanner ? (
                    // 照片左右各缩进 6px。照片比纸片窄，位于后层。
                    // 圆角画在图片上。外层不建裁剪层，所以圆角可见。
                    <div className="px-1.5">
                        <div className="relative">
                            <img
                                src={club.banner ?? undefined}
                                alt=""
                                aria-hidden
                                loading="lazy"
                                decoding="async"
                                referrerPolicy="no-referrer"
                                className="h-24 w-full rounded-lg object-cover"
                            />
                            {/* 照片下段压暗。接缝因此不突兀。 */}
                            <div className="pointer-events-none absolute inset-0 rounded-lg bg-gradient-to-t from-black/45 to-transparent" />
                            {/* 照片右上角的反光。由柔和镜面光与 1px 顶边光组成。
                            用纯白，不用前景色。底色是照片，白色表示反光。 */}
                            <div className="pointer-events-none absolute inset-0 rounded-lg bg-[radial-gradient(62%_52%_at_100%_0%,rgba(255,255,255,0.30),rgba(255,255,255,0.08)_42%,transparent_70%)] shadow-[inset_0_1px_0_rgba(255,255,255,0.20)]" />
                        </div>
                    </div>
                ) : null}

                <div
                    className={`bg-card/80 relative flex min-w-0 flex-1 flex-col overflow-hidden rounded-lg border p-3 backdrop-blur-sm ${
                        hasBanner
                            ? // 纸片满宽，比照片宽 12px。纸片上移 36px。阴影有两层。
                              // 第一层是接触阴影。第二层是照片上的压暗。
                              '-mt-9 shadow-[0_-2px_6px_-2px_rgba(0,0,0,0.7),0_-14px_26px_-12px_rgba(0,0,0,0.85)]'
                            : ''
                    }`}
                >
                    {/*
                     * 折射带。玻璃弯折背后的内容，本身不发光。
                     * 这条 36px 的带子放横幅的放大模糊副本，不放白色图形。
                     * 副本参数：`scale-105`、`blur-1.2px`、`opacity-55`。
                     * 副本在接缝处与照片对齐，向下渐变回纸的底色。
                     */}
                    {hasBanner ? (
                        <div
                            aria-hidden
                            className="pointer-events-none absolute inset-x-0 top-0 h-9 overflow-hidden"
                        >
                            <img
                                src={club.banner ?? undefined}
                                alt=""
                                aria-hidden
                                // 三个数值必须同时改。照片高 `h-24`（96px），纸片上移
                                // `-mt-9`（36px）。副本上沿 = -(96 - 36) = -60px，即 `-top-15`。
                                // 副本高度等于照片高 `h-24`。漏改会让接缝错位。
                                // `origin-bottom` 让放大时下沿保持不动。
                                className="absolute inset-x-1.5 -top-15 h-24 w-[calc(100%-12px)] origin-bottom scale-105 object-cover opacity-55 blur-[1.2px]"
                            />
                            {/* 向下渐变回纸的底色。色值与纸身相同，都是 `card/80`。
                            否则这里会出现一条比纸更实的横线。 */}
                            <div className="to-card/80 absolute inset-0 bg-gradient-to-b from-transparent via-transparent" />
                        </div>
                    ) : null}
                    {/* 斜面高光。上沿一道白线，下沿一道暗线。 */}
                    <div
                        aria-hidden
                        className="pointer-events-none absolute inset-0 rounded-lg shadow-[inset_0_1px_0_rgba(255,255,255,0.16),inset_0_-1px_0_rgba(0,0,0,0.45)]"
                    />
                    <div className="relative flex min-w-0 flex-1 flex-col gap-2">
                        <div className="flex min-w-0 items-start gap-2">
                            <span className="text-muted-foreground/60 w-5 shrink-0 text-right text-[11px] leading-6 tabular-nums">
                                {rank}
                            </span>
                            {club.icon ? (
                                <img
                                    src={club.icon}
                                    alt=""
                                    aria-hidden
                                    loading="lazy"
                                    referrerPolicy="no-referrer"
                                    className="border-border/50 size-6 shrink-0 rounded-md border object-cover"
                                />
                            ) : (
                                <span className="border-border/50 bg-muted text-muted-foreground flex size-6 shrink-0 items-center justify-center rounded-md border text-[11px] font-medium">
                                    {initial}
                                </span>
                            )}
                            <span className="min-w-0 flex-1 truncate text-sm font-medium">
                                {club.name?.trim() || club.code}
                            </span>
                        </div>

                        {/* 简介固定占两行。没有简介时留空，也占两行。
                            这样不同卡片的纸片一样高，一排的下沿才对齐。 */}
                        <p className="text-muted-foreground line-clamp-2 min-h-[2.4em] min-w-0 text-[11px] leading-snug break-words">
                            {club.lede?.trim() ?? ''}
                        </p>

                        <div className="mt-auto flex min-w-0 flex-wrap items-center gap-1.5 text-[11px]">
                            {grade !== null ? (
                                <span
                                    className={`inline-flex shrink-0 items-center rounded-full border px-1.5 py-0.5 text-[10px] font-medium ${GRADE_STYLE[grade]}`}
                                >
                                    {gradeLabel}
                                </span>
                            ) : (
                                <span className="border-border text-muted-foreground inline-flex shrink-0 items-center rounded-full border px-1.5 py-0.5 text-[10px]">
                                    {t('vrcc.clubs.grade_unknown')}
                                </span>
                            )}
                            {club.members !== null ? (
                                <span className="text-muted-foreground/80 shrink-0 tabular-nums">
                                    {t('vrcc.clubs.members', {
                                        count: club.members
                                    })}
                                </span>
                            ) : null}
                            {/* 点赞数与关注只有玩家视角给；公开列表这两项是 `null`，整项不渲染。 */}
                            {club.likes !== null ? (
                                <span className="text-muted-foreground/80 shrink-0 tabular-nums">
                                    {t('vrcc.clubs.likes', {
                                        count: club.likes
                                    })}
                                </span>
                            ) : null}
                            {club.followed === true ? (
                                <span className="text-muted-foreground/80 shrink-0">
                                    {t('vrcc.clubs.followed')}
                                </span>
                            ) : null}
                        </div>
                    </div>
                </div>
            </button>
        </li>
    );
}

/**
 * 社团列表。
 *
 * 数据源有两个。已连接且令牌含 `clubs` 权限时用 `/me/clubs`。
 * 否则用公开的 `/clubs`。切换逻辑在 `useVrccClubs`。
 *
 * 总数只有公开源提供。玩家视角没有 `total`。此时总数与分页器都不渲染。
 */
export function VrccClubsPanel({
    connection
}: {
    connection: VrccConnection | null;
}) {
    const { t, i18n } = useTranslation();
    const [page, setPage] = useState(0);
    const [sort, setSort] = useState<ClubSort>('default');
    // 模型榜的方图卡用 12°，这里用 7°。这张卡更窄，正文在纸片上。
    // `scale: 1.03` 只给一点层次。再大会与相邻卡重叠。
    const tiltRef = useTiltGrid<HTMLUListElement>({
        amplitude: 7,
        scale: 1.03
    });
    // 一次取全量（接口上限 100）。排序在客户端做，只取一页会导致排序结果失真。
    const { view, isLoading, error, source } = useVrccClubs({
        connection,
        limit: ALL_CLUBS_LIMIT,
        offset: 0
    });
    // 点卡片打开群组详情。`group` 缺失时（部分成员专属社团）退回打开社团页。
    // 这样每次点击都有结果。
    const openClub = useCallback((club: VrccClubRow) => {
        const groupId = groupIdOf(club.group);
        if (groupId) {
            openGroupDialog({
                groupId,
                title: club.name?.trim() || club.code
            });
            return;
        }
        if (club.page) {
            void openExternalLink(club.page);
        }
    }, []);

    const total = view?.total ?? null;
    const sorted = useMemo(
        // 依赖整份 `view`，不是它里面的数组：排序的输入就是「这一份数据」。
        // 排序规则由语言决定（中文按拼音），所以语言变了也要重算。
        () =>
            sortClubs(view?.rows ?? [], sort, new Intl.Collator(i18n.language)),
        [view, sort, i18n.language]
    );
    const pageCount = Math.max(1, Math.ceil(sorted.length / PAGE_SIZE));
    const safePage = Math.min(page, pageCount - 1);
    const visible = sorted.slice(
        safePage * PAGE_SIZE,
        safePage * PAGE_SIZE + PAGE_SIZE
    );

    if (isLoading && !view) {
        return (
            <div className="text-muted-foreground flex items-center gap-2 p-4 text-sm">
                <Spinner className="size-4" />
                {t('vrcc.clubs.loading')}
            </div>
        );
    }

    if (error) {
        return (
            <p className="text-muted-foreground p-4 text-sm">
                {t('vrcc.clubs.error')}
            </p>
        );
    }

    return (
        // `h-full` 不能省。页签内容是块级元素，`flex-1` 在块级父元素里无效。
        // 没有 `h-full` 时网格不会撑满，分页器浮在内容下面，沉不到底部。
        <div className="flex h-full min-h-0 flex-col gap-2 p-1">
            <div className="flex min-w-0 shrink-0 items-center gap-2">
                <ToggleGroup
                    variant="outline"
                    size="sm"
                    value={[sort]}
                    onValueChange={(next) => {
                        const value = next[0];
                        if (isClubSort(value)) {
                            setSort(value);
                            // 换排序后回到第一页，否则会停在一个可能不存在的页码上。
                            setPage(0);
                        }
                    }}
                >
                    {SORTS.map((value, index) => (
                        <Fragment key={value}>
                            {index > 0 ? <ToggleGroupSeparator /> : null}
                            <ToggleGroupItem value={value} className="text-xs">
                                {t(`vrcc.clubs.sort.${value}`)}
                            </ToggleGroupItem>
                        </Fragment>
                    ))}
                </ToggleGroup>
                {total !== null ? (
                    <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                        {t('vrcc.clubs.total', { count: total })}
                    </span>
                ) : null}
                <span className="text-muted-foreground/60 min-w-0 flex-1 truncate text-[11px]">
                    {source === 'mine'
                        ? t('vrcc.clubs.source_mine')
                        : t('vrcc.clubs.source_public')}
                </span>
            </div>

            {sorted.length === 0 ? (
                <p className="text-muted-foreground p-4 text-sm">
                    {t('vrcc.clubs.empty')}
                </p>
            ) : (
                // 倾斜会让边缘卡片超出容器。滚动容器只要一个轴是 `auto`，
                // 另一个轴的 `visible` 会被规范改成 `auto`，因此出现横向滚动条。
                //
                // `px-2.5` 留出余量。倾斜后的边角碰不到裁剪线，所以不会被切掉。
                // `overflow-x-clip` 兜底。`clip` 不创建滚动容器，不产生滚动条。
                // 纵向滚动照常。
                <ul
                    ref={tiltRef}
                    className="grid min-h-0 flex-1 auto-rows-min grid-cols-[repeat(auto-fill,minmax(min(220px,100%),1fr))] gap-2 overflow-x-clip overflow-y-auto px-2.5 pb-1"
                >
                    {visible.map((club, index) => (
                        <ClubCard
                            key={club.code || `${safePage}-${index}`}
                            club={club}
                            rank={club.rank}
                            onOpen={openClub}
                        />
                    ))}
                </ul>
            )}

            {/* 分页器一直显示，和其他榜单一致。单页时四个按钮自然全部置灰。
                不藏起来有两个原因：玩家一眼看出「这里就是全部」；翻到最后一页时
                数据变短，藏掉整行会让下面的内容往上窜一截。 */}
            <nav className="flex shrink-0 items-center justify-end gap-0.5 pt-2">
                <Button
                    variant="ghost"
                    size="sm"
                    className="size-6 p-0"
                    disabled={safePage === 0}
                    aria-label={t('table.pagination.page_number', { page: 1 })}
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
                            key={`gap-${item.key}`}
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
                                item.page === safePage + 1 ? 'page' : undefined
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
        </div>
    );
}
