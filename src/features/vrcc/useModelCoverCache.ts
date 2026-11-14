// SPDX-License-Identifier: MPL-2.0

import { useEffect, useState } from 'react';

/**
 * 模型封面的内存缓存。
 *
 * ## 为什么需要它
 *
 * 模型封面来自 BOOTH 的 CDN，和地图封面（有本地 SQLite + VRChat 记录）不是
 * 一条路。没有这层缓存时，**每翻一次页、每切一次排序，二十张封面全部重新
 * 走一遍网络**——因为列表被卸载重建，`<img>` 也是新的 DOM 节点。
 *
 * 浏览器磁盘缓存会兜住一部分（BOOTH 也会给 `max-age`），但那要等它命中，
 * 期间是空白；而且换排序再切回来时，玩家看到的就是一屏「加载中」。
 *
 * ## 只留 URL，不留图片数据
 *
 * 缓存的是「这个 img 地址我们成功加载过」，不是图片本身。图片在内存里的
 * 那份由浏览器管——我们只要在重新挂载时**立刻给同一个 URL**，浏览器就会从
 * 自己的缓存里出图，不会重新下载。自己再存一份 `Blob` 只会双倍占内存。
 *
 * ## 换账号要清空
 *
 * 封面是 BOOTH 的公开图，本身不含账号信息。但**这条约束不该靠「图片是公开的」
 * 来维持**：只要详情里以后出现任何与登录态绑定的内容，一个跨账号共享的缓存
 * 就会成为串数据的通道。`useVrccController` 在 `star` 变化时一并清掉它。
 *
 * ## 上限与淘汰
 *
 * 一页 24 张，翻十页也就 240 个 URL。超过上限时**按插入顺序淘汰最旧的一批**，
 * 而不是整表清空。
 *
 * 整表清空的代价被这个页面的结构放大了：一页 24 张卡会同时挂载、同时预载，
 * 第 501 个 URL 进来的那一刻若把整表清掉，玩家翻回上一页时就是 **24 张封面
 * 同一帧全部重新预载**——正好在「已经翻了很多页、网络本来就忙」的时候打一个
 * 请求尖峰，和这层缓存「别让玩家看到加载中」的初衷正好相反。
 *
 * `Set` 的迭代顺序就是插入顺序，所以淘汰最旧的那批不需要额外维护链表。
 */
const loaded = new Set<string>();
const MAX_ENTRIES = 500;

/** 一次淘汰掉上限的这个比例，避免每加一条都淘汰一次。 */
const EVICT_RATIO = 4;

/**
 * 测试用：清空封面缓存。
 *
 * 模块级状态跨用例共享，不重置就只能靠「每个用例用不同的 URL」绕，那种隐性
 * 约定一旦有人复用 URL 就会产生极难查的失败。
 */
export function resetModelCoverCache(): void {
    loaded.clear();
}

/** 这张封面确认可以显示过。 */
export function rememberModelCover(url: string): void {
    if (loaded.size >= MAX_ENTRIES) {
        // 按插入顺序淘汰最旧的一批：`Set` 迭代即插入序。
        const drop = Math.ceil(MAX_ENTRIES / EVICT_RATIO);
        let left = drop;
        for (const oldest of loaded) {
            if (left <= 0) break;
            loaded.delete(oldest);
            left -= 1;
        }
    }
    loaded.add(url);
}

/** 这张封面之前成功加载过吗。 */
export function hasModelCover(url: string): boolean {
    return loaded.has(url);
}

/**
 * 只在「首次见到这张封面」时才渲染 `<img>`，其余情况直接复用已知可用的地址。
 *
 * 实际上浏览器对同一个 URL 的重复 `<img>` 会走它自己的缓存，所以这层的主要
 * 作用是**避免可见性切换时先闪一下空白**：`VrccModelsPanel` 只在行可见时挂
 * 图，来回滚动会让同一张图反复挂载卸载；记住它之后，重新挂载的那一刻就知道
 * 「这张能显示」，不必再等一次 `onLoad`。
 */
export function useModelCoverReady(src: string | null): boolean {
    const [ready, setReady] = useState(() =>
        src ? hasModelCover(src) : false
    );

    useEffect(() => {
        if (!src) {
            setReady(false);
            return;
        }
        const known = hasModelCover(src);
        setReady(known);
        if (known) return;

        // 预加载一次：成功就记下来，下次直接 ready。
        let alive = true;
        const image = new Image();
        image.onload = () => {
            if (!alive) return;
            rememberModelCover(src);
            setReady(true);
        };
        // 失败保持 false，交给 `VrccCover` 的 onError 去收起那一块。
        image.onerror = () => {
            if (!alive) return;
            setReady(false);
        };
        image.src = src;

        return () => {
            alive = false;
        };
    }, [src]);

    return ready;
}
