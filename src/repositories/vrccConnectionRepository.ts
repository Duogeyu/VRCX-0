// SPDX-License-Identifier: MPL-2.0

import { queryOptions, useQuery } from '@tanstack/react-query';

import { queryKeys } from '@/lib/entityQueryCache';
import { scopeGrants } from '@/lib/vrccScope';
import { commands } from '@/platform/tauri/bindings';
import type { VrccConnectionSnapshot } from '@/platform/tauri/bindings';

/**
 * 当前的星遇同频连接。**本地调用**：读的是后端内存里的会话快照，不走网络、
 * 不占额度，所以每次挂载都重新读一遍也没有成本。
 *
 * 读失败按「没连接」处理（`null`）：不知道就不给按钮，也不发带令牌的请求。
 * 权限判定是**严格**的，宁可少给，见 [`scopeGrants`](@/lib/vrccScope)。
 */
export function vrccConnectionQueryOptions() {
    return queryOptions({
        queryKey: queryKeys.vrccConnection(),
        queryFn: async (): Promise<VrccConnectionSnapshot | null> => {
            try {
                return await commands.appVrccConnection();
            } catch {
                return null;
            }
        },
        // 每次挂载都重读：缓存里那一份可能是断开、失效或换号之前的。
        staleTime: 0,
        retry: 0,
        refetchOnWindowFocus: false
    });
}

/**
 * 星遇同频的个人数据该挂在哪个账号下，以及这个账号有哪些权限。
 *
 * 个人数据（带令牌的地图页、地图状态、玩家视角的社团）的查询键都带上这里的
 * `star`：不同星遇同频账号的数据各占一个键，断开、令牌失效、换连另一个账号时
 * 不会读到上一个账号的那一份，也不需要谁记得去清缓存。
 *
 * **只认这次挂载之后读到的那一份**（`isFetchedAfterMount`）：缓存里的旧连接
 * 可能还是上一个账号的，拿它拼键会先把别人的数据显示出来。所以 `ready` 之前
 * `star` 是 `null`、`grants` 是 `undefined`，个人数据的查询要等 `ready`。
 */
export function useVrccConnection(): {
    /** 这次挂载之后已经读到了当前连接。 */
    ready: boolean;
    /** 当前星遇同频账号；没连接（或还没读到）时为 `null`。 */
    star: number | null;
    /**
     * 令牌里有没有某项权限。还没读到时为 `undefined`，读到了才是 `true` /
     * `false`——界面要区分「还不知道」和「确实没有」。
     */
    grants: (scope: string) => boolean | undefined;
} {
    const query = useQuery(vrccConnectionQueryOptions());
    const ready = query.isFetchedAfterMount;
    const connection = ready ? (query.data ?? null) : null;
    return {
        ready,
        star: connection?.connected ? (connection.star ?? null) : null,
        grants: (scope) =>
            ready ? scopeGrants(connection?.scope ?? null, scope) : undefined
    };
}
