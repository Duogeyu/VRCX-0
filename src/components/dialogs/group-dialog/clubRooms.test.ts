// SPDX-License-Identifier: MPL-2.0
// @vitest-environment jsdom

import { describe, expect, it } from 'vitest';

import { clubRoomsToInstanceRows, launchLinkToLocation } from './clubRooms';

/** 实测 `ONLYFR.5576` 的两个房，原样抄下来。 */
const realRooms = [
    {
        cap: 40,
        link: 'https://vrchat.com/home/launch?worldId=wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5&instanceId=05026~group(grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2)~groupAccessType(public)~region(jp)',
        map: null,
        users: 37
    },
    {
        cap: 40,
        link: 'https://vrchat.com/home/launch?worldId=wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5&instanceId=96056~group(grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2)~groupAccessType(public)~region(jp)',
        map: null,
        users: 2
    }
];

describe('launchLinkToLocation', () => {
    it('把官方的 launch 链接拆成 VRCX 的 location 串', () => {
        expect(launchLinkToLocation(realRooms[0].link)).toEqual({
            worldId: 'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5',
            instanceId:
                '05026~group(grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2)~groupAccessType(public)~region(jp)',
            location:
                'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5:05026~group(grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2)~groupAccessType(public)~region(jp)'
        });
    });

    it('只认 vrchat.com，别家的链接不当房间', () => {
        // 贴着一个「看起来像启动链接」的第三方域名不该被信任。
        expect(
            launchLinkToLocation(
                'https://evil.example/home/launch?worldId=wrld_x&instanceId=1'
            )
        ).toBeNull();
        expect(
            launchLinkToLocation(
                'https://notvrchat.com/home/launch?worldId=wrld_x&instanceId=1'
            )
        ).toBeNull();
    });

    it('缺 worldId / instanceId 的不算，不拼半个 location', () => {
        expect(launchLinkToLocation('')).toBeNull();
        expect(launchLinkToLocation('not a url')).toBeNull();
        expect(
            launchLinkToLocation(
                'https://vrchat.com/home/launch?instanceId=12345'
            )
        ).toBeNull();
        expect(
            launchLinkToLocation(
                'https://vrchat.com/home/launch?worldId=wrld_x'
            )
        ).toBeNull();
        // worldId 形状不对也拒掉。
        expect(
            launchLinkToLocation(
                'https://vrchat.com/home/launch?worldId=not_a_world&instanceId=1'
            )
        ).toBeNull();
    });
});

describe('clubRoomsToInstanceRows', () => {
    it('把真实的两条房间转成可渲染的实例行', () => {
        const rows = clubRoomsToInstanceRows(realRooms, 'grp_test');

        expect(rows).toHaveLength(2);
        expect(rows[0].worldId).toBe(
            'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5'
        );
        expect(rows[0].userCount).toBe(37);
        expect(rows[0].capacity).toBe(40);
        expect(rows[1].userCount).toBe(2);
        // location 必须是 VRCX 认的形状，`parseLocation` 才吃得下。
        expect(rows[0].location).toContain('wrld_410b9d63');
        expect(rows[0].location).toContain(':05026~group(');
        expect(rows[0].groupAccessType).toBe('public');
        expect(rows[0].groupId).toBe('grp_test');
    });

    it('不编造玩家与好友——星遇不给这些', () => {
        const rows = clubRoomsToInstanceRows(realRooms, 'grp_test');

        for (const row of rows) {
            expect(row.users).toEqual([]);
            expect(row.friendCount).toBe(0);
        }
    });

    it('星遇的 map 为 null 时留空，交给 VRCX 自己的世界缓存去补', () => {
        const rows = clubRoomsToInstanceRows(realRooms, 'grp_test');
        expect(rows[0].worldName).toBeUndefined();
    });

    it('map 有值时用它', () => {
        const rows = clubRoomsToInstanceRows(
            [{ ...realRooms[0], map: '咖啡厅v1.31' }],
            'grp_test'
        );
        expect(rows[0].worldName).toBe('咖啡厅v1.31');
    });

    it('拆不出来的房间直接跳过，不产生点不开的空行', () => {
        const rows = clubRoomsToInstanceRows(
            [
                { cap: 40, link: null, map: null, users: 1 },
                { cap: 40, link: 'https://example.com/x', map: null, users: 1 },
                realRooms[0]
            ],
            'grp_test'
        );
        expect(rows).toHaveLength(1);
    });

    it('同一个 location 只出现一次', () => {
        const rows = clubRoomsToInstanceRows(
            [realRooms[0], realRooms[0]],
            'grp_test'
        );
        expect(rows).toHaveLength(1);
    });

    it('空数组 / 非数组都安全', () => {
        expect(clubRoomsToInstanceRows([], 'grp_test')).toEqual([]);
        expect(
            clubRoomsToInstanceRows(undefined as unknown as [], 'grp_test')
        ).toEqual([]);
    });
});
