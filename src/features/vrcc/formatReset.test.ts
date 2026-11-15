// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { formatReset } from './formatReset';

describe('formatReset', () => {
    it('不足一分钟也说 1m，不说 0m', () => {
        expect(formatReset(0)).toBe('1m');
        expect(formatReset(59)).toBe('1m');
    });

    it('整小时只写小时', () => {
        expect(formatReset(3600)).toBe('1h');
        expect(formatReset(7200)).toBe('2h');
    });

    it('多出来的秒向上取整成分钟', () => {
        expect(formatReset(3601)).toBe('1h 1m');
        expect(formatReset(5400)).toBe('1h 30m');
    });

    it('差几秒到整小时时进位成整小时，不写成 60m', () => {
        expect(formatReset(3599)).toBe('1h');
        expect(formatReset(7199)).toBe('2h');
        expect(formatReset(86399)).toBe('24h');
    });
});
