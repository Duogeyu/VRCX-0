// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { planMorph } from './cardMorph';

const viewport = { width: 1200, height: 800 };

function card(overrides = {}) {
    return { left: 400, top: 200, width: 160, height: 220, ...overrides };
}

describe('planMorph', () => {
    it('把中点和两侧位移算成同一条运动', () => {
        const plan = planMorph(card(), viewport)!;

        expect(plan).not.toBeNull();
        // 卡片中心 (480, 310)；弹窗静止中心 (600, 416)；中点 (540, 363)。
        expect(plan.cardDx).toBeCloseTo(60);
        expect(plan.cardDy).toBeCloseTo(53);
        expect(plan.dialogDx).toBeCloseTo(-60);
        expect(plan.dialogDy).toBeCloseTo(-53);
    });

    it('两侧位移严格互为相反数——否则交接处会跳一下', () => {
        const plan = planMorph(
            card({ left: 13, top: 907, width: 151 }),
            viewport
        )!;

        expect(plan.cardDx + plan.dialogDx).toBeCloseTo(0);
        expect(plan.cardDy + plan.dialogDy).toBeCloseTo(0);
    });

    it('两侧位移都跟着视口走，且始终互为相反数', () => {
        const small = planMorph(card(), { width: 800, height: 600 })!;
        const large = planMorph(card(), { width: 1600, height: 1000 })!;

        // 球窗变小，弹窗静止中心左移，中点跟着左移，卡片这一侧也就跟着走。
        expect(small.cardDx).not.toBeCloseTo(large.cardDx);
        expect(small.cardDx + small.dialogDx).toBeCloseTo(0);
        expect(large.cardDx + large.dialogDx).toBeCloseTo(0);
    });

    it('缩放按卡片宽度占弹窗宽度的比例算，且落在合理区间', () => {
        const plan = planMorph(card({ width: 160 }), viewport)!;
        // 视口 1200 宽时弹窗取上限 512。
        expect(plan.fromScale).toBeCloseTo(160 / 512);

        // 极窄的卡片不会缩到看不见。
        const tiny = planMorph(card({ width: 8 }), viewport)!;
        expect(tiny.fromScale).toBeGreaterThanOrEqual(0.15);
    });

    it('窄窗口下弹窗宽度由视口决定，不超上限', () => {
        // 视口 400 宽：`max-w-[calc(100%-2rem)]` → 368 宽，比 `max-w-lg` 小。
        const plan = planMorph(card({ width: 160 }), {
            width: 400,
            height: 800
        })!;

        expect(plan.fromScale).toBeCloseTo(160 / 368);
    });

    it('元素没有布局就不播动画', () => {
        // jsdom 里 `getBoundingClientRect()` 全是 0，这条让测试天然确定。
        expect(planMorph(card({ width: 0 }), viewport)).toBeNull();
        expect(planMorph(card({ height: 0 }), viewport)).toBeNull();
    });

    it('视口尺寸不合理也不播', () => {
        expect(planMorph(card(), { width: 0, height: 0 })).toBeNull();
        expect(planMorph(card(), { width: 20, height: 800 })).toBeNull();
    });
});
