// SPDX-License-Identifier: MPL-2.0

import { useQuery } from '@tanstack/react-query';
import { useEffect, useState } from 'react';

import { queryKeys, entityQueryPolicies } from '@/lib/entityQueryCache';
import { isVrccError } from '@/lib/vrccError';
import { scopeGrants } from '@/lib/vrccScope';

import { fromMyClubs, fromPublicClubs, type VrccClubListView } from './clubRow';
import { vrccApi, type VrccConnection } from './vrccApi';

/**
 * 社团列表的数据源切换。规则与地图榜相同，见 `useVrccController` 的 `refreshBoards`。
 *
 * 1. 已连接且令牌含 `clubs` 权限时用 `/me/clubs`。否则用公开的 `/clubs`。
 * 2. 玩家视角失败后，本次连接不再尝试。连接发生变化后重置。
 * 3. 本地限流不触发降级。`throttled` 原样返回给调用方。
 * 4. 缓存键包含数据源。两个数据源的缓存不混用，见 `queryKeys.vrccClubs`。
 *
 * 与地图榜的差别：玩家视角不提供 `total`。只有公开源有总数与分页。
 * 归一化层把这种情况表示为 `total: null`。面板据此隐藏总数。
 */
export function useVrccClubs({
    connection,
    limit,
    offset
}: {
    connection: VrccConnection | null;
    limit: number;
    offset: number;
}): {
    view: VrccClubListView | null;
    isLoading: boolean;
    error: unknown;
    /** 这次用的是玩家视角还是公开列表。 */
    source: 'public' | 'mine';
    refetch: () => void;
} {
    const [mineUnavailable, setMineUnavailable] = useState(false);
    const wantsMine =
        connection?.connected === true &&
        scopeGrants(connection.scope, 'clubs') &&
        !mineUnavailable;

    // 换连接（重连、重新授权、断开）就重新给玩家视角一次机会。
    const connectionKey = `${connection?.connected ?? false}|${connection?.scope ?? ''}`;
    useEffect(() => {
        setMineUnavailable(false);
    }, [connectionKey]);

    const source: 'public' | 'mine' = wantsMine ? 'mine' : 'public';

    const query = useQuery({
        queryKey: queryKeys.vrccClubs(
            source,
            source === 'public' ? limit : 0,
            source === 'public' ? offset : 0
        ),
        queryFn: async () =>
            source === 'mine'
                ? fromMyClubs(await vrccApi.myClubs())
                : fromPublicClubs(await vrccApi.clubs(limit, offset)),
        ...entityQueryPolicies.vrccClubs
    });

    useEffect(() => {
        if (source !== 'mine' || !query.error) {
            return;
        }
        // 限流只是「等一下再来」，不是「这次连接下看不了玩家视角」。
        if (isVrccError(query.error) && query.error.kind === 'throttled') {
            return;
        }
        setMineUnavailable(true);
    }, [source, query.error]);

    return {
        view: query.data ?? null,
        isLoading: query.isLoading,
        error:
            source === 'mine' && mineUnavailable ? null : (query.error ?? null),
        source,
        refetch: () => {
            void query.refetch();
        }
    };
}
