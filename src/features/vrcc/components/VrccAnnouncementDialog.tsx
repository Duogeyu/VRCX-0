// SPDX-License-Identifier: MPL-2.0

import { ChevronLeft, ChevronRight, ExternalLink } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/shadcn/button';
import { Dialog, DialogContent, DialogTitle } from '@/ui/shadcn/dialog';

import { useOpenExternalLink } from '../useOpenExternalLink';
import type { VrccAnnouncement } from '../vrccApi';
import { VrccAnnouncementBody } from './VrccAnnouncementBody';

/**
 * 站方应用界面的地址。
 *
 * 是 `/match` 而不是站根：`vrcc.cc` 根路径不是应用本身，官方文档里所有指向
 * 玩家界面的地址（连接页、星卡、地图榜）都是 `vrcc.cc/match/?...`，
 * README 也把 `https://vrcc.cc/match/` 写成星遇同频的入口。
 *
 * 公告没有独立直链，但网页里一定会展示当前公告，所以打开这个地址就等于
 * 「去官网看这条公告」。
 */
const SITE = 'https://vrcc.cc/match';

/**
 * 公告弹窗，带序列导航。
 *
 * 一次给的是一个**序列**（同作用域下所有未读），不是一条：玩家要能 `1/3`
 * 知道还剩几条、能用 `‹` 回看前面的、也能一次「跳过全部」。
 *
 * **已读时机**：只有点「下一条」/「我已知情」/「跳过全部」才记为已读。
 * 单纯用 `‹` `›` 翻看不记——随手翻过去不等于读过，否则翻到第 3 条关掉，
 * 第 1、2 条也会跟着被吞掉。
 *
 * **三种 level 的按钮差异**：
 * - `force`：不给右上角 `✕`，也不让点遮罩外部关掉（但 **Esc 保留**）
 * - `pop` / `bar`：有 `✕`，可以点外部关
 *
 * Esc 是刻意留的逃生口：万一弹窗内容渲染出错、按钮点不动，玩家至少还能出来。
 * 用一个关不掉的弹窗去「强制」传达信息，出问题时的代价远大于收益。
 */
export function VrccAnnouncementDialog({
    posts,
    onAdvance,
    onSkipAll
}: {
    /** 当前作用域下未读的公告序列（新的在前）。空数组表示不显示。 */
    posts: readonly VrccAnnouncement[];
    /** 确认当前这条：记已读并前进。 */
    onAdvance: (id: number) => void;
    /** 跳过全部：已看的记已读，其余的也一并标掉。 */
    onSkipAll: (ids: readonly number[]) => void;
}) {
    const { t } = useTranslation();
    const [index, setIndex] = useState(0);
    // 图片是外链，加载失败时收起图片区，别留一块破图占位。
    //
    // 记的是**哪一张**失败了（`coverKey`），而不是一个布尔：翻到下一条公告时
    // 布尔不会复位，第一张封面 404 之后，后面每一条的封面都不再显示。
    const [failedCover, setFailedCover] = useState<string | null>(null);
    // 公告里的「在官网查看」和正文里的链接：点下去要有即时反馈，
    // 否则玩家会以为没点上而重复点，连着弹出好几个浏览器窗口。
    const { open: openLink, pending: openLinkPending } = useOpenExternalLink();

    if (posts.length === 0) return null;

    // 序列可能因为「已读」被外部缩短，索引要夹在有效范围内。
    const current = Math.min(index, posts.length - 1);
    const post = posts[current];
    if (!post) return null;

    const total = posts.length;
    const isFirst = current === 0;
    const isLast = current === total - 1;
    const forced = post.level === 'force';
    const id = post.id ?? 0;
    const cover = (post.cover ?? '').trim();
    const link = (post.link ?? '').trim();
    const coverKey = `${id}:${cover}`;
    const showCover = cover !== '' && failedCover !== coverKey;

    return (
        <Dialog
            open
            disablePointerDismissal={forced}
            onOpenChange={(open) => {
                if (!open) onAdvance(id);
            }}
        >
            <DialogContent showCloseButton={false} className="sm:max-w-lg">
                <div className="-m-4 flex flex-col">
                    {showCover && (
                        <img
                            key={coverKey}
                            src={cover}
                            alt=""
                            className="aspect-[16/9] w-full shrink-0 rounded-t-xl object-cover"
                            onError={() => setFailedCover(coverKey)}
                        />
                    )}

                    <div className="flex flex-col gap-3 p-4">
                        <div className="flex items-start justify-between gap-2">
                            <DialogTitle className="text-base leading-snug font-semibold">
                                {post.title ?? ''}
                            </DialogTitle>
                            <div className="flex shrink-0 items-center gap-1">
                                {/* 在官网查看：公告没有独立直链，但网页一定展示
                                    当前公告，所以打开站点就等于看这条。 */}
                                <Button
                                    variant="ghost"
                                    size="icon-sm"
                                    title={t('vrcc.announcement.view_on_site')}
                                    aria-label={t(
                                        'vrcc.announcement.view_on_site'
                                    )}
                                    onClick={() => openLink(SITE)}
                                >
                                    <ExternalLink className="size-3.5" />
                                </Button>
                                {/* `force` 不给关闭按钮；Esc 仍然可用。 */}
                                {!forced && (
                                    <Button
                                        variant="ghost"
                                        size="icon-sm"
                                        title={t('vrcc.announcement.dismiss')}
                                        aria-label={t(
                                            'vrcc.announcement.dismiss'
                                        )}
                                        onClick={() => onAdvance(id)}
                                    >
                                        <span
                                            aria-hidden
                                            className="text-sm leading-none"
                                        >
                                            ✕
                                        </span>
                                    </Button>
                                )}
                            </div>
                        </div>

                        {post.body && (
                            <VrccAnnouncementBody
                                body={post.body}
                                className="text-muted-foreground max-h-[50vh] overflow-y-auto text-xs leading-relaxed"
                            />
                        )}

                        <div className="mt-1 flex items-center justify-between gap-2">
                            {/* 序列导航：永远显示。到头就置灰，不做隐藏式特判，
                                这样「现在在第几条、还能不能翻」一眼可读。 */}
                            <div className="flex items-center gap-1">
                                <Button
                                    variant="ghost"
                                    size="icon-sm"
                                    disabled={isFirst}
                                    aria-label={t('vrcc.announcement.prev')}
                                    onClick={() => setIndex(current - 1)}
                                >
                                    <ChevronLeft className="size-4" />
                                </Button>
                                <span className="text-muted-foreground text-xs tabular-nums">
                                    {current + 1}/{total}
                                </span>
                                <Button
                                    variant="ghost"
                                    size="icon-sm"
                                    disabled={isLast}
                                    aria-label={t('vrcc.announcement.next')}
                                    onClick={() => setIndex(current + 1)}
                                >
                                    <ChevronRight className="size-4" />
                                </Button>
                            </div>

                            <div className="flex items-center gap-2">
                                {/* 「打开链接」是站方指定的引导动作（进群领码、
                                    去某个页面），和「看公告」是两件事：
                                    只在有 link 时出现，点了也不关弹窗。 */}
                                {link !== '' && (
                                    <Button
                                        variant="ghost"
                                        size="sm"
                                        disabled={openLinkPending}
                                        onClick={() => openLink(link)}
                                    >
                                        {t('vrcc.announcement.open')}
                                    </Button>
                                )}
                                {/* 只有一条时「跳过全部」和「我已知情」效果相同，
                                    语义重复，所以只在多条时出现。 */}
                                {total > 1 && (
                                    <Button
                                        variant="outline"
                                        size="sm"
                                        onClick={() =>
                                            onSkipAll(
                                                posts.map(
                                                    (item) => item.id ?? 0
                                                )
                                            )
                                        }
                                    >
                                        {t('vrcc.announcement.skip_all')}
                                    </Button>
                                )}
                                <Button size="sm" onClick={() => onAdvance(id)}>
                                    {isLast
                                        ? t('vrcc.announcement.got_it')
                                        : t('vrcc.announcement.next_one')}
                                </Button>
                            </div>
                        </div>

                        {/* 来源说明。站方的公告是写给「星遇同频网页」的，正文里会
                            出现网页上的导航名（快捷栏、我的星…）和只在网页上存在的
                            功能（添加到主屏幕）。原样转发而不加说明，玩家会以为那是
                            本应用的东西。这里如实标注来源，不替站方内容做任何担保。 */}
                        <p className="text-muted-foreground text-[10px] leading-relaxed">
                            {t('vrcc.announcement.source_note')}
                        </p>
                    </div>
                </div>
            </DialogContent>
        </Dialog>
    );
}
