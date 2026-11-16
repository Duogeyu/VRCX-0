// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it, vi } from 'vitest';

vi.mock('@/platform/tauri/client', () => ({
    tauriClient: { events: { subscribe: vi.fn() } }
}));

import { vrccAccountKey } from './vrccConnectionRepository';

describe('vrccAccountKey', () => {
    it('未连接（或读不到连接）时是 offline', () => {
        expect(vrccAccountKey(null)).toBe('offline');
        expect(vrccAccountKey(undefined)).toBe('offline');
        expect(vrccAccountKey({ connected: false, star: 77 })).toBe('offline');
    });

    it('已连接时按星号区分', () => {
        expect(vrccAccountKey({ connected: true, star: 77 })).toBe('star:77');
        expect(vrccAccountKey({ connected: true, star: 88 })).toBe('star:88');
    });

    it('已连接但没有星号时，不和「未连接」共用', () => {
        // 官方文档：没有点亮编号的玩家 `star` 是 null。
        expect(vrccAccountKey({ connected: true, star: null })).toBe(
            'star:none'
        );
    });
});
