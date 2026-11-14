// SPDX-License-Identifier: MPL-2.0

import { useEffect, useState } from 'react';

import { vrccApi, type VrccModelDetail } from './vrccApi';

/**
 * 会话内的小缓存：BOOTH 编号 → 详情。
 *
 * 后端也有缓存（5 分钟），这一层不是重复——它省掉的是**一次 IPC 往返加一次
 * 可能的状态流转**。玩家点开、关掉、再点开同一条是很常见的动作，每次都先
 * 转一下圈再出内容，手感是「慢」而不是「快」。
 *
 * 只缓存成功的那份。`null`（不在榜上）不缓存：下架和重新上架都会发生，
 * 把「没有」记住会让玩家以为它消失了。
 *
 * 上限满了按插入顺序淘汰最旧的一批，而不是整表清空，和封面缓存
 * （`useModelCoverCache`）同一套做法。整表清空的后果是「刚看过的一屏模型」
 * 一起失效，下一次点开又是满屏转圈。
 *
 * ## 换账号必须清空
 *
 * **不能假设「模型详情永远与登录无关」。** 今天 `/models/{id}` 是公开数据，
 * 但权限可以再申请、接口也可能开始返回「你投过票没有」这类和登录态绑定的
 * 字段。那天到来时，一个跨账号共享的缓存会把 A 的详情给 B 看。
 *
 * 后端那边已经把账号分区写进了缓存键（`OwnedModelKey`）；前端这层只在内存里
 * 活一个会话，所以用更省事的等价做法：**登录态一变就整表清掉**。
 * [`useVrccController`] 在 `connected` / `star` 变化时调 [`resetModelDetailCache`]。
 *
 * ## 状态为什么在模块里
 *
 * 对话框每次关闭都会卸载。状态放进 hook 就等于每次重挂都清空，这层缓存也就
 * 没意义了。代价是它跨测试用例共享——所以导出 [`resetModelDetailCache`]，
 * 用到本模块的测试文件必须在 `beforeEach` 里调它，否则用例之间会互相污染。
 */
const detailCache = new Map<number, VrccModelDetail>();
const DETAIL_CACHE_MAX = 100;

/** 一次淘汰掉上限的这个比例。`Map` 迭代即插入序，不需要额外维护链表。 */
const EVICT_RATIO = 4;

/** 测试用：清空会话缓存。模块级状态必须可重置。 */
export function resetModelDetailCache(): void {
    detailCache.clear();
}

/** 缓存里那一份；没有就是 `null`。 */
export function cachedModelDetail(id: number): VrccModelDetail | null {
    return detailCache.get(id) ?? null;
}

/** 记下一份详情，超量时淘汰最旧的一批。只该在**成功**之后调用。 */
export function rememberModelDetail(id: number, detail: VrccModelDetail): void {
    if (detailCache.size >= DETAIL_CACHE_MAX) {
        const drop = Math.ceil(DETAIL_CACHE_MAX / EVICT_RATIO);
        let left = drop;
        for (const oldest of detailCache.keys()) {
            if (left <= 0) break;
            detailCache.delete(oldest);
            left -= 1;
        }
    }
    detailCache.set(id, detail);
}

export type ModelDetailState = 'loading' | 'ready' | 'gone' | 'error';

/**
 * 取单个模型的详情（含最多 2 条热评）。
 *
 * 四种状态的语义：
 *
 * - `loading`：请求在飞；
 * - `ready`：拿到了。`hots` 可能为空，那是「还没人评论」，不是失败；
 * - `gone`：后端把 `404 not_found` 收敛成 `null`，表示这个模型**已不在榜上**
 *   （下架或未过审）。这是正常状态，不是网络故障，调用方要给解释而不是弹错；
 * - `error`：其它失败，可重试。
 */
export function useModelDetail(modelId: number): {
    state: ModelDetailState;
    detail: VrccModelDetail | null;
} {
    const [detail, setDetail] = useState<VrccModelDetail | null>(() =>
        cachedModelDetail(modelId)
    );
    const [state, setState] = useState<ModelDetailState>(() =>
        cachedModelDetail(modelId) ? 'ready' : 'loading'
    );

    useEffect(() => {
        // 先在本地缓存里找：玩家常常点开、关掉、再点开同一条，
        // 那不该每次都转圈等一下。
        const hit = cachedModelDetail(modelId);
        if (hit) {
            setDetail(hit);
            setState('ready');
            return;
        }

        let alive = true;
        setDetail(null);
        setState('loading');

        void vrccApi
            .model(modelId)
            .then((result) => {
                if (!alive) return;
                // `null` = 后端把 `404 not_found` 收敛成了「不在榜上」。
                if (result == null) {
                    setState('gone');
                    return;
                }
                rememberModelDetail(modelId, result);
                setDetail(result);
                setState('ready');
            })
            .catch((error: unknown) => {
                if (!alive) return;
                console.warn('[vrcc] model detail failed:', error);
                setState('error');
            });

        return () => {
            alive = false;
        };
    }, [modelId]);

    return { state, detail };
}
