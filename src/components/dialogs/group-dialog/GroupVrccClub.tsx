// SPDX-License-Identifier: MPL-2.0

import { useQuery } from '@tanstack/react-query';
import {
    CalendarClockIcon,
    CircleIcon,
    ExternalLinkIcon,
    FlipHorizontal2Icon,
    RadioIcon,
    SparklesIcon,
    ThumbsUpIcon,
    UserCheckIcon
} from 'lucide-react';
import { useState, type MouseEvent } from 'react';
import { useTranslation } from 'react-i18next';

import { TranslatableText } from '@/components/translation/TranslatableText';
import { formatCompactDateTime } from '@/lib/dateTime';
import { queryKeys } from '@/lib/entityQueryCache';
import { useNowMs } from '@/lib/useNowMs';
import { useTiltGrid } from '@/lib/useTiltGrid';
import {
    clubCodeForGroup,
    vrccMyClubQueryOptions,
    vrccPublicClubQueryOptions
} from '@/repositories/vrccClubRepository';
import { fetchVrccScopeGrants } from '@/repositories/vrccMapRepository';
import { openExternalLink } from '@/services/shellIntegrationService';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/ui/shadcn/tooltip';

import { joinSegments } from './clubText';

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

/** 背面小节标题的统一字号与颜色。 */
const BACK_LABEL =
    'text-muted-foreground/70 text-[10px] font-medium tracking-wide uppercase';

/**
 * 群组详情左栏的「星遇同频社团」卡片。
 *
 * 正面只放 VRChat 那边没有的东西：名字与成员数不重复画——群组详情里已有权威值，
 * `members` 还是星遇的过期快照。
 *
 * 背面是 `/me/clubs/{code}` 里我们已经在拿、正面却没用上的那几项：公告、正在开
 * 的公开房间、下次活动。**不再多发一次请求**，那三样第一次查详情时就跟着回来了。
 * 简介不在其中：它就是审核过的群组简介，和上面「描述」逐字相同。
 *
 * 没有 `clubs` 权限时背面只说一句「连接后才看得到」，不假装社团什么都没写。
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

    const detailQuery = useQuery(
        vrccMyClubQueryOptions(
            code,
            // 公开页已答 404 时没必要再问带令牌的；没有 `clubs` 权限也别问。
            scopeQuery.data === true && Boolean(query.data?.club)
        )
    );

    // 倾斜外壳：钩子只喂 CSS 变量，复合变换在 `vrcc-club-card.css` 里拼。
    // 9° 是实机试出来的：这张窄卡上 5° 看不出来，模型榜那套 12° 又晃眼。
    // `track: 'area'` 让它在整页范围内跟随光标，衰减规则见该选项的文档。
    const tiltRef = useTiltGrid<HTMLDivElement>({
        amplitude: 9,
        // 左栏是窄列，悬停放大 5% 会撞到相邻元素，所以显式关掉缩放。
        scale: 1,
        track: 'area',
        areaSelector: '[data-slot="dialog-content"]'
    });

    const [flipped, setFlipped] = useState(false);

    // 公告的「已失效」要跟着时间走。`Date.now()` 在渲染期是副作用（React Compiler
    // 的 purity 规则会拦），走仓库现成的钩子；到期的精度不需要秒级。
    const nowMs = useNowMs({ intervalMs: 60_000 });

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
    // 群组成立年份。只在像年份的范围内显示——接口坏了给出 `0` 或时间戳时，
    // 编一句「0 年成立」比不显示更糟；小数（`2024.5`）同样不是年份。
    const since =
        typeof club.since === 'number' &&
        Number.isInteger(club.since) &&
        club.since >= 1990 &&
        club.since <= 2100
            ? club.since
            : null;
    // 关注是三态：`null` 表示拿不到（没连接/没权限/失败），不显示。
    // 把 `null` 当「未关注」是在编造玩家没做过的选择。
    const detail = detailQuery.data?.club;
    const likes = typeof detail?.likes === 'number' ? detail.likes : null;
    const followed =
        typeof detail?.followed === 'boolean' ? detail.followed : null;
    // 没图标时退回首字（只给成员看的社团没有图）。`name` 至少是短码，不会为空，
    // 所以这里不需要再补一个兜底字符。
    const initial = Array.from(name)[0];

    // **不画 `intro`**：星遇的「简介」就是审核过的 VRChat 群组简介，和这个对话框
    // 上方「描述」那一块是同一份文字，逐字相同。抄到背面只是同一段话看两遍。
    const rooms = Array.isArray(detail?.rooms) ? detail.rooms : [];
    const events = Array.isArray(detail?.events) ? detail.events : [];
    // 公告到期**不影响显示**，只在背面挂一个「已失效」；`until` 缺失当作长期。
    const notice = detail?.notice;
    const noticeText = joinSegments(notice?.segs);
    const noticeExpired =
        typeof notice?.until === 'number' && notice.until <= nowMs;
    const noticeLive = Boolean(noticeText) && !noticeExpired;
    // 两个布尔位：「现在有公开房间在线」「有还没结束的活动」。它们是「值不值得点
    // 进去」的即时判断，正面就得看见；具体内容翻到背面看。
    //
    // `event` 和 `events` **不是一回事**（实测有社团 `event: true` 但 `events: []`），
    // 所以两个都认。
    const roomOpen = detail?.room === true;
    const hasEvent = detail?.event === true || events.length > 0;
    // 标题行右侧的元信息：没过期给到期时间，过期了就说「已失效」，没有 `until`
    // 就什么都不写——不编一个假的期限。
    const noticeMeta = !noticeText
        ? ''
        : noticeExpired
          ? t('dialog.group.vrcc.notice.expired')
          : typeof notice?.until === 'number'
            ? t('dialog.group.vrcc.notice.until', {
                  when: formatCompactDateTime(notice.until)
              })
            : '';
    // 「背面什么都没有」这个断言必须和正面的角标用同一份事实：`room` / `event`
    // 是真的（正面写着「有房间在开」），只是 `rooms` / `events` 没给出明细时，
    // 背面不能反过来宣称「没有在开的房间或活动」——同一张卡自相矛盾。
    const backEmpty =
        !noticeText &&
        rooms.length === 0 &&
        events.length === 0 &&
        !roomOpen &&
        !hasEvent;
    const backPartial =
        !noticeText &&
        rooms.length === 0 &&
        events.length === 0 &&
        (roomOpen || hasEvent);

    /**
     * 整张卡都能点着翻面，但两种情况下不翻：
     *
     * - 落在卡里的按钮/链接上——那几下各有各的用处（社团页、翻面按钮本身）；
     * - 正在选字——玩家是在挑文字复制，不是在翻面。
     *
     * **卡面为什么不标 `role="button"`**：卡里已经有真的按钮。把容器标成按钮，
     * 会把它们吞进一个「按钮」里，屏幕阅读器读到的是一团；而键盘用户本来就有
     * 标题行那个真按钮可用。所以这里选择不给容器标角色，并在 `.oxlintrc.json`
     * 里为本文件关掉对应的两条 a11y 规则——关掉的理由就是这一段。
     */
    const flipOnClick = (event: MouseEvent<HTMLDivElement>) => {
        if ((event.target as HTMLElement).closest('button, a')) {
            return;
        }
        const selection = window.getSelection();
        if (
            selection &&
            !selection.isCollapsed &&
            selection.toString().trim()
        ) {
            return;
        }
        setFlipped((value) => !value);
    };

    return (
        <div ref={tiltRef} className="mt-3 min-w-0">
            <div data-tilt="" className="vrcc-club-card">
                <div className="vrcc-club-card__rotor">
                    <div
                        className="vrcc-club-card__flipper"
                        data-flipped={flipped ? '' : undefined}
                    >
                        <div
                            data-tour="group-vrcc-club-card"
                            className="vrcc-club-card__face bg-card relative min-w-0 rounded-lg border p-3"
                            inert={flipped}
                            onClick={flipOnClick}
                        >
                            {/* 高光层：圆心与强度由 useTiltGrid 写成 `--tilt-gx/gy/glare`。 */}
                            <div
                                className="vrcc-club-card__glare"
                                aria-hidden
                            />
                            {/* 开卡扫光：只在挂载时播一次（详情页打开、数据到手那一刻）。 */}
                            <div className="vrcc-club-card__sheen" aria-hidden>
                                <div className="vrcc-club-card__sheen-band" />
                            </div>
                            {/* 标题是普通文字（可以选中复制），翻面靠整卡点击；
                                右边这个小按钮是「能翻」的暗示，也是键盘和读屏
                                唯一够得着的入口——所以别把它去掉。 */}
                            <div className="flex min-w-0 items-center justify-between gap-2">
                                <span className="text-muted-foreground min-w-0 truncate text-[10px] font-medium tracking-wide uppercase">
                                    {t('dialog.group.vrcc.club')}
                                </span>
                                <div className="relative shrink-0">
                                    <button
                                        type="button"
                                        className="text-muted-foreground/60 hover:text-foreground -mt-1 -mr-1 rounded p-1"
                                        aria-label={t(
                                            'dialog.group.vrcc.flip_back'
                                        )}
                                        onClick={() => setFlipped(true)}
                                    >
                                        <FlipHorizontal2Icon className="size-3" />
                                    </button>
                                    {/* 背面有还没失效的公告时亮一个小点：藏一层
                                        可以，但不能藏得毫无痕迹。 */}
                                    {noticeLive ? (
                                        <span
                                            data-notice-live=""
                                            className="bg-primary pointer-events-none absolute top-0 right-0 size-1.5 rounded-full"
                                        />
                                    ) : null}
                                </div>
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
                            {/* 成立年份单独一行：塞进上面的标签行会读成又一个标签。 */}
                            {since !== null ? (
                                <div className="text-muted-foreground/70 mt-2 min-w-0 truncate text-[11px]">
                                    {t('dialog.group.vrcc.since', {
                                        year: since
                                    })}
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
                                            {t(
                                                'dialog.group.vrcc.not_followed'
                                            )}
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
                                    {t('dialog.group.vrcc.here', {
                                        count: here
                                    })}
                                    <SparklesIcon className="text-muted-foreground size-3" />
                                </div>
                            ) : null}
                            {/* 「现在值不值得点进去」：有房在开、有活动没结束。翻到背面
                                才看得到详情，但翻之前得先知道有没有——所以放在正面。 */}
                            {roomOpen || hasEvent ? (
                                <div className="text-muted-foreground mt-1 flex min-w-0 items-center justify-end gap-2 text-[11px]">
                                    {roomOpen ? (
                                        <span className="inline-flex items-center gap-1">
                                            <RadioIcon className="size-3" />
                                            {t('dialog.group.vrcc.room_open')}
                                        </span>
                                    ) : null}
                                    {hasEvent ? (
                                        <span className="inline-flex items-center gap-1">
                                            <CalendarClockIcon className="size-3" />
                                            {t('dialog.group.vrcc.event_open')}
                                        </span>
                                    ) : null}
                                </div>
                            ) : null}
                            {/* 来源那句话一句话说清「这是另一家平台的数据」。文案缩短后
                                单行放得下，不再折行——折成两行会把卡片底边撑得很重。 */}
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
                                                void openExternalLink(
                                                    club.page
                                                );
                                            }
                                        }}
                                    >
                                        {t('dialog.group.vrcc.open_page')}
                                        <ExternalLinkIcon className="size-2.5" />
                                    </button>
                                ) : null}
                            </div>
                        </div>

                        <div
                            className="vrcc-club-card__face vrcc-club-card__face--back bg-card relative min-w-0 rounded-lg border p-3"
                            inert={!flipped}
                            onClick={flipOnClick}
                        >
                            <div className="flex min-w-0 items-center justify-between gap-2">
                                <span className="text-muted-foreground min-w-0 truncate text-[10px] font-medium tracking-wide uppercase">
                                    {t('dialog.group.vrcc.back_title')}
                                </span>
                                <button
                                    type="button"
                                    className="text-muted-foreground/60 hover:text-foreground -mt-1 -mr-1 shrink-0 rounded p-1"
                                    aria-label={t(
                                        'dialog.group.vrcc.flip_front'
                                    )}
                                    onClick={() => setFlipped(false)}
                                >
                                    <FlipHorizontal2Icon className="size-3" />
                                </button>
                            </div>

                            {/*
                             * 背面内容区。正反两面同占一格，卡片高度取两者较高的一份，
                             * 所以这里必须**封顶 + 内部滚动**：一段长公告若原样铺开，
                             * 会把正面也撑出一大块空白。`overscroll-contain` 让滚到底
                             * 之后不再把左栏一起带走；标题行留在滚动区外面，滚到最底下
                             * 也翻得回正面。
                             */}
                            <div
                                className="mt-2 max-h-56 min-w-0 overflow-y-auto overscroll-contain pr-1"
                                tabIndex={0}
                                aria-label={t('dialog.group.vrcc.back_title')}
                            >
                                {noticeText ? (
                                    <div>
                                        {/* 到期时间是**公告**的属性，就挂在公告自己
                                            这行标题右侧，不放到卡级标题上——放上面会被
                                            读成「整张卡 01/06 过期」。 */}
                                        <div className="flex min-w-0 items-center justify-between gap-2">
                                            <div className={BACK_LABEL}>
                                                {t(
                                                    'dialog.group.vrcc.notice.title'
                                                )}
                                            </div>
                                            {noticeMeta ? (
                                                <span className="text-muted-foreground/70 shrink-0 text-[10px] leading-none tabular-nums">
                                                    {noticeMeta}
                                                </span>
                                            ) : null}
                                        </div>
                                        <TranslatableText
                                            source={noticeText}
                                            entityId={`vrcc-club-notice:${code}`}
                                            density="button"
                                        >
                                            {({
                                                action,
                                                meta,
                                                error,
                                                text: visibleText
                                            }) => (
                                                <div className="mt-0.5 min-w-0">
                                                    {meta}
                                                    <pre className="text-muted-foreground m-0 font-sans text-[11px] leading-snug whitespace-pre-wrap">
                                                        {visibleText ||
                                                            '\u2014'}
                                                    </pre>
                                                    {error}
                                                    <div className="mt-0.5 flex min-w-0 items-center justify-between gap-2">
                                                        {action}
                                                    </div>
                                                </div>
                                            )}
                                        </TranslatableText>
                                    </div>
                                ) : null}

                                {backEmpty ? (
                                    <p className="text-muted-foreground/70 mt-2 min-w-0 text-[11px] leading-snug break-words">
                                        {scopeQuery.data === true
                                            ? t('dialog.group.vrcc.back_empty')
                                            : t(
                                                  'dialog.group.vrcc.back_locked'
                                              )}
                                    </p>
                                ) : null}

                                {/* 角标说有房/有活动，但明细没给。此时唯一诚实的
                                    说法是「看不到明细」，不是「没有」。 */}
                                {backPartial ? (
                                    <p className="text-muted-foreground/70 mt-2 min-w-0 text-[11px] leading-snug break-words">
                                        {t('dialog.group.vrcc.back_partial')}
                                    </p>
                                ) : null}

                                {rooms.length ? (
                                    <div className="mt-2">
                                        <div className={BACK_LABEL}>
                                            {t('dialog.group.vrcc.back_rooms')}
                                        </div>
                                        <ul className="mt-0.5 space-y-0.5">
                                            {rooms.map((room, index) => {
                                                const occupied =
                                                    typeof room.users ===
                                                    'number'
                                                        ? typeof room.cap ===
                                                          'number'
                                                            ? `${room.users}/${room.cap}`
                                                            : String(room.users)
                                                        : null;
                                                return (
                                                    <li
                                                        key={
                                                            room.link ??
                                                            `room-${index}`
                                                        }
                                                        className="flex min-w-0 items-center justify-between gap-2 text-[11px]"
                                                    >
                                                        <span className="min-w-0 truncate">
                                                            {room.map?.trim() ||
                                                                t(
                                                                    'dialog.group.vrcc.back_room_unlabeled'
                                                                )}
                                                        </span>
                                                        {occupied ? (
                                                            <span className="text-muted-foreground shrink-0 tabular-nums">
                                                                {occupied}
                                                            </span>
                                                        ) : null}
                                                    </li>
                                                );
                                            })}
                                        </ul>
                                    </div>
                                ) : null}

                                {events.length ? (
                                    <div className="mt-2">
                                        <div className={BACK_LABEL}>
                                            {t('dialog.group.vrcc.back_events')}
                                        </div>
                                        <ul className="mt-0.5 space-y-0.5">
                                            {events.map((event, index) => {
                                                const when =
                                                    typeof event.at === 'number'
                                                        ? formatCompactDateTime(
                                                              event.at
                                                          )
                                                        : '';
                                                return (
                                                    <li
                                                        key={
                                                            event.link ??
                                                            `event-${index}`
                                                        }
                                                        className="flex min-w-0 items-center justify-between gap-2 text-[11px]"
                                                    >
                                                        <span className="min-w-0 truncate">
                                                            {event.title?.trim() ||
                                                                t(
                                                                    'dialog.group.vrcc.back_event_untitled'
                                                                )}
                                                        </span>
                                                        {when ? (
                                                            <span className="text-muted-foreground shrink-0 tabular-nums">
                                                                {when}
                                                            </span>
                                                        ) : null}
                                                    </li>
                                                );
                                            })}
                                        </ul>
                                    </div>
                                ) : null}
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    );
}
