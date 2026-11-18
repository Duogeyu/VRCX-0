import { ExternalLinkIcon, TriangleAlertIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { networkCopyKey } from '@/lib/vrccError';
import type { VrccHealth } from '@/lib/vrccHealth';
import { Alert, AlertDescription, AlertTitle } from '@/ui/shadcn/alert';
import { Button } from '@/ui/shadcn/button';

type LoginVrccStatusAlertProps = {
    health: VrccHealth;
    onOpenStatusPage: () => void;
};

/**
 * 登录页上的「星遇同频（vrcc.cc）连不上」提示。
 *
 * 和 VRChat 那条告警并排，但说的是另一件事：VRChat 那条讲**服务本身有没有故障**
 * （statuspage 的分级），这条讲**我们够不着 vrcc.cc 的接口**。玩家最常遇到的是
 * 后者 —— 本机实测 vrcc.cc 直连会 DNS 超时，必须走代理。
 *
 * 所以这里**刻意分成两种说法**：
 *
 * - `unreachable`（网络层失败）：可能是代理没开、DNS 不对、被拦截。旁边就是
 *   「代理设置」按钮，这是玩家能自己解决的那一半。
 * - `failing`（服务端回了 5xx）：玩家做什么都没用，只能说"等一会"。
 *
 * 两种都不用 `destructive`：红色留给定级明确的重大故障，而这两件事的严重度
 * 我们并不知道。
 */
export function LoginVrccStatusAlert({
    health,
    onOpenStatusPage
}: LoginVrccStatusAlertProps) {
    const { t } = useTranslation();

    // `ok` 和 `unknown` 都不显示：`unknown` 是"还没探到 / 被本机限流挡下"，
    // 启动瞬间闪一下提示比不显示更糟。
    if (health.state === 'ok' || health.state === 'unknown') {
        return null;
    }

    const isUnreachable = health.state === 'unreachable';

    return (
        <Alert>
            <TriangleAlertIcon />
            <AlertTitle>
                {isUnreachable
                    ? t('vrcc.health.unreachable')
                    : t('vrcc.health.failing')}
            </AlertTitle>
            <AlertDescription className="flex flex-col items-start gap-1">
                {/*
                  服务端 5xx 时把它的原文摆出来。

                  这不是"技术细节，玩家不该看"：`VrccError` 对服务端错误的 message
                  本来就写成了人话（`星遇同频接口返回 HTTP 500（http_500）`），里面
                  带着状态码。玩家想弄清"到底谁的问题"，这是唯一的一手信息 ——
                  只写一句「接口暂时不可用」，等于让人对着一个红点猜。
                */}
                {health.state === 'failing' && health.reason ? (
                    <span className="text-xs">
                        {t('vrcc.health.server_said', {
                            message: health.reason
                        })}
                    </span>
                ) : null}
                {/*
                  "这事意味着什么"分两种，因为处置方向相反：

                  - 网络层失败：玩家自己能修（代理、DNS、网络），所以按
                    `transportKind` 说清卡在哪一步 —— 域名解析失败和连不上是
                    两件不同的事，前者换 DNS 或者换网络可能就好，后者多半要代理；
                  - 服务端拒绝：本地改什么都没用，所以说明了这一点，并告诉他
                    我们这边会自己重试 —— 否则玩家会一直找自己的问题。

                  映射和 `describeVrccError` 共用 `networkCopyKey`：同一个 DNS
                  失败不该在两个地方叫两个名字。
                */}
                <span className="text-xs">
                    {health.state === 'unreachable'
                        ? t(networkCopyKey(health.transportKind))
                        : t('vrcc.health.failing_hint')}
                </span>
                <Button
                    type="button"
                    variant="link"
                    size="xs"
                    onClick={onOpenStatusPage}
                >
                    {t('vrcc.health.view_site')}
                    <ExternalLinkIcon data-icon="inline-end" />
                </Button>
            </AlertDescription>
        </Alert>
    );
}
