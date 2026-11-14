// SPDX-License-Identifier: MPL-2.0

import { useTranslation } from 'react-i18next';

import { useModelCoverReady } from '../useModelCoverCache';
import type { VrccBoardSort, VrccModelBoard } from '../vrccApi';
import { VrccCover } from './VrccCover';

export type ModelRow = NonNullable<VrccModelBoard['models']>[number];

/**
 * 卡片封面。单独提出来是为了用 [`useModelCoverReady`]——hook 不能写在
 * `map()` 的回调里。它记住「这张图我们加载过」，重新挂载时立刻出图。
 */
function ModelCardCover({ src }: { src: string | null }) {
    const ready = useModelCoverReady(src);
    if (!ready) {
        return null;
    }
    return <VrccCover src={src} className="size-full object-cover" />;
}

/**
 * 卡片正面。
 *
 * ## 为什么单独抽出来
 *
 * 展开成弹窗时，飞行体是这张卡片的**副本**——它必须和网格里那一张在起飞
 * 那一帧逐像素一致，否则一动就会跳。保证一致最省事的办法不是把样式抄一遍，
 * 而是让两边渲染同一个组件、同一个宽度（飞行层用的是卡片实测宽度）。
 *
 * ## 一个动作
 *
 * 整张卡只有一个可点区域：封面。点它开评论弹窗。外链按钮不在这里，它跟着
 * 「打开评论」一起挪进了弹窗右上角。
 */
export function ModelCardFace({
    model,
    place,
    sort,
    showCover,
    onOpen
}: {
    model: ModelRow;
    /** 名次：总榜是 `rank`，本周榜是 `place`；都没有就不显示角标。 */
    place: number | null | undefined;
    sort: VrccBoardSort;
    /** 这一张是不是已经在视口里——不在就先不请求封面。 */
    showCover: boolean;
    onOpen: (model: ModelRow, card: HTMLElement | null) => void;
}) {
    const { t } = useTranslation();
    const name = model.short?.trim() || model.name;

    return (
        <div className="vrcc-model-card__face vrcc-model-card__face--front">
            <button
                type="button"
                onClick={(event) =>
                    onOpen(
                        model,
                        event.currentTarget.closest<HTMLElement>(
                            '.vrcc-model-card'
                        )
                    )
                }
                className="vrcc-model-card__cover bg-muted relative block aspect-square w-full overflow-hidden"
                aria-label={`${t('vrcc.models.comments')}: ${name}`}
            >
                {/* 跟随指针的反光。 */}
                <span className="vrcc-model-card__glare" aria-hidden="true" />
                {/* 封面直接来自星遇（官方 `img`），不走 VRChat 的世界图
                    转发，也没有本地缓存可退。滚到视口里才开始取。 */}
                {showCover ? <ModelCardCover src={model.img ?? null} /> : null}
                {place != null && (
                    <span
                        data-model-rank=""
                        className={
                            place <= 3
                                ? 'bg-primary text-primary-foreground absolute top-1.5 left-1.5 min-w-4 rounded px-1 text-center text-[11px] font-bold tabular-nums'
                                : 'absolute top-1.5 left-1.5 min-w-4 rounded bg-black/55 px-1 text-center text-[11px] font-semibold text-white tabular-nums'
                        }
                    >
                        {place}
                    </span>
                )}
                <span className="absolute top-1.5 right-1.5 flex items-baseline gap-0.5 rounded bg-black/55 px-1 py-0.5 text-[11px] font-semibold text-white tabular-nums">
                    {model.likes}
                    <span className="text-[9px] font-normal opacity-80">
                        {t('vrcc.models.likes')}
                    </span>
                </span>
            </button>

            <div className="flex min-w-0 flex-1 flex-col gap-0.5 p-2">
                <div className="truncate text-xs font-medium">{name}</div>
                {/* 热评优先：官方说模型榜只给热评一条，那是对「这模型好在哪」
                    最有用的一句；没有才退回店铺名。 */}
                <div className="text-muted-foreground line-clamp-2 min-h-7 text-[11px] leading-snug">
                    {model.hot?.text
                        ? `「${model.hot.text}」${
                              model.hot.by ? ` · ${model.hot.by}` : ''
                          }`
                        : (model.shop ?? '—')}
                </div>
                <div className="text-muted-foreground mt-auto flex items-center gap-1 text-[11px] tabular-nums">
                    {/* 本周榜显示本周几个喜欢，和网页「本周 N」一样。 */}
                    <span className="min-w-0 truncate">
                        {sort === 'week' && model.week != null
                            ? t('vrcc.models.week_count', {
                                  count: model.week
                              })
                            : (model.price ?? '')}
                    </span>
                </div>
            </div>
        </div>
    );
}
