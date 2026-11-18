// SPDX-License-Identifier: MPL-2.0

import { useEffect } from 'react';

import { useVrccHealthStore } from '@/state/vrccHealthStore';

import { refreshVrccHealth } from './vrccHealthService';

/** 后台多久重探一次。服务端自己缓存一分钟，问得更勤没有意义。 */
const PROBE_INTERVAL_MS = 5 * 60 * 1_000;

/** 回到窗口时，距上次探测超过这么久才重探。 */
const FOCUS_REFRESH_MS = 60 * 1_000;

/**
 * 星遇同频服务可达性的常驻探针。
 *
 * 挂在 `GlobalHosts` 下，**刻意不看登录态**：`GET /api/v1/status` 不要令牌也不要
 * `X-Client`（见 `vrccHealthService` 的文件头），所以登录页也该有结论 —— 那正是
 * 玩家最需要知道"vrcc.cc 现在能不能连"的时刻之一（本机实测访问 vrcc.cc 要代理）。
 *
 * 同理也不过问"有没有连过星遇同频"：站点在不在线，和这个账号用不用它无关。
 */
export function VrccHealthProbeHost(): null {
    useEffect(() => {
        void refreshVrccHealth();

        const timer = window.setInterval(() => {
            void refreshVrccHealth();
        }, PROBE_INTERVAL_MS);

        // 回到窗口时补一次：窗口切走再切回来，最长可能已经隔了一个间隔。
        // 一分钟内的重复聚焦不重探（客户端那层有 30 秒间隔，但没必要每次都走 IPC）。
        const onFocus = () => {
            const checkedAt = useVrccHealthStore.getState().checkedAt;
            if (
                checkedAt !== null &&
                Date.now() - checkedAt < FOCUS_REFRESH_MS
            ) {
                return;
            }
            void refreshVrccHealth();
        };
        window.addEventListener('focus', onFocus);

        return () => {
            window.clearInterval(timer);
            window.removeEventListener('focus', onFocus);
        };
    }, []);

    return null;
}
