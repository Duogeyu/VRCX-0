// SPDX-License-Identifier: MPL-2.0

import { useQuery } from '@tanstack/react-query';

import type { GroupInstanceRecord } from '@/domain/entities/group';
import { queryKeys } from '@/lib/entityQueryCache';
import {
    clubCodeForGroup,
    vrccMyClubQueryOptions,
    vrccPublicClubQueryOptions
} from '@/repositories/vrccClubRepository';
import { fetchVrccScopeGrants } from '@/repositories/vrccMapRepository';

import { clubRoomsToInstanceRows, type VrccClubRoom } from './clubRooms';

/**
 * 星遇同频社团的公开房间，投影成群组实例记录，和 VRCX 自己那份一起交给
 * `mergeGroupInstances`。
 *
 * **必须在 `useGroupDialogActiveInstances` 之前调用**：好友是本地 presence
 * 来的，由 `mergeGroupInstances` 塞进**已经存在**的行；晚到的房间永远分不到
 * 好友。
 *
 * 也**不要**把合并挪回渲染层那个组件里：那样社团房间会在好友分配之后才出现，
 * 那几行永远是空的，好友头像和好友数全丢（这个坑踩过一次）。
 *
 * 拿不到（没连接 / 没 `clubs` 权限 / `404 club_off` / 失败 / 缺短码）就返回
 * 空数组，补位失败不该让群组详情报错。公开的 `/clubs/{code}` 不返回 `rooms`，
 * 但不需要令牌也不占额度，先用它挡一道——大多数群组没入驻社团。
 *
 * **`clubs` 权限要单独查一次**：社团卡那边挡了一道，这里不挡的话，没连星遇的
 * 用户每开一次群组详情都会发一条注定失败的 IPC（Rust 侧第一件事是
 * `ensure_connected()`）。
 */
export function useGroupClubInstanceRecords(
    group: {
        id?: string;
        shortCode?: string | null;
        discriminator?: string | null;
    } | null
): GroupInstanceRecord[] {
    const code = clubCodeForGroup(group ?? {});

    const clubQuery = useQuery({
        ...vrccPublicClubQueryOptions(code),
        enabled: Boolean(code)
    });

    const scopeQuery = useQuery({
        queryKey: queryKeys.vrccScope('clubs'),
        queryFn: () => fetchVrccScopeGrants('clubs'),
        staleTime: 60 * 1000,
        retry: 0,
        refetchOnWindowFocus: false
    });

    // 两个条件都要：没有 `clubs` 权限时那条 IPC 注定失败；公开页已答 404 时
    // 这个群组根本没入驻社团，问带令牌的只是白花额度。
    const detailQuery = useQuery(
        vrccMyClubQueryOptions(
            code,
            scopeQuery.data === true && Boolean(clubQuery.data?.club)
        )
    );

    if (!code) {
        return EMPTY;
    }
    const rooms = Array.isArray(detailQuery.data?.club?.rooms)
        ? (detailQuery.data?.club?.rooms as VrccClubRoom[])
        : [];
    if (!rooms.length) {
        return EMPTY;
    }

    // 直接返回：`clubRoomsToInstanceRows` 的产物就是 `GroupInstanceRecord` 的超集，
    // 那里已经写死 `friendCount: 0` 与 `origin: 'vrcc'`。以前这里再 `.map()` 一遍，
    // 唯一效果是丢掉 `users`——而合并时对 `undefined` 走 `[]`，结果一样，只多了
    // 一处「14 个字段名要和 clubRooms 保持同步」的维护面。
    return clubRoomsToInstanceRows(rooms, String(group?.id ?? ''));
}

/**
 * 同一个空数组常量。
 *
 * 每次返回新数组会让调用方的 `useMemo` / effect 依赖每帧都变，把「没有社团」
 * 变成永不停歇的重算。空状态是常态（绝大多数群组没入驻），所以这一条值得
 * 一个稳定引用。
 */
const EMPTY: GroupInstanceRecord[] = [];
