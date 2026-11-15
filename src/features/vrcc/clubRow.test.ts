// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { fromMyClubs, fromPublicClubs } from './clubRow';

const PUBLIC_CLUB = {
    code: 'KIPFEL.2104',
    name: '中文kipfel厅',
    lede: '来我们这喵！',
    tags: ['聊天', '新人友好'],
    members: 6911,
    here: 13,
    since: 2025,
    grade: 4,
    gradeName: '北极星',
    icon: 'https://vrcc.cc/c/KIPFEL.2104.icon.jpg',
    page: 'https://vrcc.cc/c/KIPFEL.2104',
    group: 'https://vrchat.com/home/group/grp_x'
};

describe('fromPublicClubs', () => {
    it('把公开列表拉平，并保留 total / next', () => {
        const view = fromPublicClubs({
            total: 16,
            next: 20,
            clubs: [PUBLIC_CLUB]
        });

        expect(view.source).toBe('public');
        expect(view.total).toBe(16);
        expect(view.next).toBe(20);
        expect(view.rows).toHaveLength(1);
        expect(view.rows[0]).toMatchObject({
            code: 'KIPFEL.2104',
            name: '中文kipfel厅',
            members: 6911,
            here: 13,
            grade: 4,
            gradeName: '北极星'
        });
    });

    it('公开接口不提供的字段一律是 null，不是 false / 0', () => {
        const [row] = fromPublicClubs({ clubs: [PUBLIC_CLUB] }).rows;

        // 这几个只有玩家视角给。落 `false` 会被界面读成「你明确没关注」，
        // 那是在编造玩家没做过的选择。
        expect(row.likes).toBeNull();
        expect(row.followed).toBeNull();
        expect(row.member).toBeNull();
        expect(row.room).toBeNull();
        expect(row.event).toBeNull();
    });

    it('老服务器没有 total / next 时保持 null，不拿条数顶替', () => {
        const view = fromPublicClubs({ clubs: [PUBLIC_CLUB, PUBLIC_CLUB] });

        // 用 `rows.length` 冒充总数，显示出来就是「共 2 个」——而真实总数可能是 16。
        expect(view.total).toBeNull();
        expect(view.next).toBeNull();
        expect(view.rows).toHaveLength(2);
    });

    it('缺字段、空列表、null 都不炸', () => {
        expect(fromPublicClubs(null).rows).toEqual([]);
        expect(fromPublicClubs(undefined).total).toBeNull();
        expect(fromPublicClubs({}).rows).toEqual([]);

        const [row] = fromPublicClubs({
            clubs: [{ code: null, tags: ['', '聊天'] }]
        }).rows;
        expect(row.code).toBe('');
        expect(row.tags).toEqual(['聊天']);
        expect(row.members).toBeNull();
        expect(row.here).toBe(0);
    });
});

describe('fromMyClubs', () => {
    it('保留玩家视角独有的字段', () => {
        const view = fromMyClubs({
            clubs: [
                {
                    ...PUBLIC_CLUB,
                    likes: 42,
                    followed: true,
                    member: false,
                    room: true,
                    event: false
                }
            ],
            mine: ['KIPFEL.2104']
        });

        expect(view.source).toBe('mine');
        expect(view.rows[0]).toMatchObject({
            code: 'KIPFEL.2104',
            likes: 42,
            followed: true,
            member: false,
            room: true,
            event: false
        });
    });

    it('total / next 恒为 null——这个接口根本没有这两个字段', () => {
        const view = fromMyClubs({ clubs: [PUBLIC_CLUB] });

        // 这里最容易被写错：拿 `rows.length` 顶上，界面就会断言「一共 N 个」，
        // 而玩家视角看到的是全量但不等于公开社团的总数（还含只给成员看的）。
        expect(view.total).toBeNull();
        expect(view.next).toBeNull();
    });

    it('公共字段与公开列表同形：同一个适配器产出的行', () => {
        const [publicRow] = fromPublicClubs({ clubs: [PUBLIC_CLUB] }).rows;
        const [myRow] = fromMyClubs({ clubs: [PUBLIC_CLUB] }).rows;

        for (const key of [
            'code',
            'name',
            'lede',
            'tags',
            'members',
            'here',
            'since',
            'grade',
            'gradeName',
            'icon',
            'page',
            'group'
        ] as const) {
            expect(myRow[key]).toEqual(publicRow[key]);
        }
    });
});
