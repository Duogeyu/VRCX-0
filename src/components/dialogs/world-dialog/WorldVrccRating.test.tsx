// SPDX-License-Identifier: MPL-2.0

// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, render, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi, beforeEach } from 'vitest';

const appVrccMapPage = vi.fn();
const appVrccMyMapPage = vi.fn();
const appVrccConnection = vi.fn();

vi.mock('@/platform/tauri/bindings', () => ({
    commands: {
        appVrccMapPage: (...args: unknown[]) => appVrccMapPage(...args),
        appVrccMyMapPage: (...args: unknown[]) => appVrccMyMapPage(...args),
        appVrccConnection: (...args: unknown[]) => appVrccConnection(...args)
    }
}));

vi.mock('react-i18next', () => ({
    useTranslation: () => ({
        // 这里刻意模仿真实语言包的两个特征，而不是随便返回字符串：
        // 1. 复数键带 `_one` / `_other` 后缀；
        // 2. 插值是**单**花括号（`{count}`），配置见
        //    `src/services/i18nService.ts` 的 `interpolation.prefix`。
        //    写成 `{{count}}` 会原样显示出来，那是真出过的 bug。
        t: (key: string, options?: { count?: number }) => {
            const count = options?.count ?? 0;
            if (key === 'dialog.world.vrcc.raters') {
                return count === 1 ? '1 人打分' : `${count} 人打分`;
            }
            return key;
        }
    })
}));

import { WorldVrccRating } from './WorldVrccRating';

function renderWithClient(ui: ReactNode) {
    const client = new QueryClient({
        defaultOptions: { queries: { retry: false } }
    });
    return render(
        <QueryClientProvider client={client}>{ui}</QueryClientProvider>
    );
}

/** 造一份够用的公开地图页响应。 */
function mapPage(overrides: Record<string, unknown> = {}) {
    return {
        map: {
            world: 'wrld_test',
            name: '测试世界',
            avg: 8.4,
            raters: 412,
            rank: 37,
            ...overrides
        },
        spread: [],
        went: 100,
        cap: 32,
        perf: null,
        at: 1
    };
}

describe('WorldVrccRating', () => {
    beforeEach(() => {
        appVrccMapPage.mockReset();
        appVrccMyMapPage.mockReset();
        appVrccConnection.mockReset();
        // 默认**没连上**：带令牌的那条请求不该发出去。要测介绍时由用例自己
        // 把 `scope` 换成带 `maps` 的串。
        appVrccConnection.mockResolvedValue({ scope: 'profile' });
        appVrccMyMapPage.mockResolvedValue(null);
    });

    afterEach(() => {
        cleanup();
    });

    it('有评分时显示星级、均分与打分人数', async () => {
        appVrccMapPage.mockResolvedValue(mapPage());
        renderWithClient(<WorldVrccRating worldId="wrld_test" />);

        expect(await screen.findByText('8.4')).toBeTruthy();
        expect(screen.getByText('412 人打分')).toBeTruthy();
    });

    it('星级与分数在标题下面一行，不和标题挤在一起', async () => {
        appVrccMapPage.mockResolvedValue(mapPage());
        renderWithClient(<WorldVrccRating worldId="wrld_test" />);

        const title = await screen.findByText('dialog.world.vrcc.rating');
        // 等真数据到齐。只等标题会停在 `⋯` 那一帧，那时还没有星星。
        await screen.findByText('8.4');
        const stars = screen.getByRole('img');

        // 在同一个容器里就又变回「左边一个词、右边一堆数字」那一版了
        expect(title.contains(stars)).toBe(false);
        // 顺序是标题在前、读数在后
        expect(
            title.compareDocumentPosition(stars) &
                Node.DOCUMENT_POSITION_FOLLOWING
        ).toBeTruthy();
        // 自己独占一行之后星星该是放大那一档（24px），不是原来的 12px
        expect(stars.firstElementChild?.className).toContain('size-6');
        // 换行不等于换对齐：读数与人数仍然贴右
        expect(screen.getByText('412 人打分').className).toContain(
            'text-right'
        );
        expect(stars.closest('div')?.className).toContain('justify-end');
    });

    it('名次不出现在概览里', async () => {
        appVrccMapPage.mockResolvedValue(mapPage());
        renderWithClient(<WorldVrccRating worldId="wrld_test" />);

        await screen.findByText('8.4');
        // 37 是 rank，概览刻意不显示，免得作者之间攀比
        expect(screen.queryByText(/#?\s*37/)).toBeNull();
    });

    it('不在榜上时说「不在榜上」，不是空白也不是错误', async () => {
        // 后端把 404 not_found 转成 null
        appVrccMapPage.mockResolvedValue(null);
        renderWithClient(<WorldVrccRating worldId="wrld_test" />);

        expect(
            await screen.findByText('dialog.world.vrcc.not_listed')
        ).toBeTruthy();
    });

    it('没人打分时不显示满分误导成 0.0', async () => {
        appVrccMapPage.mockResolvedValue(
            mapPage({ avg: null, raters: 0, rank: null })
        );
        renderWithClient(<WorldVrccRating worldId="wrld_test" />);

        expect(
            await screen.findByText('dialog.world.vrcc.no_rating')
        ).toBeTruthy();
        expect(screen.queryByText('0.0')).toBeNull();
    });

    it('世界 ID 不是 wrld_ 时一个请求都不发', () => {
        renderWithClient(<WorldVrccRating worldId="" />);

        // enabled 为 false，queryFn 不该被调用
        expect(appVrccMapPage).not.toHaveBeenCalled();
    });

    it('来源声明始终在', async () => {
        appVrccMapPage.mockResolvedValue(mapPage());
        renderWithClient(<WorldVrccRating worldId="wrld_test" />);

        expect(
            await screen.findByText('dialog.world.vrcc.source')
        ).toBeTruthy();
    });

    it('只有一个人打分时走单数文案', async () => {
        appVrccMapPage.mockResolvedValue(mapPage({ raters: 1 }));
        renderWithClient(<WorldVrccRating worldId="wrld_test" />);

        expect(await screen.findByText('1 人打分')).toBeTruthy();
    });

    it('连上且带 maps 权限时，标题下显示地图介绍', async () => {
        appVrccMapPage.mockResolvedValue(mapPage());
        appVrccConnection.mockResolvedValue({ scope: 'profile maps' });
        appVrccMyMapPage.mockResolvedValue({
            intro: { id: 1, text: '夜色温柔的老地方，适合一个人待着' }
        });
        renderWithClient(<WorldVrccRating worldId="wrld_test" />);

        expect(
            await screen.findByText('夜色温柔的老地方，适合一个人待着')
        ).toBeTruthy();
    });

    it('没连上时不发带令牌的请求，也不显示介绍', async () => {
        // 公开的 `/maps/{world}` 与 `/maps/{world}/page` 都不返回 `intro`，
        // 只有 `/me/maps/{world}/page` 有。没连上就别白撞一次 403。
        appVrccMapPage.mockResolvedValue(mapPage());
        renderWithClient(<WorldVrccRating worldId="wrld_test" />);

        await screen.findByText('8.4');
        // 先确认那条判断权限的查询确实跑过了，否则「没发请求」是因为还没轮到。
        expect(appVrccConnection).toHaveBeenCalled();
        expect(appVrccMyMapPage).not.toHaveBeenCalled();
    });

    it('介绍封在三行半的高度里，不把卡片撑成一面墙', async () => {
        appVrccMapPage.mockResolvedValue(mapPage());
        appVrccConnection.mockResolvedValue({ scope: 'profile maps' });
        appVrccMyMapPage.mockResolvedValue({
            intro: { id: 1, text: '很长的介绍' }
        });
        renderWithClient(<WorldVrccRating worldId="wrld_test" />);

        const intro = await screen.findByText('很长的介绍');
        // 服务器给的是**完整介绍**，不像榜单那条已经截成第一行。
        expect(intro.className).toContain('max-h-[3.5lh]');
        expect(intro.className).toContain('overflow-hidden');
    });

    it('介绍在标题之下、星级之上', async () => {
        appVrccMapPage.mockResolvedValue(mapPage());
        appVrccConnection.mockResolvedValue({ scope: 'profile maps' });
        appVrccMyMapPage.mockResolvedValue({
            intro: { id: 1, text: '夜色温柔的老地方' }
        });
        renderWithClient(<WorldVrccRating worldId="wrld_test" />);

        const intro = await screen.findByText('夜色温柔的老地方');
        await screen.findByText('8.4');
        const title = screen.getByText('dialog.world.vrcc.rating');
        const stars = screen.getByRole('img');

        expect(
            title.compareDocumentPosition(intro) &
                Node.DOCUMENT_POSITION_FOLLOWING
        ).toBeTruthy();
        expect(
            intro.compareDocumentPosition(stars) &
                Node.DOCUMENT_POSITION_FOLLOWING
        ).toBeTruthy();
    });
});

/**
 * 语言包层面的守卫。
 *
 * 这个项目的 i18next 把插值定成了**单**花括号
 * （`src/services/i18nService.ts` 里 `interpolation: { prefix: '{', suffix: '}' }`），
 * 所以写成 `{{count}}` 不会被替换，会原样渲染成 `{{count}} 人打分`。
 * 这个 bug 真出过一次，而且**单元测试测不到**——那个文件里 mock 了 `t`。
 * 所以在这里直接读语言包原文断言。
 *
 * 只查 `dialog.world.vrcc` 这一块。语言包里还有别处的 `{{…}}`（那是另一批
 * 早就存在的键，不在本任务范围内），全文件扫描会把它们算进来，变成误报。
 */
describe('语言包里的插值语法', () => {
    const locales = [
        'zh-CN',
        'zh-TW',
        'en',
        'ja',
        'ko',
        'de',
        'es',
        'fr',
        'pt',
        'ru',
        'cs'
    ];

    it.each(locales)('%s 的 raters 用单花括号', async (code) => {
        const mod = await import(`@/localization/${code}.json`);
        const json = (mod.default ?? mod) as {
            dialog?: { world?: { vrcc?: Record<string, string> } };
        };
        const vrcc = json.dialog?.world?.vrcc;
        expect(vrcc).toBeTruthy();

        const raters = [vrcc?.raters_one, vrcc?.raters_other]
            .filter((text): text is string => typeof text === 'string')
            .join('\n');

        expect(raters.length).toBeGreaterThan(0);
        // 双花括号会原样显示，是 bug
        expect(raters).not.toContain('{{count}}');
        expect(raters).toContain('{count}');
    });
});
