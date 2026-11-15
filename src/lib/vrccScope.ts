// SPDX-License-Identifier: MPL-2.0

/**
 * 令牌的 scope 串里有没有某一项权限。
 *
 * ## 为什么单独一个纯函数
 *
 * 同一个判断有两个消费方，语义**故意不同**：
 *
 * - `VrccPage` 判断「要不要显示星轨相交这个入口」——不知道就不显示；
 * - `repositories/vrccMapRepository` 判断「要不要给玩家看打分按钮」——
 *   同样是不知道就不给（那边用 `useVrccConnection` 读连接状态）。
 *
 * 两者都比契约层的 `scope_grants` **严格**。那个是 Rust 侧决定「该不该替
 * 服务器先把请求拦下来」：不知道就放行（`None => true`），交给服务器判——
 * 多问一次最多吃一个 403，少问一次却可能把请求发到一个本来能用的地方。
 * 界面上的判断反过来：多给一个按钮会让玩家点下去必然失败，少给只是让他
 * 重新连一次。方向不同，不能共用一个默认值。
 *
 * ## 分隔符
 *
 * 官方用空格分开，也认逗号和 `+`（`profile+notifications` 这种写法在
 * OAuth 系里很常见）。三种都认，和 Rust 侧保持一致。
 */
export function scopeGrants(
    scope: string | null | undefined,
    needed: string
): boolean {
    if (!scope) {
        // 未连接、或是旧版本没记 scope。严格：当作没有。
        return false;
    }
    return scope
        .split(/[\s,+]+/)
        .map((part) => part.trim())
        .includes(needed);
}
