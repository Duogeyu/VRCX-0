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
 * `track: 'area'` 的边界。
 *
 * 这一支最危险的地方是**几何退化**：卡片或详情页拿不到尺寸（0×0、元素还没布
 * 局）时，归一化的分母会变成 0，算出来是 `NaN`。CSS 里写进 `NaN` 不会报错，
 * 只会让那条 `transform` 整条失效——卡片静止不动，没人知道为什么。
 */

let frames: FrameRequestCallback[] = [];
let clock = 0;

function tick(count = 1) {
    for (let index = 0; index < count; index++) {
        clock += 16;
        advanceTo(clock);
    }
}

/** 推进到指定时刻，只跑一帧（用来造「停了很久才动」的 dt 钳制场景）。 */
function advanceTo(now: number) {
    const pending = frames;
    frames = [];
    for (const callback of pending) {
        act(() => {
            callback(now);
        });
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
    delete (window as { matchMedia?: unknown }).matchMedia;
});

type Box = { left: number; top: number; width: number; height: number };

function stubBox(element: Element, box: Box) {
    element.getBoundingClientRect = () =>
        ({
            ...box,
            right: box.left + box.width,
            bottom: box.top + box.height,
            x: box.left,
            y: box.top,
            toJSON: () => ({})
        }) as DOMRect;
}

const CARD: Box = { left: 0, top: 0, width: 400, height: 400 };
const AREA: Box = { left: 0, top: 0, width: 1200, height: 800 };

function moveInArea(
    area: Element,
    x: number,
    y: number,
    pointerType = 'mouse',
    cardBox: Box = CARD,
    areaBox: Box = AREA
) {
    const card = document.querySelector('[data-tilt]');
    if (card) {
        stubBox(card, cardBox);
    }
    stubBox(area, areaBox);
    const event = new MouseEvent('pointermove', {
        bubbles: true,
        clientX: x,
        clientY: y
    });
    Object.defineProperty(event, 'pointerType', { value: pointerType });
    fireEvent(area, event);
}

function AreaGrid({
    options,
    withArea = true,
    withTip = false
}: {
    options?: TiltGridOptions;
    withArea?: boolean;
    withTip?: boolean;
}) {
    const ref = useTiltGrid<HTMLUListElement>(options);
    const grid = (
        <ul ref={ref} data-testid="grid">
            <li data-tilt="" data-testid="card-0">
                {withTip ? <span data-tilt-tip="" data-testid="tip-0" /> : null}
            </li>
        </ul>
    );
    return withArea ? (
        <div data-slot="dialog-content" data-testid="area">
            {grid}
        </div>
    ) : (
        grid
    );
}

const AREA_OPTIONS: TiltGridOptions = {
    track: 'area',
    areaSelector: '[data-slot="dialog-content"]'
};

function css(element: HTMLElement, name: string): string {
    return element.style.getPropertyValue(name);
}

function number(element: HTMLElement, name: string): number {
    return Number.parseFloat(css(element, name) || '0');
}

describe('useTiltGrid · area 模式的退化几何', () => {
    it('卡片 0×0 时算出来是有限值，不是 NaN', () => {
        render(<AreaGrid options={AREA_OPTIONS} />);
        const card = screen.getByTestId('card-0');

        moveInArea(screen.getByTestId('area'), 600, 200, 'mouse', {
            left: 0,
            top: 0,
            width: 0,
            height: 0
        });
        tick(60);

        // `halfW = Math.max(1, width)/2` 是这里的护栏：写成 `width/2` 就会除零。
        expect(Number.isFinite(number(card, '--tilt-x'))).toBe(true);
        expect(Number.isFinite(number(card, '--tilt-y'))).toBe(true);
        expect(Number.isFinite(number(card, '--tilt-scale'))).toBe(true);
        expect(css(card, '--tilt-y')).not.toContain('NaN');
        expect(css(card, '--tilt-gx')).not.toContain('NaN');
    });

    it('详情页 0×0 时同样不写 NaN', () => {
        render(<AreaGrid options={AREA_OPTIONS} />);
        const card = screen.getByTestId('card-0');

        // 区域整个塌成一个点：`reach` 全是 0，`span` 靠 `Math.max(0.001, …)` 兜住。
        moveInArea(screen.getByTestId('area'), 400, 200, 'mouse', CARD, {
            left: 0,
            top: 0,
            width: 0,
            height: 0
        });
        tick(60);

        expect(css(card, '--tilt-x')).not.toContain('NaN');
        expect(css(card, '--tilt-y')).not.toContain('NaN');
        expect(css(card, '--tilt-glare')).not.toContain('NaN');
    });

    it('卡片在详情页外面时角度有限且被衰减，不会反过来放大', () => {
        render(<AreaGrid options={AREA_OPTIONS} />);
        const card = screen.getByTestId('card-0');

        // 卡片在 (900..1300, 400..800)，指针在详情页左上角。
        moveInArea(screen.getByTestId('area'), 0, 0, 'mouse', {
            left: 900,
            top: 400,
            width: 400,
            height: 400
        });
        tick(80);

        const angle = Math.abs(number(card, '--tilt-y'));
        expect(Number.isFinite(angle)).toBe(true);
        // 指针在卡片那一侧的反方向 → 方向对、幅度不超过满幅。
        expect(angle).toBeLessThanOrEqual(12.5);
    });

    it('amplitude 为 0 也不除零：反光仍是 50%', () => {
        render(<AreaGrid options={{ ...AREA_OPTIONS, amplitude: 0 }} />);
        const card = screen.getByTestId('card-0');

        moveInArea(screen.getByTestId('area'), 400, 200);
        tick(60);

        // `reach = Math.max(amplitude, 1)`：没有它，`--tilt-gx` 会变成 Infinity。
        expect(css(card, '--tilt-gx')).toBe('50.00%');
        expect(css(card, '--tilt-gy')).toBe('50.00%');
        expect(number(card, '--tilt-y')).toBe(0);
    });

    it('scale 小于 1 时抬起是缩小而不是放大', () => {
        render(<AreaGrid options={{ ...AREA_OPTIONS, scale: 0.8 }} />);
        const card = screen.getByTestId('card-0');

        // 指针压在卡片正中（细节区里），lift → 1，缩放 = 1 + (0.8-1)*1 = 0.8。
        moveInArea(screen.getByTestId('area'), 200, 200);
        tick(120);

        expect(number(card, '--tilt-scale')).toBeCloseTo(0.8, 2);
    });
});

describe('useTiltGrid · area 模式的输入来源', () => {
    it('触屏走不进鼠标那条路：area 模式也不倾', () => {
        render(<AreaGrid options={AREA_OPTIONS} />);
        const card = screen.getByTestId('card-0');

        moveInArea(screen.getByTestId('area'), 400, 200, 'touch');
        tick(5);

        // `pointerType !== 'mouse'` 的闸在最前面，两种模式共用。
        expect(css(card, '--tilt-x')).toBe('');
        expect(css(card, '--tilt-y')).toBe('');
    });

    it('指针离开详情页时整批回正并清变量', () => {
        render(<AreaGrid options={AREA_OPTIONS} />);
        const card = screen.getByTestId('card-0');

        moveInArea(screen.getByTestId('area'), 400, 200);
        tick(60);
        expect(Math.abs(number(card, '--tilt-y'))).toBeGreaterThan(1);

        fireEvent.pointerLeave(screen.getByTestId('area'));
        tick(200);

        expect(css(card, '--tilt-y')).toBe('');
        expect(css(card, '--tilt-glare')).toBe('');
    });

    it('指针压在卡片的右边界上就算「在卡上」，高光亮起', () => {
        render(<AreaGrid options={AREA_OPTIONS} />);
        const card = screen.getByTestId('card-0');

        // 卡片的 rect 是 0..400；`inX` 用的是闭区间。
        moveInArea(screen.getByTestId('area'), 400, 200);
        tick(60);

        expect(number(card, '--tilt-glare')).toBeGreaterThan(0.8);
    });

    it('卡片的右外侧一格就不亮（倾斜照旧）', () => {
        render(<AreaGrid options={AREA_OPTIONS} />);
        const card = screen.getByTestId('card-0');

        moveInArea(screen.getByTestId('area'), 401, 200);
        tick(80);

        expect(number(card, '--tilt-glare')).toBeLessThan(0.05);
        // 角度还有（只是被衰减），不是整条链断掉。
        expect(Math.abs(number(card, '--tilt-y'))).toBeGreaterThan(1);
    });
});

describe('useTiltGrid · 减少动态效果', () => {
    it('系统级 prefers-reduced-motion 也拦住，不只看 class', () => {
        // 代码读两处：`.reduce-effects` class 与 matchMedia。后者常被漏掉。
        (window as { matchMedia?: unknown }).matchMedia = vi.fn(() => ({
            matches: true,
            addEventListener: () => {},
            removeEventListener: () => {},
            addListener: () => {},
            removeListener: () => {},
            media: '(prefers-reduced-motion: reduce)',
            onchange: null,
            dispatchEvent: () => false
        })) as never;

        render(<AreaGrid options={AREA_OPTIONS} />);
        const card = screen.getByTestId('card-0');

        moveInArea(screen.getByTestId('area'), 400, 200);
        tick(5);

        expect(css(card, '--tilt-x')).toBe('');
        expect(css(card, '--tilt-y')).toBe('');
    });

    it('只在挂载时读一次：挂载后再打开开关也不停', () => {
        // 这份「一次」是刻意写进注释的行为（之后的翻转交给 CSS）。钉住它，
        // 免得有人以为这里是响应式的、把力气花在订阅上。
        render(<AreaGrid options={AREA_OPTIONS} />);
        const card = screen.getByTestId('card-0');

        document.documentElement.classList.add('reduce-effects');
        moveInArea(screen.getByTestId('area'), 400, 200);
        tick(60);

        expect(Math.abs(number(card, '--tilt-y'))).toBeGreaterThan(1);
    });

    it('挂载前就开着的话，一条监听都不挂', () => {
        document.documentElement.classList.add('reduce-effects');
        render(<AreaGrid options={AREA_OPTIONS} />);
        const card = screen.getByTestId('card-0');

        moveInArea(screen.getByTestId('area'), 400, 200);
        tick(60);

        expect(css(card, '--tilt-x')).toBe('');
        expect(screen.getByTestId('card-0')).toBe(card);
    });
});

describe('useTiltGrid · 帧与缓存', () => {
    it('停了很久之后的一帧不会把弹簧推飞（dt 被钳到 50ms）', () => {
        render(<AreaGrid options={AREA_OPTIONS} />);
        const card = screen.getByTestId('card-0');

        moveInArea(screen.getByTestId('area'), 300, 150);
        // 直接跳到 10 秒后：`dt` 会被钳成 0.05，否则一整段弹簧在一步里发散。
        advanceTo(clock + 10_000);
        tick(200);

        const angle = Math.abs(number(card, '--tilt-y'));
        expect(Number.isFinite(angle)).toBe(true);
        expect(angle).toBeLessThanOrEqual(12.5);
        expect(css(card, '--tilt-y')).not.toContain('Infinity');
    });

    it('标签节点被换掉之后写的是新节点，不是往空气里写', () => {
        render(<AreaGrid options={AREA_OPTIONS} withTip />);
        const card = screen.getByTestId('card-0');

        moveInArea(screen.getByTestId('area'), 300, 150);
        tick(20);
        const staleTip = screen.getByTestId('tip-0');
        expect(staleTip.style.transform).toContain('translate3d');
        const staleTransform = staleTip.style.transform;

        // React 换掉节点：旧的脱离文档，缓存里那一份失效。
        staleTip.remove();
        const freshTip = document.createElement('span');
        freshTip.setAttribute('data-tilt-tip', '');
        freshTip.setAttribute('data-testid', 'tip-0');
        card.append(freshTip);
        freshTip.style.transform = '';

        moveInArea(screen.getByTestId('area'), 380, 180);
        tick(20);

        // `isConnected` 检查让 `paintTip` 重查一次，于是新节点拿到了变换。
        expect(freshTip.style.transform).toContain('translate3d');
        expect(freshTip.style.opacity).not.toBe('');
        // 旧节点没被继续写（它已经不在文档里，写进去就是纯粹的空气）。
        expect(staleTip.style.transform).toBe(staleTransform);
    });

    it('卡片模式没有「区域」，靠 closest 认目标：area 选项不给 selector 就退回卡片模式', () => {
        render(<AreaGrid options={{ track: 'area' }} />);
        const card = screen.getByTestId('card-0');

        // `areaSelector` 缺省 → `areaAncestor` 是 null → 走卡片那条。
        moveInArea(screen.getByTestId('area'), 600, 200);
        tick(60);

        // 指针没压在卡上（卡片 0..400），所以不动。
        expect(Math.abs(number(card, '--tilt-y'))).toBeLessThan(0.05);
    });
});
