// SPDX-License-Identifier: MPL-2.0

import { ExternalLink, ShieldCheck, Unplug } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useVrccFirstRunStore } from '@/state/vrccFirstRunStore';
import { Badge } from '@/ui/shadcn/badge';
import { Button } from '@/ui/shadcn/button';
import { Progress } from '@/ui/shadcn/progress';
import { Spinner } from '@/ui/shadcn/spinner';

import { formatReset } from '../formatReset';
import { useOpenExternalLink } from '../useOpenExternalLink';
import type {
    VrccConnection,
    VrccDeviceAuthPrompt,
    VrccUsage
} from '../vrccApi';

const SITE = 'https://vrcc.cc';

/**
 * 「我的」页：连接状态、设备授权、今日用量。
 *
 * 用量口径是「今日已用 N/上限」，不是「剩余」，且**全部来自服务端的
 * `RateLimit-*` 响应头**。官方没有查询额度的接口，那组头只在发请求的那一刻
 * 回一次，所以这里显示的是「最近一次请求时服务端说的数」。还没拿到过就显示
 * 「未知」——不拿本地日期去推算，因为官方按北京时间零点重置，本地算不准。
 */
export function VrccMePanel({
    connection,
    usage,
    deviceAuth,
    deviceAuthBusy,
    deviceAuthMessage,
    deviceAuthCooldown,
    onBegin,
    onDisconnect,
    onOpenLink
}: {
    connection: VrccConnection | null;
    usage: VrccUsage | null;
    deviceAuth: VrccDeviceAuthPrompt | null;
    deviceAuthBusy: boolean;
    deviceAuthMessage: string | null;
    /** 距离下一次允许点「申请连接码」还有多少秒；`0` 表示可以点。 */
    deviceAuthCooldown: number;
    onBegin: () => void;
    onDisconnect: () => void | Promise<void>;
    onOpenLink: (url: string) => void;
}) {
    const { t } = useTranslation();
    const connected = connection?.connected ?? false;
    // 底部「第三方客户端，不是星遇同频官方」那句里的链接：点下去要有
    // 即时反馈，否则玩家会以为没点上而重复点。
    const { open: openLink, pending: openLinkPending } = useOpenExternalLink();

    return (
        <div className="flex h-full min-h-0 flex-col gap-3 overflow-y-auto">
            {/* 连接状态 */}
            <section
                data-tour="vrcc-me-connect"
                className="rounded-xl border p-3"
            >
                <div className="mb-2 flex items-center gap-2">
                    <ShieldCheck className="text-muted-foreground size-4" />
                    <h3 className="text-sm font-semibold">
                        {t('vrcc.me.status')}
                    </h3>
                    <Badge
                        variant={connected ? 'outline' : 'secondary'}
                        className="ml-auto shrink-0"
                    >
                        {connected
                            ? t('vrcc.connected')
                            : t('vrcc.not_connected')}
                    </Badge>
                </div>

                {connected ? (
                    <div className="space-y-1 text-xs">
                        <Row
                            label={t('vrcc.me.star')}
                            value={
                                connection?.star != null
                                    ? `#${connection.star}`
                                    : '—'
                            }
                        />
                        <Row
                            label={t('vrcc.me.vrc_name')}
                            value={connection?.vrcName ?? '—'}
                        />
                        <Row
                            label={t('vrcc.me.scope')}
                            value={connection?.scope ?? '—'}
                        />
                        <div className="flex items-baseline gap-2">
                            <span className="text-muted-foreground shrink-0">
                                {t('vrcc.me.client')}
                            </span>
                            <span className="min-w-0 flex-1 truncate text-right font-mono text-[11px]">
                                {connection?.client ?? '—'}
                            </span>
                            {/* dev- 标识只能要普通权限，且在同意页会被标「测试中」。
                                在这里点出来，比事后排查 403 scope_unreviewed 省事。 */}
                            {connection?.clientIsDev && (
                                <Badge
                                    variant="secondary"
                                    className="shrink-0 text-[10px]"
                                >
                                    {t('vrcc.me.client_dev')}
                                </Badge>
                            )}
                        </div>
                        {connection?.nextPollAt && (
                            <Row
                                label={t('vrcc.me.next_poll')}
                                value={new Date(
                                    connection.nextPollAt
                                ).toLocaleTimeString()}
                            />
                        )}
                        <Button
                            variant="outline"
                            size="sm"
                            className="mt-2 w-full"
                            onClick={() => void onDisconnect()}
                        >
                            <Unplug className="mr-1 size-3" />
                            {t('vrcc.me.disconnect')}
                        </Button>
                    </div>
                ) : (
                    <div className="space-y-2">
                        {/* 令牌被服务器判了失效（`404 gone`）：说清楚为什么断了。 */}
                        {connection?.expired && (
                            <p className="text-xs leading-relaxed">
                                {t('vrcc.me.expired')}
                            </p>
                        )}
                        {/* 授权和当前构建对不上，两种结局的话不一样：
                            - `grantMismatch`：令牌整个作废，得重新授权
                            - `scopeMismatch`：令牌还能用，只是权限少了几项
                            混成一句会让后者的玩家以为连接坏了。 */}
                        {connection?.credential === 'grantMismatch' && (
                            <p className="text-xs leading-relaxed">
                                {t('vrcc.me.grant_mismatch')}
                            </p>
                        )}
                        {connection?.credential === 'scopeMismatch' && (
                            <p className="text-xs leading-relaxed">
                                {t('vrcc.me.scope_mismatch')}
                            </p>
                        )}
                        <p className="text-muted-foreground text-xs leading-relaxed">
                            {t('vrcc.me.connect_hint')}
                        </p>
                        <Button
                            size="sm"
                            className="w-full"
                            disabled={
                                deviceAuthBusy ||
                                deviceAuth != null ||
                                deviceAuthCooldown > 0
                            }
                            onClick={onBegin}
                        >
                            {deviceAuthBusy && (
                                <Spinner className="mr-1 size-3" />
                            )}
                            {deviceAuthCooldown > 0
                                ? t('vrcc.me.begin_cooldown', {
                                      sec: deviceAuthCooldown
                                  })
                                : t('vrcc.me.begin')}
                        </Button>
                    </div>
                )}
            </section>

            {/* 设备授权进行中 */}
            {deviceAuth && (
                <section
                    data-tour="vrcc-me-auth"
                    className="rounded-xl border p-3"
                >
                    <h3 className="mb-2 text-sm font-semibold">
                        {t('vrcc.me.auth_pending')}
                    </h3>
                    <div className="text-muted-foreground text-xs">
                        {t('vrcc.me.auth_hint')}
                    </div>
                    <div className="bg-muted my-2 rounded-lg border border-dashed py-3 text-center font-mono text-lg font-bold tracking-[0.3em]">
                        {deviceAuth.userCode}
                    </div>
                    {(deviceAuth.verificationUriComplete ??
                        deviceAuth.verificationUri) && (
                        <Button
                            variant="outline"
                            size="sm"
                            className="w-full"
                            onClick={() =>
                                onOpenLink(
                                    deviceAuth.verificationUriComplete ??
                                        deviceAuth.verificationUri ??
                                        SITE
                                )
                            }
                        >
                            <ExternalLink className="mr-1 size-3" />
                            {t('vrcc.me.open_browser')}
                        </Button>
                    )}
                    <div className="text-muted-foreground mt-2 text-center text-[10px]">
                        {t('vrcc.me.auth_expires', {
                            minutes: Math.floor(deviceAuth.expiresInSecs / 60),
                            sec: deviceAuth.intervalSecs
                        })}
                    </div>
                </section>
            )}

            {deviceAuthMessage && (
                <div className="border-destructive/40 bg-destructive/10 text-destructive rounded-lg border p-2 text-xs">
                    {deviceAuthMessage}
                </div>
            )}

            {/* 今日用量：全部来自服务端响应头 */}
            {usage && (
                <section
                    data-tour="vrcc-me-usage"
                    className="rounded-xl border p-3"
                >
                    <div className="mb-2 flex items-baseline gap-2">
                        <h3 className="text-sm font-semibold">
                            {t('vrcc.me.usage')}
                        </h3>
                        {usage.resetSecs != null && usage.resetSecs > 0 && (
                            <span className="text-muted-foreground ml-auto text-[10px] tabular-nums">
                                {t('vrcc.me.usage_reset_in', {
                                    time: formatReset(usage.resetSecs)
                                })}
                            </span>
                        )}
                    </div>
                    <UsageBar
                        label={t('vrcc.me.personal_used')}
                        used={usage.personalUsedToday}
                        limit={usage.personalDailyLimit}
                        unknownHint={t('vrcc.me.usage_unknown')}
                    />
                    <UsageBar
                        label={t('vrcc.me.unread_used')}
                        used={usage.unreadUsedToday ?? null}
                        limit={usage.unreadDailyLimit ?? null}
                        unknownHint={t('vrcc.me.usage_unknown')}
                    />
                    <UsageBar
                        label={t('vrcc.me.window')}
                        used={usage.windowUsed}
                        limit={usage.windowLimit}
                        unknownHint={t('vrcc.me.usage_unknown')}
                    />
                    <p className="text-muted-foreground mt-2 text-[10px] leading-relaxed">
                        {t('vrcc.me.usage_note')}
                    </p>
                </section>
            )}

            <div className="text-muted-foreground text-center text-[10px] leading-relaxed">
                {/* 常驻入口：自动弹只弹一次，想看的时候要能自己找回来。 */}
                <button
                    type="button"
                    onClick={() =>
                        useVrccFirstRunStore.getState().openManually()
                    }
                    className="hover:text-primary mr-2 inline-flex items-center gap-0.5 hover:underline"
                >
                    {t('vrcc.tour.reopen')}
                </button>
                {t('vrcc.me.disclaimer')}{' '}
                <button
                    type="button"
                    disabled={openLinkPending}
                    onClick={() => openLink(SITE)}
                    className="hover:text-primary inline-flex items-center gap-0.5 hover:underline"
                >
                    vrcc.cc
                    <ExternalLink className="size-2.5" />
                </button>
            </div>
        </div>
    );
}

function Row({ label, value }: { label: string; value: string }) {
    return (
        <div className="flex items-baseline gap-2">
            <span className="text-muted-foreground shrink-0">{label}</span>
            <span className="min-w-0 flex-1 truncate text-right font-mono text-[11px]">
                {value}
            </span>
        </div>
    );
}

function UsageBar({
    label,
    used,
    limit,
    unknownHint
}: {
    label: string;
    used: number | null;
    limit: number | null;
    unknownHint: string;
}) {
    // 口径：已用 N/上限，不显示「剩余」。
    //
    // `null` 是「服务端还没说过」——这时不画进度条、不填假数字。我们刻意不
    // 拿本地日期去推算，因为官方按北京时间零点重置，本地算出来的是错的。
    if (used == null || limit == null) {
        return (
            <div className="mb-1.5">
                <div className="mb-0.5 flex items-baseline gap-2 text-[11px]">
                    <span className="text-muted-foreground">{label}</span>
                    <span className="text-muted-foreground/70 ml-auto text-[10px]">
                        {unknownHint}
                    </span>
                </div>
            </div>
        );
    }
    const percent = limit > 0 ? Math.min(100, (used / limit) * 100) : 0;
    return (
        <div className="mb-1.5">
            <div className="mb-0.5 flex items-baseline gap-2 text-[11px]">
                <span className="text-muted-foreground">{label}</span>
                <span className="ml-auto tabular-nums">
                    {used}/{limit}
                </span>
            </div>
            <Progress value={percent} className="h-1" />
        </div>
    );
}
