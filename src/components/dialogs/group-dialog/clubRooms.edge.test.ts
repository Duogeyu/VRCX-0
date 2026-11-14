// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { clubRoomsToInstanceRows, launchLinkToLocation } from './clubRooms';

/** 实测 `ONLYFR.5576` 的一个房，链接原样照抄。 */
const REAL_LINK =
    'https://vrchat.com/home/launch?worldId=wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5&instanceId=05026~group(grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2)~groupAccessType(public)~region(jp)';

const REAL_LOCATION =
    'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5:05026~group(grp_f0b1a7b9-6237-491b-ad63-2cd88a60edf2)~groupAccessType(public)~region(jp)';

/**
 * 星遇下发的 `link` 是**外部输入**。这里判错不会有异常：最多是多一条点不开的
 * 空行，或者反过来把别人家的链接当成 VRChat 的房间。
 */
describe('launchLinkToLocation · 域名与协议边界', () => {
    it('认 vrchat.com 与它的子域', () => {
        expect(launchLinkToLocation(REAL_LINK)?.location).toBe(REAL_LOCATION);
        // 官方短链、api 等子域是同一家。
        expect(
            launchLinkToLocation(
                'https://api.vrchat.com/home/launch?worldId=wrld_x&instanceId=1'
            )?.location
        ).toBe('wrld_x:1');
        // 大小写不该影响判断：`URL` 会把 hostname 归一成小写。
        expect(
            launchLinkToLocation(
                'HTTPS://VRCHAT.COM/home/launch?worldId=wrld_x&instanceId=1'
            )?.location
        ).toBe('wrld_x:1');
    });

    it('只有「真子域」算：后缀拼接与内嵌域名都不算', () => {
        // `endsWith('.vrchat.com')` 的正确性全靠这几位——写成 `includes` 就全漏。
        expect(
            launchLinkToLocation(
                'https://vrchat.com.evil.example/home/launch?worldId=wrld_x&instanceId=1'
            )
        ).toBeNull();
        expect(
            launchLinkToLocation(
                'https://evilvrchat.com/home/launch?worldId=wrld_x&instanceId=1'
            )
        ).toBeNull();
        expect(
            launchLinkToLocation(
                'https://vrchat.co/home/launch?worldId=wrld_x&instanceId=1'
            )
        ).toBeNull();
        // 用户名伪装：`vrchat.com@evil.example` 的 hostname 是 evil.example。
        expect(
            launchLinkToLocation(
                'https://vrchat.com@evil.example/home/launch?worldId=wrld_x&instanceId=1'
            )
        ).toBeNull();
    });

    it('非 http(s) 的协议拿不到 hostname，一律拒绝', () => {
        // `javascript:` / `about:` / `data:` 都过 `new URL`，但 hostname 是空串。
        expect(
            launchLinkToLocation(
                'javascript:alert(1)?worldId=wrld_x&instanceId=1'
            )
        ).toBeNull();
        expect(
            launchLinkToLocation('about:blank#worldId=wrld_x&instanceId=1')
        ).toBeNull();
        expect(
            launchLinkToLocation(
                'data:text/html,<a>?worldId=wrld_x&instanceId=1'
            )
        ).toBeNull();
        // 相对路径解析不出域名。
        expect(
            launchLinkToLocation('/home/launch?worldId=wrld_x&instanceId=1')
        ).toBeNull();
    });

    /**
     * 协议必须是 https。只查 hostname 的话，`http://vrchat.com/...` 这种明文链接
     * 也会被当成可加入的房间——这一条原本钉的是「现状接受」，现在改成钉拒绝。
     */
    it('只认 https，明文 http 拒绝', () => {
        expect(
            launchLinkToLocation(
                'http://vrchat.com/home/launch?worldId=wrld_x&instanceId=1'
            )
        ).toBeNull();
    });

    /**
     * 路径必须是 `/home/launch`。域名对、两个参数恰好在，也不代表它是启动链接：
     * `/world/…` 这种页面被拆成 location 之后会生成一条点了打不开的行。
     */
    it('只认 /home/launch，别的路径即使参数齐全也拒绝', () => {
        expect(
            launchLinkToLocation(
                'https://vrchat.com/world/wrld_x?worldId=wrld_x&instanceId=1'
            )
        ).toBeNull();
    });

    it('参数写在 hash 里读不到——只认 search', () => {
        expect(
            launchLinkToLocation(
                'https://vrchat.com/home/launch#worldId=wrld_x&instanceId=1'
            )
        ).toBeNull();
    });

    it('worldId 的大小写与前缀是敏感的，光有前缀也不行', () => {
        expect(
            launchLinkToLocation(
                'https://vrchat.com/home/launch?worldId=WRLD_x&instanceId=1'
            )
        ).toBeNull();
        // 空的 `wrld_` 会拼出 `wrld_:1` 这种打不开的行，必须挡掉。
        expect(
            launchLinkToLocation(
                'https://vrchat.com/home/launch?worldId=wrld_&instanceId=1'
            )
        ).toBeNull();
    });

    it('instanceId 全是空白等于没给', () => {
        expect(
            launchLinkToLocation(
                'https://vrchat.com/home/launch?worldId=wrld_x&instanceId=%20%20'
            )
        ).toBeNull();
        // 两端的空白会被 trim 掉，不写进 location（写了就和 presence 的 tag 对不上）。
        expect(
            launchLinkToLocation(
                'https://vrchat.com/home/launch?worldId=%20wrld_x%20&instanceId=%201%20'
            )
        ).toEqual({ worldId: 'wrld_x', instanceId: '1', location: 'wrld_x:1' });
    });

    it('重复参数取第一个——顺序决定结果，这一点别改错', () => {
        expect(
            launchLinkToLocation(
                'https://vrchat.com/home/launch?worldId=wrld_a&worldId=wrld_b&instanceId=1'
            )?.worldId
        ).toBe('wrld_a');
    });

    it('整条链接前后的空白不影响解析', () => {
        expect(launchLinkToLocation(`  ${REAL_LINK}\n`)?.location).toBe(
            REAL_LOCATION
        );
        expect(launchLinkToLocation('   ')).toBeNull();
    });
});

describe('clubRoomsToInstanceRows · 行投影边界', () => {
    it('按 location 去重，而不是按链接字符串', () => {
        // 参数顺序不同、链接字符串不同，但拆出来是同一局。
        const reordered =
            'https://vrchat.com/home/launch?instanceId=1&worldId=wrld_x';
        const rows = clubRoomsToInstanceRows(
            [
                {
                    link: 'https://vrchat.com/home/launch?worldId=wrld_x&instanceId=1'
                },
                { link: reordered }
            ],
            'grp_test'
        );
        expect(rows).toHaveLength(1);
    });

    it('同图不同 instanceId 是两个房，不能合并', () => {
        const rows = clubRoomsToInstanceRows(
            [
                {
                    link: 'https://vrchat.com/home/launch?worldId=wrld_x&instanceId=1'
                },
                {
                    link: 'https://vrchat.com/home/launch?worldId=wrld_x&instanceId=2'
                }
            ],
            'grp_test'
        );
        expect(rows.map((row) => row.instanceId)).toEqual(['1', '2']);
    });

    it('分组 id 原样进行，`group` 只带 id', () => {
        const rows = clubRoomsToInstanceRows([{ link: REAL_LINK }], 'grp_x');
        expect(rows[0].groupId).toBe('grp_x');
        expect(rows[0].group).toEqual({ id: 'grp_x' });
        expect(rows[0].origin).toBe('vrcc');
        expect(rows[0].groupAccessType).toBe('public');
        // `ref` 是 `LocationWorld` 的投影，必须自带 location 与 worldId。
        expect(rows[0].ref).toMatchObject({
            id: rows[0].instanceId,
            location: REAL_LOCATION,
            worldId: 'wrld_410b9d63-4b7f-4fce-a580-f4d05dcdffd5'
        });
        expect(rows[0].tag).toBe(REAL_LOCATION);
    });

    it('人数 / 上限只认「像数字」的值，0 人是有意义的 0', () => {
        const rows = clubRoomsToInstanceRows(
            [
                // 上限 0 或负数：当成没给，否则界面会写出 8/0。
                {
                    cap: 0,
                    link: 'https://vrchat.com/home/launch?worldId=wrld_a&instanceId=1',
                    users: 0
                },
                {
                    cap: -5,
                    link: 'https://vrchat.com/home/launch?worldId=wrld_b&instanceId=1',
                    users: -1
                }
            ],
            'grp_test'
        );

        expect(rows[0].userCount).toBe(0);
        expect(rows[0].capacity).toBeUndefined();
        // 负人数是脏数据，宁可不写。
        expect(rows[1].userCount).toBeUndefined();
        expect(rows[1].capacity).toBeUndefined();
    });

    it('字符串形状的人数不猜', () => {
        const rows = clubRoomsToInstanceRows(
            [
                {
                    cap: '40' as unknown as number,
                    link: 'https://vrchat.com/home/launch?worldId=wrld_a&instanceId=1',
                    users: '37' as unknown as number
                }
            ],
            'grp_test'
        );
        // `typeof` 判断把字符串挡住了——这是对的：不把 "37" 猜成 37。
        expect(rows[0].userCount).toBeUndefined();
        expect(rows[0].capacity).toBeUndefined();
    });

    it('地图名 trim；纯空白当成没给', () => {
        const rows = clubRoomsToInstanceRows(
            [
                {
                    link: 'https://vrchat.com/home/launch?worldId=wrld_a&instanceId=1',
                    map: '  咖啡厅  '
                },
                {
                    link: 'https://vrchat.com/home/launch?worldId=wrld_b&instanceId=1',
                    map: '   '
                },
                {
                    link: 'https://vrchat.com/home/launch?worldId=wrld_c&instanceId=1',
                    map: 42 as unknown as string
                }
            ],
            'grp_test'
        );
        expect(rows[0].worldName).toBe('咖啡厅');
        expect(rows[1].worldName).toBeUndefined();
        expect(rows[2].worldName).toBeUndefined();
    });

    it('遍历时跳过非对象元素，不整批报废', () => {
        const rows = clubRoomsToInstanceRows(
            [
                null as unknown as Record<string, never>,
                {
                    link: 'https://vrchat.com/home/launch?worldId=wrld_a&instanceId=1'
                }
            ],
            'grp_test'
        );
        expect(rows).toHaveLength(1);
    });

    it('每行都是独立对象，改一行不会串到另一行', () => {
        const rows = clubRoomsToInstanceRows(
            [
                {
                    link: 'https://vrchat.com/home/launch?worldId=wrld_a&instanceId=1'
                },
                {
                    link: 'https://vrchat.com/home/launch?worldId=wrld_b&instanceId=1'
                }
            ],
            'grp_test'
        );
        expect(rows[0].ref).not.toBe(rows[1].ref);
        expect(rows[0].group).not.toBe(rows[1].group);
    });
});
