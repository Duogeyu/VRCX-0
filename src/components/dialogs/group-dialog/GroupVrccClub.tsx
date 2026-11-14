// SPDX-License-Identifier: MPL-2.0

import { useQuery } from '@tanstack/react-query';
import {
    CircleIcon,
    ExternalLinkIcon,
    SparklesIcon,
    ThumbsUpIcon,
    UserCheckIcon
} from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { queryKeys } from '@/lib/entityQueryCache';
import { useTiltGrid } from '@/lib/useTiltGrid';
import {
    clubCodeForGroup,
    vrccPublicClubQueryOptions
} from '@/repositories/vrccClubRepository';
import { fetchVrccScopeGrants } from '@/repositories/vrccMapRepository';
import { openExternalLink } from '@/services/shellIntegrationService';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/ui/shadcn/tooltip';

/**
 * 评级徽章配色：金(4) → 紫(3) → 蓝(2) → 绿(1)。
 *
 * 名字用官方 `gradeName`，强弱交给颜色。**不从红开始**：`rose` 徽章在深色底
 * 上读出来是「危险」，而这套 UI 里的红色已经被报警语义占满。
 */
const GRADE_STYLES: Record<number, string> = {
    4: 'border-amber-500/50 from-amber-400/30 to-yellow-500/20 text-amber-600 ring-1 ring-amber-500/30 dark:text-amber-300',
    3: 'border-violet-500/45 from-violet-500/25 to-fuchsia-500/20 text-violet-600 dark:text-violet-300',
    2: 'border-sky-500/45 from-sky-500/25 to-blue-500/20 text-sky-600 dark:text-sky-300',
    1: 'border-emerald-500/45 from-emerald-500/25 to-teal-500/20 text-emerald-600 dark:text-emerald-300'
};

/** 没有评级：中性白，落在四档**之外**（官方没评，不是「最弱一档」）。 */
const UNKNOWN_STYLE =
    'border-border from-slate-400/20 to-slate-500/15 text-muted-foreground';

/**
 * 群组详情左栏的「星遇同频社团」卡片。
 *
 * 只放 VRChat 那边没有的：图标、评级、标签、一句话、点赞、关注、挂星数。
 * 名字与成员数不重复画——群组详情里已有权威值，`members` 还是星遇的过期快照。
 *
 * 缺短码或 404（没入驻/审核中/下架/只给成员看，服务器回同一个码）时整块不
 * 渲染，不留占位；请求失败同理，不把网络问题说成「没有社团」。
 */
export function GroupVrccClub({
    group
}: {
    group: { shortCode?: string | null; discriminator?: string | null };
}) {
    const { t } = useTranslation();
    const code = clubCodeForGroup(group);
    const query = useQuery({
        ...vrccPublicClubQueryOptions(code),
        enabled: Boolean(code)
    });

    // 必须在提前 return 之前调用：写在后面会让 Hook 顺序随渲染路径变化。
    const scopeQuery = useQuery({
        queryKey: queryKeys.vrccScope('clubs'),
        queryFn: () => fetchVrccScopeGrants('clubs'),
        staleTime: 60 * 1000,
        retry: 0,
        refetchOnWindowFocus: false
    });

    const detailQuery = useQuery({
        queryKey: ['group', 'vrcc-club-detail', code] as const,
        queryFn: async () => {
            const { commands } = await import('@/platform/tauri/bindings');
            return commands.appVrccMyClub(code);
        },
        // 公开页已答 404 时没必要再问带令牌的。
        enabled:
            Boolean(code) &&
            scopeQuery.data === true &&
            Boolean(query.data?.club),
        staleTime: 60_000,
        gcTime: 5 * 60_000,
        retry: 0
    });

    // 倾斜外壳：钩子只喂 CSS 变量，复合变换在 `vrcc-club-card.css` 里拼。
    // 9° 是实机试出来的：这张窄卡上 5° 看不出来，模型榜那套 12° 又晃眼。
    const tiltRef = useTiltGrid<HTMLDivElement>({
        amplitude: 9,
        scale: 1,
        glare: true
    });

    if (!code || !query.data) {
        return null;
    }

    const club = query.data.club;
    if (!club) {
        return null;
    }

    const name = club.name?.trim() || code;
    const grade =
        typeof club.grade === 'number' && club.grade >= 1 && club.grade <= 4
            ? club.grade
            : null;
    const gradeLabel =
        grade === null
            ? t('dialog.group.vrcc.grade_unknown')
            : club.gradeName?.trim() || t(`dialog.group.vrcc.grade_${grade}`);
    const gradeStyle = grade === null ? UNKNOWN_STYLE : GRADE_STYLES[grade];
    const gradeHint = t(
        grade === null
            ? 'dialog.group.vrcc.grade_hint_unknown'
            : `dialog.group.vrcc.grade_hint_${grade}`
    );

    const lede = club.lede?.trim() || '';
    const here = typeof club.here === 'number' ? club.here : 0;
    const tags = Array.isArray(club.tags) ? club.tags.filter(Boolean) : [];
    // 关注是三态：`null` 表示拿不到（没连接/没权限/失败），不显示。
    // 把 `null` 当「未关注」是在编造玩家没做过的选择。
    const detail = detailQuery.data?.club;
    const likes = typeof detail?.likes === 'number' ? detail.likes : null;
    const followed =
        typeof detail?.followed === 'boolean' ? detail.followed : null;
    // 没图标时退回首字（只给成员看的社团没有图）。
    const initial = Array.from(name)[0] ?? '?';

    return (
        <div ref={tiltRef} className="mt-3 min-w-0">
            <div data-tilt="" className="vrcc-club-card">
                <div className="vrcc-club-card__rotor">
                    <div
                        data-tour="group-vrcc-club-card"
                        className="bg-card relative rounded-lg border p-3"
                    >
                        {/* 高光层：圆心与强度由 useTiltGrid 写成 `--tilt-gx/gy/glare`。 */}
                        <div className="vrcc-club-card__glare" aria-hidden />
                        <div className="text-muted-foreground min-w-0 truncate text-[10px] font-medium tracking-wide uppercase">
                            {t('dialog.group.vrcc.club')}
                        </div>

                        <div className="mt-1.5 flex min-w-0 items-center justify-between gap-2">
                            {club.icon ? (
                                <img
                                    alt=""
                                    aria-hidden
                                    className="border-border/50 size-8 shrink-0 rounded-md border object-cover"
                                    loading="lazy"
                                    referrerPolicy="no-referrer"
                                    src={club.icon}
                                />
                            ) : (
                                <div className="border-border/50 bg-muted text-muted-foreground flex size-8 shrink-0 items-center justify-center rounded-md border text-sm font-medium">
                                    {initial}
                                </div>
                            )}
                            {/* 徽章不是按钮，trigger 用 `render` 挂普通 span。 */}
                            <Tooltip>
                                <TooltipTrigger
                                    render={
                                        <span
                                            className={`inline-flex shrink-0 cursor-help items-center gap-1 rounded-full border bg-gradient-to-r px-2 py-0.5 text-[11px] font-medium ${gradeStyle}`}
                                        >
                                            <SparklesIcon className="size-3.5" />
                                            {gradeLabel}
                                        </span>
                                    }
                                />
                                <TooltipContent side="left">
                                    {gradeHint}
                                </TooltipContent>
                            </Tooltip>
                        </div>

                        {lede ? (
                            <div className="text-muted-foreground mt-2 line-clamp-2 min-w-0 text-[11px] leading-snug break-words">
                                {lede}
                            </div>
                        ) : null}
                        {tags.length ? (
                            <div className="mt-2 flex flex-wrap gap-1">
                                {tags.slice(0, 4).map((tag) => (
                                    <span
                                        key={tag}
                                        className="bg-muted text-foreground/80 max-w-full truncate rounded px-2 py-0.5 text-[11px]"
                                    >
                                        {tag}
                                    </span>
                                ))}
                            </div>
                        ) : null}
                        {/* 三项都是只读：接口侧没有任何社团写入口。 */}
                        {followed !== null || likes !== null ? (
                            <div className="mt-2 flex min-w-0 items-center justify-between gap-2 text-[11px] tabular-nums">
                                {followed === true ? (
                                    <span className="text-muted-foreground inline-flex items-center gap-1">
                                        <UserCheckIcon className="size-3" />
                                        {t('dialog.group.vrcc.followed')}
                                    </span>
                                ) : followed === false ? (
                                    <span className="text-muted-foreground/70 inline-flex items-center gap-1">
                                        <CircleIcon className="size-3" />
                                        {t('dialog.group.vrcc.not_followed')}
                                    </span>
                                ) : (
                                    <span />
                                )}
                                {likes !== null ? (
                                    <span className="inline-flex items-center gap-1">
                                        {t('dialog.group.vrcc.likes', {
                                            count: likes
                                        })}
                                        <ThumbsUpIcon className="text-muted-foreground size-3" />
                                    </span>
                                ) : null}
                            </div>
                        ) : null}
                        {here > 0 ? (
                            <div className="text-foreground mt-1 flex items-center justify-end gap-1 text-[11px] tabular-nums">
                                {t('dialog.group.vrcc.here', { count: here })}
                                <SparklesIcon className="text-muted-foreground size-3" />
                            </div>
                        ) : null}
                        <div className="border-border/60 mt-2 flex min-w-0 items-center justify-between gap-2 border-t pt-1.5">
                            <span className="text-muted-foreground/70 min-w-0 flex-1 truncate text-[10px] leading-tight">
                                {t('dialog.group.vrcc.source')}
                            </span>
                            {club.page ? (
                                <button
                                    type="button"
                                    className="text-primary hover:text-primary/80 inline-flex shrink-0 items-center gap-0.5 text-[10px] font-medium"
                                    onClick={() => {
                                        if (club.page) {
                                            void openExternalLink(club.page);
                                        }
                                    }}
                                >
                                    {t('dialog.group.vrcc.open_page')}
                                    <ExternalLinkIcon className="size-2.5" />
                                </button>
                            ) : null}
                        </div>
                    </div>
                </div>
            </div>
        </div>
    );
}
