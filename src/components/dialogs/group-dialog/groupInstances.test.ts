import { describe, expect, it } from 'vitest';

import type { GroupInstanceRecord } from '@/domain/entities/group';
import { onlinePresence, travelingPresence } from '@/test/presenceFixtures';

import {
    instanceLocation,
    mergeGroupInstances,
    normalizeLocation,
    userGroupLocation
} from './groupInstances';

describe('groupInstances', () => {
    it('normalizes ids and locations for group instance rows', () => {
        expect(normalizeLocation(' offline ')).toBe('');
        expect(normalizeLocation('private')).toBe('');
        expect(normalizeLocation('wrld_1:2')).toBe('wrld_1:2');
        expect(
            userGroupLocation({
                $presence: travelingPresence('wrld_2:1~group(grp_target)')
            })
        ).toBe('wrld_2:1~group(grp_target)');
        expect(
            instanceLocation({
                worldId: 'wrld_fallback',
                instanceId: 'fallback~group(grp_target)'
            })
        ).toBe('wrld_fallback:fallback~group(grp_target)');
    });

    it('enriches projected instances without creating presence-only rows', () => {
        const currentLocation = 'wrld_current:current~group(grp_target)';
        const rows = mergeGroupInstances(
            [
                {
                    id: 'base',
                    location: 'wrld_base:base~group(grp_target)',
                    userCount: 3,
                    users: [{ id: 'usr_existing', displayName: 'Existing' }]
                }
            ],
            {
                groupId: 'grp_target',
                currentLocation,
                currentUserSnapshot: {
                    id: 'usr_self',
                    displayName: 'Self',
                    $presence: onlinePresence(currentLocation)
                },
                friendsById: {
                    usr_existing: {
                        id: 'usr_existing',
                        displayName: 'Existing Duplicate',
                        $presence: onlinePresence(
                            'wrld_base:base~group(grp_target)'
                        )
                    },
                    usr_friend: {
                        id: 'usr_friend',
                        displayName: 'Friend',
                        $presence: onlinePresence(
                            'wrld_base:base~group(grp_target)'
                        )
                    },
                    usr_traveling: {
                        id: 'usr_traveling',
                        displayName: 'Traveling',
                        $presence: travelingPresence(currentLocation)
                    },
                    usr_wrong_group: {
                        id: 'usr_wrong_group',
                        displayName: 'Wrong Group',
                        $presence: onlinePresence(
                            'wrld_wrong:1~group(grp_other)'
                        )
                    }
                }
            }
        );

        expect(rows.map((row) => row.location)).toEqual([
            'wrld_base:base~group(grp_target)'
        ]);
        expect(rows[0].users.map((user) => user.id)).toEqual([
            'usr_existing',
            'usr_friend'
        ]);
        expect(rows[0].friendCount).toBe(3);
        expect(
            rows.some((row) =>
                row.users.some((user) =>
                    ['usr_self', 'usr_traveling', 'usr_wrong_group'].includes(
                        String(user.id || '')
                    )
                )
            )
        ).toBe(false);
    });

    it('sorts non-current rows by user count and keeps users display-name sorted', () => {
        const rows = mergeGroupInstances(
            [
                {
                    location: 'wrld_small:1~group(grp_target)',
                    users: [{ id: 'usr_z', displayName: 'Zed' }]
                },
                {
                    location: 'wrld_big:1~group(grp_target)',
                    users: [
                        { id: 'usr_b', displayName: 'Beta' },
                        { id: 'usr_a', displayName: 'Alpha' }
                    ]
                }
            ],
            {
                groupId: 'grp_target',
                currentLocation: '',
                currentUserSnapshot: null,
                friendsById: {}
            }
        );

        expect(rows.map((row) => row.location)).toEqual([
            'wrld_big:1~group(grp_target)',
            'wrld_small:1~group(grp_target)'
        ]);
        expect(rows[0].users.map((user) => user.displayName)).toEqual([
            'Alpha',
            'Beta'
        ]);
    });

    it('keeps existing users and counts when duplicate base instances merge', () => {
        const location = 'wrld_dup:1~group(grp_target)';
        const explicitRef = { source: 'repository' };
        const rows = mergeGroupInstances(
            [
                {
                    id: 'first',
                    location,
                    userCount: 4,
                    users: [{ id: 'usr_first', displayName: 'First' }]
                },
                {
                    id: 'second',
                    tag: location,
                    worldId: 'wrld_dup_override',
                    ref: explicitRef,
                    users: [{ id: 'usr_second', displayName: 'Second' }]
                }
            ],
            {
                groupId: 'grp_target',
                currentLocation: '',
                currentUserSnapshot: null,
                friendsById: {
                    bad: null
                }
            }
        );

        expect(rows).toHaveLength(1);
        expect(rows[0]).toMatchObject({
            id: 'second',
            location,
            tag: location,
            worldId: 'wrld_dup_override',
            friendCount: 4,
            ref: explicitRef
        });
        // 两份 `users` **取并集**。
        //
        // 改动前这里只留 `usr_first`（`users: existing.users`），第二份的人
        // 被静默丢掉。同源重复本不该出现，真出现了也不该靠「谁先到」来决定
        // 谁能留下——并集只会变多不会变少。
        expect(rows[0].users.map((user) => user.id).sort()).toEqual([
            'usr_first',
            'usr_second'
        ]);
    });

    /**
     * 星遇同频那份房间（`clubRoomsToInstanceRows` 造出来的行）也能吃到好友。
     *
     * 这条钉住的是**两个数据源能不能接上**：VRCX 自己拉群组实例走
     * `users/{me}/instances/groups/{group}`，自己没加入的公开群组一个房都
     * 拿不到，那条路是空的；星遇补进来的房只是「这里有一局」，**不含任何
     * 玩家**。
     *
     * 而好友是本地 presence 来的：`mergeGroupInstances` 先按群组实例建行，
     * 再遍历好友名单、按 location 把人塞进**已经存在**的行。所以只要星遇
     * 那行的 location 拼得和 presence 的 tag 一样，好友头像和好友数就会自动
     * 对上——这一条就是验证那个「一样」。
     */
    it('matches friends onto club-provided rooms by location', () => {
        // 实测 `ONLYFR.5576` 的两个房，location 形状原样照抄。
        const clubRoomLocation =
            'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5:05026~group(grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2)~groupAccessType(public)~region(jp)';
        const otherClubRoomLocation =
            'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5:96056~group(grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2)~groupAccessType(public)~region(jp)';

        const rows = mergeGroupInstances(
            [
                {
                    id: '05026~group(grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2)~groupAccessType(public)~region(jp)',
                    location: clubRoomLocation,
                    worldId: 'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5',
                    userCount: 37,
                    // 星遇不给玩家列表，所以这里必须是空的。
                    users: []
                },
                {
                    id: '96056~group(grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2)~groupAccessType(public)~region(jp)',
                    location: otherClubRoomLocation,
                    worldId: 'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5',
                    userCount: 2,
                    users: []
                }
            ],
            {
                groupId: 'grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2',
                currentLocation: '',
                currentUserSnapshot: null,
                friendsById: {
                    usr_in_club_room: {
                        id: 'usr_in_club_room',
                        displayName: '在星夜吧的朋友',
                        $presence: onlinePresence(clubRoomLocation)
                    },
                    usr_elsewhere: {
                        id: 'usr_elsewhere',
                        displayName: '别的图',
                        $presence: onlinePresence(
                            'wrld_other:1~group(grp_other)'
                        )
                    }
                }
            }
        );

        // 两条房都还在（好友 presence **不会**凭空造行，也不会吞掉行）。
        expect(rows).toHaveLength(2);

        const clubRow = rows.find((row) => row.location === clubRoomLocation);
        const otherRow = rows.find(
            (row) => row.location === otherClubRoomLocation
        );
        expect(clubRow).toBeTruthy();
        expect(otherRow).toBeTruthy();

        // 好友落到了星遇那一行。
        expect(clubRow?.users.map((user) => user.id)).toEqual([
            'usr_in_club_room'
        ]);
        expect(clubRow?.friendCount).toBeGreaterThan(0);

        // 另一局**没有好友，但也不是 0**。
        //
        // `mergeGroupInstances` 的兜底是 `friendCount || users.length`，而
        // 星遇造的行把 `userCount`（37 / 2 那种**总人数**）写进了
        // `friendCount` 的种子（`Number(seed.friendCount || seed.userCount)`）。
        // 所以这一局的好友数是 2——是一个**错的**数字，只是恰好没有好友
        // 时会漏出来。这条断言把现状钉住，不假装它是对的。
        expect(otherRow?.users).toEqual([]);
        expect(otherRow?.friendCount).toBe(2);
    });

    /**
     * 同一个 location 两边都有时，**VRChat 那一份是权威**。
     *
     * 这条钉住的是调用方的**传参顺序**：`mergeGroupInstances` 用
     * `Object.assign(existing, embeddedInstance, seed, …)` 合并，后写的 `seed`
     * 覆盖先写的值。所以 `useGroupDialogState` 必须把星遇的房间排在前面、
     * VRChat 的排在后面——冲突时 VRChat 压得住星遇的快照。
     *
     * 顺序反过来的话人数、容量、世界名会全部变成星遇那个**非实时**的值，
     * 而玩家列表和好友数仍然是 VRChat 的，同一行里两套数据互相矛盾。
     */
    it('lets the authoritative source win when both sides describe one instance', () => {
        const location =
            'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5:05026~group(grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2)~groupAccessType(public)~region(jp)';

        // 星遇那一份：快照人数 37、上限 32、没有地图名。
        const fromClub = {
            id: '05026~group(grp_f0b1a7b9)~region(jp)',
            location,
            worldId: 'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5',
            userCount: 37,
            capacity: 32,
            users: [],
            friendCount: 0
        };
        // VRChat 那一份：实时 39、上限 40、带地图名。
        const fromVrchat = {
            id: '05026~group(grp_f0b1a7b9)~region(jp)',
            location,
            worldId: 'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5',
            userCount: 39,
            capacity: 40,
            worldName: '星夜吧',
            users: [],
            friendCount: 0
        };

        // 调用方约定的顺序：星遇在前。
        const rows = mergeGroupInstances([fromClub, fromVrchat], {
            groupId: 'grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2',
            currentLocation: '',
            currentUserSnapshot: null,
            friendsById: {}
        });

        expect(rows).toHaveLength(1);
        // 人数、上限、世界名三样都是 VRChat 的，不是星遇那个快照。
        expect(rows[0].userCount).toBe(39);
        expect(rows[0].capacity).toBe(40);
        expect(rows[0].worldName).toBe('星夜吧');
    });

    /**
     * 同一个 location 两边都有时，**VRChat 那一份是权威**，而且**与顺序无关**。
     *
     * 这条钉住的是权威规则本身，不是「谁排在后面」。权威来自数据自己声明的
     * `origin`，所以下面的两个顺序必须给出**同一个结果**——这正是这次
     * 改造的目的：不再靠调用方的数组顺序隐式决定谁赢。
     */
    it('lets the authoritative source win no matter which side comes first', () => {
        const location =
            'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5:05026~group(grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2)~groupAccessType(public)~region(jp)';

        // 星遇那一份：快照人数 37、上限 32、没有地图名。
        const fromClub: GroupInstanceRecord = {
            id: '05026~group(grp_f0b1a7b9)~region(jp)',
            location,
            worldId: 'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5',
            userCount: 37,
            capacity: 32,
            users: [],
            friendCount: 0,
            origin: 'vrcc'
        };
        // VRChat 那一份：实时 39、上限 40、带地图名。
        const fromVrchat: GroupInstanceRecord = {
            id: '05026~group(grp_f0b1a7b9)~region(jp)',
            location,
            worldId: 'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5',
            userCount: 39,
            capacity: 40,
            worldName: '星夜吧',
            users: [],
            friendCount: 0,
            origin: 'vrchat'
        };

        const options = {
            groupId: 'grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2',
            currentLocation: '',
            currentUserSnapshot: null,
            friendsById: {}
        };

        for (const [label, order] of [
            ['星遇在前', [fromClub, fromVrchat]],
            ['星遇在后', [fromVrchat, fromClub]]
        ] as const) {
            const rows = mergeGroupInstances([...order], options);
            expect(rows, label).toHaveLength(1);
            // 人数、上限、世界名三样全是 VRChat 的。
            expect(rows[0].userCount, label).toBe(39);
            expect(rows[0].capacity, label).toBe(40);
            expect(rows[0].worldName, label).toBe('星夜吧');
            // 这一局归 VRChat，不是「只有星遇知道」。
            expect(rows[0].origin, label).toBe('vrchat');
        }
    });

    /**
     * VRChat 没给值的字段，星遇可以把它补上。
     *
     * 权威只体现在**冲突**上：VRChat 说了就听它的，没说的地方社团那份仍然是
     * 有用信息，不该一起丢掉。
     */
    it('still lets the club fill fields VRChat left empty', () => {
        const location =
            'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5:05026~group(grp_f0b1a7b9)~region(jp)';

        const rows = mergeGroupInstances(
            [
                {
                    id: 'a',
                    location,
                    userCount: 39,
                    capacity: 40,
                    // VRChat 这份没给地图名。
                    users: [],
                    origin: 'vrchat'
                },
                {
                    id: 'a',
                    location,
                    userCount: 37,
                    worldName: '星夜吧',
                    users: [],
                    friendCount: 0,
                    origin: 'vrcc'
                }
            ],
            {
                groupId: 'grp_f0b1a7b9',
                currentLocation: '',
                currentUserSnapshot: null,
                friendsById: {}
            }
        );

        // 人数听 VRChat 的……
        expect(rows[0].userCount).toBe(39);
        // ……但地图名 VRChat 没给，社团那份补进来。
        expect(rows[0].worldName).toBe('星夜吧');
        // 这一局最后是 VRChat 在说话。
        expect(rows[0].origin).toBe('vrchat');
    });

    /**
     * 如果 VRChat 压根没有这一局，那它就是**只有星遇知道**的房间。
     *
     * `origin` 保持 `'vrcc'`，界面据此在那一条卡片里挂来源说明；
     * 一旦 VRChat 也报了同一局（上一条），它就变成 `'vrchat'`，说明消失。
     */
    it('keeps the club origin when only the club knows about the room', () => {
        const rows = mergeGroupInstances(
            [
                {
                    id: 'a',
                    location: 'wrld_x:05026~group(grp_f0b1a7b9)~region(jp)',
                    userCount: 37,
                    capacity: 40,
                    users: [],
                    friendCount: 0,
                    origin: 'vrcc'
                }
            ],
            {
                groupId: 'grp_f0b1a7b9',
                currentLocation: '',
                currentUserSnapshot: null,
                friendsById: {}
            }
        );

        expect(rows[0].origin).toBe('vrcc');
    });

    /**
     * 认不出的来源落进 `'unknown'`，排名最低，压不过 VRChat。
     *
     * 这条钉住「宽容」的边界：上游将来传了个没见过的来源名，不该崩、也不该
     * 让它冒充权威数据。
     */
    it('parks an unrecognised origin at unknown and lets vrchat win', () => {
        const location = 'wrld_x:05026~group(grp_f0b1a7b9)~region(jp)';
        const rows = mergeGroupInstances(
            [
                {
                    id: 'a',
                    location,
                    userCount: 39,
                    capacity: 40,
                    users: [],
                    origin: 'vrchat'
                },
                {
                    id: 'a',
                    location,
                    userCount: 37,
                    users: [],
                    origin: 'some-new-provider' as never
                }
            ],
            {
                groupId: 'grp_f0b1a7b9',
                currentLocation: '',
                currentUserSnapshot: null,
                friendsById: {}
            }
        );

        expect(rows[0].userCount).toBe(39);
        expect(rows[0].origin).toBe('vrchat');
    });
});
