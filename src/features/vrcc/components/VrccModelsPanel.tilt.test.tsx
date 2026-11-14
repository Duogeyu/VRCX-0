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

vi.mock('react-i18next', async (importOriginal) => {
    const actual = await importOriginal<typeof import('react-i18next')>();
    return {
        ...actual,
        useTranslation: () => ({ t: (key: string) => key })
    };
});

vi.mock('../useOpenExternalLink', () => ({
    useOpenExternalLink: () => ({ open: vi.fn(), pending: false })
}));

const apiModel = vi.hoisted(() => vi.fn());
vi.mock('../vrccApi', async (importOriginal) => {
    const actual = await importOriginal<typeof import('../vrccApi')>();
    return {
        ...actual,
        vrccApi: { ...actual.vrccApi, model: apiModel }
    };
});

import { MORPH_ENTRY_MS } from '../cardMorph';
import type { VrccBoardModel, VrccModelBoard } from '../vrccApi';
import { VrccModelsPanel } from './VrccModelsPanel';

/**
 * 这一组测**接线**，不测手感（数值行为由 `useTiltGrid.test.tsx` 覆盖）。
 *
 * 它存在的理由是一个真踩过的坑：模型榜异步加载，首帧网格不渲染，
 * `ref.current` 是 null，`useEffect(…, [])` 当场退出且永不重跑——界面上
 * 「鼠标移上去完全没反应」，而单元测试全绿。
 */
let frames: FrameRequestCallback[] = [];
let clock = 0;

function tick(count = 1) {
    for (let i = 0; i < count; i++) {
        clock += 16;
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
    // 详情请求走 IPC；测试里没有 `commands`。这条文件的用例只关心形变接线，
    // 让请求直接失败即可，需要验 404 收回的用例自己铺 `mockResolvedValueOnce`。
    apiModel.mockReset();
    apiModel.mockRejectedValue(new Error('no ipc'));
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
});

function model(overrides: Partial<VrccBoardModel> = {}): VrccBoardModel {
    return {
        id: 1,
        name: 'オリジナル3Dモデル「星」',
        short: '星',
        shop: '星屋',
        price: '¥1,500',
        likes: 42,
        week: null,
        rank: 1,
        page: null,
        img: null,
        booth: null,
        hot: null,
        place: null,
        move: null,
        fresh: null,
        ...overrides
    };
}

function board(models: VrccBoardModel[]): VrccModelBoard {
    return {
        board: 'models',
        sort: 'top',
        total: models.length,
        next: null,
        weeks: [],
        w: null,
        at: 1,
        models
    };
}

function stubRect(element: Element) {
    element.getBoundingClientRect = () =>
        ({
            left: 0,
            top: 0,
            width: 200,
            height: 300,
            right: 200,
            bottom: 300,
            x: 0,
            y: 0,
            toJSON: () => ({})
        }) as DOMRect;
}

describe('VrccModelsPanel 倾斜接线', () => {
    it('数据异步到齐之后，网格仍然接上了指针监听', () => {
        const { container, rerender } = render(
            <VrccModelsPanel
                models={null}
                sort="top"
                sortSupported
                available
                onSortChange={() => {}}
                onRefresh={() => {}}
            />
        );
        expect(container.querySelector('ul')).toBeNull();
        rerender(
            <VrccModelsPanel
                models={board([model()])}
                sort="top"
                sortSupported
                available
                onSortChange={() => {}}
                onRefresh={() => {}}
            />
        );

        const card = container.querySelector<HTMLElement>('[data-tilt]')!;
        expect(card).not.toBeNull();
        stubRect(card);

        // 派发到**卡片**上：真实指针也是落在卡片里再冒泡到网格。
        const event = new MouseEvent('pointermove', {
            bubbles: true,
            clientX: 200,
            clientY: 300
        });
        Object.defineProperty(event, 'pointerType', { value: 'mouse' });
        fireEvent(card, event);
        tick(120);

        expect(card.style.getPropertyValue('--tilt-y')).not.toBe('');
    });

    it('点封面打开评论弹窗', () => {
        const { container } = render(
            <VrccModelsPanel
                models={board([model()])}
                sort="top"
                sortSupported
                available
                onSortChange={() => {}}
                onRefresh={() => {}}
            />
        );

        // 卡片上只有一个动作：点它开评论弹窗。翻面不再是独立状态，
        // 它是形变的前半程（见 cardMorph.ts）。
        expect(screen.queryByText('vrcc.models.hot_comments')).toBeNull();

        fireEvent.click(
            container.querySelector<HTMLElement>('.vrcc-model-card__cover')!
        );

        expect(screen.getByText('vrcc.models.hot_comments')).toBeTruthy();
    });

    it('同一时刻只开一个弹窗', () => {
        const { container } = render(
            <VrccModelsPanel
                models={board([model({ id: 1 }), model({ id: 2 })])}
                sort="top"
                sortSupported
                available
                onSortChange={() => {}}
                onRefresh={() => {}}
            />
        );

        const covers = [
            ...container.querySelectorAll<HTMLElement>(
                '.vrcc-model-card__cover'
            )
        ];

        fireEvent.click(covers[0]);
        expect(screen.getAllByText('vrcc.models.hot_comments')).toHaveLength(1);

        // 点第二张时第一张的弹窗被换掉，不会两个一起开着。
        fireEvent.click(covers[1]);
        expect(screen.getAllByText('vrcc.models.hot_comments')).toHaveLength(1);
    });

    it('换排序时把弹窗收起来', () => {
        const { container, rerender } = render(
            <VrccModelsPanel
                models={board([model()])}
                sort="top"
                sortSupported
                available
                onSortChange={() => {}}
                onRefresh={() => {}}
            />
        );

        fireEvent.click(
            container.querySelector<HTMLElement>('.vrcc-model-card__cover')!
        );
        expect(screen.getByText('vrcc.models.hot_comments')).toBeTruthy();

        // 数据换了还留着旧详情，看到的就是另一条东西的内容。
        rerender(
            <VrccModelsPanel
                models={board([model()])}
                sort="week"
                sortSupported
                available
                onSortChange={() => {}}
                onRefresh={() => {}}
            />
        );

        expect(screen.queryByText('vrcc.models.hot_comments')).toBeNull();
    });

    it('飞行途中要求关闭只是排队：入场落地之后才收（决策十）', () => {
        vi.useFakeTimers();
        try {
            const { container } = render(
                <VrccModelsPanel
                    models={board([model()])}
                    sort="top"
                    sortSupported
                    available
                    onSortChange={() => {}}
                    onRefresh={() => {}}
                />
            );

            const card = container.querySelector<HTMLElement>('[data-tilt]')!;
            stubRect(card);
            fireEvent.click(
                container.querySelector<HTMLElement>('.vrcc-model-card__cover')!
            );

            // 飞行层在 → 入场没落地。
            const flight = document.querySelector('.vrcc-morph-flight');
            expect(flight).not.toBeNull();
            expect(flight!.hasAttribute('data-closing')).toBe(false);

            // 此刻要求关闭（点弹窗的关闭按钮）：不该立刻消失，而是排队。
            fireEvent.click(
                document.querySelector<HTMLElement>(
                    '[data-slot="dialog-close"]'
                )!
            );
            expect(
                document
                    .querySelector('.vrcc-morph-flight')!
                    .hasAttribute('data-closing')
            ).toBe(false);

            // 入场时长走完 → 排队的关闭这时才生效。
            //
            // 这里不断言 `data-closing`：定时器和 Base UI 的收起动画在 jsdom 里
            // 没有真实时间轴，`advanceTimersByTime` 一推，收起、卸载、收尾
            // 可能在同一拍里走完。真正要钉住的是**排队这件事本身** ——
            // 也就是上一拍「要求关闭时没有立刻收」，以及下面这一拍「收掉了」。
            act(() => {
                vi.advanceTimersByTime(MORPH_ENTRY_MS + 10);
            });
            expect(document.querySelector('[data-slot="dialog-content"]')).toBe(
                null
            );
            expect(document.querySelector('.vrcc-morph-flight')).toBeNull();
        } finally {
            vi.useRealTimers();
        }
    });

    it('入场途中被确认「不在榜上」就当场收回，不飞完（决策十三）', async () => {
        // 404 是**正常状态**（下架/未过审），不是故障；但继续飞完 800ms 再
        // 告诉用户「没有」，比当场收回去更怪。这是唯一允许中途反悔的情况。
        apiModel.mockResolvedValue(null);

        const { container } = render(
            <VrccModelsPanel
                models={board([model()])}
                sort="top"
                sortSupported
                available
                onSortChange={() => {}}
                onRefresh={() => {}}
            />
        );

        const card = container.querySelector<HTMLElement>('[data-tilt]')!;
        stubRect(card);
        fireEvent.click(
            container.querySelector<HTMLElement>('.vrcc-model-card__cover')!
        );

        // 入场刚开始：飞行层在。
        expect(document.querySelector('.vrcc-morph-flight')).not.toBeNull();

        // 请求回来是 null → 立刻收起，不等 MORPH_ENTRY_MS。
        await act(async () => {
            await Promise.resolve();
        });

        expect(document.querySelector('.vrcc-morph-flight')).toBeNull();
    });

    it('落地之后才收到 404：照常显示解释，不收回（决策十三的另一半）', async () => {
        vi.useFakeTimers();
        try {
            apiModel.mockResolvedValue(null);

            const { container } = render(
                <VrccModelsPanel
                    models={board([model()])}
                    sort="top"
                    sortSupported
                    available
                    onSortChange={() => {}}
                    onRefresh={() => {}}
                />
            );

            const card = container.querySelector<HTMLElement>('[data-tilt]')!;
            stubRect(card);
            fireEvent.click(
                container.querySelector<HTMLElement>('.vrcc-model-card__cover')!
            );

            // 先让入场走完，请求再回来：这时弹窗已经在位，`onGone` 不该收回。
            act(() => {
                vi.advanceTimersByTime(MORPH_ENTRY_MS + 10);
            });
            await act(async () => {
                await Promise.resolve();
            });

            expect(
                document.querySelector('[data-slot="dialog-content"]')
            ).not.toBeNull();
        } finally {
            vi.useRealTimers();
        }
    });

    it('减动效命中时不挂飞行层，直接开弹窗', () => {
        // 两条通道都要关掉飞行：系统偏好，和设置里那个 `.reduce-effects`。
        // 这里测系统偏好这一条。
        const previous = window.matchMedia;
        window.matchMedia = ((query: string) => ({
            matches: query.includes('prefers-reduced-motion'),
            media: query,
            onchange: null,
            addListener: () => {},
            removeListener: () => {},
            addEventListener: () => {},
            removeEventListener: () => {},
            dispatchEvent: () => false
        })) as typeof window.matchMedia;

        try {
            const { container } = render(
                <VrccModelsPanel
                    models={board([model()])}
                    sort="top"
                    sortSupported
                    available
                    onSortChange={() => {}}
                    onRefresh={() => {}}
                />
            );

            const card = container.querySelector<HTMLElement>('[data-tilt]')!;
            stubRect(card);
            fireEvent.click(
                container.querySelector<HTMLElement>('.vrcc-model-card__cover')!
            );

            // 没有飞行体，弹窗照样开。
            expect(document.querySelector('.vrcc-morph-flight')).toBeNull();
            expect(
                document.querySelector('[data-slot="dialog-content"]')
            ).not.toBeNull();
        } finally {
            window.matchMedia = previous;
        }
    });

    it('设置里关掉动效（.reduce-effects）时同样不挂飞行层', () => {
        document.documentElement.classList.add('reduce-effects');
        try {
            const { container } = render(
                <VrccModelsPanel
                    models={board([model()])}
                    sort="top"
                    sortSupported
                    available
                    onSortChange={() => {}}
                    onRefresh={() => {}}
                />
            );

            const card = container.querySelector<HTMLElement>('[data-tilt]')!;
            stubRect(card);
            fireEvent.click(
                container.querySelector<HTMLElement>('.vrcc-model-card__cover')!
            );

            expect(document.querySelector('.vrcc-morph-flight')).toBeNull();
        } finally {
            document.documentElement.classList.remove('reduce-effects');
        }
    });

    it('过期的入场定时器不会把下一个弹窗提前收掉', async () => {
        // 入场定时器有六个清理点：卸载、换页/换排序、`openCard` 起表之前、
        // 无动画分支、`onSettled`、`onGone`。它们互为冗余——单删任意一处，其余
        // 会兜住，所以只能从**外部行为**钉住「整个系统不会留下过期定时器」。
        //
        // 这里构造唯一能让过期定时器露头的序列：A 在入场途中被 404 收回（它的
        // 定时器本该随之作废）→ 300ms 后打开 B → B 还在入场时要求关闭（排队）
        // → 若 A 那个定时器还活着，它会在 t≈800 把 B 提前收掉，而不是等 B
        // 自己的 t≈1100。
        vi.useFakeTimers();
        try {
            apiModel.mockResolvedValueOnce(null); // 第一张：不在榜上
            apiModel.mockResolvedValue({ model: null, hots: [], at: 1 }); // 第二张：正常

            const { container } = render(
                <VrccModelsPanel
                    models={board([model({ id: 1 }), model({ id: 2 })])}
                    sort="top"
                    sortSupported
                    available
                    onSortChange={() => {}}
                    onRefresh={() => {}}
                />
            );

            const cards = () =>
                container.querySelectorAll<HTMLElement>('[data-tilt]');
            expect(cards()).toHaveLength(2);
            stubRect(cards()[0]);
            stubRect(cards()[1]);

            // A：点开 → 404 回来 → 当场收回。
            fireEvent.click(
                cards()[0].querySelector<HTMLElement>(
                    '.vrcc-model-card__cover'
                )!
            );
            expect(document.querySelector('.vrcc-morph-flight')).not.toBeNull();
            await act(async () => {
                await Promise.resolve();
            });
            expect(document.querySelector('.vrcc-morph-flight')).toBeNull();

            // t=300：打开 B。
            act(() => {
                vi.advanceTimersByTime(300);
            });
            fireEvent.click(
                cards()[1].querySelector<HTMLElement>(
                    '.vrcc-model-card__cover'
                )!
            );
            await act(async () => {
                await Promise.resolve();
            });
            expect(document.querySelector('.vrcc-morph-flight')).not.toBeNull();

            // B 还在入场：此时要求关闭只是排队，不该在半空中掉头。
            fireEvent.click(
                document.querySelector<HTMLElement>(
                    '[data-slot="dialog-close"]'
                )!
            );
            expect(
                document
                    .querySelector('.vrcc-morph-flight')!
                    .hasAttribute('data-closing')
            ).toBe(false);

            // 推到 t≈900：跨过 A 那个定时器本该触发的时刻，还没到 B 的 1100。
            act(() => {
                vi.advanceTimersByTime(600);
            });
            expect(document.querySelector('.vrcc-morph-flight')).not.toBeNull();
            expect(
                document
                    .querySelector('.vrcc-morph-flight')!
                    .hasAttribute('data-closing')
            ).toBe(false);
        } finally {
            vi.useRealTimers();
        }
    });
});
