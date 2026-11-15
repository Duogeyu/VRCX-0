// SPDX-License-Identifier: MPL-2.0

import { queryOptions, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';

import { queryKeys } from '@/lib/entityQueryCache';
import { queryClient } from '@/lib/queryClient';
import { vrccErrorKind } from '@/lib/vrccError';
import { scopeGrants } from '@/lib/vrccScope';
import { commands } from '@/platform/tauri/bindings';
import type { VrccConnectionSnapshot } from '@/platform/tauri/bindings';
import { tauriClient } from '@/platform/tauri/client';

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
 * 后端在令牌被判失效等时机发出的「连接变了」事件，与
 * `VRCC_CONNECTION_CHANGED_EVENT`（Rust 侧）必须一致。
 */
const VRCC_CONNECTION_CHANGED_EVENT = 'vrccConnectionChanged';

/**
 * 个人数据查询键里的账号标识。
 *
 * - 已连接、有星号：`star:<编号>`；
 * - 已连接、没有星号：`star:none`——官方文档写明没有点亮编号的玩家 `star` 是
 *   `null`，这时不能和「未连接」共用一个键；
 * - 未连接（或连接读不到）：`offline`。
 *
 * 公开数据不用它。
 */
export function vrccAccountKey(
    connection:
        | Pick<VrccConnectionSnapshot, 'connected' | 'star'>
        | null
        | undefined
): string {
    if (!connection?.connected) {
        return 'offline';
    }
    return `star:${connection.star ?? 'none'}`;
}

/**
 * 星遇同频的个人数据该挂在哪个账号下，以及这个账号有哪些权限。
 *
 * 个人数据（带令牌的地图页、地图状态、玩家视角的社团）的查询键都带上这里的
 * `account`（见 [`vrccAccountKey`]）：不同星遇同频账号的数据各占一个键，断开、
 * 令牌失效、换连另一个账号时不会读到上一个账号的那一份，也不需要谁记得去清缓存。
 *
 * **只认这次挂载之后读到的那一份**（`isFetchedAfterMount`）：缓存里的旧连接
 * 可能还是上一个账号的，拿它拼键会先把别人的数据显示出来。所以 `ready` 之前
 * `account` 是 `offline`、`grants` 是 `undefined`，个人数据的查询要等 `ready`。
 *
 * 面板开着的时候连接也会变，三条路都会让已经挂着的面板换键：
 *
 * - 星遇同频页断开或重连时，把新连接写进同一份缓存
 *   （`useVrccController.refreshConnection`）；
 * - 后台轮询发现令牌失效时，后端发 `vrccConnectionChanged`，这里收到就重读；
 * - 前台调用撞上令牌失效时，[`refreshVrccConnectionAfterFailure`] 让它重读。
 */
export function useVrccConnection(): {
    /** 这次挂载之后已经读到了当前连接。 */
    ready: boolean;
    /** 个人数据查询键里的账号标识，见 [`vrccAccountKey`]。 */
    account: string;
    /**
     * 令牌里有没有某项权限。还没读到时为 `undefined`，读到了才是 `true` /
     * `false`——界面要区分「还不知道」和「确实没有」。
     */
    grants: (scope: string) => boolean | undefined;
} {
    const queryClient = useQueryClient();
    const query = useQuery(vrccConnectionQueryOptions());

    useEffect(() => {
        let disposed = false;
        let off: (() => void) | null = null;
        tauriClient.events
            .subscribe(VRCC_CONNECTION_CHANGED_EVENT, () => {
                void queryClient.invalidateQueries({
                    queryKey: queryKeys.vrccConnection()
                });
            })
            .then((unsubscribe) => {
                if (disposed) {
                    unsubscribe();
                    return;
                }
                off = unsubscribe;
            })
            .catch(() => {
                // 订阅不上（例如不在桌面壳里）就只靠挂载时重读，不报错。
            });
        return () => {
            disposed = true;
            off?.();
        };
    }, [queryClient]);

    const ready = query.isFetchedAfterMount;
    const connection = ready ? (query.data ?? null) : null;
    return {
        ready,
        account: vrccAccountKey(connection),
        grants: (scope) =>
            ready ? scopeGrants(connection?.scope ?? null, scope) : undefined
    };
}

/**
 * 一次星遇同频调用失败后调用：令牌失效时让连接查询重读。
 *
 * 令牌被服务器判失效时，后端当场就把它清掉了（`VrccService::settle`），但只有
 * 后台轮询发现时才发 `vrccConnectionChanged`。前台调用撞上的（例如在世界详情里
 * 提交评分收到 `404 gone`）不发，已经挂着的世界、群组面板就会一直拿着旧账号的
 * 键和权限，直到重新打开。所以星遇同频调用的失败都过一遍这里。
 *
 * 只认 `needsReauth`：缺权限（`missingScope`）时令牌还在，连接没变。
 */
export function refreshVrccConnectionAfterFailure(error: unknown): void {
    if (vrccErrorKind(error) !== 'needsReauth') {
        return;
    }
    void queryClient.invalidateQueries({
        queryKey: queryKeys.vrccConnection()
    });
}

/**
 * 调一次星遇同频命令。失败时先过 [`refreshVrccConnectionAfterFailure`]，再把
 * 错误原样抛出（要不要归一化由调用方决定）。
 */
export async function callVrcc<T>(run: () => Promise<T>): Promise<T> {
    try {
        return await run();
    } catch (error) {
        refreshVrccConnectionAfterFailure(error);
        throw error;
    }
}
