// SPDX-License-Identifier: MPL-2.0

import { useCallback, useEffect, useState } from 'react';

/**
 * 让网格里的卡片跟着光标倾斜，并带一个跟随指针的标签。
 *
 * 手感参数是照演示站试出来的默认值，不是推出来的量。归一化用 -1..1（按卡片
 * 半宽）：写成 -0.5..0.5，倾角会小一半。
 *
 * 整个网格只挂一个 `pointermove` 和一个 rAF，状态按元素存在 Map 里逐张推进，
 * 不是每张卡各起一套——那在 24 格里就是 24 套。
 *
 * ## 调用方要自己提供的东西
 *
 * 钩子只写 CSS 变量，**不动 `transform`**。要用它就得自己写那几行：
 *
 * ```css
 * .card > .card__rotor {
 *     transform: rotateX(calc(var(--tilt-x, 0) * 1deg))
 *         rotateY(calc(var(--tilt-y, 0) * 1deg))
 *         scale(var(--tilt-scale, 1));
 *     transform-style: preserve-3d;
 * }
 * ```
 *
 * - 卡片根元素带 `data-tilt`；
 * - 想要跟随标签，就在卡片里放一个 `data-tilt-tip`。**没有也行**——查不到时
 *   整条标签链路直接短路，不推进弹簧、也不算速度；
 * - 反光按 `--tilt-gx` / `--tilt-gy` / `--tilt-glare` 画；不画就关掉
 *   [`TiltGridOptions.glare`]，省掉每帧三次写入；
 * - 「减少动态效果」的守卫得自己写（`perspective: none` / `transform: none`）。
 *   钩子只在挂载时读一次偏好，之后的翻转由 CSS 负责：画不出来，而且弹簧收敛
 *   后循环会自己停，所以不订阅那个开关也出不了错。
 */

/** 光标在卡片边缘时的倾角（度）。 */
const AMPLITUDE = 12;
/** 弹簧的软硬：越大越快，经下面两条换算成 ω / ζ。 */
const SMOOTHNESS = 0.6;
/** `ω = 20·0.2^smoothness`：固有角频率。 */
const OMEGA = 20 * Math.pow(0.2, SMOOTHNESS);
/** `ζ = 1-0.3·smoothness`：阻尼比。小于 1 才有回弹。 */
const ZETA = 1 - 0.3 * SMOOTHNESS;
const SCALE_ON_HOVER = 1.05;

/** 标签跟随指针的时间常数（秒）。小=黏手，大=飘。 */
const TIP_FOLLOW_SECONDS = 0.035;
const TIP_OMEGA = 18;
const TIP_ZETA = 0.62;
const TIP_MAX_DEG = 14;
const TIP_SHOW_OMEGA = 16;

/**
 * 可调项。默认值就是上面那批手感常量——外面不传，行为与从前一致。
 *
 * 只开三个口子，不把 `SMOOTHNESS` 之类的弹簧参数也暴露出去：那些是「调坏了
 * 会发散或抖」的量，不是「换个卡片类型就要改」的量。
 */
export type TiltGridOptions = {
    /** 指针在卡片边缘时的倾角（度）。默认 `AMPLITUDE`。 */
    amplitude?: number;
    /** 悬停时的缩放倍数。默认 `SCALE_ON_HOVER`；传 `1` 表示不缩放。 */
    scale?: number;
    /** 写不写反光那三个变量。纯信息卡关掉，省掉每帧三次写入。默认开。 */
    glare?: boolean;
};

type Spring = {
    value: number;
    velocity: number;
};

type CardState = {
    /** 绕 X / 绕 Y 轴的倾角。 */
    rx: Spring;
    ry: Spring;
    lift: Spring;
    tip: Spring;
    shown: Spring;
    tx: number;
    ty: number;
    /** 指针在卡片里的归一化位置 -1..1，与下面的像素位置是两套。 */
    nx: number;
    ny: number;
    px: number;
    py: number;
    vx: number;
    lastX: number;
    lastAt: number;
    hovering: boolean;
    /**
     * 卡片里的标签，建状态时查一次。
     *
     * 原来每帧都 `querySelector` 一遍——24 张卡 60fps 是每秒约 1400 次查询，
     * 而没有标签的卡片（纯信息卡）每次都是空手而归。查不到就一直是 `null`，
     * 整条标签链路短路。
     *
     * React 若把那个节点换掉，缓存会指向一个已经脱离文档的元素；用之前看一眼
     * `isConnected`，失效就重查一次，不让它静默地往空气里写。
     */
    tipElement: HTMLElement | null;
};

function spring(value = 0): Spring {
    return { value, velocity: 0 };
}

/**
 * 半隐式欧拉推进一个弹簧，返回「还在动吗」。
 *
 * 子步进到 1/240 秒：直接用整帧步长时 ω 一大就发散，表现为角度越抖越大。
 */
function step(
    target: Spring,
    goal: number,
    omega: number,
    zeta: number,
    dt: number
): boolean {
    const steps = Math.max(1, Math.ceil(dt / (1 / 240)));
    const h = dt / steps;
    for (let index = 0; index < steps; index++) {
        target.velocity +=
            (omega * omega * (goal - target.value) -
                2 * zeta * omega * target.velocity) *
            h;
        target.value += target.velocity * h;
    }
    const settled =
        Math.abs(goal - target.value) < 0.001 &&
        Math.abs(target.velocity) < 0.01;
    if (settled) {
        target.value = goal;
        target.velocity = 0;
    }
    return !settled;
}

function clearCard(element: HTMLElement, tip: HTMLElement | null) {
    for (const name of [
        '--tilt-x',
        '--tilt-y',
        '--tilt-scale',
        '--tilt-gx',
        '--tilt-gy',
        '--tilt-glare',
        'will-change'
    ]) {
        element.style.removeProperty(name);
    }
    if (tip) {
        tip.style.removeProperty('transform');
        tip.style.removeProperty('opacity');
    }
}

/**
 * 把光标事件接到网格上，返回要挂到 `<ul>` 的 ref。
 *
 * 用「回调 ref + state」而不是 `useRef`：模型榜是异步加载的，首帧网格整个不
 * 渲染，`useRef.current` 是 `null`，而 `useEffect(…, [])` 只在挂载时跑一次——
 * 那一瞬间拿不到节点就永远接不上了。
 */
export function useTiltGrid<T extends HTMLElement>({
    amplitude = AMPLITUDE,
    scale = SCALE_ON_HOVER,
    glare = true
}: TiltGridOptions = {}) {
    const [grid, setGrid] = useState<T | null>(null);

    const ref = useCallback((node: T | null) => {
        setGrid(node);
    }, []);

    useEffect(() => {
        if (!grid) {
            return;
        }
        // 「减少动态效果」下不做倾斜：那是晕动症的来源，不是装饰。
        if (
            document.documentElement.classList.contains('reduce-effects') ||
            window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
        ) {
            return;
        }

        const states = new Map<HTMLElement, CardState>();
        let active: HTMLElement | null = null;
        let raf = 0;
        let last = 0;

        const paint = (element: HTMLElement, state: CardState) => {
            // 写 CSS 变量而不是直接写 `transform`：角度、缩放和位移在
            // `model-card.css` 里拼成一条复合变换，这里只负责喂数。两边各拼
            // 自己那份，谁改公式都不会把对方那次写入覆盖掉。
            const liftScale = 1 + (scale - 1) * state.lift.value;
            element.style.setProperty('--tilt-x', state.rx.value.toFixed(3));
            element.style.setProperty('--tilt-y', state.ry.value.toFixed(3));
            element.style.setProperty('--tilt-scale', liftScale.toFixed(4));

            if (!glare) {
                return;
            }
            // 反光圆心由倾角反推，不是直接跟指针：这样光和卡片一起转，
            // 像打在倾斜的表面上，而不是贴着一张跟着手跑的白纸。
            const reach = Math.max(amplitude, 1);
            element.style.setProperty(
                '--tilt-gx',
                `${(50 + (state.ry.value / reach) * 46).toFixed(2)}%`
            );
            element.style.setProperty(
                '--tilt-gy',
                `${(50 - (state.rx.value / reach) * 46).toFixed(2)}%`
            );
            const energy = Math.min(
                1,
                Math.hypot(state.rx.value, state.ry.value) / reach
            );
            element.style.setProperty(
                '--tilt-glare',
                Math.max(state.lift.value, energy).toFixed(3)
            );
        };

        const paintTip = (element: HTMLElement, state: CardState) => {
            // 缓存可能指向一个被 React 换掉、已经脱离文档的节点。
            if (!state.tipElement?.isConnected) {
                state.tipElement =
                    element.querySelector<HTMLElement>('[data-tilt-tip]');
            }
            const tip = state.tipElement;
            if (!tip) {
                return;
            }
            // 不要把位置夹回卡片范围内：贴着边缘时把它拽回来，看起来就像
            // 一张被钉住的贴纸，而不是跟着手走。
            tip.style.transform = `translate3d(${state.tx.toFixed(1)}px, ${state.ty.toFixed(1)}px, 0) rotate(${state.tip.value.toFixed(2)}deg)`;
            tip.style.opacity = Math.max(0, state.shown.value).toFixed(3);
        };

        const frame = (now: number) => {
            raf = 0;
            const dt = Math.min(0.05, Math.max(0.001, (now - last) / 1000));
            last = now;

            let busy = false;

            for (const [element, state] of states) {
                const inputX = state.hovering ? state.nx : 0;
                const inputY = state.hovering ? state.ny : 0;

                let moving = false;
                moving =
                    step(state.rx, -inputY * amplitude, OMEGA, ZETA, dt) ||
                    moving;
                moving =
                    step(state.ry, inputX * amplitude, OMEGA, ZETA, dt) ||
                    moving;
                moving =
                    step(
                        state.lift,
                        state.hovering ? 1 : 0,
                        OMEGA,
                        Math.max(ZETA, 0.85),
                        dt
                    ) || moving;

                paint(element, state);

                // 没有 `[data-tilt-tip]` 就把整条标签链路短路：纯信息卡不该为
                // 「跟着手走的标签」每帧推进两条弹簧、算一次速度。
                if (state.tipElement) {
                    moving =
                        step(
                            state.shown,
                            state.hovering ? 1 : 0,
                            TIP_SHOW_OMEGA,
                            1,
                            dt
                        ) || moving;
                    const follow = 1 - Math.exp(-dt / TIP_FOLLOW_SECONDS);
                    state.tx += (state.px - state.tx) * follow;
                    state.ty += (state.py - state.ty) * follow;
                    if (
                        Math.abs(state.px - state.tx) +
                            Math.abs(state.py - state.ty) >
                        0.2
                    ) {
                        moving = true;
                    }
                    state.vx *= Math.exp(-dt / 0.08);
                    moving =
                        step(
                            state.tip,
                            Math.max(
                                -TIP_MAX_DEG,
                                Math.min(TIP_MAX_DEG, -state.vx * 0.012)
                            ),
                            TIP_OMEGA,
                            TIP_ZETA,
                            dt
                        ) || moving;
                    paintTip(element, state);
                }

                if (moving) {
                    busy = true;
                } else if (!state.hovering) {
                    clearCard(element, state.tipElement);
                    states.delete(element);
                }
            }

            // 只在真有东西在动时续帧。改回 `states.size > 0` 会 60fps 空转：
            // 指针停在卡片上不动、弹簧早已收敛，那张卡仍然留在表里。而收敛后
            // 指针再动时 `onMove` 本来就会 `wake()`，不需要循环一直醒着守。
            if (busy) {
                raf = requestAnimationFrame(frame);
            }
        };

        const wake = () => {
            if (raf) {
                return;
            }
            last = performance.now();
            raf = requestAnimationFrame(frame);
        };

        const stateFor = (element: HTMLElement): CardState => {
            let state = states.get(element);
            if (!state) {
                state = {
                    rx: spring(),
                    ry: spring(),
                    lift: spring(),
                    tip: spring(),
                    shown: spring(),
                    tx: 0,
                    ty: 0,
                    nx: 0,
                    ny: 0,
                    px: 0,
                    py: 0,
                    vx: 0,
                    lastX: 0,
                    lastAt: 0,
                    hovering: false,
                    tipElement:
                        element.querySelector<HTMLElement>('[data-tilt-tip]')
                };
                states.set(element, state);
                element.style.setProperty('will-change', 'transform');
            }
            return state;
        };

        const onMove = (event: PointerEvent) => {
            // 触屏没有「悬停」这回事，跟随按下的点只会显得黏。
            if (event.pointerType !== 'mouse') {
                return;
            }
            const target = (
                event.target as Element | null
            )?.closest<HTMLElement>('[data-tilt]');
            if (!target || !grid.contains(target)) {
                if (active) {
                    const state = states.get(active);
                    if (state) {
                        state.hovering = false;
                    }
                    active = null;
                    wake();
                }
                return;
            }

            if (active && active !== target) {
                const previous = states.get(active);
                if (previous) {
                    previous.hovering = false;
                }
            }
            active = target;

            const rect = target.getBoundingClientRect();
            const width = Math.max(1, rect.width);
            const height = Math.max(1, rect.height);
            const state = stateFor(target);

            // -1..1：按卡片半宽归一化。用 -0.5..0.5 会把倾角砍掉一半。
            state.nx = Math.max(
                -1,
                Math.min(
                    1,
                    (event.clientX - rect.left - width / 2) / (width / 2)
                )
            );
            state.ny = Math.max(
                -1,
                Math.min(
                    1,
                    (event.clientY - rect.top - height / 2) / (height / 2)
                )
            );
            state.px = event.clientX - rect.left;
            state.py = event.clientY - rect.top;
            state.hovering = true;

            // 横向速度：标签靠它甩。首次移动没有参照，跳过。
            const nowAt = performance.now();
            if (state.lastAt > 0) {
                const elapsed = (nowAt - state.lastAt) / 1000;
                if (elapsed > 0.001) {
                    const dx = event.clientX - state.lastX;
                    state.vx += (dx / elapsed - state.vx) * 0.5;
                }
            }
            state.lastX = event.clientX;
            state.lastAt = nowAt;
            // 第一次进来时把标签直接放到指针处，避免它从左上角飞过来。
            if (state.shown.value === 0) {
                state.tx = state.px;
                state.ty = state.py;
                state.vx = 0;
            }

            wake();
        };

        const onLeave = () => {
            for (const state of states.values()) {
                state.hovering = false;
            }
            active = null;
            wake();
        };

        grid.addEventListener('pointermove', onMove);
        grid.addEventListener('pointerleave', onLeave);

        return () => {
            grid.removeEventListener('pointermove', onMove);
            grid.removeEventListener('pointerleave', onLeave);
            if (raf) {
                cancelAnimationFrame(raf);
            }
            for (const [element, state] of states) {
                clearCard(element, state.tipElement);
            }
            states.clear();
        };
    }, [grid, amplitude, scale, glare]);

    return ref;
}
