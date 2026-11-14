// SPDX-License-Identifier: MPL-2.0
// @vitest-environment jsdom

import {
    act,
    cleanup,
    fireEvent,
    render,
    screen
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useTiltGrid, type TiltGridOptions } from './useTiltGrid';

/**
 * 把 rAF 换成手动驱动：弹簧一帧只走一小段，真等几百毫秒既慢又依赖机器速度。
 * 每帧固定 16ms，结果才是确定的。
 */
let frames: FrameRequestCallback[] = [];
const FRAME_MS = 16;
let clock = 0;

function tick(count = 1) {
    for (let i = 0; i < count; i++) {
        clock += FRAME_MS;
        const pending = frames;
        frames = [];
        for (const callback of pending) {
            act(() => {
                callback(clock);
            });
        }
    }
}

beforeEach(() => {
    frames = [];
    clock = 0;
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
        frames.push(callback);
        return frames.length;
    });
    vi.stubGlobal('cancelAnimationFrame', () => {
        frames = [];
    });
});

afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
    document.documentElement.classList.remove('reduce-effects');
});

/**
 * 给元素一个固定的几何：左边 0、上边 0、400×400。
 *
 * jsdom 不做布局，`getBoundingClientRect()` 全是 0，不补这个归一化算出来的
 * 永远是 0 度，测不出符号对不对。
 */
function stubRect(element: Element) {
    element.getBoundingClientRect = () =>
        ({
            left: 0,
            top: 0,
            width: 400,
            height: 400,
            right: 400,
            bottom: 400,
            x: 0,
            y: 0,
            toJSON: () => ({})
        }) as DOMRect;
}

/**
 * 在**卡片**上派发 pointermove。
 *
 * 真实事件也是这样：目标是被指到的那张卡，然后冒泡到网格上的监听器。
 * 直接往网格上发的话 `event.target` 就是网格，`closest('[data-tilt]')` 找
 * 不到卡片，什么都不会发生。
 */
function moveOn(element: Element, x: number, y: number, pointerType = 'mouse') {
    stubRect(element);
    const event = new MouseEvent('pointermove', {
        bubbles: true,
        clientX: x,
        clientY: y
    });
    Object.defineProperty(event, 'pointerType', { value: pointerType });
    fireEvent(element, event);
}

function degrees(element: HTMLElement, name: string): number {
    return Number.parseFloat(element.style.getPropertyValue(name) || '0');
}

function Grid({
    count = 1,
    options,
    withTip = true
}: {
    count?: number;
    options?: TiltGridOptions;
    withTip?: boolean;
}) {
    const ref = useTiltGrid<HTMLUListElement>(options);
    return (
        <ul ref={ref} data-testid="grid">
            {Array.from({ length: count }, (_, index) => (
                <li key={index} data-tilt="" data-testid={`card-${index}`}>
                    {withTip ? (
                        <span data-tilt-tip="" data-testid={`tip-${index}`} />
                    ) : null}
                </li>
            ))}
        </ul>
    );
}

describe('useTiltGrid', () => {
    it('归一化到 -1..1：右下角是满幅 12°', () => {
        render(<Grid />);
        const card = screen.getByTestId('card-0');

        // (400, 400) 是右下角 → nx = ny = 1 → 绕 Y +12°、绕 X -12°。
        moveOn(card, 400, 400);
        tick(120);

        expect(degrees(card, '--tilt-y')).toBeCloseTo(12, 1);
        expect(degrees(card, '--tilt-x')).toBeCloseTo(-12, 1);
    });

    it('中心点不倾斜', () => {
        render(<Grid />);
        const card = screen.getByTestId('card-0');

        moveOn(card, 200, 200);
        tick(120);

        expect(Math.abs(degrees(card, '--tilt-y'))).toBeLessThan(0.05);
        expect(Math.abs(degrees(card, '--tilt-x'))).toBeLessThan(0.05);
    });

    it('是弹簧不是补间：一帧到不了目标，要几十帧才收敛', () => {
        render(<Grid />);
        const card = screen.getByTestId('card-0');

        moveOn(card, 400, 400);
        tick(1);
        const afterOneFrame = degrees(card, '--tilt-y');
        expect(afterOneFrame).toBeGreaterThan(0);
        expect(afterOneFrame).toBeLessThan(6);

        tick(120);
        expect(degrees(card, '--tilt-y')).toBeCloseTo(12, 1);
    });

    it('悬停抬起 1.05 倍', () => {
        render(<Grid />);
        const card = screen.getByTestId('card-0');

        moveOn(card, 200, 200);
        tick(120);

        expect(Number(card.style.getPropertyValue('--tilt-scale'))).toBeCloseTo(
            1.05,
            2
        );
    });

    it('反光圆心由倾角反推，不是直接跟指针', () => {
        render(<Grid />);
        const card = screen.getByTestId('card-0');

        moveOn(card, 400, 400);
        tick(120);

        // ry = +12 → gx = 50 + (12/12)*46 = 96%；rx = -12 → gy = 96%。
        expect(
            Number.parseFloat(card.style.getPropertyValue('--tilt-gx'))
        ).toBeCloseTo(96, 0);
        expect(
            Number.parseFloat(card.style.getPropertyValue('--tilt-gy'))
        ).toBeCloseTo(96, 0);
    });

    it('触屏不倾斜：只有鼠标才有「悬停」', () => {
        render(<Grid />);
        const card = screen.getByTestId('card-0');

        moveOn(card, 400, 400, 'touch');
        tick(3);

        expect(card.style.getPropertyValue('--tilt-y')).toBe('');
    });

    it('指针离开网格时回正，卡片不会一直歪着', () => {
        render(<Grid />);
        const card = screen.getByTestId('card-0');

        moveOn(card, 400, 400);
        tick(120);
        expect(degrees(card, '--tilt-y')).toBeGreaterThan(1);

        fireEvent.pointerLeave(screen.getByTestId('grid'));
        tick(200);

        // 回正后元素被从循环里摘掉，变量也被清掉。
        expect(card.style.getPropertyValue('--tilt-y')).toBe('');
    });

    it('打开「减少动态效果」时完全不挂监听', () => {
        document.documentElement.classList.add('reduce-effects');
        render(<Grid />);
        const card = screen.getByTestId('card-0');

        moveOn(card, 400, 400);
        tick(3);

        expect(card.style.getPropertyValue('--tilt-x')).toBe('');
        expect(card.style.getPropertyValue('--tilt-y')).toBe('');
    });

    it('光标移到另一张卡时，上一张回正', () => {
        render(<Grid count={2} />);
        const first = screen.getByTestId('card-0');
        const second = screen.getByTestId('card-1');

        moveOn(first, 400, 400);
        tick(120);
        expect(degrees(first, '--tilt-y')).toBeGreaterThan(1);

        moveOn(second, 200, 200);
        tick(200);

        expect(first.style.getPropertyValue('--tilt-y')).toBe('');
    });

    it('指针移到网格里的非卡片区域时，同样回正', () => {
        render(<Grid />);
        const card = screen.getByTestId('card-0');
        const grid = screen.getByTestId('grid');

        moveOn(card, 400, 400);
        tick(120);
        expect(degrees(card, '--tilt-y')).toBeGreaterThan(1);

        // 目标是网格本身（卡片之间的空隙），没有 `[data-tilt]` 祖先。
        const event = new MouseEvent('pointermove', {
            bubbles: true,
            clientX: 10,
            clientY: 10
        });
        Object.defineProperty(event, 'pointerType', { value: 'mouse' });
        fireEvent(grid, event);
        tick(200);

        expect(card.style.getPropertyValue('--tilt-y')).toBe('');
    });

    it('标签跟着指针走', () => {
        render(<Grid />);
        const card = screen.getByTestId('card-0');
        const tip = screen.getByTestId('tip-0');

        moveOn(card, 300, 100);
        tick(20);

        // 第一次进入时标签先落到指针处，再指数逼近。
        expect(tip.style.transform).toContain('translate3d');
        expect(Number(tip.style.opacity)).toBeGreaterThan(0);
    });

    it('全部静止后不再排新的帧（循环会自己停下）', () => {
        render(<Grid />);
        const card = screen.getByTestId('card-0');

        moveOn(card, 200, 200);
        tick(300);

        // 指针还在卡片上但已经收敛：此时不该再有排队的回调。
        const pendingAfterSettle = frames.length;
        expect(pendingAfterSettle).toBe(0);
    });

    it('关掉反光就不再写那三个变量，倾斜照旧', () => {
        // 纯信息卡没有反光元素。每帧写三次没人读的 CSS 变量是纯空转。
        render(<Grid options={{ glare: false }} />);
        const card = screen.getByTestId('card-0');

        moveOn(card, 400, 400);
        tick(120);

        expect(card.style.getPropertyValue('--tilt-gx')).toBe('');
        expect(card.style.getPropertyValue('--tilt-gy')).toBe('');
        expect(card.style.getPropertyValue('--tilt-glare')).toBe('');
        expect(degrees(card, '--tilt-y')).toBeCloseTo(12, 1);
    });

    it('倾角可以按卡片类型调小', () => {
        render(<Grid options={{ amplitude: 6 }} />);
        const card = screen.getByTestId('card-0');

        moveOn(card, 400, 400);
        tick(120);

        expect(degrees(card, '--tilt-y')).toBeCloseTo(6, 1);
        expect(degrees(card, '--tilt-x')).toBeCloseTo(-6, 1);
    });

    it('scale 传 1 表示不缩放', () => {
        render(<Grid options={{ scale: 1 }} />);
        const card = screen.getByTestId('card-0');

        moveOn(card, 200, 200);
        tick(120);

        expect(Number(card.style.getPropertyValue('--tilt-scale'))).toBeCloseTo(
            1,
            3
        );
    });

    it('标签节点只查一次，不是每帧查', () => {
        // 原来 `paintTip` 每帧 `querySelector('[data-tilt-tip]')`：24 张卡 60fps
        // 是每秒约 1400 次查询，而卡片没有标签时全是空手而归。这条数查询次数。
        const spy = vi.spyOn(Element.prototype, 'querySelector');
        try {
            render(<Grid />);
            const card = screen.getByTestId('card-0');

            moveOn(card, 300, 100);
            tick(60);

            const tipQueries = spy.mock.calls.filter(
                ([selector]) => selector === '[data-tilt-tip]'
            ).length;
            // 建状态时一次，之后走缓存。
            expect(tipQueries).toBeLessThanOrEqual(2);
        } finally {
            spy.mockRestore();
        }
    });

    it('卡片里没有标签时照样倾斜，循环照样会停', () => {
        // 没有 `[data-tilt-tip]` 就该整条标签链路短路，而不是每帧对着空气
        // querySelector、推进两条用不上的弹簧。
        render(<Grid withTip={false} />);
        const card = screen.getByTestId('card-0');

        moveOn(card, 400, 400);
        tick(300);

        expect(degrees(card, '--tilt-y')).toBeCloseTo(12, 1);
        expect(frames.length).toBe(0);
    });
});
