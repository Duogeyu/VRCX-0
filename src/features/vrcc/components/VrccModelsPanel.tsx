// SPDX-License-Identifier: MPL-2.0

import {
    ChevronLeft,
    ChevronRight,
    ChevronsLeft,
    ChevronsRight,
    RefreshCw
} from 'lucide-react';
import {
    Fragment,
    useCallback,
    useEffect,
    useMemo,
    useRef,
    useState
} from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/shadcn/button';
import { Spinner } from '@/ui/shadcn/spinner';
import {
    ToggleGroup,
    ToggleGroupItem,
    ToggleGroupSeparator
} from '@/ui/shadcn/toggle-group';

import { buildPageItems } from '../boardPages';
import {
    MORPH_ENTRY_MS,
    planMorph,
    type MorphPlan,
    type MorphRect
} from '../cardMorph';
import { useVrccRowVisibility } from '../useVrccRowVisibility';
import type { VrccBoardSort, VrccModelBoard } from '../vrccApi';
import { ModelCardFace, type ModelRow } from './ModelCardFace';
import { useTiltGrid } from './useTiltGrid';
import { VrccModelDialog } from './VrccModelDialog';

const SORTS: readonly VrccBoardSort[] = ['top', 'week', 'new'];

/**
 * 一页多少张卡。榜单一次取 100 条（接口上限），本地按这个数切页。
 * 取 24：列数在 2–12 之间随窗口变，24 能被最常见的几种整除，末行不空半排。
 */
const PAGE_SIZE = 24;

function isBoardSort(value: unknown): value is VrccBoardSort {
    return SORTS.includes(value as VrccBoardSort);
}

function toMorphRect(rect: DOMRect): MorphRect {
    return {
        left: rect.left,
        top: rect.top,
        width: rect.width,
        height: rect.height
    };
}

/**
 * 减动效是否生效。
 *
 * 两条通道：系统偏好，和设置里那个 `.reduce-effects` 类。只在点击那一刻读
 * 一次——播放途中改设置的概率可以忽略，而订阅要多挂一个全局监听和一处
 * class 观察，不划算。
 */
function prefersReducedMotion(): boolean {
    if (typeof window === 'undefined') {
        return true;
    }
    if (document.documentElement.classList.contains('reduce-effects')) {
        return true;
    }
    return (
        window.matchMedia?.('(prefers-reduced-motion: reduce)').matches === true
    );
}

/**
 * 模型榜（1.17.0 起）。开放数据，不需要连接。
 *
 * 用卡片而不是一行一条：接口给的封面是 300×300 的方图，玩家判断一个模型靠
 * 的就是「它长什么样」，塞进 40px 缩略图等于把唯一的信息源压成了色块。
 *
 * `price` 是 BOOTH 原文（`¥1,500` / `無料`），**不解析成数字**：各店写法不
 * 统一，自己解析只会显示错价。
 *
 * 同一时刻只允许一张翻着——网格是拿来扫的，全翻过去就谁也认不出谁了。
 *
 * 点卡片除了翻面，还打开 [`VrccModelDialog`] 显示该模型的热评。两件事现在
 * 同时发生：翻面是「展开」动画的第一版实现，后续会把它和弹窗的形变接成一段
 * 连续动作，那时背面不再需要留白，也不需要「翻回正面」的按钮。
 */
export function VrccModelsPanel({
    models,
    sort,
    sortSupported,
    available,
    onSortChange,
    onRefresh
}: {
    models: VrccModelBoard | null;
    sort: VrccBoardSort;
    sortSupported: boolean;
    /** 服务器上到底有没有这个接口；false 表示旧服务器，显示「暂时没有」。 */
    available: boolean;
    onSortChange: (sort: VrccBoardSort) => void;
    onRefresh: () => void;
}) {
    const { t } = useTranslation();
    // 整个网格共用一个指针监听；具体见 useTiltGrid。
    const gridRef = useTiltGrid<HTMLUListElement>();
    // 封面要不要现在就去取。原生 `loading="lazy"` 在距离视口约 1250px 处就
    // 开始请求，一页 24 张全部落在这个半径里，等于没有懒加载——所以这里和
    // 地图榜用同一个 `IntersectionObserver`，严格按视口算。
    const { visible, observe } = useVrccRowVisibility();
    const rows = models?.models ?? [];

    const [page, setPage] = useState(0);
    /** 当前开着评论弹窗的那张。 */
    const [opened, setOpened] = useState<ModelRow | null>(null);
    /**
     * 弹窗开关。
     *
     * 不能写死 `open`：底座要靠它变 false 才挂上 `data-closed`，收起动画才有
     * 得播。关的时候先置 false，等 `onOpenChangeComplete` 再卸载——否则关窗
     * 只会变成「组件被移除」，形变直接消失。
     */
    const [dialogOpen, setDialogOpen] = useState(false);
    /**
     * 正在飞的那一张：卡片一直在格子里占位（`invisible`），飞行体是它的副本。
     *
     * 它要活到收起的形变播完才清，所以和 `opened` 不同步：`opened` 一置空，
     * 形变就进入返程，而副本还要再飞 275ms 才回家。
     */
    const [flight, setFlight] = useState<{
        model: ModelRow;
        rect: MorphRect;
        plan: MorphPlan;
    } | null>(null);
    /** 入场形变是否已经落地。收起要等它落地（决策十：打断一律排队）。 */
    const [entryDone, setEntryDone] = useState(true);
    const pendingClose = useRef(false);
    /** 入场计时的句柄。用 ref：它不该触发重渲染，卸载时也能读到最新值。 */
    const entryTimer = useRef<number | null>(null);

    /**
     * 停掉入场计时。
     *
     * 所有「这次入场已经结束了」的路径都要调它——卸载、换排序/页码、
     * 确认不在榜上收回、收起完成。漏一处就会留下一个过期计时器，它稍后触发时
     * 会**再关一次弹窗**，表现成「刚点开又自己关掉」，而且极难查。
     */
    const clearEntryTimer = useCallback(() => {
        if (entryTimer.current != null) {
            window.clearTimeout(entryTimer.current);
            entryTimer.current = null;
        }
    }, []);

    // 卸载时把计时器清掉，否则回调会打到已卸载的组件上。
    useEffect(() => clearEntryTimer, [clearEntryTimer]);

    const pageCount = Math.max(1, Math.ceil(rows.length / PAGE_SIZE));
    // 数据变短时把页码收回范围内，否则会停在空白页上。
    const safePage = Math.min(page, pageCount - 1);

    // 跟着 `sort` 走，而不是在每个排序回调里各写一遍。
    useEffect(() => {
        setPage(0);
    }, [sort]);

    // 换页 / 换排序后，之前开着的那张可能已经不在这一页了，飞行体也会指向
    // 一条已经不该存在的数据——一起清掉。
    useEffect(() => {
        setOpened(null);
        setDialogOpen(false);
        setFlight(null);
        clearEntryTimer();
        pendingClose.current = false;
    }, [sort, safePage, clearEntryTimer]);

    /**
     * 点卡片：量下它现在的位置，交给形变播入场，同时把弹窗挂出来。
     *
     * 两侧各自计时、各吃一条完整缓动：飞行体跑 `--morph-expand-half` 到侧棱，
     * 弹窗延后同样的时长再跑一半转回来（见 `card-morph.css` 文件头）。
     */
    const openCard = (model: ModelRow, card: HTMLElement | null) => {
        const rect = card?.getBoundingClientRect();
        // 减动效命中、元素没有布局（jsdom）、视口尺寸不合理：直接开弹窗。
        // 飞行层根本不会挂出来，也就没有「等它落地」这回事。
        const plan =
            rect && !prefersReducedMotion()
                ? planMorph(toMorphRect(rect), {
                      width: window.innerWidth,
                      height: window.innerHeight
                  })
                : null;

        if (plan && rect) {
            setFlight({ model, rect: toMorphRect(rect), plan });
            setEntryDone(false);
            pendingClose.current = false;
            // 用定时器而不是 `animationend`：那个事件在动画被取消、元素被隐藏、
            // 或 jsdom 里都不触发，状态机会永远卡在「飞行中」。
            // 动画归 CSS，状态归 JS，两边只共享一个时长常量。
            clearEntryTimer();
            entryTimer.current = window.setTimeout(() => {
                entryTimer.current = null;
                setEntryDone(true);
                if (pendingClose.current) {
                    pendingClose.current = false;
                    setDialogOpen(false);
                }
            }, MORPH_ENTRY_MS);
        } else {
            // 不播形变：入场立刻算完成，关闭不必排队。
            clearEntryTimer();
            setEntryDone(true);
        }
        setOpened(model);
        setDialogOpen(true);
    };

    const pageRows = useMemo(
        () =>
            (models?.models ?? []).slice(
                safePage * PAGE_SIZE,
                (safePage + 1) * PAGE_SIZE
            ),
        // 依赖 `models` 而不是上面那个 `?? []` 出来的 `rows`：后者为空时每次
        // 渲染都是新数组，会让这个 useMemo 形同虚设。
        [models, safePage]
    );

    // 「正在加载」和「这台服务器还没有」要分开：都转圈的话，玩家会一直等一个
    // 永远不会来的东西。
    const showEmpty = rows.length === 0;

    /**
     * 条数说明：这个榜上到底有几个，我们手上有几个。
     *
     * 一次只取 100 条（接口上限），分页器按手上这 100 条算页数——不写这一句，
     * 它等于向玩家断言「这个榜一共就 100 个」。`total` 是服务器 1.19.2 起才有
     * 的字段，老服务器给 `null`，那时整条不渲染：宁可不说，也不能编一个数字。
     *
     * 只在**确实被截断**（总数比手上多）时才说明只列了前几条。榜本来就只有
     * 三条的时候，多一句「只列出前 3」纯属噪音。
     */
    const boardTotal = models?.total ?? null;
    let totalNote: string | null = null;
    if (boardTotal != null && boardTotal > 0) {
        totalNote =
            boardTotal > rows.length
                ? t('vrcc.models.total_capped', {
                      total: boardTotal.toLocaleString(),
                      shown: rows.length.toLocaleString()
                  })
                : t('vrcc.models.total', {
                      total: boardTotal.toLocaleString()
                  });
    }

    return (
        <div className="flex h-full min-h-0 flex-col">
            <div className="text-muted-foreground flex shrink-0 items-center gap-2 px-1 pb-2 text-xs">
                {sortSupported && available && (
                    <ToggleGroup
                        variant="outline"
                        size="sm"
                        value={[sort]}
                        onValueChange={(next) => {
                            const value = next[0];
                            if (isBoardSort(value)) onSortChange(value);
                        }}
                    >
                        {SORTS.map((value, index) => (
                            <Fragment key={value}>
                                {index > 0 ? <ToggleGroupSeparator /> : null}
                                <ToggleGroupItem
                                    value={value}
                                    className="text-xs"
                                >
                                    {t(`vrcc.models.sort.${value}`)}
                                </ToggleGroupItem>
                            </Fragment>
                        ))}
                    </ToggleGroup>
                )}
                {/* 模型榜是「每天投一票」，和地图的「打分」不是一回事，
                    提示文案要分开写，否则玩家会以为这里也能打 1–10 分。 */}
                {available && (
                    <span className="min-w-0 truncate">
                        {t('vrcc.models.hint')}
                    </span>
                )}
                <Button
                    variant="ghost"
                    size="icon"
                    className="ml-auto size-6"
                    onClick={onRefresh}
                    aria-label={t('common.actions.refresh')}
                >
                    <RefreshCw className="size-3" />
                </Button>
            </div>

            {showEmpty ? (
                <div className="text-muted-foreground flex h-32 items-center justify-center gap-2 text-sm">
                    {available ? <Spinner className="size-3" /> : null}
                    {available
                        ? t('vrcc.models.empty')
                        : t('vrcc.models.unavailable')}
                </div>
            ) : (
                <div className="relative min-h-0 flex-1 overflow-x-hidden overflow-y-auto pb-2">
                    {/* `overflow-x-hidden` 不是装饰：跟随指针的标签是绝对定位
                        的，会飘到卡片外面去；而 CSS 规定一个轴设成 `auto` 之后
                        另一轴就不能是 `visible`，于是它会撑出一条横向滚动条。
                        裁掉横轴只是不让标签把滚动条顶出来——它浮在**网格内部**
                        本来就够用了。 */}
                    <ul
                        ref={gridRef}
                        className="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-3"
                    >
                        {pageRows.map((model, index) => {
                            // 本周榜只按本周的喜欢排，`rank` 仍是总榜名次；
                            // 本周的名次是 `place`，没有就按列表位置。
                            const place =
                                sort === 'week'
                                    ? (model.place ??
                                      safePage * PAGE_SIZE + index + 1)
                                    : model.rank;
                            const flying = flight?.model.id === model.id;
                            return (
                                <li
                                    key={model.id}
                                    ref={(element) =>
                                        observe(element, String(model.id))
                                    }
                                    data-tilt=""
                                    // 起飞的那一张仍然占着格子：`invisible`
                                    // 只关可见性、不改布局，所以网格不重排，
                                    // 遮罩后面看到的就是一个洞。
                                    className={
                                        flying
                                            ? 'vrcc-model-card group invisible relative'
                                            : 'vrcc-model-card group relative'
                                    }
                                >
                                    <div className="vrcc-model-card__rotor">
                                        <div className="vrcc-model-card__flip">
                                            <ModelCardFace
                                                model={model}
                                                place={place}
                                                sort={sort}
                                                showCover={visible.has(
                                                    String(model.id)
                                                )}
                                                onOpen={openCard}
                                            />
                                        </div>
                                    </div>

                                    {/* 跟随指针的标签，内容和位置都由
                                        useTiltGrid 那边驱动。名字优先用
                                        完整 BOOTH 原文——卡片正面放的是
                                        简称，这里是它被截掉的那部分。 */}
                                    <span
                                        data-tilt-tip=""
                                        className="vrcc-model-card__tip"
                                        aria-hidden="true"
                                    >
                                        {model.name}
                                    </span>
                                </li>
                            );
                        })}
                    </ul>
                </div>
            )}

            {/* 始终显示，哪怕只有一页（`1`）。单页时每个按钮都自然落在
                `disabled` 上：`safePage === 0` 让「上一页/第一页」灰掉，
                `safePage >= pageCount - 1` 让「下一页/最后一页」灰掉。

                不藏起来是因为**藏了会让版面跳动**：翻到第 5 页、数据变短
                回到 1 页时，底下那条一消失，整块内容往下窜一截。留着它、
                让它全程占位，翻页时视线不用重新定位。 */}
            <nav className="flex shrink-0 items-center justify-end gap-0.5 pt-2">
                {/* 条数说明靠左，翻页键留在右边。`mr-auto` 把它顶到行的另一头：
                    用 `justify-between` 的话，这条不渲染时（老服务器没有 `total`）
                    按钮会被留在左边。 */}
                {totalNote ? (
                    <span className="text-muted-foreground mr-auto truncate pr-2 text-xs tabular-nums">
                        {totalNote}
                    </span>
                ) : null}
                <Button
                    variant="ghost"
                    size="sm"
                    className="size-6 p-0"
                    disabled={safePage === 0}
                    aria-label={t('table.pagination.page_number', {
                        page: 1
                    })}
                    onClick={() => setPage(0)}
                >
                    <ChevronsLeft className="size-3.5" />
                </Button>
                <Button
                    variant="ghost"
                    size="sm"
                    className="size-6 p-0"
                    disabled={safePage === 0}
                    aria-label={t('table.pagination.previous')}
                    onClick={() => setPage(safePage - 1)}
                >
                    <ChevronLeft className="size-3.5" />
                </Button>
                {buildPageItems(safePage + 1, pageCount).map((item) =>
                    item.kind === 'gap' ? (
                        <span
                            key={item.key}
                            aria-hidden="true"
                            className="text-muted-foreground w-4 text-center text-xs"
                        >
                            …
                        </span>
                    ) : (
                        <Button
                            key={item.page}
                            variant={
                                item.page === safePage + 1
                                    ? 'secondary'
                                    : 'ghost'
                            }
                            size="sm"
                            className="h-6 min-w-6 px-1 text-xs tabular-nums"
                            aria-current={
                                item.page === safePage + 1 ? 'page' : undefined
                            }
                            aria-label={t('table.pagination.page_number', {
                                page: item.page
                            })}
                            onClick={() => setPage(item.page - 1)}
                        >
                            {item.page}
                        </Button>
                    )
                )}
                <Button
                    variant="ghost"
                    size="sm"
                    className="size-6 p-0"
                    disabled={safePage >= pageCount - 1}
                    aria-label={t('table.pagination.next')}
                    onClick={() => setPage(safePage + 1)}
                >
                    <ChevronRight className="size-3.5" />
                </Button>
                <Button
                    variant="ghost"
                    size="sm"
                    className="size-6 p-0"
                    disabled={safePage >= pageCount - 1}
                    aria-label={t('table.pagination.page_number', {
                        page: pageCount
                    })}
                    onClick={() => setPage(pageCount - 1)}
                >
                    <ChevronsRight className="size-3.5" />
                </Button>
            </nav>

            {/* 飞行层。卡片副本从这里出发，一路移到中点并转到侧棱；遮罩在它
                后面（`z-50`），所以看上去是「网格里那一格被抽走了」。 */}
            {flight &&
                createPortal(
                    <div
                        className="vrcc-morph-flight"
                        // 弹窗一进入关闭流程，副本就开始准备返程：先等弹窗
                        // 反向走回侧棱（`--morph-collapse-half`），再从那里飞回来。
                        data-closing={dialogOpen ? undefined : ''}
                        style={{
                            left: flight.rect.left,
                            top: flight.rect.top,
                            width: flight.rect.width,
                            height: flight.rect.height,
                            '--morph-card-dx': `${flight.plan.cardDx}px`,
                            '--morph-card-dy': `${flight.plan.cardDy}px`
                        }}
                    >
                        <div className="vrcc-morph-flight__card">
                            <ModelCardFace
                                model={flight.model}
                                place={
                                    sort === 'week'
                                        ? flight.model.place
                                        : flight.model.rank
                                }
                                sort={sort}
                                showCover
                                // 副本不接受点击：飞行期间它只是画面。
                                onOpen={() => {}}
                            />
                        </div>
                    </div>,
                    document.body
                )}

            {opened && (
                <VrccModelDialog
                    model={opened}
                    morph={flight?.plan ?? null}
                    open={dialogOpen}
                    onOpenChange={(next) => {
                        if (next) return;
                        // 还在飞就先排队：让它落地再收，不在半空中掉头
                        // （决策十：打断一律排队，只有 404 例外）。
                        if (!entryDone) {
                            pendingClose.current = true;
                            return;
                        }
                        setDialogOpen(false);
                    }}
                    onSettled={() => {
                        // 收起动画播完：卸载弹窗，洞补上、副本归位。
                        clearEntryTimer();
                        setOpened(null);
                        setDialogOpen(false);
                        setFlight(null);
                        setEntryDone(true);
                    }}
                    onGone={() => {
                        // 还在飞就被告知「不在榜上」：当场收回，不飞完
                        // （决策十三）。落地之后才收到的话，照常显示那句解释。
                        if (entryDone) return;
                        clearEntryTimer();
                        setOpened(null);
                        setDialogOpen(false);
                        setFlight(null);
                        setEntryDone(true);
                    }}
                />
            )}
        </div>
    );
}
