// SPDX-License-Identifier: MPL-2.0

import { readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

import { MORPH_COLLAPSE_MS, MORPH_ENTRY_MS } from './cardMorph';

/**
 * `card-morph.css` 的两条硬约束。
 *
 * 这两条都属于「写错了不会报错、也不会让任何测试变红，只会在真人看的时候
 * 表现成怪动作」的坑，而且都真的踩过：
 *
 * 1. 整份文件不能包在 `@layer` 里。Tailwind 的工具类在 `@layer utilities`，
 *    分层规则里层序优先于特异性，写进 components 的 `animation` 会被
 *    `data-open:animate-in` 直接盖掉。
 * 2. 关键帧里不能再套一层 `translate(-50%, -50%)`。弹窗的居中由 Tailwind 的
 *    `-translate-x-1/2 -translate-y-1/2` 负责，而 v4 把它们编译成 **`translate`
 *    属性**，和 `transform` 是两套独立机制；再补一次 `-50%` 会把弹窗整体挪偏
 *    半个自身尺寸，表现成「从卡片左上角冒出来」。
 *
 * ## 为什么用 `node:fs` 而不是 Vite 的 `?raw`
 *
 * `?raw` 更干净，但 Vitest 默认把 CSS 当样式表 stub 掉，`*.css?raw` 拿到的是
 * **空串**（实测），断言会静默变成「什么都没匹配到」——比不用更危险。
 *
 * 用 `fs` 又要面对一个坑：`npm run typecheck` 走 `tsconfig.app.json`，它的
 * `types` 只有 `["vite/client"]`，`node:fs` 会报 TS2591；而 `npx tsc --noEmit`
 * 走根配置，看不出来。所以文件头加了一行 `/// <reference types="node" />`，
 * 只给这一个文件补 node 类型，不去放宽整个应用的 tsconfig。
 */
const css = readFileSync(
    new URL('../../styles/card-morph.css', import.meta.url),
    'utf8'
);

const globalsCss = readFileSync(
    new URL('../../styles/globals.css', import.meta.url),
    'utf8'
);

/** 注释里会提到这些约束本身，断言前先把注释剥掉，否则测的是自己的文档。 */
const code = css.replace(/\/\*[\s\S]*?\*\//g, '');

describe('card-morph.css 的硬约束', () => {
    it('引用的每个自定义属性都有来源——写错名字会静默退化成动画不播', () => {
        // 这条是血写的：收起动画曾经写成 `var(--morph-collapse)`，而实际定义
        // 的是 `--motion-collapse`。`var()` 无 fallback 时替换失败，整条
        // `animation` 在计算值阶段判无效、退化成 `animation: none`——收起
        // 一次都没播过，而且没有任何报错。
        //
        // 所以：文件里引用的每个 `--xxx` 必须要么在 `globals.css` 里有定义，
        // 要么是 JS 在运行时写到元素上的那几个。
        const jsProvided = new Set([
            '--morph-card-dx',
            '--morph-card-dy',
            '--morph-dialog-dx',
            '--morph-dialog-dy',
            '--morph-from-scale'
        ]);

        const referenced = new Set(
            [...code.matchAll(/var\((--[a-z0-9-]+)/gi)].map((m) => m[1])
        );
        const defined = new Set(
            [...globalsCss.matchAll(/^\s*(--[a-z0-9-]+)\s*:/gim)].map(
                (m) => m[1]
            )
        );

        const missing = [...referenced].filter(
            (name) => !defined.has(name) && !jsProvided.has(name)
        );
        expect(missing).toEqual([]);
    });

    it('不包在 @layer 里——否则被 utilities 层静默盖掉', () => {
        expect(code).not.toMatch(/@layer\s/);
    });

    it('关键帧里不重复居中——居中交给工具类的 translate 属性', () => {
        expect(code).not.toMatch(/translate\(-50%/);
    });

    it('位移不进 transform，只留旋转和缩放', () => {
        // 透视除法在整个矩阵乘完之后统一发生。位移若和 `rotateY` 写在一个
        // `transform` 里，`x' = dx / (1 + u/800)` 会随卡片各点的深度散开，
        // 转到 90° 剩下的就是一块梯形而不是一条线——这正是「转过去之后还挂着
        // 一片斜卡片」的成因。
        const transforms = code.match(/transform:[^;]+;/g) ?? [];
        expect(transforms.length).toBeGreaterThan(0);
        for (const rule of transforms) {
            expect(rule).not.toMatch(/translate\(/);
        }
    });

    it('飞行层把平移放在父层，子层只做投影', () => {
        expect(code).toContain('@keyframes vrcc-morph-move');
        expect(code).toMatch(
            /@keyframes vrcc-morph-out\s*\{[\s\S]*?rotateY\(90deg\)/
        );
    });

    it('收起用的是另一组关键帧，不是同名 + reverse', () => {
        // CSS 动画只在 `animation-name` 变化时重启。入场那个已经播完并停在
        // `fill: both` 的末态，同名再声明一次等于「更新一个已结束的动画」，
        // 收起会完全不播——这条踩过。
        expect(code).toContain('@keyframes vrcc-morph-collapse');
        expect(code).toContain('@keyframes vrcc-morph-return');
        expect(code).not.toMatch(/animation-direction:\s*reverse/);
    });

    it('两组关键帧都在：入场一组、飞行副本一组', () => {
        expect(code).toContain('@keyframes vrcc-morph-in');
        expect(code).toContain('@keyframes vrcc-morph-out');
    });

    it('飞行层不用 perspective 属性——消失点要跟着元素自己走', () => {
        // 属性版把消失点钉死在容器中心；函数版跟着元素自己的 transform-origin。
        expect(code).not.toMatch(/^\s*perspective:/m);
        expect(code).toMatch(
            /@keyframes vrcc-morph-out\s*\{[\s\S]*perspective\(800px\)/
        );
        expect(code).toMatch(
            /@keyframes vrcc-morph-return-spin\s*\{[\s\S]*perspective\(800px\)/
        );
    });

    it('返程也拆成位移与旋转两层，否则卡片在中心原地转回来', () => {
        expect(code).toContain('@keyframes vrcc-morph-return-move');
        expect(code).toContain('@keyframes vrcc-morph-return-spin');
    });

    it('交接不靠透明度——靠几何把自己收成一条线', () => {
        // 淡出是补丁：`rotateY(90deg)` 的平面在透视下本来是梯形，用 opacity 盖
        // 掉只是看不见了，几何上仍然挂着一块。位移挪出 `transform` 之后，
        // 90° 是真的零宽，就不需要任何淡出。
        //
        // 范围要收窄：**遮罩**（`vrcc-morph-overlay`）用 opacity 是正当的——
        // 那是一层真实的变暗/模糊要淡出；这里只约束负责几何的那几组关键帧。
        for (const name of [
            'vrcc-morph-in',
            'vrcc-morph-out',
            'vrcc-morph-collapse',
            'vrcc-morph-move',
            'vrcc-morph-return-move',
            'vrcc-morph-return-spin'
        ]) {
            const block = code.match(
                new RegExp(`@keyframes\\s+${name}\\s*\\{[\\s\\S]*?\\n\\}`)
            );
            expect(block, `缺少 @keyframes ${name}`).not.toBeNull();
            expect(block![0], `${name} 不该用 opacity 掩盖几何`).not.toMatch(
                /opacity\s*:/
            );
        }
    });

    it('遮罩跟弹窗同步收尾，且不会播完弹回', () => {
        // 底座给遮罩的退场是 100ms 淡出，而 tw-animate-css 的 `--animate-out`
        // 用 `fill-mode: none`——播完弹回不透明。肉眼看就是「模糊先没了、
        // 又突然回来、最后才消失」。
        //
        // 所以这里要：认领这个弹窗的遮罩（`:has()`——遮罩是 portal 里的兄弟
        // 节点，从弹窗侧选不到）、时长和收起一致、并且带 `both` 保持末态。
        const rule = code.match(
            /body:has\(\s*\[data-slot='dialog-content'\]\[data-morph\]\[data-closed\]\s*\)[\s\S]{0,80}?\[data-slot='dialog-overlay'\]\s*\{[^}]*\}/
        );
        expect(rule, '缺少遮罩的收尾覆盖').not.toBeNull();
        expect(rule![0]).toContain('var(--motion-collapse)');
        expect(rule![0]).toContain('both');
        expect(code).toContain('@keyframes vrcc-morph-overlay');
    });

    it('遮罩两半各管一半：卡片离开时不遮、弹窗出现时才淡入', () => {
        // 不覆盖的话底座给遮罩的是 150ms 淡入，卡片飞出格子的全程背景已经是
        // 模糊的——和收起时「前一半保持、后一半揭开」不对称。
        const rule = code.match(
            /body:has\(\s*\[data-slot='dialog-content'\]\[data-morph\]:not\(\[data-closed\]\)\s*\)[\s\S]{0,80}?\[data-slot='dialog-overlay'\]\s*\{[^}]*\}/
        );
        expect(rule, '缺少遮罩的入场覆盖').not.toBeNull();
        // 时长必须覆盖整条展开，前 50% 用来「不遮」。
        expect(rule![0]).toContain('var(--motion-expand)');
        expect(rule![0]).toContain('both');

        const keyframes = code.match(
            /@keyframes\s+vrcc-morph-overlay-in\s*\{[\s\S]*?\n\}/
        );
        expect(keyframes).not.toBeNull();
        // 前一半透明。
        expect(keyframes![0]).toMatch(/0%,\s*50%\s*\{[\s\S]*opacity:\s*0/);
        // 末态不透明。
        expect(keyframes![0]).toMatch(/100%\s*\{[\s\S]*opacity:\s*1/);
    });

    it('减动效有两条通道的覆盖', () => {
        expect(code).toContain('.reduce-effects');
        expect(code).toContain('prefers-reduced-motion');
    });
});

describe('时长常量与 CSS token 不漂', () => {
    it('MORPH_ENTRY_MS 与 --motion-expand 一致', () => {
        // 这两个数字必须相等：JS 的计时驱动状态机，CSS 的动画驱动画面。
        // 漂了就会「状态说落地了，画面还在飞」或者反过来——而且不会有任何
        // 报错，只有肉眼看得出。
        const globals = globalsCss;
        const match = globals.match(/--motion-expand:\s*(\d+)ms/);
        expect(match).not.toBeNull();
        expect(Number(match![1])).toBe(MORPH_ENTRY_MS);
    });

    it('两半各自计时：--morph-expand-half 由主 token 派生', () => {
        // 一条 800ms 被缓动切成两段时，前快后慢的曲线会把时间几乎全给后半段
        // （进度到 50% 时时间才走了约 13%）。所以两半必须各自计时。
        //
        // 而且必须**派生**而不是写死：写死就又多了一对要手动对齐的数字。
        const globals = globalsCss;
        expect(globals).toMatch(
            /--morph-expand-half:\s*calc\(var\(--motion-expand\)\s*\/\s*2\)/
        );
    });

    it('收起的两半同样由主 token 派生', () => {
        // 展开那半一直是 `calc`，收起这半曾经写死 275ms。两个 550/2 恰好相等，
        // 所以看不出差别——直到有人去调 `--motion-collapse`：那时收起就会变成
        // 「弹窗缩回侧棱」和「卡片飞回去」各走各的长度，而全程零报错。
        expect(globalsCss).toMatch(
            /--morph-collapse-half:\s*calc\(var\(--motion-collapse\)\s*\/\s*2\)/
        );
    });

    it('卡片的展开动画用的是半个时长，不是整条', () => {
        expect(code).toMatch(
            /\.vrcc-morph-flight\s*\{[^}]*animation:\s*vrcc-morph-move\s+var\(--morph-expand-half\)/
        );
        expect(code).toMatch(
            /\.vrcc-morph-flight__card\s*\{[^}]*animation:\s*vrcc-morph-out\s+var\(--morph-expand-half\)/
        );
    });

    it('弹窗的入场动画延后半个时长再跑', () => {
        expect(code).toMatch(
            /\[data-slot='dialog-content'\]\[data-morph\]\s*\{[^}]*var\(--morph-expand-half\)\s+var\(--ease-out-ui\)\s+var\(--morph-expand-half\)/
        );
    });

    it('MORPH_COLLAPSE_MS 与 --motion-collapse 一致', () => {
        const globals = globalsCss;
        const match = globals.match(/--motion-collapse:\s*(\d+)ms/);
        expect(match).not.toBeNull();
        expect(Number(match![1])).toBe(MORPH_COLLAPSE_MS);
    });
});
