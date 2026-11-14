// SPDX-License-Identifier: MPL-2.0

/** 星遇把社团简介/公告切成若干段，每段是一个 `["文字", 0|1]`；后端已经转成对象。 */
export type ClubTextSeg = { text?: string | null; hidden?: boolean };

/**
 * 把分段拼回一整段文字。
 *
 * 联系方式（`hidden: true` 的那些段）照常显示、不打码——同屏的「描述」「规则」
 * 「动态」都是原样铺开，只遮一段会让玩家以为那是另一类内容。
 */
export function joinSegments(segs: ClubTextSeg[] | null | undefined): string {
    if (!Array.isArray(segs)) {
        return '';
    }
    const parts: string[] = [];
    for (const seg of segs) {
        const text = typeof seg?.text === 'string' ? seg.text : '';
        if (text) {
            parts.push(text);
        }
    }
    return parts.join('').trim();
}
