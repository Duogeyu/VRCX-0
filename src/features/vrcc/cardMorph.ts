// SPDX-License-Identifier: MPL-2.0

/**
 * 卡片 → 弹窗的形变几何。
 *
 * ## 这是一条运动，不是两段动画
 *
 * 卡片从格子里出发，边往视口中心移动边转；转到 **90°（侧棱朝前、零宽度）**
 * 的那一刻把画面交给弹窗本体，弹窗接着转回 0° 并长到本体尺寸。交接点选在
 * 侧棱，是因为那一刻两边都看不见——正面排版不必和弹窗逐像素对齐，这正是不
 * 得不这么做的原因：卡片是 150–175px 的方图卡，弹窗是 512px 的竖版盒子。
 *
 * ## 为什么不用量弹窗
 *
 * 弹窗静止时的**中心**只取决于视口，和它自己的尺寸无关：
 * `DialogContent` 是 `left-1/2 top-[calc(50%+1rem)]` 再回移自身一半，所以
 * 中心恒为 `(视口宽/2, 视口高/2 + 16)`。只有宽度需要知道，而它有上限定值。
 * 于是整个计划不需要先挂载弹窗、也不需要 `getBoundingClientRect` 去量它。
 */

export type MorphRect = {
    left: number;
    top: number;
    width: number;
    height: number;
};

export type MorphViewport = {
    width: number;
    height: number;
};

export type MorphPlan = {
    /** 飞行体（卡片副本）从原位走到中点的位移。 */
    cardDx: number;
    cardDy: number;
    /** 弹窗从中点走回自己静止位置的位移；值和 `cardDx/Dy` 方向相反。 */
    dialogDx: number;
    dialogDy: number;
    /** 弹窗在中点那一刻的缩放。 */
    fromScale: number;
};

/** 弹窗宽度上限，和 `VrccModelDialog` 自己那句 `sm:max-w-lg` 覆盖保持一致。
 *
 * 注意基座 `DialogContent` 用的是 `sm:max-w-sm`（384），去那边核对会读到
 * 384——照它「顺手改正」就是把整条几何改坏 25%，而测试全绿。
 */
const DIALOG_MAX_WIDTH = 512;

/** 弹窗中心比视口正中低多少，和 `top-[calc(50%+1rem)]` 保持一致。 */
const DIALOG_CENTER_Y_OFFSET = 16;

/** 弹窗左右各留的边距之和，和 `max-w-[calc(100%-2rem)]` 保持一致。 */
const DIALOG_SIDE_GUTTER = 32;

/** 缩放的下限：再小就成了一个看不见的点。 */
const MIN_SCALE = 0.15;

/**
 * 算出这次形变要用的四组数。
 *
 * 返回 `null` 表示不该播动画——元素没有布局（jsdom、被隐藏），视口尺寸也不
 * 合理。调用方遇到 `null` 就直接开弹窗，这也让测试天然确定。
 */
export function planMorph(
    card: MorphRect,
    viewport: MorphViewport
): MorphPlan | null {
    if (card.width <= 0 || card.height <= 0) {
        return null;
    }
    if (viewport.width <= 0 || viewport.height <= 0) {
        return null;
    }

    const dialogWidth = Math.min(
        DIALOG_MAX_WIDTH,
        viewport.width - DIALOG_SIDE_GUTTER
    );
    if (dialogWidth <= 0) {
        return null;
    }

    const cardCx = card.left + card.width / 2;
    const cardCy = card.top + card.height / 2;
    // 弹窗中心与自身尺寸无关，见文件头。
    const dialogCx = viewport.width / 2;
    const dialogCy = viewport.height / 2 + DIALOG_CENTER_Y_OFFSET;

    const midCx = (cardCx + dialogCx) / 2;
    const midCy = (cardCy + dialogCy) / 2;

    return {
        cardDx: midCx - cardCx,
        cardDy: midCy - cardCy,
        dialogDx: midCx - dialogCx,
        dialogDy: midCy - dialogCy,
        // 卡到 90° 时整个元素压成一条竖线，只剩高度看得见；但高度取决于弹窗
        // 内容，量不到。这里用宽度比：差的那一点在侧棱上是一瞬间的事，
        // 换来的是不必先挂载弹窗去测量。
        fromScale: Math.min(1, Math.max(MIN_SCALE, card.width / dialogWidth))
    };
}

/**
 * 入场形变的时长（毫秒），和 `--motion-expand` 必须一致。
 *
 * ## 为什么用定时器而不是 `animationend`
 *
 * 状态机原本挂了 `onAnimationEnd`。问题是那个事件**不可靠**：
 *
 * - 元素被隐藏、动画被 `animation: none` 取消、或者中途被打断，它都不触发；
 * - jsdom 里 `animationend` 根本不派发到 React（实测连普通的都收不到），
 *   于是这条分支只能靠肉眼验证，测试永远绿。
 *
 * 改成定时常量显式驱动：动画归 CSS，**状态归 JS**，两者只共享一个数字。
 * 这也让「飞行中按 Esc 要排队」这类行为能在测试里直接验证。
 */
export const MORPH_ENTRY_MS = 800;

/**
 * 收起形变的时长，和 CSS 里的 `--motion-collapse` 必须一致。
 *
 * 收起是两段串行：弹窗用 `--morph-collapse-half` 退回侧棱，卡片副本再用同样
 * 的时长飞回原位。总时长等于 `--motion-collapse`。
 *
 * 目前 JS 侧不需要按它计时——收尾由底座的 `onOpenChangeComplete` 负责；这个
 * 常量存在是为了让 `cardMorphCss` 的测试能断言「CSS 与 JS 的时长别漂」。
 */
export const MORPH_COLLAPSE_MS = 550;
