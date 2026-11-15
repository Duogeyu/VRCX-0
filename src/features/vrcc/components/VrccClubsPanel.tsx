// SPDX-License-Identifier: MPL-2.0

import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Spinner } from '@/ui/shadcn/spinner';

import { buildPageItems } from '../boardPages';
import type { VrccClubRow } from '../clubRow';
import { useVrccClubs } from '../useVrccClubs';
import type { VrccConnection } from '../vrccApi';

/** 一页几张。24 在左栏到全宽之间都能排成整齐的网格。 */
const PAGE_SIZE = 24;

/** 评级配色，和群组详情那张卡保持一致：金(4) → 紫(3) → 蓝(2) → 绿(1)。 */
const GRADE_STYLE: Record<number, string> = {
    4: 'border-amber-500/50 text-amber-600 dark:text-amber-300',
    3: 'border-violet-500/45 text-violet-600 dark:text-violet-300',
    2: 'border-sky-500/45 text-sky-600 dark:text-sky-300',
    1: 'border-emerald-500/45 text-emerald-600 dark:text-emerald-300'
};

function ClubCard({ club, rank }: { club: VrccClubRow; rank: number }) {
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

    return (
        <li className="bg-card relative flex min-w-0 flex-col gap-2 rounded-lg border p-3">
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

            {club.lede?.trim() ? (
                <p className="text-muted-foreground line-clamp-2 min-w-0 text-[11px] leading-snug break-words">
                    {club.lede}
                </p>
            ) : null}

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
                        {t('vrcc.clubs.members', { count: club.members })}
                    </span>
                ) : null}
                {/* 点赞数与关注只有玩家视角给；公开列表这两项是 `null`，整项不渲染。 */}
                {club.likes !== null ? (
                    <span className="text-muted-foreground/80 shrink-0 tabular-nums">
                        {t('vrcc.clubs.likes', { count: club.likes })}
                    </span>
                ) : null}
                {club.followed === true ? (
                    <span className="text-muted-foreground/80 shrink-0">
                        {t('vrcc.clubs.followed')}
                    </span>
                ) : null}
            </div>

            {club.page ? (
                <a
                    href={club.page}
                    target="_blank"
                    rel="noreferrer"
                    className="text-primary hover:text-primary/80 text-[10px] font-medium"
                >
                    {t('vrcc.clubs.open')}
                </a>
            ) : null}
        </li>
    );
}

/**
 * 社团列表（公开目录）。
 *
 * 数据来源在两套接口之间切换：连上且令牌带 `clubs` 时用 `/me/clubs`（多了点赞、
 * 关注、只给成员看的社团），否则用公开 `/clubs`。切换逻辑在 `useVrccClubs`。
 *
 * **总数只在公开来源下存在**：玩家视角没有 `total`，这时「共 N 个」和分页器
 * 整条不渲染——拿拿到的条数冒充总数就是撒谎。
 */
export function VrccClubsPanel({
    connection
}: {
    connection: VrccConnection | null;
}) {
    const { t } = useTranslation();
    const [page, setPage] = useState(0);
    const { view, isLoading, error, source } = useVrccClubs({
        connection,
        limit: PAGE_SIZE,
        offset: page * PAGE_SIZE
    });

    const rows = view?.rows ?? [];
    const total = view?.total ?? null;
    const pageCount =
        total !== null ? Math.max(1, Math.ceil(total / PAGE_SIZE)) : 1;
    const safePage = Math.min(page, pageCount - 1);

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
        <div className="flex min-h-0 flex-1 flex-col gap-3 p-1">
            <div className="flex min-w-0 items-center gap-3">
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

            {rows.length === 0 ? (
                <p className="text-muted-foreground p-4 text-sm">
                    {t('vrcc.clubs.empty')}
                </p>
            ) : (
                <ul className="grid min-h-0 flex-1 auto-rows-min grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-2 overflow-y-auto">
                    {rows.map((club, index) => (
                        <ClubCard
                            key={club.code || `${safePage}-${index}`}
                            club={club}
                            rank={safePage * PAGE_SIZE + index + 1}
                        />
                    ))}
                </ul>
            )}

            {total !== null && pageCount > 1 ? (
                <div className="flex shrink-0 items-center gap-1">
                    <button
                        type="button"
                        className="text-muted-foreground hover:text-foreground rounded px-2 py-1 text-xs disabled:opacity-40"
                        disabled={safePage <= 0}
                        onClick={() => setPage(safePage - 1)}
                    >
                        {t('vrcc.clubs.prev')}
                    </button>
                    {buildPageItems(safePage + 1, pageCount).map((item) =>
                        item.kind === 'gap' ? (
                            <span
                                key={`gap-${item.key}`}
                                className="text-muted-foreground/60 px-1 text-xs"
                            >
                                …
                            </span>
                        ) : (
                            <button
                                key={item.page}
                                type="button"
                                className={`min-w-7 rounded px-2 py-1 text-xs tabular-nums ${
                                    item.page === safePage + 1
                                        ? 'bg-muted text-foreground'
                                        : 'text-muted-foreground hover:text-foreground'
                                }`}
                                onClick={() => setPage(item.page - 1)}
                            >
                                {item.page}
                            </button>
                        )
                    )}
                    <button
                        type="button"
                        className="text-muted-foreground hover:text-foreground rounded px-2 py-1 text-xs disabled:opacity-40"
                        disabled={safePage >= pageCount - 1}
                        onClick={() => setPage(safePage + 1)}
                    >
                        {t('vrcc.clubs.next')}
                    </button>
                </div>
            ) : null}
        </div>
    );
}
