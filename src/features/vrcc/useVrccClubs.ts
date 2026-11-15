// SPDX-License-Identifier: MPL-2.0

import { useQuery } from '@tanstack/react-query';
import { useEffect, useState } from 'react';

import { queryKeys, entityQueryPolicies } from '@/lib/entityQueryCache';
import { isVrccError } from '@/lib/vrccError';
import { scopeGrants } from '@/lib/vrccScope';

import { fromMyClubs, fromPublicClubs, type VrccClubListView } from './clubRow';
import { vrccApi, type VrccConnection } from './vrccApi';

/**
 * 社团列表：公开列表与玩家视角的切换。
 *
 * 照着地图榜那套规矩（见 `useVrccController` 的 `refreshBoards`）：
 *
 * 1. **能力判定**：连上且令牌带 `clubs` 才走 `/me/clubs`，否则走公开 `/clubs`。
 * 2. **一次性降级记忆**：玩家视角失败（`403 scope`、服务器不认、传输失败）之后
 *    这次连接不再试，免得每次翻页都先撞一次墙。连接变了就重置。
 * 3. **限流不是「用不了」**：`throttled` 原样留给调用方，不降级——否则玩家刷新
 *    一下就会看到列表来源被静默换掉。
 * 4. **键里带来源**：两套接口的缓存不能混（见 `queryKeys.vrccClubs`）。
 *
 * 和地图榜不同的地方：玩家视角**拿不到 `total`**，所以降级方向反过来——公开列表
 * 才有总数和分页。归一层把这件事表达成 `total: null`，面板据此隐藏「共 N 个」。
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
