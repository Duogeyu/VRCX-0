// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it, vi } from 'vitest';

import {
    clubCodeForGroup,
    isClubCodeShaped,
    vrccPublicClubQueryOptions
} from './vrccClubRepository';

const mocks = vi.hoisted(() => ({ appVrccClub: vi.fn() }));

vi.mock('@/platform/tauri/bindings', () => ({
    commands: { appVrccClub: mocks.appVrccClub }
}));

/**
 * 这个模块的两件事都不会在出错时报错：
 *
 * - 短码形状判错 → 该问的不问（或把明显不是短码的东西发去后端）；
 * - 拼码拼错 → 后端回 404，界面只是「没有社团」，没人会察觉。
 */
describe('isClubCodeShaped', () => {
    it('短码本体 3..6 位字母数字，位数边界两边都拒绝', () => {
        expect(isClubCodeShaped('abc.1234')).toBe(true);
        expect(isClubCodeShaped('abcdef.1234')).toBe(true);
        // 2 位太短、7 位太长：都超出官方 `pattern`。
        expect(isClubCodeShaped('ab.1234')).toBe(false);
        expect(isClubCodeShaped('abcdefg.1234')).toBe(false);
    });

    it('discriminator 必须**正好** 4 位数字', () => {
        expect(isClubCodeShaped('KIPFEL.2104')).toBe(true);
        expect(isClubCodeShaped('KIPFEL.0001')).toBe(true);
        expect(isClubCodeShaped('KIPFEL.210')).toBe(false);
        expect(isClubCodeShaped('KIPFEL.21045')).toBe(false);
        // 混字母、混符号都不算。
        expect(isClubCodeShaped('KIPFEL.21a4')).toBe(false);
        expect(isClubCodeShaped('KIPFEL.21-4')).toBe(false);
        // 全角数字目视像数字，但不是 `\d`。
        expect(isClubCodeShaped('KIPFEL.２１０４')).toBe(false);
    });

    it('大小写都不限，但短码本体里不能有分隔符', () => {
        expect(isClubCodeShaped('KIPFEL.2104')).toBe(true);
        expect(isClubCodeShaped('kipfel.2104')).toBe(true);
        expect(isClubCodeShaped('KiPfEl.2104')).toBe(true);
        expect(isClubCodeShaped('KIP_FEL.2104')).toBe(false);
        expect(isClubCodeShaped('KIP-FEL.2104')).toBe(false);
        expect(isClubCodeShaped('KIP FEL.2104')).toBe(false);
    });

    it('首尾空白先 trim，但中间的空格不会', () => {
        expect(isClubCodeShaped('  KIPFEL.2104  ')).toBe(true);
        expect(isClubCodeShaped('\tKIPFEL.2104\n')).toBe(true);
        expect(isClubCodeShaped('KIPFEL .2104')).toBe(false);
        expect(isClubCodeShaped('KIPFEL. 2104')).toBe(false);
    });

    it('点多余 / 缺项 / 空串都不算', () => {
        expect(isClubCodeShaped('')).toBe(false);
        expect(isClubCodeShaped('   ')).toBe(false);
        expect(isClubCodeShaped('KIPFEL')).toBe(false);
        expect(isClubCodeShaped('.2104')).toBe(false);
        expect(isClubCodeShaped('KIPFEL.')).toBe(false);
        expect(isClubCodeShaped('KIPFEL.2104.1')).toBe(false);
        expect(isClubCodeShaped('.')).toBe(false);
    });
});

describe('clubCodeForGroup', () => {
    it('拼出来是 shortCode + "." + discriminator，中间不加空格', () => {
        expect(
            clubCodeForGroup({ shortCode: 'KIPFEL', discriminator: '2104' })
        ).toBe('KIPFEL.2104');
    });

    it('两端空白各自 trim 掉再拼', () => {
        // 群组接口回来的字段偶尔带空白；拼出 "KIPFEL .2104" 会被形状检查挡下，
        // 于是「有社团的群组」也会静默变成没有。
        expect(
            clubCodeForGroup({ shortCode: ' KIPFEL ', discriminator: ' 2104 ' })
        ).toBe('KIPFEL.2104');
    });

    it('缺任何一半返回空串，不拼半截短码', () => {
        expect(clubCodeForGroup({})).toBe('');
        expect(clubCodeForGroup({ shortCode: 'KIPFEL' })).toBe('');
        expect(clubCodeForGroup({ discriminator: '2104' })).toBe('');
        expect(clubCodeForGroup({ shortCode: '', discriminator: '2104' })).toBe(
            ''
        );
        expect(
            clubCodeForGroup({ shortCode: 'KIPFEL', discriminator: null })
        ).toBe('');
        expect(
            clubCodeForGroup({ shortCode: null, discriminator: undefined })
        ).toBe('');
        // 纯空白也不算「有值」——`' '.trim()` 是空串。
        expect(
            clubCodeForGroup({ shortCode: '   ', discriminator: '2104' })
        ).toBe('');
    });

    it('数字形状的字段照样能拼（`String()` 兜底）', () => {
        // 后端把 discriminator 给成数字时不该拼出 "KIPFEL.undefined"。
        expect(
            clubCodeForGroup({
                shortCode: '520',
                discriminator: 6088 as unknown as string
            })
        ).toBe('520.6088');
    });
});

describe('vrccPublicClubQueryOptions', () => {
    it('按短码分键，且用的是 trim 之后的值', () => {
        const options = vrccPublicClubQueryOptions('  KIPFEL.2104  ');
        // 同一个社团从别处打开要命中同一份缓存，键里混进空格就分成了两份。
        expect(options.queryKey).toEqual(['group', 'vrcc-club', 'KIPFEL.2104']);
    });

    it('形状不对的码不发请求，直接 null', async () => {
        mocks.appVrccClub.mockReset();
        const options = vrccPublicClubQueryOptions('KIPFEL');

        await expect(options.queryFn?.({} as never)).resolves.toBeNull();
        expect(mocks.appVrccClub).not.toHaveBeenCalled();
    });

    it('形状对时把 trim 过的码交给后端', async () => {
        mocks.appVrccClub.mockReset();
        mocks.appVrccClub.mockResolvedValue({ club: { name: 'x' } });
        const options = vrccPublicClubQueryOptions(' kipfel.2104 ');

        await expect(options.queryFn?.({} as never)).resolves.toEqual({
            club: { name: 'x' }
        });
        expect(mocks.appVrccClub).toHaveBeenCalledWith('kipfel.2104');
    });

    it('404 是常态：不重试', () => {
        // 绝大多数群组没入驻社团。重试只会把已经确定的空结论多问两遍。
        expect(vrccPublicClubQueryOptions('KIPFEL.2104').retry).toBe(0);
        // 官方写「最多 1 分钟更新一次」，缓存该按半小时算。
        expect(
            vrccPublicClubQueryOptions('KIPFEL.2104').staleTime
        ).toBeGreaterThanOrEqual(60_000);
    });
});
