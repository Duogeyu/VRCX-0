# 交接文档（HANDOFF）

> 分支：`feat/wrist-web-overlay`（**不是 master**）
> 基线：`master` @ `0386fa6`，VRCX-0 `2.32.0`
> 日期：2026-10-25
> 详细版：**[`README-HANDOFF.zh-CN.md`](./README-HANDOFF.zh-CN.md)**（含完整证据链、代码结构、配置表、构建环境坑）
> 设计文档：[`docs/WEB_OVERLAY.zh-CN.md`](./docs/WEB_OVERLAY.zh-CN.md)（部分内容已过时，以 README-HANDOFF 为准）

---

## 一句话

把 VRCX-0 原本显示静态信息的**手腕叠加层**换成**真实网页**（WebView2 离屏渲染），支持点击 / 滚动 / 键盘，
内容以插件形式存在、改文件即热更新；并附带一个基于 [星遇同频开放接口](https://docs.vrcc.cc/) 的腕表面板。

## 先跑起来（1 分钟）

```
<部署目录>\vrcx-0.exe        ← 双击
```

⚠️ **必须放在纯英文路径下**。放在中文路径（如 `<含中文的目录>`）会启动即崩
（事件日志 `0xc0000409`），同一个 exe 换到 ASCII 路径就正常。原因与修复建议见
README-HANDOFF **§2.1**（要改的是 `crates/platform/src/app_paths.rs` 的解析降级逻辑）。

数据目录与程序位置无关：`%APPDATA%\VRCX-0\`（`VRCX-0.sqlite3` 的 `configs` 表）。

## 从源码构建

```bat
call <你的构建环境脚本>          :: 本机环境坑都固化在这里（README-HANDOFF §9）
npm install && npm run build                        :: 前端必须先构建
cargo build --release -p vrcx-0 --features custom-protocol
```

`--features custom-protocol` **不能省**，否则 release 版会去连 `http://localhost:9000` → 界面「localhost 拒绝连接」。

## 这个分支改了什么

**新增**

| 路径 | 作用 |
| --- | --- |
| `crates/web-surface/` | 新 crate：WebView2 离屏窗口 → `CapturePreview` → RGBA；CDP 输入（鼠标/滚轮/键盘/文本）；注入可见光标；内置静态服务器；插件清单；两个自测示例 |
| `crates/overlay-runtime/src/web_overlay.rs` | 网页模式配置（8 个 config 键） |
| `crates/host-desktop/src/vr_overlay/pointer_ray.rs` | 控制器射线 → 面板坐标的数学（含 4 个单测） |
| `plugins/vrcc/` | 内置「星遇同频」腕表面板（地图榜 / 提醒 / 设备授权登录） |

**主要修改**

| 文件 | 改动 |
| --- | --- |
| `overlay-runtime/src/runtime.rs` | 生产者 web 分支、占位帧、热重载（mtime）、刷新间隔跟随配置 |
| `overlay-runtime/src/{service,manager}.rs` | 输入事件透传 `take_input_events` |
| `host-desktop/src/vr_overlay/policy.rs` | **按一下开、再按一下关**（原来按住 + 10 秒自动关闭） |
| `host-desktop/src/vr_overlay/openvr_backend.rs` | 射线光标、摇杆按下点击、边缘滚动、交互标志、叠加层鼠标事件、高频 tick、上传间隔取配置 |
| `host-desktop/src/vr_overlay/{types,mod}.rs` | `min_upload_interval_ms`、`OverlayInputEvent` |
| `src-tauri/Cargo.toml` | 新增 `custom-protocol` feature |
| 根 `Cargo.toml` | workspace 加入 `crates/web-surface` |

## 已经验证过的

- 离屏渲染出帧（截图为 vrcc.cc 官网与内置面板，640×640，真实 API 数据）
- 点击（模拟点「我的」→ 页面切到登录页）、滚动（列表 1–11 → 11–22 行）、键盘（输入框写入 `hello 星遇`）、热重载（改文件 → 标题变「已更新 v2」）
- `cargo check --workspace` 通过；`cargo test`（web-surface + overlay-runtime + host-desktop）**174 项通过**
- 应用实机启动并拉起叠加层：日志 `VR overlay service started backend="openvr"` →
  `web overlay: surface started plugin=vrcc` → `web overlay: page loading`

## 还没做完（按优先级）

1. **中文路径启动崩溃** → `crates/platform/src/app_paths.rs`（README-HANDOFF §2.1 有完整证据）
2. **「星遇同频」原生页面**（VRCX-0 主界面里还没有）：路由 `src/app/routes.tsx`、导航
   `src/shared/constants/ui.ts` 的 `navDefinitions`、页面放 `src/features/vrcc/`；
   **一键加好友**直接用现成 Tauri 命令 `appSocialFriendRequestSend`；
   星轨相遇 / 互相连上 / 最近提醒 / 当前地图评分走 `https://vrcc.cc/api/v1/`（设备授权那套已在网页面板里实现过）
3. **VR 输入隔离只做了一半**：光标已改成射线（不碰摇杆 → 人物不走），点击用摇杆按下
   （VRChat 里是"跑步"，站着无副作用），滚动用面板边缘。但 VRChat 仍会收到摇杆按下 —— OpenVR
   公共接口无法让背景叠加层"消费"手柄输入。可选方向见 README-HANDOFF §2.2
4. **跨平台**：`crates/web-surface` 目前只有 Windows 实现，其它平台 `WebSurface::spawn` 直接返回错误并回落原 Slint 面板

## 为什么不能直接跑官方那个 VRCX 插件脚本

`xingyu-tongpin-vrcx` 的 `vrcx.js` 是 **VRCX 1.x（Electron 时代）用户脚本**，依赖
`window.AppApi.OpenLink`、`window.electron.desktopNotification`，并把自己**注入 VRCX 左侧导航的 DOM**。
VRCX-0 是 Tauri + React 重写：没有这套桥，DOM 注入也会被 React 重渲染冲掉。
所以走"原生页面"（上面第 2 条）是正路；要做字面兼容，需要额外写 AppApi/electron 垫片 + 独立脚本宿主容器。

## 调试命令

```bat
cargo run -p vrcx-0-web-surface --example capture -- https://vrcc.cc/
cargo run -p vrcx-0-web-surface --example capture -- --plugin %LOCALAPPDATA%\vrcx-0\overlay-plugins\vrcc 320,88
cargo run -p vrcx-0-web-surface --example input_probe
```

截图落在 `<输出目录>\web-surface-capture.png` / `<输出目录>\probe-*.png`。

## 许可

本分支基于 VRCX-0（GPL-3.0），同样以 **GPL-3.0** 分发。`plugins/vrcc` 调用的「星遇同频」接口与名称归 vrcc.cc 所有，
请遵守其[使用要求](https://docs.vrcc.cc/)（写明是第三方客户端，不要用其名称/图标作自己的品牌）。
