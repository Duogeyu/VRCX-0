// SPDX-License-Identifier: MPL-2.0

import type { GroupDialogInstanceRow } from '@/domain/entities/group';

/**
 * 星遇同频社团的公开房间（`/me/clubs/{code}` 的 `rooms[]`）。VRCX 自己拉群组
 * 实例只返回「我作为成员能看到的」，自己没加入的公开群组一个房都拿不到，
 * 星遇补的是那一块。
 */
export type VrccClubRoom = {
    map?: string | null;
    users?: number | null;
    cap?: number | null;
    link?: string | null;
};

/** 一个 `vrchat.com/home/launch?…` 链接拆出来的东西。 */
type LaunchTarget = {
    worldId: string;
    instanceId: string;
    location: string;
};

/**
 * 从启动链接里拆出 `worldId` 与 `instanceId`，拼成 VRCX 的 location 串
 * （`wrld_x:instanceId`，`parseLocation` 直接吃）。
 *
 * 拆不出来就返回 `null`——半个 location 会变成一条点不开的空行。
 */
export function launchLinkToLocation(link: string): LaunchTarget | null {
    const trimmed = link.trim();
    if (!trimmed) {
        return null;
    }

    let parsed: URL;
    try {
        parsed = new URL(trimmed);
    } catch {
        return null;
    }

    // 只认 VRChat 自己的域名：贴着别的域名的「链接」不该被当成可加入的房间。
    const host = parsed.hostname.toLowerCase();
    if (host !== 'vrchat.com' && !host.endsWith('.vrchat.com')) {
        return null;
    }

    // **协议与路径也要认**：只查域名的话，`http://vrchat.com/…`（明文）和
    // `https://vrchat.com/world/xxx?worldId=…`（参数恰好同名）都会被当成可加入
    // 的房间。启动链接的规范形状是 `https://vrchat.com/home/launch?worldId=…`。
    if (parsed.protocol !== 'https:') {
        return null;
    }
    if (parsed.pathname.replace(/\/+$/, '') !== '/home/launch') {
        return null;
    }

    const worldId = (parsed.searchParams.get('worldId') || '').trim();
    const instanceId = (parsed.searchParams.get('instanceId') || '').trim();
    // `wrld_` 后面必须有东西：只查前缀的话，空的 `wrld_` 会拼出一条
    // `wrld_:12345` 这种打不开的行。这里不要求完整 UUID——形状不对的世界 id
    // 最多让这一行打不开，而真正危险的是协议/域名/路径那三关。
    if (!/^wrld_\S+$/.test(worldId) || !instanceId) {
        return null;
    }

    return {
        worldId,
        instanceId,
        location: `${worldId}:${instanceId}`
    };
}

/**
 * 把星遇的 `rooms[]` 转成 `GroupDialogInstanceRow[]`。
 *
 * 造出来的行只带**能确定的东西**：location、人数、上限、地图名（有就用）。
 * 玩家列表、好友数、房主一概不编——星遇不给这些，编出来就是假数据。
 */
export function clubRoomsToInstanceRows(
    rooms: VrccClubRoom[],
    groupId: string
): GroupDialogInstanceRow[] {
    const rows: GroupDialogInstanceRow[] = [];
    const seen = new Set<string>();

    for (const room of Array.isArray(rooms) ? rooms : []) {
        const link = typeof room?.link === 'string' ? room.link : '';
        const target = launchLinkToLocation(link);
        if (!target) {
            continue;
        }
        // 同一局不该出现两次。星遇偶尔会给两个不同 instanceId 的同图房，
        // 那是真的两个房，所以只按 location 去重。
        if (seen.has(target.location)) {
            continue;
        }
        seen.add(target.location);

        const capacity =
            typeof room.cap === 'number' && room.cap > 0 ? room.cap : undefined;
        const userCount =
            typeof room.users === 'number' && room.users >= 0
                ? room.users
                : undefined;
        const worldName =
            typeof room.map === 'string' && room.map.trim()
                ? room.map.trim()
                : undefined;

        rows.push({
            // `id` / `instanceId` 都放 instanceId：VRCX 的实例行按它去重、
            // 也按它拼 location，两边必须是同一份。
            id: target.instanceId,
            instanceId: target.instanceId,
            worldId: target.worldId,
            location: target.location,
            tag: target.location,
            // `ref` 是给 `LocationWorld` 读的投影对象，塞最小可用形状。
            ref: {
                id: target.instanceId,
                location: target.location,
                worldId: target.worldId
            },
            users: [],
            // **必须是 0**，理由见文件头。
            friendCount: 0,
            worldName,
            capacity,
            userCount,
            // 公开群组实例：星遇只列公开房，所以这两项是确定的。
            groupAccessType: 'public',
            groupId,
            group: { id: groupId },
            // 标记来源，供上层统计「列表里有没有社团房间」。放在行上而不是
            // 另开一个集合：合并去重之后，只有行自己知道它是从哪来的。
            origin: 'vrcc'
        });
    }

    return rows;
}
