// SPDX-License-Identifier: MPL-2.0

import { beforeEach, describe, expect, it } from 'vitest';

import {
    cachedModelDetail,
    rememberModelDetail,
    resetModelDetailCache
} from './useModelDetail';
import type { VrccModelDetail } from './vrccApi';

function detail(id: number): VrccModelDetail {
    return {
        model: { id, name: `モデル${id}`, likes: 1 },
        hots: [],
        at: 1
    } as VrccModelDetail;
}

/**
 * 会话详情缓存的行为。
 *
 * 这个模块的状态是**模块级**的（对话框每次关闭都会卸载，放进 hook 就等于
 * 每次重挂都清空），所以每个用例开头都要重置，否则用例之间互相污染。
 */
describe('useModelDetail 的会话缓存', () => {
    beforeEach(() => {
        resetModelDetailCache();
    });

    it('记住之后能取回同一份', () => {
        expect(cachedModelDetail(1)).toBeNull();

        rememberModelDetail(1, detail(1));

        expect(cachedModelDetail(1)?.model?.id).toBe(1);
    });

    it('按 id 分开，不串号', () => {
        rememberModelDetail(1, detail(1));
        rememberModelDetail(2, detail(2));

        expect(cachedModelDetail(1)?.model?.id).toBe(1);
        expect(cachedModelDetail(2)?.model?.id).toBe(2);
        expect(cachedModelDetail(3)).toBeNull();
    });

    it('超过上限淘汰最旧的一批，而不是整表清空', () => {
        // 整表清空的后果是「刚看过的一屏模型」一起失效，下次点开又满屏转圈。
        for (let id = 0; id < 100; id += 1) {
            rememberModelDetail(id, detail(id));
        }

        rememberModelDetail(100, detail(100));

        expect(cachedModelDetail(100)).not.toBeNull();
        expect(cachedModelDetail(0)).toBeNull();
        // 关键：只淘汰最旧的四分之一，新的那批还在。
        expect(cachedModelDetail(99)).not.toBeNull();
    });

    it('上限之内的那些一个都不丢，不是只记得最后几个', () => {
        // 上面那条只断言「最新几个还在」。把上限从 100 改成 3，它照样全绿——
        // 淘汰逻辑没坏，坏的是容量。所以这里钉的是**容量本身**：一百个都还在，
        // 只是最旧的那四分之一没了。
        for (let id = 0; id < 100; id += 1) {
            rememberModelDetail(id, detail(id));
        }

        rememberModelDetail(100, detail(100));

        // 淘汰 0–24，25 到 100 全部保留。
        expect(cachedModelDetail(24)).toBeNull();
        expect(cachedModelDetail(25)).not.toBeNull();
        expect(cachedModelDetail(70)).not.toBeNull();
    });

    it('重置之后一切归零', () => {
        rememberModelDetail(1, detail(1));
        resetModelDetailCache();

        expect(cachedModelDetail(1)).toBeNull();
    });
});
