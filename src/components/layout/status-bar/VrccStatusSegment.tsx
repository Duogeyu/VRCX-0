// SPDX-License-Identifier: MPL-2.0

import { useTranslation } from 'react-i18next';

import { cn } from '@/lib/utils';
import { openExternalLink } from '@/services/shellIntegrationService';
import { toast } from '@/services/toastService';
import { links } from '@/shared/constants/link';
import { useVrccHealthStore } from '@/state/vrccHealthStore';

import { formatStatusDate } from './statusBarFormat';
import { StatusSegment } from './StatusBarParts';
import { resolveVrccStatusIndicator } from './statusBarVrcc';
import { StatusIndicatorTooltip } from './StatusIndicatorTooltip';

/**
 * 星遇状态（vrcc.cc）。和 VRChat 服务器那段同权：默认显示，右键菜单里可以关掉。
 *
 * 结论和「上次拿到结论的时刻」由 `VrccHealthProbeHost` 常驻写进 store，这里只读。
 * 单独一个文件，是为了让上游改状态栏时不和下游的这一段撞在一起。
 */
export function VrccStatusSegment() {
    const { t } = useTranslation();
    const health = useVrccHealthStore((state) => state.health);
    // `checkedAt` 是**拿到结论**的时间，不是「上次尝试」的时间 ——
    // 探测被本机限流挡下时不产生结论，那时这个值不动。
    const checkedAt = useVrccHealthStore((state) => state.checkedAt);
    // 后端正在探。圆点据此脉冲，和 VRChat 那段一致。
    const refreshing = useVrccHealthStore((state) => state.refreshing);
    const indicator = resolveVrccStatusIndicator(
        health,
        checkedAt,
        refreshing,
        t,
        // 时间格式和 VRChat 那段的「上次获取」保持一致。
        (ms) => formatStatusDate(new Date(ms).toISOString())
    );

    /**
     * 打开 vrcc.cc 的状态页。
     *
     * 不顺手触发一次重探：探针在 `VrccHealthProbeHost` 里按自己的节奏跑，
     * 而 `status` 有 30 秒本地间隔 + 服务端 1 分钟缓存，点一下再问一遍多半拿到同一份。
     */
    function openStatusPage() {
        openExternalLink(links.vrccStatus).catch((error: unknown) => {
            toast.add({
                type: 'error',
                title:
                    error instanceof Error
                        ? error.message
                        : t(
                              'component.app_status_bar.toast.failed_to_open_vrchat_status'
                          )
            });
        });
    }

    return (
        <StatusSegment
            active={indicator.active}
            dotClassName={cn(
                refreshing && 'motion-safe:animate-pulse',
                indicator.dotClassName
            )}
            label={t('status_bar.vrcc')}
            className="cursor-pointer"
            onClick={openStatusPage}
            tooltip={<StatusIndicatorTooltip indicator={indicator} />}
        />
    );
}
