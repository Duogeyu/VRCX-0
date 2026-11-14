// SPDX-License-Identifier: MPL-2.0

import { ExternalLink, MessageSquareText, ThumbsUp, XIcon } from 'lucide-react';
import { useEffect, useState, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/shadcn/button';
import {
    Dialog,
    DialogClose,
    DialogContent,
    DialogDescription,
    DialogTitle
} from '@/ui/shadcn/dialog';

import type { MorphPlan } from '../cardMorph';
import { useModelDetail } from '../useModelDetail';
import { useOpenExternalLink } from '../useOpenExternalLink';
import { type VrccModelBoard, type VrccModelHot } from '../vrccApi';

type ModelRow = NonNullable<VrccModelBoard['models']>[number];

/** 星遇同频上模型榜的公开页。 */
const SITE = 'https://vrcc.cc';

/**
 * 模型评论对话框。
 *
 * ## 数据来源与边界
 *
 * 开放接口里**只有两个**和模型有关的地址：`/boards/models`（榜）和
 * `/models/{id}`（单个模型）。后者给的 `hots` 是「公开页上的热评，最多 2 条」，
 * 不是完整评论区——官方没有提供拉取全部短评的接口，也没有「给模型投票」
 * 的接口。所以这里能显示的就是这两条，再想多看只能去网页。
 *
 * 这一点必须说清楚，否则玩家会以为「只有两条评论」是客户端漏了。
 *
 * ## 三种状态，三种说法
 *
 * - **加载中**：转圈；
 * - **拿到了**：显示热评；一条都没有时说「还没有人写」；
 * - **`404 not_found`**：这个模型**已不在榜上**（下架或未过审）。那是正常
 *   状态，不是网络故障，所以给一句解释加一个「去网页看看」，不弹错误。
 */
export function VrccModelDialog({
    model,
    morph,
    open,
    onOpenChange,
    onSettled,
    onGone
}: {
    model: ModelRow;
    /** 形变参数；`null` 表示不播形变（减动效、元素没有布局）。 */
    morph: MorphPlan | null;
    /**
     * 开关由外层持有，**不能写死 `open`**：底座要靠 `open` 变 false 才会挂上
     * `data-closed`，收起动画才有得播；写死 true 的话关窗只会变成「组件被
     * 卸载」，形变直接消失。
     */
    open: boolean;
    onOpenChange: (open: boolean) => void;
    /** 关闭动画播完了：飞行副本可以归位了。 */
    onSettled: () => void;
    /** 展开还没落地就被确认「不在榜上」。 */
    onGone: () => void;
}) {
    const { t } = useTranslation();
    const { open: openLink, pending: openLinkPending } = useOpenExternalLink();
    const { state, detail } = useModelDetail(model.id);

    // 入场途中被后端告知「这个模型不在榜上」：当场收回（决策十三）。
    // 这是唯一允许中途反悔的情况——继续飞完再告诉用户没有，比收回去更怪。
    useEffect(() => {
        if (state === 'gone') {
            onGone();
        }
    }, [state, onGone]);

    const name = model.short?.trim() || model.name;
    const page = model.page ?? `${SITE}/models/${model.id}`;

    // **以榜单那一行为准**：它是这次刷新刚拿到的，比缓存里那份新。
    // 详情接口的 `model` 只在行上缺字段时兜底。
    //
    // 反过来写（`detail?.model ?? model`）会让「看过一次的模型」整场会话的
    // 点赞数、价格、热评都不再更新——前端这层没有 TTL，等于把后端的 5 分钟
    // 过期完全抹掉。
    const shown = { ...detail?.model, ...model };
    const hots: VrccModelHot[] = detail?.hots ?? [];

    return (
        <Dialog
            open={open}
            onOpenChange={onOpenChange}
            // 关闭动画播完才轮到收尾：早一步洞就补上了、卡片和飞行体同时存在，
            // 晚一步会留一段空白。
            onOpenChangeComplete={(next) => {
                if (!next) onSettled();
            }}
        >
            <DialogContent
                showCloseButton={false}
                className="sm:max-w-lg"
                data-morph={morph ? '' : undefined}
                style={
                    morph
                        ? ({
                              '--morph-dialog-dx': `${morph.dialogDx}px`,
                              '--morph-dialog-dy': `${morph.dialogDy}px`,
                              '--morph-from-scale': String(morph.fromScale)
                          } as CSSProperties)
                        : undefined
                }
            >
                <div className="-m-4 flex flex-col">
                    {/* 封面照旧只在拿到图时显示；BOOTH 的图偶尔会 403。 */}
                    <ModelDialogCover src={shown.img ?? null} />

                    <div className="flex flex-col gap-3 p-4">
                        <div className="flex items-start justify-between gap-2">
                            <div className="min-w-0">
                                <DialogTitle className="text-base leading-snug font-semibold">
                                    <span className="line-clamp-2">{name}</span>
                                </DialogTitle>
                                <DialogDescription className="text-muted-foreground mt-1 text-xs">
                                    {[
                                        shown.shop,
                                        shown.price,
                                        t('vrcc.models.likes_count', {
                                            count: shown.likes
                                        })
                                    ]
                                        .filter(Boolean)
                                        .join(' · ')}
                                </DialogDescription>
                            </div>
                            {/* 外链与关闭并排在右上角，和公告弹窗同一套排版：
                                先外链、后关闭，`ghost` + `icon-sm` + `gap-1`。
                                关闭用 `DialogClose` 而不是自绘 `onClick`，
                                焦点管理、Esc、标题栏保活都留在底座手里。 */}
                            <div className="flex shrink-0 items-center gap-1">
                                <Button
                                    variant="ghost"
                                    size="icon-sm"
                                    title={t('vrcc.models.open')}
                                    aria-label={t('vrcc.models.open')}
                                    disabled={openLinkPending}
                                    onClick={() => openLink(page)}
                                >
                                    <ExternalLink className="size-3.5" />
                                </Button>
                                <DialogClose
                                    render={
                                        <Button
                                            variant="ghost"
                                            size="icon-sm"
                                        />
                                    }
                                >
                                    <XIcon />
                                    <span className="sr-only">
                                        {t('common.actions.close')}
                                    </span>
                                </DialogClose>
                            </div>
                        </div>

                        <div className="border-t pt-3">
                            <div className="text-muted-foreground mb-2 flex items-center gap-1.5 text-xs font-medium">
                                <MessageSquareText className="size-3.5" />
                                {t('vrcc.models.hot_comments')}
                            </div>

                            {state === 'loading' && (
                                <div className="text-muted-foreground py-6 text-center text-xs">
                                    {t('common.loading', '加载中…')}
                                </div>
                            )}

                            {state === 'gone' && (
                                <div className="text-muted-foreground py-6 text-center text-xs">
                                    {t('vrcc.models.comments_gone')}
                                </div>
                            )}

                            {state === 'error' && (
                                <div className="text-muted-foreground py-6 text-center text-xs">
                                    {t('vrcc.models.comments_error')}
                                </div>
                            )}

                            {state === 'ready' && hots.length === 0 && (
                                <div className="text-muted-foreground py-6 text-center text-xs">
                                    {t('vrcc.models.comments_empty')}
                                </div>
                            )}

                            {state === 'ready' && hots.length > 0 && (
                                <ul className="flex flex-col gap-2">
                                    {hots.map((hot, index) => (
                                        <li
                                            key={`${index}:${hot.text ?? ''}`}
                                            className="bg-muted/40 rounded-lg px-3 py-2"
                                        >
                                            <p className="text-sm leading-relaxed break-words">
                                                {hot.text ?? ''}
                                            </p>
                                            <div className="text-muted-foreground mt-1 flex items-center gap-2 text-xs">
                                                {hot.by && (
                                                    <span className="truncate">
                                                        {hot.by}
                                                    </span>
                                                )}
                                                {hot.up != null && (
                                                    <span className="flex shrink-0 items-center gap-0.5 tabular-nums">
                                                        <ThumbsUp className="size-3" />
                                                        {hot.up}
                                                    </span>
                                                )}
                                            </div>
                                        </li>
                                    ))}
                                </ul>
                            )}

                            {/* 说清边界：接口只给热评，不是我们只显示两条。
                                不写这一句，玩家会以为客户端漏了数据。 */}
                            {state === 'ready' && (
                                <p className="text-muted-foreground mt-3 text-[11px] leading-relaxed">
                                    {t('vrcc.models.comments_note')}
                                </p>
                            )}
                        </div>
                    </div>
                </div>
            </DialogContent>
        </Dialog>
    );
}

/**
 * 对话框顶部的封面。
 *
 * 单独一个小节是因为它只需要一个「加载失败就收起」的局部状态：BOOTH 的图
 * 挂在 pximg 上，偶尔会 403 或 404，留一块破图比不显示更难看。
 */
function ModelDialogCover({ src }: { src: string | null }) {
    const [failed, setFailed] = useState(false);
    if (!src || failed) return null;
    return (
        <img
            src={src}
            alt=""
            aria-hidden
            loading="lazy"
            decoding="async"
            className="bg-muted aspect-[16/9] w-full shrink-0 rounded-t-xl object-cover"
            onError={() => setFailed(true)}
        />
    );
}
