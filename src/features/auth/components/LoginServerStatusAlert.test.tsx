// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { ServerHealth } from '@/lib/serverHealth';

import { LoginServerStatusAlert } from './LoginServerStatusAlert';

function renderAlert(health: ServerHealth) {
    const onOpenStatusPage = vi.fn();
    const result = render(
        <LoginServerStatusAlert
            health={health}
            onOpenStatusPage={onOpenStatusPage}
        />
    );
    return { ...result, onOpenStatusPage };
}

describe('LoginServerStatusAlert', () => {
    afterEach(cleanup);

    // 判定本身在 `@/lib/serverHealth` 里测过（包括"刷新失败不能读成健康"）。
    // 这里只钉"每个结论长什么样"，不重复测判定。
    describe('不该出现的结论', () => {
        it('健康时什么都不渲染', () => {
            const { container } = renderAlert({
                state: 'healthy',
                checkedAt: '2026-11-20T00:00:00.000Z'
            });

            expect(container.childElementCount).toBe(0);
        });

        it('还没取到过时也不渲染：启动瞬间闪告警比不弹更糟', () => {
            const { container } = renderAlert({ state: 'unknown' });

            expect(container.childElementCount).toBe(0);
        });
    });

    describe('服务报了异常', () => {
        it('显示组件名，并给出去状态页的入口', () => {
            const { onOpenStatusPage } = renderAlert({
                state: 'issue',
                severity: 'major',
                detail: 'API, Website',
                stale: false,
                checkedAt: '2026-11-20T00:00:00.000Z'
            });

            expect(screen.getByRole('alert').textContent).toContain(
                'API, Website'
            );
            fireEvent.click(
                screen.getByRole('button', {
                    name: 'status_bar.view_status'
                })
            );
            expect(onOpenStatusPage).toHaveBeenCalledTimes(1);
        });

        it('重大故障用 destructive', () => {
            const { container } = renderAlert({
                state: 'issue',
                severity: 'major',
                detail: 'API',
                stale: false,
                checkedAt: null
            });

            const alert = container.querySelector('[data-slot="alert"]');
            expect(alert?.className).toMatch(/destructive/);
        });

        it('轻微异常不用 destructive', () => {
            const { container } = renderAlert({
                state: 'issue',
                severity: 'minor',
                detail: 'API',
                stale: false,
                checkedAt: null
            });

            const alert = container.querySelector('[data-slot="alert"]');
            expect(alert?.className).not.toMatch(/destructive/);
        });

        it('陈旧结论额外说明它不是此刻的', () => {
            renderAlert({
                state: 'issue',
                severity: 'major',
                detail: 'Major Service Outage',
                stale: true,
                checkedAt: '2026-11-20T00:00:00.000Z'
            });

            expect(screen.getByRole('alert').textContent).toContain(
                'status_bar.servers_stale'
            );
        });

        it('结论不是陈旧的时候不出现那句话', () => {
            renderAlert({
                state: 'issue',
                severity: 'major',
                detail: 'Major Service Outage',
                stale: false,
                checkedAt: '2026-11-20T00:00:00.000Z'
            });

            expect(screen.getByRole('alert').textContent).not.toContain(
                'status_bar.servers_stale'
            );
        });
    });

    describe('取不到状态', () => {
        // 这是这次改动的主要目的：以前这种情况登录页一声不吭。
        it('要说出来，而不是沉默', () => {
            renderAlert({
                state: 'unreachable',
                reason: 'error sending request',
                checkedAt: '2026-11-20T00:00:00.000Z'
            });

            const text = screen.getByRole('alert').textContent ?? '';
            expect(text).toContain('status_bar.servers_unreachable');
            expect(text).toContain('error sending request');
        });

        it('标题不说"服务器异常"：取不到不等于服务坏了', () => {
            renderAlert({
                state: 'unreachable',
                reason: 'error sending request',
                checkedAt: null
            });

            const text = screen.getByRole('alert').textContent ?? '';
            expect(text).not.toContain('status_bar.servers_issue');
        });

        it('不用 destructive：玩家的网络问题不该染成重大故障的红', () => {
            const { container } = renderAlert({
                state: 'unreachable',
                reason: 'error sending request',
                checkedAt: null
            });

            const alert = container.querySelector('[data-slot="alert"]');
            expect(alert?.className).not.toMatch(/destructive/);
        });
    });
});
