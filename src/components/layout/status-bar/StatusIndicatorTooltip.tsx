// SPDX-License-Identifier: MPL-2.0

/**
 * 状态栏里「VRChat 服务器」和「星遇状态」两段共用的提示布局：
 * 首行结论，之后是灰色次要行。两段必须长得一样，所以抽成一个组件。
 */
export function StatusIndicatorTooltip({
    indicator
}: {
    indicator: { headline: string; details: string[] };
}) {
    return (
        <div className="flex flex-col gap-1 text-xs">
            <span>{indicator.headline}</span>
            {indicator.details.map((line) => (
                <span key={line} className="text-muted-foreground break-all">
                    {line}
                </span>
            ))}
        </div>
    );
}
