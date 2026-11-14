// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { joinSegments, type ClubTextSeg } from './clubText';

/**
 * `joinSegments` 是**纯函数、没有任何调用方会报错**的那一类：拼错一个字符，界面
 * 上只是少一段字或差一个空格，不抛错、不进日得。星遇下发的分段又是
 * `["文字", 0|1]` 这种最容易在边界上出错的形状（空段、纯换行段、`hidden`
 * 段），所以这里把边界逐个钉死。
 */
describe('joinSegments', () => {
    it('非数组一律空串，不抛错', () => {
        expect(joinSegments(null)).toBe('');
        expect(joinSegments(undefined)).toBe('');
        // 后端换了形状（比如把 `segs` 直接给成字符串）时不能炸。
        expect(joinSegments('一段话' as unknown as ClubTextSeg[])).toBe('');
        expect(joinSegments({ text: 'x' } as unknown as ClubTextSeg[])).toBe(
            ''
        );
        expect(joinSegments([])).toEqual('');
    });

    it('按原顺序直接首尾相接：段与段之间不加任何分隔符', () => {
        // 后端已经把标点留在段里了。这里多插一个空格就会在中文里露出来。
        expect(
            joinSegments([
                { text: '星夜吧是 VRChat 里的', hidden: false },
                { text: '星空主题酒吧', hidden: false }
            ])
        ).toBe('星夜吧是 VRChat 里的星空主题酒吧');
    });

    it('hidden 段照常拼进来——联系方识不打码', () => {
        // 同屏的「描述」「规则」都是原样铺开，只遮一段会让人以为那是别的内容。
        expect(
            joinSegments([
                { text: '欢迎来玩 ', hidden: false },
                { text: 'QQ 群：123', hidden: true },
                { text: ' 谢谢', hidden: false }
            ])
        ).toBe('欢迎来玩 QQ 群：123 谢谢');
    });

    it('非字符串 / 缺字段的段被跳过，不写出 "undefined"', () => {
        expect(
            joinSegments([
                { text: 'A' },
                { text: null },
                { text: undefined },
                // 后端偶尔给数字，`String()` 化会写出 "0" —— 那也是编造的内容。
                { text: 0 as unknown as string },
                {},
                null as unknown as ClubTextSeg,
                { text: 'B' }
            ])
        ).toBe('AB');
    });

    it('空段不占位：段里是空串就跳过', () => {
        expect(joinSegments([{ text: '' }, { text: 'A' }, { text: '' }])).toBe(
            'A'
        );
    });

    it('纯换行段只贡献一个换行，整体仍按 trim 收边', () => {
        // `'\n'` 是真值，所以它**会**被 push，随后由末尾的 `trim()` 收掉边缘那个。
        expect(joinSegments([{ text: '\n' }])).toBe('');
        expect(joinSegments([{ text: '\n' }, { text: 'A' }])).toBe('A');
        expect(joinSegments([{ text: 'A' }, { text: '\n' }])).toBe('A');
        // 中间的换行必须留着：公告里的空行是排版，不是噪声。
        expect(
            joinSegments([{ text: 'A' }, { text: '\n' }, { text: 'B' }])
        ).toBe('A\nB');
    });

    it('只有 hidden 段时照样拼出完整文字，不会拼成空', () => {
        // 「只遮联系方式」的社团一旦被误判成「没有公告」，背面就会说「什么都没写」。
        expect(
            joinSegments([
                { text: 'QQ：1', hidden: true },
                { text: ' / TG：2', hidden: true }
            ])
        ).toBe('QQ：1 / TG：2');
    });

    it('首尾空白收掉，中间的空白原样保留', () => {
        expect(
            joinSegments([
                { text: '  公告：' },
                { text: '今晚八点' },
                { text: '  ' }
            ])
        ).toBe('公告：今晚八点');
        expect(joinSegments([{ text: '   ' }])).toBe('');
        // 段内的行首缩进是排版意图，不能被吃掉。
        expect(joinSegments([{ text: '· 八点开场' }])).toBe('· 八点开场');
    });
});
