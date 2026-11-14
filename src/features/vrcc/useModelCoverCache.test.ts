// SPDX-License-Identifier: MPL-2.0
// @vitest-environment jsdom

import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
    hasModelCover,
    rememberModelCover,
    resetModelCoverCache,
    useModelCoverReady
} from './useModelCoverCache';

/**
 * 封面缓存的小测试。
 *
 * 只测这几个纯函数：`useModelCoverReady` 依赖 `new Image()` 和 `onload`，
 * 在 jsdom 里不会真的触发（没有网络），测它等于测 mock。真正要钉住的是
 * 「同一个 URL 记住之后就不再当成未缓存」和「上限满了淘汰谁」。
 *
 * `beforeEach` 里重置模块级状态。不重置的话只能靠「每个用例用不同的 URL」绕，
 * 那种隐性约定一旦有人复用 URL 就会产生极难查的失败——上一版就是这么写的。
 */
describe('useModelCoverCache', () => {
    beforeEach(() => {
        resetModelCoverCache();
    });

    it('记住之前是未知，记住之后是已知', () => {
        const url = 'https://booth.pximg.net/test-unknown.jpg';
        expect(hasModelCover(url)).toBe(false);

        rememberModelCover(url);
        expect(hasModelCover(url)).toBe(true);
    });

    it('只对记住的 URL 返回已知，不串号', () => {
        const a = 'https://booth.pximg.net/a.jpg';
        const b = 'https://booth.pximg.net/b.jpg';

        rememberModelCover(a);

        expect(hasModelCover(a)).toBe(true);
        expect(hasModelCover(b)).toBe(false);
    });

    it('超出上限时淘汰最旧的一批，而不是整表清空', () => {
        const first = 'https://booth.pximg.net/first.jpg';
        const last = 'https://booth.pximg.net/last.jpg';

        // 先塞满 500 个；`first` 是最早那个。
        rememberModelCover(first);
        for (let i = 0; i < 499; i += 1) {
            rememberModelCover(`https://booth.pximg.net/fill-${i}.jpg`);
        }
        expect(hasModelCover(first)).toBe(true);

        // 第 501 个：触发淘汰。
        rememberModelCover(last);

        // 最旧的被淘汰，新的一定进得去。
        expect(hasModelCover(first)).toBe(false);
        expect(hasModelCover(last)).toBe(true);
        // 关键：只是淘汰最旧的四分之一，不是整表失忆。
        // 整表清空会让「翻回上一页」变成一屏 24 张同时重新预载。
        expect(hasModelCover('https://booth.pximg.net/fill-498.jpg')).toBe(
            true
        );
    });

    it('一直塞下去不会无限增长', () => {
        for (let i = 0; i < 2000; i += 1) {
            rememberModelCover(`https://booth.pximg.net/grow-${i}.jpg`);
        }

        // 有新条目就一定还有旧条目被淘汰，不会把 2000 个全留着。
        expect(hasModelCover('https://booth.pximg.net/grow-1999.jpg')).toBe(
            true
        );
        expect(hasModelCover('https://booth.pximg.net/grow-0.jpg')).toBe(false);
    });
});

/**
 * `useModelCoverReady` 的行为。
 *
 * 这一层之前完全没有被测到：jsdom 不会真的加载图片，`new Image()` 的
 * `onload` 永远不触发，于是「预加载成功 → 记住 → 下次直接 ready」这条链
 * 在测试里一次都没跑过。这里用一个可控的 `Image` 桩把它跑起来。
 */
describe('useModelCoverReady', () => {
    /** 可控的 Image 桩：测试自己决定什么时候 onload / onerror。 */
    class FakeImage {
        static instances: FakeImage[] = [];
        onload: (() => void) | null = null;
        onerror: (() => void) | null = null;
        private _src = '';

        constructor() {
            FakeImage.instances.push(this);
        }

        set src(value: string) {
            this._src = value;
        }

        get src(): string {
            return this._src;
        }
    }

    function probe(src: string | null) {
        return renderHook(() => useModelCoverReady(src)).result;
    }

    beforeEach(() => {
        resetModelCoverCache();
        FakeImage.instances = [];
        vi.stubGlobal('Image', FakeImage);
    });

    afterEach(() => {
        vi.unstubAllGlobals();
    });

    it('第一次见到就预加载，成功之后才 ready', () => {
        const result = probe('https://booth.pximg.net/ready.jpg');
        expect(result.current).toBe(false);
        expect(FakeImage.instances).toHaveLength(1);

        act(() => {
            FakeImage.instances[0].onload?.();
        });

        expect(result.current).toBe(true);
        expect(hasModelCover('https://booth.pximg.net/ready.jpg')).toBe(true);
    });

    it('记过的地址重新挂载时立刻 ready，不再新建 Image', () => {
        rememberModelCover('https://booth.pximg.net/known.jpg');

        const result = probe('https://booth.pximg.net/known.jpg');

        expect(result.current).toBe(true);
        expect(FakeImage.instances).toHaveLength(0);
    });

    it('加载失败保持 false，也不记进缓存', () => {
        const result = probe('https://booth.pximg.net/broken.jpg');
        act(() => {
            FakeImage.instances[0].onerror?.();
        });

        expect(result.current).toBe(false);
        expect(hasModelCover('https://booth.pximg.net/broken.jpg')).toBe(false);
    });

    it('没有地址时不重新挂载 Image', () => {
        const result = probe(null);

        expect(result.current).toBe(false);
        expect(FakeImage.instances).toHaveLength(0);
    });
});
