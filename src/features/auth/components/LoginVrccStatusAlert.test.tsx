// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { VrccHealth } from '@/lib/vrccHealth';

import { LoginVrccStatusAlert } from './LoginVrccStatusAlert';

/**
 * 插值要看得见。
 *
 * 全局 setup 里的 `t` 是 `(key) => key`，会把插值参数丢掉 —— 于是"服务端原文有没有
 * 显示出来"这件事在测试里根本断言不到，之前就因此漏掉了 `reason` 没渲染。
 * 这里把参数拼进返回值，让它暴露出来。
 */
vi.mock('react-i18next', () => ({
    useTranslation: () => ({
        t: (key: string, vars?: Record<string, unknown>) =>
            vars && typeof vars.message === 'string'
                ? `${key}(${vars.message})`
                : key,
        i18n: { language: 'en', changeLanguage: () => Promise.resolve() }
    })
}));

function renderAlert(health: VrccHealth) {
    const onOpenStatusPage = vi.fn();
    const result = render(
        <LoginVrccStatusAlert
            health={health}
            onOpenStatusPage={onOpenStatusPage}
        />
    );
    return { ...result, onOpenStatusPage };
}

describe('LoginVrccStatusAlert', () => {
    afterEach(cleanup);

    // 判定本身在 `@/lib/vrccHealth` 里测过（含"网络失败 vs 服务端 5xx"）。
    // 这里只钉"每个结论长什么样"。
    describe('不该出现的结论', () => {
        it('正常时什么都不渲染', () => {
            const { container } = renderAlert({ state: 'ok', at: 1 });

            expect(container.childElementCount).toBe(0);
        });

        it('还没探到时也不渲染：启动瞬间闪提示比不弹更糟', () => {
            const { container } = renderAlert({ state: 'unknown' });

            expect(container.childElementCount).toBe(0);
        });
    });

    // 这次改动的核心：两种情况处置方向相反，界面上不能长得一样。
    describe('区分"谁的锅"', () => {
        it('连不上时说明是网络/代理的问题', () => {
            renderAlert({
                state: 'unreachable',
                reason: 'error sending request',
                transportKind: 'connect'
            });

            const text = screen.getByRole('alert').textContent ?? '';
            expect(text).toContain('vrcc.health.unreachable');
            expect(text).toContain('vrcc.error.network.connect');
        });

        // ★ 卡在哪一步要说出来：DNS 失败和连不上是两件事，处置方式不同。
        it.each([
            ['dns', 'vrcc.error.network.dns'],
            ['connect', 'vrcc.error.network.connect'],
            ['timeout', 'vrcc.error.network.timeout'],
            // 响应中断、其它、以及拿不到环节：都落到通用那条 ——
            // 它们对玩家没有可执行的区别。
            ['body', 'vrcc.error.network.generic'],
            ['other', 'vrcc.error.network.generic'],
            [null, 'vrcc.error.network.generic']
        ] as const)(
            'transportKind=%s 时用 %s',
            (transportKind, expectedKey) => {
                renderAlert({
                    state: 'unreachable',
                    reason: 'error sending request',
                    transportKind
                });

                const text = screen.getByRole('alert').textContent ?? '';
                expect(text).toContain(expectedKey);
            }
        );

        it('服务端 5xx 时不说网络，也不给建议', () => {
            renderAlert({
                state: 'failing',
                reason: '星遇同频接口返回 HTTP 500（http_500）'
            });

            const text = screen.getByRole('alert').textContent ?? '';
            expect(text).toContain('vrcc.health.failing');
            // 玩家什么也做不了，多说一句"查网络"只会误导。
            expect(text).not.toContain('vrcc.error.network');
        });

        // ★ 服务端到底回了什么，必须摆出来。
        // 只写一句「接口当前不可用」，玩家只能对着一个红点猜是谁的问题。
        it('把服务端的原文显示出来，包括 HTTP 状态码', () => {
            renderAlert({
                state: 'failing',
                reason: '星遇同频接口返回 HTTP 500（http_500）'
            });

            const text = screen.getByRole('alert').textContent ?? '';
            expect(text).toContain('vrcc.health.server_said');
            expect(text).toContain('星遇同频接口返回 HTTP 500（http_500）');
        });

        it('说明这不是本地设置能修的，并说清会自动重试', () => {
            renderAlert({ state: 'failing', reason: 'HTTP 500' });

            const text = screen.getByRole('alert').textContent ?? '';
            expect(text).toContain('vrcc.health.failing_hint');
        });

        // 网络层没有"服务端原文"这回事：请求根本没到对面。
        it('连不上时不显示"服务端原文"', () => {
            renderAlert({
                state: 'unreachable',
                reason: 'error sending request',
                transportKind: 'connect'
            });

            const text = screen.getByRole('alert').textContent ?? '';
            expect(text).not.toContain('vrcc.health.server_said');
        });

        it('两种都用同一套外观：严重度我们并不知道，红色留给定级明确的故障', () => {
            const unreachable = renderAlert({
                state: 'unreachable',
                reason: 'boom',
                transportKind: null
            });
            const unreachableClass =
                unreachable.container.querySelector('[data-slot="alert"]')
                    ?.className ?? '';
            cleanup();

            const failing = renderAlert({ state: 'failing', reason: 'boom' });
            const failingClass =
                failing.container.querySelector('[data-slot="alert"]')
                    ?.className ?? '';

            expect(unreachableClass).not.toMatch(/destructive/);
            expect(failingClass).not.toMatch(/destructive/);
        });
    });

    it('给出去 vrcc.cc 的入口', () => {
        const { onOpenStatusPage } = renderAlert({
            state: 'unreachable',
            reason: 'boom',
            transportKind: 'dns'
        });

        fireEvent.click(
            screen.getByRole('button', { name: /vrcc\.health\.view_site/ })
        );
        expect(onOpenStatusPage).toHaveBeenCalledTimes(1);
    });
});
