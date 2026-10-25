# VRCX-0 腕表网页 Overlay + 星遇同频面板

这是对 [VRCX-0](https://github.com/Map1en/VRCX-0)（Tauri 2 + Rust + React，GPL-3.0）的一份**功能扩展分支**：
把原本显示静态信息的手腕叠加层，换成**真实网页**（WebView2 离屏渲染），支持点击 / 滚动 / 键盘输入，
内容以「插件」形式存在、改文件即热更新；并自带一个基于
[星遇同频开放接口](https://docs.vrcc.cc/) 的腕表面板。

> 交接说明：这份 README 是给接手的人看的。先读 **§2 已知问题**，那里有几个坑必须知道。

---

## 1. 快速开始

### 1.1 直接运行（推荐位置：纯英文路径）

```
<部署目录>\vrcx-0.exe        ← 双击运行
```

**⚠️ 必须放在纯英文路径下**（例如 `<部署目录>\`）。
放在中文路径（如 `<源码目录>\`）会启动即崩（事件日志 `0xc0000409`）或静默退出，原因见 §2.1。

数据目录不受位置影响，仍然在 `%APPDATA%\VRCX-0\`：
配置表 `VRCX-0.sqlite3` 的 `configs(key,value)`，登录态、好友缓存等都在这里，换位置不用重新登录。

### 1.2 从源码构建

```bat
:: 环境脚本（本机已经踩过的坑都固化在这里，见 §6）
call <你的构建环境脚本>

cd /d <源码目录>
npm install          :: 只需一次
npm run build        :: 构建前端（必须先做，dist/ 会被打进 exe）
cargo build --release -p vrcx-0 --features custom-protocol
```

产物：`<target>\release\vrcx-0.exe`（约 39.5 MB）。

> `--features custom-protocol` **不能省**。少了它，release 版会把自己当开发模式，去连
> `http://localhost:9000`（Vite 开发服务器），界面显示「localhost 拒绝连接」。

---

## 2. 已知问题（接手必读）

### 2.1 中文路径启动崩溃（0xc0000409）

- **现象**：exe 放在含中文的目录（如 `<源码目录>\`）时，双击后进程消失，
  事件查看器记录 `异常代码 0xc0000409`（`STATUS_STACK_BUFFER_OVERRUN` / fail-fast），
  没有留下 panic 日志（说明不是 Rust panic，或发生在日志初始化之前）。
- **已确认**：*同一个 exe* 放在 `<临时目录>\`、`<纯英文路径>\`、`<部署目录>\` 都能正常启动并运行；
  放在 `<含中文的目录>`、`<含中文的目录>` 都失败。所以是**路径**，不是构建。
- **已验证的更早现象**：用仓库里 `crates/platform/examples/resolve_app_dir.rs` 做对照实验，
  exe 在中文路径时 `resolve_app_data_dir()` 返回 `IO error: 拒绝访问 (os error 5)`；
  同一份 exe 挪到 ASCII 路径就返回正常结果。失败点落在
  `crates/platform/src/app_paths.rs::validate_app_data_dir_for_mode`
  （`create_dir_all` / `canonicalize` / 写探测文件 中的某一步），具体是哪一步还没定位。
- **建议的下一步**：在 `resolve_app_data_dir()` 里加**降级路径**——`dirs::config_dir()` 或
  `canonicalize` 失败时退回 `%APPDATA%\VRCX-0`（环境变量拼），并把探测文件失败降级为警告而不是错误；
  或者直接查清 `SHGetKnownFolderPath` 在该场景下的返回值。修好后中文路径即可恢复。
- **临时规避**：程序放 ASCII 路径（本包已放在 `<部署目录>\`）。

### 2.2 VR 里的输入隔离只做了一半

- 目标：操作腕表面板时，**VRChat 不应该同时响应**。
- 现状：
  - 光标已改为**控制器射线**（`pointer_ray.rs`），**不再用摇杆移动光标** → 人物不会因为移动光标而走动；
  - 点击用**摇杆按下**（VRChat 里是"跑步"，站着不动时无副作用），扳机完全留给游戏；
  - 滚动用**指到面板上下边缘**（不占按键）。
  - 也就是说：**用摇杆按下点击时，VRChat 仍会收到"跑步"**。这是 OpenVR 公共接口的硬限制：
    背景叠加层无法"消费"手柄输入（`MakeOverlaysInteractiveIfVisible` 只让叠加层可被 dashboard
    激光指向，实测在背景叠加层上不产生激光，也不隔离输入）。
- **可能的突破方向**（未验证）：
  1. 让面板变成 SteamVR dashboard 类叠加层（没有公开 API）；
  2. 与 VRChat 的 OSC/输入系统配合（OSC 只发不收，无法屏蔽）；
  3. 只读取 **不占用手柄按键** 的输入源：头显注视（gaze）+ 语音/手势触发点击。
- 代码位置：`crates/host-desktop/src/vr_overlay/openvr_backend.rs::poll_web_pointer`。

### 2.3 「星遇同频」原生页面**还没做**

- 目前 VRCX-0 主界面里没有「星遇同频」页面；已完成的是**腕表上的网页面板**
  （`plugins/vrcc/index.html`：地图榜 / 提醒 / 我的，含设备授权登录）。
- 计划中的原生页面（功能对齐 [vrcx.js](https://github.com/vrcc-cc/xingyu-tongpin-vrcx)）：
  连接码/设备授权、最近提醒、星轨相遇、互相连上、当前地图的地图榜信息与打分短评、
  **一键加好友**（走已存在的 Tauri 命令 `appSocialFriendRequestSend`）。
- 为什么不能直接跑那个插件脚本：`vrcx.js` 是 **VRCX 1.x（Electron 时代）用户脚本**，
  依赖 `window.AppApi.OpenLink`、`window.electron.desktopNotification`，并**把自己注入 VRCX 的左侧导航 DOM**；
  VRCX-0 是 Tauri + React 重写，没有这套桥，DOM 注入也会被 React 重渲染冲掉。
  要么写原生页面（推荐），要么做一层 AppApi/electron 垫片 + 独立脚本宿主容器。

### 2.4 其他

- 页面刷新率默认 250ms（4fps），CPU 占用会随刷新率线性上升；`wristOverlayWebRefreshMs` 可调（33–5000）。
- `CapturePreview` 在 WebView2 刚创建时必然先失败一次
  （`HRESULT 0x8007139F 组或资源的状态不是执行请求操作的正确状态`），属正常：下一帧就好。
- 只支持 Windows（WebView2）。非 Windows 上 `WebSurface::spawn` 直接返回错误，腕表回落原来的 Slint 面板。
- 网页里的 `window.open` 会跳到系统浏览器（VRCC 登录链接就是这样打开的）。

---

## 3. 已经做完的功能

| 功能 | 实现位置 | 验证方式 |
| --- | --- | --- |
| WebView2 离屏渲染 → RGBA 帧 | `crates/web-surface/src/engine.rs` | `cargo run -p vrcx-0-web-surface --example capture -- https://vrcc.cc/` 出 PNG，肉眼可见官网首页 |
| 指针点击 | CDP `Input.dispatchMouseEvent` | `examples/input_probe.rs` 模拟点击「我的」标签，页面真的切到登录页 |
| 滚动 | CDP `mouseWheel` | 同上，列表从 1–11 行滚到 11–22 行 |
| 键盘/中文输入 | CDP `Input.insertText` / `dispatchKeyEvent` | 同上，输入框成功写入 `hello 星遇` |
| 页面内可见光标 | 注入脚本 `CURSOR_SCRIPT`（`engine.rs`） | 截图里能看到白色圆环，点击时放大闪烁 |
| 插件机制 + 热重载 | `crates/web-surface/src/manifest.rs`、`overlay-runtime/src/web_overlay.rs` | 改 `index.html` → 程序比对 mtime → 自动 `Reload`，标题变成「已更新 v2」 |
| 内置静态服务器（给插件页一个正常 HTTP 源） | `crates/web-surface/src/static_server.rs` | `--plugin <dir>` 模式截图正常；含路径穿越防护单测 |
| 手腕表面显示网页帧 | `crates/overlay-runtime/src/runtime.rs` | 日志 `web overlay: surface started` / `page loading` |
| 面板开关式显示（按一下开、再按一下关） | `crates/host-desktop/src/vr_overlay/policy.rs` | 策略单测；实机需确认 |
| 控制器射线光标 | `crates/host-desktop/src/vr_overlay/pointer_ray.rs` | 4 个数学单测（中心命中、象限映射、背向不命中、矩阵复合） |
| 上传帧率跟随刷新率 | `overlay-runtime/src/runtime.rs`、`host-desktop/.../openvr_backend.rs` | `min_upload_interval_ms` 取配置值 |

`cargo test -p vrcx-0-host-desktop --features steamvr-overlay`：通过（此数字需在装有 cmake + MSVC 的机器上复核）。
`cargo check --workspace`：通过。

---

## 4. 代码结构（新增 / 修改）

**新增**

```
crates/web-surface/                 ← 新 crate（vrcx-0-web-surface）
  Cargo.toml
  src/lib.rs                        公共 API：WebSurfaceConfig / WebInputEvent / WebSurface
  src/engine.rs                     WebView2 离屏窗口 + 帧捕获 + CDP 输入 + 光标注入
  src/static_server.rs              内置静态文件服务器（插件页 HTTP 源）
  src/manifest.rs                   插件清单解析 + 内置插件落盘
  examples/capture.rs               抓一个 URL 存 PNG（可加 --plugin / 点击坐标）
  examples/input_probe.rs           点击 + 键盘 + 滚动 + 热重载的自测探针
crates/overlay-runtime/src/web_overlay.rs   网页模式配置（8 个 config 键）
crates/host-desktop/src/vr_overlay/pointer_ray.rs   射线/平面数学 + 单测
plugins/vrcc/{index.html,manifest.json}     内置「星遇同频」腕表面板
docs/WEB_OVERLAY.zh-CN.md                   早期设计/使用文档（部分内容已过时）
crates/platform/examples/resolve_app_dir.rs 排查中文路径问题的诊断程序
```

**修改**

```
Cargo.toml                                   workspace members 增加 crates/web-surface
src-tauri/Cargo.toml                         新增 custom-protocol feature
crates/overlay-runtime/Cargo.toml            依赖 vrcx-0-web-surface
crates/overlay-runtime/src/lib.rs            mod web_overlay
crates/overlay-runtime/src/config.rs         配置解析（web dims）
crates/overlay-runtime/src/runtime.rs        WebSurfaceDims / 生产者 web 分支 / 占位帧 / 热重载 / 刷新间隔
crates/overlay-runtime/src/service.rs        take_input_events 透传
crates/overlay-runtime/src/manager.rs        take_input_events 透传
crates/overlay-runtime/src/service/tests.rs  测试构造补字段
crates/host-desktop/src/vr_overlay/types.rs                       OverlaySurfaceConfig.min_upload_interval_ms / OverlayInputEvent
crates/host-desktop/src/vr_overlay/mod.rs                         mod pointer_ray / 导出 OverlayInputEvent
crates/host-desktop/src/vr_overlay/openvr_backend.rs             输入队列、射线指针、交互标志、鼠标事件、高频 tick、上传间隔
crates/host-desktop/src/vr_overlay/openvr_helpers.rs             set_overlay_interactive
crates/host-desktop/src/vr_overlay/policy.rs                     开关式可见性
crates/host-desktop/src/vr_overlay/openvr_backend/tests.rs       测试构造补字段
crates/host-desktop/tests/vr_overlay_actor.rs                    测试构造补字段
```

---

## 5. 配置项（`%APPDATA%\VRCX-0\VRCX-0.sqlite3` → `configs` 表）

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `wristOverlayEnabled` | true | 腕表总开关 |
| `wristOverlayStartMode` | `vrchat` | `vrchat` = 仅 VRChat 以 VR 模式启动时；`steamvr` = 只要 SteamVR 起着 |
| `wristOverlayHand` | `left` | `left` / `right` / `both` |
| `wristOverlayButton` | `grip` | 面板开关按键（现在是**按一下切换**） |
| `wristOverlayWebEnabled` | true | 网页模式（false 回落原 Slint 面板） |
| `wristOverlayWebPlugin` | `vrcc` | 插件 id（= 插件目录名） |
| `wristOverlayWebUrl` | 空 | 直接指定 URL，优先于插件 |
| `wristOverlayWebWidth` / `Height` | 640 / 640 | 采集尺寸 |
| `wristOverlayWebZoom` | 1000 | 页面缩放（千分之一） |
| `wristOverlayWebRefreshMs` | 250 | 采集/上传间隔（33–5000） |
| `wristOverlayWebPhysicalWidthMm` | 260 | 面板物理宽度 |

写配置示例：

```python
import sqlite3, os
db = os.path.join(os.environ["APPDATA"], "VRCX-0", "VRCX-0.sqlite3")
con = sqlite3.connect(db)
con.execute("insert into configs(key,value) values(?,?) on conflict(key) do update set value=excluded.value",
            ("wristOverlayWebRefreshMs", "250"))
con.commit()
```

---

## 6. 插件机制

插件目录：`%LOCALAPPDATA%\vrcx-0\overlay-plugins\`

```
overlay-plugins/
└── vrcc/
    ├── manifest.json
    └── index.html
```

`manifest.json`：`id` / `name` / `url`（相对路径或绝对 URL）/ `width` / `height` / `zoom` /
`refresh_ms` / `inject_css` / `inject_js` / `scopes`。

- 相对路径的页面由**内置静态服务器**以 `http://127.0.0.1:<随机端口>/` 提供，
  这样页面是正常 HTTP 源，请求 `vrcc.cc` 不会被 CORS 拦（该站会回显 `Access-Control-Allow-Origin`）。
- **热重载**：改 `index.html` → 主程序每帧比对 mtime → 变化即 `Reload`，不用重启程序。
- 内置 `vrcc` 插件在首次运行时自动写入（已存在不覆盖，用户改动不会被升级冲掉）。
- 新增插件：新建目录 + manifest，把 `wristOverlayWebPlugin` 改成该 id 即可。

---

## 7. 星遇同频接口对接

- 文档：<https://docs.vrcc.cc/>，OpenAPI：<https://github.com/vrcc-cc/xingyu-tongpin-api>
- 基址：`https://vrcc.cc/api/v1/`，请求头 `X-Client: <标识>`，个人接口再加 `Authorization: Bearer <token>`
- 设备授权：`POST /device/code` → 显示 `user_code` + `verification_uri_complete` →
  每 `interval` 秒 `POST /device/token` → 拿 48 位令牌
- 令牌只存本机（WebView2 配置目录 `%LOCALAPPDATA%\vrcx-0\web-overlay-profile`），只发往 `vrcc.cc`
- 当前用的是 `dev-vrcx0-wrist`（`dev-` 前缀，只能要普通权限 `profile notifications maps`）；
  正式使用需在
  [登记客户端](https://github.com/vrcc-cc/xingyu-tongpin-api/issues/new?title=登记客户端)
  后改 `plugins/vrcc/index.html` 顶部的 `DEFAULT_CLIENT`
- 页面已实现：地图榜（`/boards/maps`）、最近提醒（`/me/notifications/recent`）、我的（设备授权/断开）

---

## 8. 调试与自测

```bat
call <你的构建环境脚本>
cd /d <源码目录>

cargo run -p vrcx-0-web-surface --example capture -- https://vrcc.cc/
cargo run -p vrcx-0-web-surface --example capture -- --plugin %LOCALAPPDATA%\vrcx-0\overlay-plugins\vrcc
cargo run -p vrcx-0-web-surface --example capture -- --plugin <dir> 320,88   :: 抓完再模拟点击
cargo run -p vrcx-0-web-surface --example input_probe                        :: 点击/键盘/滚动/热重载
cargo test -p vrcx-0-web-surface -p vrcx-0-overlay-runtime -p vrcx-0-host-desktop
```

- 截图写在 `<输出目录>\web-surface-capture.png` / `<输出目录>\probe-*.png`
- `VRCX_WEB_DEBUG_HWND=1` 会打印 WebView2 子窗口类别（输入早期版本依赖 HWND，现在走 CDP，不再需要）
- 看叠加层是否真的起来：启动应用后看 stdout 是否有
  `VR overlay service started backend="openvr"` / `web overlay: surface started` / `web overlay: page loading`

---

## 9. 构建环境（本机踩过的坑）

仓库自带 `.cargo/config.toml` 要求 `lld-link.exe`，本机没装 LLVM，所以用环境变量覆盖成 MSVC 的 `link.exe`。
`<你的构建环境脚本>` 目前是：

```bat
call "C:\Program Files\Microsoft Visual Studio\18\Insiders\VC\Auxiliary\Build\vcvars64.bat"
set "PATH=%USERPROFILE%\.cargo\bin;D:\Python\Python313\Scripts;%PATH%"
set "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=<VC>\bin\Hostx64\x64\link.exe"
set "CARGO_TARGET_DIR=<短路径 target>"                    :: 构建路径必须短，否则 lib.exe 命令行超限 LNK1104
set "CMAKE_ARGS=-DCMAKE_POLICY_VERSION_MINIMUM=3.5"   :: openvr_sys 的旧 CMakeLists 与 CMake 4 不兼容
set "LIBCLANG_PATH=D:\Python\Python313\Lib\site-packages\clang\native"  :: openvr_sys 用 bindgen
set "TMP=<临时目录>" & set "TEMP=<临时目录>"                :: 临时目录不能含中文
```

依赖工具（都是本机额外装的）：`cmake 4.4.4`（`pip install cmake`）、`libclang`（`pip install libclang`）。

---

## 10. 接手建议（按优先级）

1. **修中文路径启动崩溃**（§2.1）——这是用户当前最直接的痛点，改动集中在
   `crates/platform/src/app_paths.rs` 的解析/校验降级逻辑。
2. **做「星遇同频」原生页面**（§2.3）：路由 `src/app/routes.tsx`、导航 `src/shared/constants/ui.ts`
   （`navDefinitions`）、页面放 `src/features/vrcc/`；加好友用 `appSocialFriendRequestSend`。
3. **VR 输入再优化**（§2.2）：先确认射线方向是否正确（`pointer_ray.rs` 的轴映射只能实机验证），
   再考虑 gaze 光标 / 手势点击以减少按键占用。
4. 把 `docs/WEB_OVERLAY.zh-CN.md` 与本 README 合并去重（前者部分内容已过时）。

---

## 11. 许可

本分支基于 VRCX-0（GPL-3.0），同样以 **GPL-3.0** 分发；`plugins/vrcc` 里的面板页为本项目编写，
其调用的「星遇同频」接口与名称归 vrcc.cc 所有，请遵守其
[使用要求](https://docs.vrcc.cc/)（不要把其名字/图标当作自己的品牌，写明是第三方客户端）。
