// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { GroupInstanceRecord } from '@/domain/entities/group';

import { mergeGroupInstances } from './groupInstances';

const LOCATION = 'wrld_x:05026~group(grp_target)~region(jp)';

const OPTIONS = {
    groupId: 'grp_target',
    currentLocation: '',
    currentUserSnapshot: null,
    friendsById: {}
};

function merge(instances: GroupInstanceRecord[], options = OPTIONS) {
    return mergeGroupInstances(instances, options);
}

/**
 * `mergeGroupInstances` 的「谁说话」由记录自己声明的 `origin` 决定。
 *
 * 这里每一种错都不会报错：expired 的来源能静默压过实时数据，或者反过来——一份
 * 认不出的来源被当成 VRChat 权威，把实时的地图名、人数、上限换成星遇的**过期
 * 快照**，而玩家列表仍然来自 VRChat。同一行里两套数据互相矛盾，界面照常渲染。
 */
describe('mergeGroupInstances · 来源权威', () => {
    it('缺省 origin 不是 VRChat：压不过 vrcc', () => {
        // 上游如果哪天忘了给 VRChat 那份打标记，它就该「没有权威」，而不是
        // 悄悄继承最高权威。
        for (const order of [
            [
                { id: 'a', location: LOCATION, userCount: 37, origin: 'vrcc' },
                { id: 'a', location: LOCATION, userCount: 39 }
            ],
            [
                { id: 'a', location: LOCATION, userCount: 39 },
                { id: 'a', location: LOCATION, userCount: 37, origin: 'vrcc' }
            ]
        ] as GroupInstanceRecord[][]) {
            const rows = merge(order);
            expect(rows).toHaveLength(1);
            expect(rows[0].userCount).toBe(37);
            expect(rows[0].origin).toBe('vrcc');
        }
    });

    it('大小写 / 空白变形的来源名认不出，落到 unknown', () => {
        for (const bogus of ['VRChat', ' VRCHAT ', 'vrchat ', 'vrchat2']) {
            const rows = merge([
                {
                    id: 'a',
                    location: LOCATION,
                    userCount: 39,
                    origin: bogus as never
                },
                { id: 'a', location: LOCATION, userCount: 37, origin: 'vrcc' }
            ]);
            // 认不出就不能让它冒充权威。
            expect(rows[0].userCount, bogus).toBe(37);
            expect(rows[0].origin, bogus).toBe('vrcc');
        }
    });

    it('排名相同（都是 unknown）时后到者赢，但 origin 仍是同一个 unknown', () => {
        const rows = merge([
            { id: 'a', location: LOCATION, userCount: 1 },
            { id: 'a', location: LOCATION, userCount: 2 }
        ]);
        expect(rows[0].userCount).toBe(2);
        // 两边都没有 `origin` 时合并结果也没有——**这一点与新建行不同**（新建
        // 行走 `parseInstanceOrigin` 落成 `'unknown'`）。只是它不会被误认成
        // VRChat，所以无害；钉住现状，别在别处依赖它是 `'unknown'`。
        expect(rows[0].origin).not.toBe('vrchat');
        expect(rows[0].origin).not.toBe('vrcc');
    });

    it('三份记录混合时，顺序不影响结论', () => {
        const vrchat: GroupInstanceRecord = {
            id: 'a',
            location: LOCATION,
            userCount: 39,
            capacity: 40,
            origin: 'vrchat'
        };
        const club: GroupInstanceRecord = {
            id: 'a',
            location: LOCATION,
            userCount: 37,
            capacity: 32,
            worldName: '星夜吧',
            origin: 'vrcc'
        };
        const mystery: GroupInstanceRecord = {
            id: 'a',
            location: LOCATION,
            userCount: 5,
            capacity: 8
        };

        const permutations = [
            [vrchat, club, mystery],
            [mystery, club, vrchat],
            [club, vrchat, mystery],
            [vrchat, mystery, club],
            [mystery, vrchat, club],
            [club, mystery, vrchat]
        ] as GroupInstanceRecord[][];

        for (const order of permutations) {
            const rows = merge(order);
            expect(rows).toHaveLength(1);
            expect(rows[0].userCount).toBe(39);
            expect(rows[0].capacity).toBe(40);
            expect(rows[0].origin).toBe('vrchat');
            // 星遇独有的字段仍然补进来了。
            expect(rows[0].worldName).toBe('星夜吧');
        }
    });

    it('赢家给的「没给」不覆盖输家的真值，只有真值才覆盖', () => {
        const rows = merge([
            {
                id: 'a',
                location: LOCATION,
                capacity: 32,
                worldName: '星夜吧',
                groupName: '社团名',
                origin: 'vrcc'
            },
            {
                id: 'a',
                location: LOCATION,
                // VRChat 这份三项都是「没给」的形状：`undefined` / `null` / `''`。
                capacity: null as unknown as number,
                worldName: '',
                groupName: '',
                origin: 'vrchat'
            }
        ]);

        // 三种「没给」都被过滤掉，于是补位数据留住了。
        expect(rows[0].worldName).toBe('星夜吧');
        expect(rows[0].capacity).toBe(32);
        expect(rows[0].groupName).toBe('社团名');
        expect(rows[0].origin).toBe('vrchat');
    });

    it('纯空白不算「给了值」，不覆盖输家的真值', () => {
        // 一个 `'   '` 会把星遇的地图名盖成看不见的东西，所以空白与 `''` 同等对待；
        // `false` / `0` 仍然算真值（见下一条）。
        const rows = merge([
            {
                id: 'a',
                location: LOCATION,
                worldName: '星夜吧',
                origin: 'vrcc'
            },
            { id: 'a', location: LOCATION, worldName: '   ', origin: 'vrchat' }
        ]);
        expect(rows[0].worldName).toBe('星夜吧');
    });

    it('赢家的显式 false 也压得住输家的 true', () => {
        // 过滤只挡 `undefined` / `null` / `''`——`false` 是「明确说了没有」。
        const rows = merge([
            {
                id: 'a',
                location: LOCATION,
                queueEnabled: true,
                full: true,
                origin: 'vrcc'
            },
            {
                id: 'a',
                location: LOCATION,
                queueEnabled: false,
                full: false,
                origin: 'vrchat'
            }
        ]);
        expect(rows[0].queueEnabled).toBe(false);
        expect(rows[0].full).toBe(false);
    });

    it('两边都不给 worldId / instanceId 时从 location 拆出来', () => {
        const rows = merge([{ location: LOCATION }, { location: LOCATION }]);
        expect(rows[0].worldId).toBe('wrld_x');
        expect(rows[0].instanceId).toBe('05026~group(grp_target)~region(jp)');
        expect(rows[0].tag).toBe(LOCATION);
        expect(rows[0].location).toBe(LOCATION);
    });

    it('`id` 比 location 里的 instanceId 优先（现状，钉住）', () => {
        // `instanceId` 的取值顺序是 `instanceId || id || 拆 location`。上游若给了
        // 一个不是实例 id 的 `id`（比如数据库行号），拼出来的 location 就不是那
        // 一局了——这一条只是把现成的顺序写下来，改顺序会让它红。
        const rows = merge([{ id: 'row_42', location: LOCATION }]);
        expect(rows[0].instanceId).toBe('row_42');
        expect(rows[0].worldId).toBe('wrld_x');
    });
});

describe('mergeGroupInstances · 显式 friendCount', () => {
    it('明确给了 0 就保留 0，不被 userCount 顶掉', () => {
        // 星遇造的行就是这样：知道这一局有 37 人，但一个好友都不知道。写成
        // `friendCount || userCount` 会把 37 人当成 37 个好友。
        const rows = merge([
            { id: 'a', location: LOCATION, userCount: 37, friendCount: 0 }
        ]);
        expect(rows[0].friendCount).toBe(0);
        expect(rows[0].userCount).toBe(37);
    });

    it('没给 friendCount 的 VRChat 行仍回落到 userCount（现状）', () => {
        const rows = merge([{ id: 'a', location: LOCATION, userCount: 12 }]);
        expect(rows[0].friendCount).toBe(12);
    });

    it('合并时两边都取大，不因先后而变小', () => {
        for (const order of [
            [
                {
                    id: 'a',
                    location: LOCATION,
                    friendCount: 3,
                    origin: 'vrchat'
                },
                { id: 'a', location: LOCATION, friendCount: 7, origin: 'vrcc' }
            ],
            [
                { id: 'a', location: LOCATION, friendCount: 7, origin: 'vrcc' },
                {
                    id: 'a',
                    location: LOCATION,
                    friendCount: 3,
                    origin: 'vrchat'
                }
            ]
        ] as GroupInstanceRecord[][]) {
            expect(merge(order)[0].friendCount).toBe(7);
        }
    });
});

describe('mergeGroupInstances · 用户合并', () => {
    it('两边的 users 取并集，同一 id 只留一份', () => {
        const rows = merge([
            {
                id: 'a',
                location: LOCATION,
                users: [
                    { id: 'u1', displayName: 'Zed' },
                    { userId: 'u2', displayName: 'Ann' }
                ]
            },
            {
                id: 'a',
                location: LOCATION,
                users: [
                    { id: 'u1', displayName: 'Zed' },
                    // 没有 id 的条目无法去重，但不该丢。
                    { displayName: 'Anon' }
                ]
            }
        ]);

        expect(rows[0].users).toHaveLength(3);
        // 排序按 displayName 的 locale 序：'Ann' 在 'Anon' 之前。
        expect(rows[0].users.map((user) => user.displayName)).toEqual([
            'Ann',
            'Anon',
            'Zed'
        ]);
    });

    it('当前所在的那一局永远排最前，哪怕人更少', () => {
        const current = 'wrld_mine:1~group(grp_target)';
        const rows = merge(
            [
                {
                    id: 'a',
                    location: 'wrld_big:1~group(grp_target)',
                    users: [{ id: 'u1' }, { id: 'u2' }, { id: 'u3' }]
                },
                { id: 'b', location: current, users: [] }
            ],
            { ...OPTIONS, currentLocation: current }
        );
        expect(rows.map((row) => row.location)).toEqual([
            current,
            'wrld_big:1~group(grp_target)'
        ]);
    });
});
