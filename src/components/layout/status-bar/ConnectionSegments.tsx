import { useTranslation } from 'react-i18next';

import { resolveServerHealth } from '@/lib/serverHealth';
import { cn } from '@/lib/utils';
import { commands } from '@/platform/tauri/bindings';
import { openExternalLink } from '@/services/shellIntegrationService';
import { toast } from '@/services/toastService';
import { refreshVrcStatusNow } from '@/services/vrcStatusService';
import { links } from '@/shared/constants/link';
import { useRuntimeStore } from '@/state/runtimeStore';

import { realtimeRecoveryLabel } from './realtimeRecovery';
import { formatStatusDate } from './statusBarFormat';
import { StatusSegment } from './StatusBarParts';
import { useStatusNowMs } from './statusBarTicker';
import { resolveVrcStatusIndicator } from './statusBarVrc';
import { StatusIndicatorTooltip } from './StatusIndicatorTooltip';

export function ServersSegment() {
    const { t } = useTranslation();
    const indicator = useRuntimeStore((state) => state.vrcStatus.indicator);
    const summary = useRuntimeStore((state) => state.vrcStatus.summary);
    const status = useRuntimeStore((state) => state.vrcStatus.status);
    const lastFetchedAt = useRuntimeStore(
        (state) => state.vrcStatus.lastFetchedAt
    );
    const refreshing = useRuntimeStore((state) => state.vrcStatus.refreshing);
    const error = useRuntimeStore((state) => state.vrcStatus.error);
    // 判定在 `@/lib/serverHealth`：四态（healthy / issue / unreachable / unknown）。
    // 以前这里是就地写的 `indicator && indicator !== 'none'`，那条判据会把
    // "刷新失败、其实什么也不知道"读成健康。
    const vrcIndicator = resolveVrcStatusIndicator(
        resolveServerHealth({
            indicator,
            status,
            summary,
            error,
            lastFetchedAt
        }),
        { refreshing },
        t,
        formatStatusDate
    );

    async function openStatusPage() {
        const refreshStatusPromise = refreshVrcStatusNow().catch(
            (error: unknown) => {
                console.warn('VRChat status refresh failed:', error);
            }
        );

        try {
            await openExternalLink(links.vrchatStatus);
        } catch (error) {
            toast.add({
                type: 'error',
                title:
                    error instanceof Error
                        ? error.message
                        : t(
                              'component.app_status_bar.toast.failed_to_open_vrchat_status'
                          )
            });
        }

        await refreshStatusPromise;
    }

    return (
        <StatusSegment
            active={vrcIndicator.active}
            alert={vrcIndicator.alert}
            dotClassName={cn(
                refreshing && 'motion-safe:animate-pulse',
                vrcIndicator.dotClassName
            )}
            label={t('status_bar.servers')}
            className="cursor-pointer"
            onClick={() => {
                openStatusPage();
            }}
            tooltip={<StatusIndicatorTooltip indicator={vrcIndicator} />}
        />
    );
}

export function RealtimeConnectionSegment() {
    const { t } = useTranslation();
    const websocketConnected = useRuntimeStore(
        (state) => state.transport.websocketConnected
    );
    const interrupted = useRuntimeStore((state) => state.transport.interrupted);
    const retryWaiting = useRuntimeStore(
        (state) => state.transport.retryWaiting
    );
    const retryAt = useRuntimeStore((state) => state.transport.retryAt);
    const lastError = useRuntimeStore((state) => state.transport.lastError);
    const outageStartedAt = useRuntimeStore(
        (state) => state.transport.outageStartedAt
    );
    const lastConnectedAt = useRuntimeStore(
        (state) => state.transport.lastConnectedAt
    );
    const showRecovery = interrupted && !websocketConnected;
    const retryAtMs = retryAt ? Date.parse(retryAt) : Number.NaN;
    const nowMs = useStatusNowMs(
        showRecovery && retryWaiting && Number.isFinite(retryAtMs)
    );
    const canRetryNow = showRecovery && retryWaiting;

    const recovery = showRecovery
        ? realtimeRecoveryLabel({
              retryWaiting,
              retryAtMs,
              nowMs,
              reconnecting: Boolean(outageStartedAt)
          })
        : null;
    const recoveryText = recovery
        ? recovery.key === 'status_bar.realtime_retry_in'
            ? t(recovery.key, { seconds: recovery.seconds })
            : t(recovery.key)
        : null;

    return (
        <StatusSegment
            active={websocketConnected}
            label={t('status_bar.realtime_connection')}
            value={recoveryText ? `· ${recoveryText}` : undefined}
            onClick={
                canRetryNow
                    ? () => {
                          commands
                              .appRealtimeRetryNow()
                              .catch((error: unknown) => {
                                  console.warn(
                                      'Realtime retry request failed:',
                                      error
                                  );
                              });
                      }
                    : undefined
            }
            tooltip={
                <div className="flex flex-col gap-1 text-xs">
                    <span>
                        WebSocket{' '}
                        {websocketConnected
                            ? t('status_bar.ws_connected')
                            : t('status_bar.ws_disconnected')}
                    </span>
                    {recoveryText ? <span>{recoveryText}</span> : null}
                    {showRecovery && outageStartedAt ? (
                        <span className="text-muted-foreground">
                            {t('status_bar.realtime_disconnected_since', {
                                time: formatStatusDate(outageStartedAt)
                            })}
                        </span>
                    ) : null}
                    {lastConnectedAt ? (
                        <span className="text-muted-foreground">
                            {t('status_bar.realtime_last_connected', {
                                time: formatStatusDate(lastConnectedAt)
                            })}
                        </span>
                    ) : null}
                    {showRecovery && lastError ? (
                        <span className="text-muted-foreground break-words">
                            {t('status_bar.realtime_reason', {
                                reason: lastError
                            })}
                        </span>
                    ) : null}
                    {canRetryNow ? (
                        <span className="text-muted-foreground">
                            {t('status_bar.realtime_retry_now')}
                        </span>
                    ) : null}
                </div>
            }
        />
    );
}
