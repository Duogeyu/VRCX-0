import { ExternalLinkIcon, TriangleAlertIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { ServerHealth } from '@/lib/serverHealth';
import { Alert, AlertDescription, AlertTitle } from '@/ui/shadcn/alert';
import { Button } from '@/ui/shadcn/button';

type LoginServerStatusAlertProps = {
    /**
     * 已经判好的服务状态。
     *
     * 传结论而不是传 `indicator` / `status` / `summary` 三个原始字段：判定规则住在
     * `@/lib/serverHealth`，这里只决定"这个结论长什么样"。早先这里自己写了一遍
     * `indicator !== 'none'`，和状态栏、隐私锁各写一遍 —— 判据等价，但三处
     * 一起漏掉了"刷新失败"（那时 `indicator` 保持原值，看起来一切正常）。
     */
    health: ServerHealth;
    onOpenStatusPage: () => void;
};

export function LoginServerStatusAlert({
    health,
    onOpenStatusPage
}: LoginServerStatusAlertProps) {
    const { t } = useTranslation();

    // `unknown`（还没取到过）和 `healthy` 都不弹：启动瞬间闪一下告警比不弹更糟。
    if (health.state === 'healthy' || health.state === 'unknown') {
        return null;
    }

    const isUnreachable = health.state === 'unreachable';
    // 取不到不等于服务坏了 —— 玩家的网络、代理、DNS 都会这样。所以 `unreachable`
    // 一律用 `default` 而不是 `destructive`，标题也不说"服务器异常"。
    const isMajor =
        health.state === 'issue' &&
        (health.severity === 'major' || health.severity === 'critical');

    const message = isUnreachable
        ? health.reason
        : health.state === 'issue'
          ? health.detail
          : '';

    return (
        <Alert variant={isMajor ? 'destructive' : 'default'}>
            <TriangleAlertIcon />
            <AlertTitle>
                {isUnreachable
                    ? t('status_bar.servers_unreachable')
                    : t('status_bar.servers_issue')}
            </AlertTitle>
            <AlertDescription className="flex flex-col items-start gap-1">
                {message ? (
                    <span className="line-clamp-2">{message}</span>
                ) : null}
                {/* 结论来自更早的一次成功检查：结论照旧显示，但要说明它不是此刻的。 */}
                {health.state === 'issue' && health.stale ? (
                    <span className="text-muted-foreground text-xs">
                        {t('status_bar.servers_stale')}
                    </span>
                ) : null}
                <Button
                    type="button"
                    variant="link"
                    size="xs"
                    onClick={onOpenStatusPage}
                >
                    {t('status_bar.view_status')}
                    <ExternalLinkIcon data-icon="inline-end" />
                </Button>
            </AlertDescription>
        </Alert>
    );
}
