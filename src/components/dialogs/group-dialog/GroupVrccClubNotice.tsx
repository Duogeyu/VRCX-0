// SPDX-License-Identifier: MPL-2.0

import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { TranslatableText } from '@/components/translation/TranslatableText';
import { queryKeys } from '@/lib/entityQueryCache';
import { useNowMs } from '@/lib/useNowMs';
import {
    clubCodeForGroup,
    vrccPublicClubQueryOptions
} from '@/repositories/vrccClubRepository';
import { fetchVrccScopeGrants } from '@/repositories/vrccMapRepository';

import { joinSegments } from './clubText';

/**
 * 群组详情「概览」右栏里的一块：星遇同频社团的公告。
 *
 * 插在「公告」（VRChat 群组动态第一条）与「规则」之间——它和上面那块不是同
 * 一种东西：上面是群组动态，这里是社团管理员写的置顶公告、带到期时间。
 *
 * `notice` 只有 `/me/clubs/{code}` 给，所以依赖 `clubs` 权限。拿不到就整块
 * 不渲染，不留空壳。
 *
 * **过期不隐藏**：`until` 过了照常显示内容，只在右下角挂一个「已失效」——
 * 直接隐藏会让人以为社团从没发过公告。
 */
export function GroupVrccClubNotice({
    group
}: {
    group: {
        shortCode?: string | null;
        discriminator?: string | null;
        id?: string;
    };
}) {
    const { t } = useTranslation();
    const code = clubCodeForGroup(group);
    // `Date.now()` 在渲染期调用是副作用（React Compiler 的 purity 规则会拦）。
    // 走仓库现成的钩子，每分钟刷一次就够——公告的到期精度不需要秒级。
    const nowMs = useNowMs({ intervalMs: 60_000 });

    const clubQuery = useQuery({
        ...vrccPublicClubQueryOptions(code),
        enabled: Boolean(code)
    });

    const scopeQuery = useQuery({
        queryKey: queryKeys.vrccScope('clubs'),
        queryFn: () => fetchVrccScopeGrants('clubs'),
        staleTime: 60 * 1000,
        retry: 0,
        refetchOnWindowFocus: false
    });

    const detailQuery = useQuery({
        queryKey: ['group', 'vrcc-club-detail', code] as const,
        queryFn: async () => {
            const { commands } = await import('@/platform/tauri/bindings');
            return commands.appVrccMyClub(code);
        },
        enabled:
            Boolean(code) &&
            scopeQuery.data === true &&
            Boolean(clubQuery.data?.club),
        staleTime: 60_000,
        gcTime: 5 * 60_000,
        retry: 0
    });

    if (!code || !clubQuery.data?.club) {
        return null;
    }

    const notice = detailQuery.data?.club?.notice;
    if (!notice) {
        return null;
    }
    // 过期**不影响显示**，只在右下角挂一个「已失效」。`until` 缺失当作长期。
    const expired = typeof notice.until === 'number' && notice.until <= nowMs;

    const text = joinSegments(notice.segs);
    if (!text) {
        return null;
    }

    return (
        <TranslatableText
            source={text}
            entityId={`vrcc-club-notice:${code}`}
            density="button"
        >
            {({ action, meta, error, text: visibleText }) => (
                <section className="bg-card/40 relative flex min-w-0 flex-col gap-2 rounded-md border p-3">
                    <div className="flex min-w-0 items-center justify-between gap-2">
                        <div className="truncate text-sm font-medium">
                            {t('dialog.group.vrcc.notice.title')}
                        </div>
                        {action}
                    </div>
                    <div className="min-w-0">
                        {meta}
                        <pre className="text-muted-foreground max-h-40 overflow-auto font-sans text-xs whitespace-pre-wrap">
                            {visibleText || '\u2014'}
                        </pre>
                        {error}
                    </div>
                    {expired ? (
                        <span className="text-muted-foreground/70 pointer-events-none absolute right-3 bottom-2 text-[10px] leading-none">
                            {t('dialog.group.vrcc.notice.expired')}
                        </span>
                    ) : null}
                </section>
            )}
        </TranslatableText>
    );
}
