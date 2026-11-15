// SPDX-License-Identifier: MPL-2.0

/**
 * 把服务端给的「还有几秒重置」写成人话。
 *
 * 这个数直接来自 `RateLimit-Reset`，量的就是到北京时间的零点，**不要**拿本地
 * 时区去换算成具体时刻——那会在非 UTC+8 的机器上显示出错误的时间。
 *
 * 先整体向上取整到分钟，再拆成小时和分钟。早先是拆完再各自取整，余下不到
 * 一小时的部分一旦超过 59 分，就会写成「1h 60m」（7199 秒）或「60m」（3599 秒）。
 */
export function formatReset(secs: number): string {
    const totalMinutes = Math.max(
        1,
        Math.ceil(Math.max(0, Math.floor(secs)) / 60)
    );
    const hours = Math.floor(totalMinutes / 60);
    const minutes = totalMinutes % 60;
    if (hours > 0) {
        return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
    }
    return `${minutes}m`;
}
