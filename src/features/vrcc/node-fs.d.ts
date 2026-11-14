// SPDX-License-Identifier: MPL-2.0

/**
 * 一个只够测试用的极小 `node:fs` 声明。
 *
 * ## 为什么需要它
 *
 * `src/features/vrcc/cardMorphCss.test.ts` 要读 `.css` 原文来断言几条硬约束
 * （不包 `@layer`、位移不进 `transform`、收起另起关键帧……）。这些约束的共同
 * 特点是**写错了不报错、也不会让别的测试变红**，只会在真人看的时候表现成怪动作。
 *
 * `?raw` 本来更干净，但 Vitest 默认把 CSS 当样式表 stub 掉，`*.css?raw` 拿到
 * 的是**空串**，断言会静默变成「什么都没匹配到」——比不写还危险。所以只能读文件。
 *
 * 而 `npm run typecheck` 走 `tsconfig.app.json`，它的 `types` 只有
 * `["vite/client"]`（浏览器侧不需要 node 类型），`node:fs` 会报 TS2591。
 *
 * ## 为什么不用 `/// <reference types="node" />`
 *
 * 试过，不行：三斜线引用会把 node 的类型带进**整个程序**，于是
 * `setTimeout` 的返回值从 `number` 变成 `Timeout`，`authAutoLoginService` 和
 * `updateLoopService` 里那几处立刻报 TS2322。看起来是「只给一个文件加」，
 * 实际是全局的。
 *
 * 所以这里只声明真正用到的那一个函数。`src` 里没有任何其它文件 import node
 * 内建模块，不存在「声明太窄挡住别人」的问题。
 */
declare module 'node:fs' {
    export function readFileSync(path: URL | string, encoding: 'utf8'): string;
}
