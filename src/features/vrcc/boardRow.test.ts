// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import {
    fromMyBoard,
    fromMyBoardRow,
    fromPublicBoard,
    fromPublicBoardRow,
    hotTextFor
} from './boardRow';

describe('fromPublicBoardRow', () => {
    it('把公开榜的字段搬过来，玩家视角独有的字段一律 null', () => {
        const row = fromPublicBoardRow({
            world: 'wrld_a',
            name: '孤',
            author: 'Miro-cn',
            avg: 9.6,
            raters: 60,
            rank: 1,
            weight: 'mid',
            page: 'https://vrcc.cc/maps/wrld_a',
            img: 'https://vrcc.cc/maps/wrld_a.jpg',
            hot: { text: '愿夜色温柔', by: '某位派对点灯人', up: 12 }
        });

        expect(row.world).toBe('wrld_a');
        expect(row.rank).toBe(1);
        expect(row.hotTexts).toEqual(['愿夜色温柔']);
        // 公开榜的 hot 有出处，这是玩家视角拿不到的那一项。
        expect(row.hotBy).toBe('某位派对点灯人');
        expect(row.page).toBe('https://vrcc.cc/maps/wrld_a');

        // 下面这些只有 /me/board 给，必须是 null 而不是 0 / false。
        expect(row.went).toBeNull();
        expect(row.room).toBeNull();
        expect(row.cap).toBeNull();
        expect(row.teen).toBeNull();
        expect(row.adj).toBeNull();
        expect(row.mine).toBeNull();
    });

    it('没有 hot 时短评候选是空数组，不会读出 undefined', () => {
        const row = fromPublicBoardRow({ world: 'wrld_a' });

        expect(row.hotTexts).toEqual([]);
        expect(row.hotBy).toBeNull();
        expect(row.name).toBeNull();
        expect(row.avg).toBeNull();
        // 介绍首行只有玩家视角给。
        expect(row.intro).toBeNull();
    });

    it('本周榜带 week，且 rank 照旧是总榜名次', () => {
        const row = fromPublicBoardRow({
            world: 'wrld_a',
            week: 2,
            rank: 7
        });

        expect(row.week).toBe(2);
        expect(row.rank).toBe(7);
    });
});

describe('fromMyBoardRow', () => {
    it('搬玩家视角的字段，page 与 hotBy 为 null（接口不给）', () => {
        const row = fromMyBoardRow({
            world: 'wrld_a',
            id: 'wrld_a',
            name: '孤',
            avg: 9.6,
            adj: 9.47,
            raters: 60,
            went: 158,
            rank: 1,
            cap: 64,
            room: false,
            teen: true,
            weight: 'mid',
            img: '/api/match?xdimg=wrld_a',
            imgUrl: 'https://vrcc.cc/api/match?xdimg=wrld_a',
            hot: { id: 181, text: '愿夜色温柔', up: 12, vote: 0, went: true }
        });

        expect(row.went).toBe(158);
        expect(row.room).toBe(false);
        expect(row.teen).toBe(true);
        expect(row.cap).toBe(64);
        expect(row.adj).toBe(9.47);
        expect(row.rank).toBe(1);
        expect(row.hotTexts).toEqual(['愿夜色温柔']);

        // 玩家视角的 hot 里没有出处；这不是解析失败，是接口不给。
        expect(row.hotBy).toBeNull();
        // 也不给 page，面板用 world 自己拼。
        expect(row.page).toBeNull();
        // top 排序下没有「我的评分」。
        expect(row.mine).toBeNull();
    });

    it('room 为 false 时是 false，不能当成「不知道」', () => {
        const row = fromMyBoardRow({ world: 'wrld_a', room: false });

        // 这正是 null 与 false 要分开的原因：公开榜的 room 是 null（不知道），
        // 玩家视角的 false 是「确定没人在开房」。
        expect(row.room).toBe(false);
        expect(fromPublicBoardRow({ world: 'wrld_a' }).room).toBeNull();
    });

    it('老服务器只给 id 时用 id 当 world', () => {
        const row = fromMyBoardRow({ id: 'wrld_legacy' });

        expect(row.world).toBe('wrld_legacy');
    });

    it('本周榜没有 rank 时保持 null，交给面板用列表位置', () => {
        const row = fromMyBoardRow({ world: 'wrld_a', week: 2 });

        expect(row.week).toBe(2);
        expect(row.rank).toBeNull();
    });

    it('went 排序带我的评分与去过次数', () => {
        const row = fromMyBoardRow({
            world: 'wrld_a',
            mine: 9,
            went: 16,
            wentAt: 1794335213188
        });

        expect(row.mine).toBe(9);
        expect(row.went).toBe(16);
    });

    it('多条短评全部收进候选，不是只留第一条', () => {
        // 面板会在候选里随机挑一条展示，所以三条都要留着。
        const row = fromMyBoardRow({
            world: 'wrld_a',
            hots: [
                { id: 181, text: '愿夜色温柔', up: 12 },
                { id: 187, text: '好玩', up: 0 },
                { id: 173, text: '非常有想法的一张图', up: 0 }
            ]
        });

        expect(row.hotTexts).toEqual([
            '愿夜色温柔',
            '好玩',
            '非常有想法的一张图'
        ]);
    });

    it('hots 为空时退回单条 hot，老服务器只给这一条', () => {
        const row = fromMyBoardRow({
            world: 'wrld_a',
            hots: [],
            hot: { id: 1, text: '只有这一句', up: 3 }
        });

        expect(row.hotTexts).toEqual(['只有这一句']);
    });

    it('介绍首行只在玩家视角有', () => {
        const row = fromMyBoardRow({
            world: 'wrld_a',
            intro: '赞最多的那段介绍的第一行'
        });

        expect(row.intro).toBe('赞最多的那段介绍的第一行');
    });
});

describe('hotTextFor', () => {
    const three = fromMyBoardRow({
        world: 'wrld_a',
        hots: [
            { id: 1, text: '愿夜色温柔' },
            { id: 2, text: '好玩' },
            { id: 3, text: '非常有想法的一张图' }
        ]
    });

    it('没有短评时返回 null', () => {
        expect(hotTextFor(fromMyBoardRow({ world: 'wrld_a' }))).toBeNull();
    });

    it('只有一条时直接用那一条', () => {
        const one = fromMyBoardRow({
            world: 'wrld_a',
            hots: [{ id: 1, text: '只有这一句' }]
        });
        expect(hotTextFor(one)).toBe('只有这一句');
    });

    it('多条时按世界 ID 挑，同一张图永远同一条', () => {
        // 稳定是关键：滚动、悬停、切语言都会重渲染，每次换一句的话
        // 玩家会看到文字在自己眼前跳。
        const first = hotTextFor(three);
        expect(three.hotTexts).toContain(first);
        expect(hotTextFor(three)).toBe(first);
        expect(hotTextFor(three)).toBe(first);
    });

    it('不同的世界会落到不同的候选上', () => {
        const other = { ...three, world: 'wrld_b' };
        // 这两个 ID 在 3 个候选下分别落到下标 0 和 1：不追求「真随机」，
        // 只要各张图看起来不一样就行。
        expect(hotTextFor(three)).toBe('愿夜色温柔');
        expect(hotTextFor(other)).toBe('好玩');
    });
});

describe('fromPublicBoard / fromMyBoard', () => {
    it('公开榜保留 min 与 source', () => {
        const view = fromPublicBoard({
            min: 1,
            maps: [{ world: 'wrld_a' }, { world: 'wrld_b' }]
        });

        expect(view.source).toBe('public');
        expect(view.min).toBe(1);
        expect(view.maps.map((row) => row.world)).toEqual(['wrld_a', 'wrld_b']);
    });

    it('玩家视角丢掉既没 world 也没 id 的行', () => {
        const view = fromMyBoard({
            maps: [{ world: 'wrld_a' }, { name: '没有 id 的行' }]
        });

        // 这样的行当 React 的 key 是空串，开详情窗也没法定位，直接丢。
        expect(view.maps).toHaveLength(1);
        expect(view.maps[0].world).toBe('wrld_a');
        expect(view.source).toBe('mine');
    });

    it('maps 缺失时给空数组，不给 undefined', () => {
        expect(fromPublicBoard({}).maps).toEqual([]);
        expect(fromMyBoard({}).maps).toEqual([]);
    });

    it('搜索答复（sort 被服务器改写为 find）照常归一化', () => {
        const view = fromMyBoard({
            sort: 'find',
            q: '孤',
            maps: [{ world: 'wrld_a', name: '孤' }]
        });

        expect(view.maps).toHaveLength(1);
        expect(view.maps[0].name).toBe('孤');
    });
});
