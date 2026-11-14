// SPDX-License-Identifier: MPL-2.0
// @vitest-environment jsdom

import { cleanup, render } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('react-i18next', async (importOriginal) => {
    const actual = await importOriginal<typeof import('react-i18next')>();
    return {
        ...actual,
        useTranslation: () => ({ t: (key: string) => key })
    };
});

// 详情请求会走 IPC，测试环境里没有 `commands`。这里只关心形变参数有没有落到
// DOM 上，让请求直接失败即可（组件会落到 `error` 态，不影响外层属性）。
vi.mock('../vrccApi', async (importOriginal) => {
    const actual = await importOriginal<typeof import('../vrccApi')>();
    return {
        ...actual,
        vrccApi: {
            ...actual.vrccApi,
            model: vi.fn(() => Promise.reject(new Error('no ipc')))
        }
    };
});

import { VrccModelDialog } from './VrccModelDialog';

const model = {
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
    fresh: null
};

function renderDialog(morph: Parameters<typeof VrccModelDialog>[0]['morph']) {
    render(
        <VrccModelDialog
            model={model}
            morph={morph}
            open
            onOpenChange={() => {}}
            onSettled={() => {}}
            onGone={() => {}}
        />
    );
    return document.querySelector<HTMLElement>('[data-slot="dialog-content"]')!;
}

afterEach(cleanup);

describe('VrccModelDialog 的形变参数', () => {
    it('把位移与起始缩放写成 CSS 变量，并打上 data-morph', () => {
        // 这条钉的是「参数真的穿过底座落到了 DOM 上」：`data-*` 和 `style`
        // 都要经 `DialogPrimitive.Popup` 透传，被吞掉的话形变会静默不播，
        // 而且没有任何报错。
        const content = renderDialog({
            cardDx: 12,
            cardDy: -8,
            dialogDx: -12,
            dialogDy: 8,
            fromScale: 0.3125
        });

        expect(content.hasAttribute('data-morph')).toBe(true);
        expect(content.style.getPropertyValue('--morph-dialog-dx')).toBe(
            '-12px'
        );
        expect(content.style.getPropertyValue('--morph-dialog-dy')).toBe('8px');
        expect(content.style.getPropertyValue('--morph-from-scale')).toBe(
            '0.3125'
        );
    });

    it('没有形变计划时不留痕——减动效和 jsdom 走的就是这条路', () => {
        const content = renderDialog(null);

        expect(content.hasAttribute('data-morph')).toBe(false);
        expect(content.style.getPropertyValue('--morph-from-scale')).toBe('');
    });

    it('关闭按钮走底座的 DialogClose，不再自绘 ✕', () => {
        const content = renderDialog(null);

        // 右上角那组是「外链 + 关闭」并排，和公告弹窗同一套排版。
        const buttons = [
            ...content.querySelectorAll<HTMLElement>(
                '[data-slot="dialog-close"]'
            )
        ];
        expect(buttons).toHaveLength(1);
    });
});
