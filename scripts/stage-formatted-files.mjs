import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { extname } from 'node:path';

const formattedExtensions = new Set([
    '.cjs',
    '.css',
    '.cts',
    '.html',
    '.js',
    '.json',
    '.jsonc',
    '.jsx',
    '.md',
    '.mdx',
    '.mjs',
    '.mts',
    '.rs',
    '.ts',
    '.tsx',
    '.yaml',
    '.yml'
]);

/**
 * 把「格式化命令改过的文件」补进暂存区。
 *
 * 为什么需要这一步：`lint-staged` 只回写它自己跑出来的任务产物；而本仓库的
 * 钩子调用的是全量命令（`oxfmt .` / `cargo fmt --all`），它们会把暂存区里的
 * 文件在工作区再写一遍，`lint-staged` 不认这些改动，于是提交里留着**格式化前**
 * 的版本。这一步就是把它们补回去。
 *
 * ## 候选集只来自「本来就已经暂存」的文件
 *
 * 早先用的是 `git ls-files --modified --others --exclude-standard`，那等于把
 * **未跟踪文件**也当成候选：只要工作区里躺着一个 `.json`／`.md`／`.ts`，任何
 * 一次提交都会把它顺手 `git add` 进来（实测：工作区没暂存任何东西，跑一次这个
 * 脚本就凭空多出一份暂存）。`.agent-teams/` 的运行态账本就是这么进过一次提交的。
 *
 * 现在改成两步求交：
 *
 * 1. `git diff --cached --name-only`：这一次提交真正要包含的文件；
 * 2. `git ls-files --modified`：工作区与索引不一致的文件。
 *
 * 只有「已暂存 且 被格式化改过」的交集才重新入索引。于是：
 *
 * - 未跟踪文件永远不会被带进提交（哪怕扩展名在格式化白名单里）；
 * - 开发者故意没暂存的 WIP 改动也不会被拖走；
 * - 格式化没有实际改动时是空操作。
 */
const stagedResult = spawnSync(
    'git',
    ['diff', '--cached', '--name-only', '-z'],
    {
        encoding: 'buffer'
    }
);

if (stagedResult.status !== 0) {
    process.stderr.write(stagedResult.stderr);
    process.exit(stagedResult.status ?? 1);
}

const modifiedResult = spawnSync('git', ['ls-files', '--modified', '-z'], {
    encoding: 'buffer'
});

if (modifiedResult.status !== 0) {
    process.stderr.write(modifiedResult.stderr);
    process.exit(modifiedResult.status ?? 1);
}

const splitNul = (buffer) =>
    buffer
        .toString('utf8')
        .split('\0')
        .filter((filePath) => filePath.length > 0);

const modifiedFiles = new Set(splitNul(modifiedResult.stdout));

const formattedFiles = splitNul(stagedResult.stdout).filter(
    (filePath) =>
        modifiedFiles.has(filePath) &&
        existsSync(filePath) &&
        !filePath.replaceAll('\\', '/').startsWith('signatures/') &&
        formattedExtensions.has(extname(filePath).toLowerCase())
);

if (formattedFiles.length > 0) {
    const addResult = spawnSync(
        'git',
        ['add', '--pathspec-from-file=-', '--pathspec-file-nul'],
        { input: `${formattedFiles.join('\0')}\0` }
    );

    if (addResult.status !== 0) {
        process.stderr.write(addResult.stderr);
        process.exit(addResult.status ?? 1);
    }
}
