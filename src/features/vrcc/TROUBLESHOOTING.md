# 星遇同频（vrcc）排查指南

给帮忙复现问题的人看。目标是：**只靠一个日志文件，能判断问题出在哪一层。**

## 一、先拿日志

诊断日志在应用数据目录：

```
%APPDATA%\VRCX-0\diagnostic-log.txt
```

完整路径通常是：

```
C:\Users\<你的用户名>\AppData\Roaming\VRCX-0\diagnostic-log.txt
```

在文件管理器地址栏粘 `%APPDATA%\VRCX-0` 就能直接打开。

**这个文件和 `error-log.txt` 是两回事**：`error-log.txt` 只装 error，`diagnostic-log.txt` 装 debug 及以上的完整时间线。排查星遇同频问题要后者。

## 二、想要更详细的日志

默认级别是 `info`。要看每一轮轮询的细节（包括请求耗时、跳过原因），用环境变量启动：

```powershell
$env:RUST_LOG = "vrcx_0=debug"
& "路径\vrcx-0.exe"
```

改完记得新开终端，或 `Remove-Item Env:RUST_LOG` 复位。

## 三、日志怎么看

### 启动阶段（正常应该看到这些）

```
vrcc: starting background poll  has_desktop_notifier=true token_connected=false client=dev-VRCX-0-Galaxies client_is_dev=true
vrcc: background poll task started  interval_secs=60 jitter_secs=20
vrcc: background poll scheduled
```

逐个字段的含义：

| 字段                   | 期望                  | 不对时的含义                                                          |
| ---------------------- | --------------------- | --------------------------------------------------------------------- |
| `has_desktop_notifier` | `true`                | `false` = 通知桥没挂上（Headless 或装配顺序问题），**不会有系统通知** |
| `token_connected`      | 连过之后 `true`       | `false` = 还没连星遇同频，轮询会跳过                                  |
| `client`               | `dev-VRCX-0-Galaxies` | 换正式标识后应是 `app_` 开头                                          |
| `client_is_dev`        | 开发态 `true`         | `true` 时拿不到敏感权限，`mutuals` 等必然 403                         |

**如果连这四行都没有**，说明轮询根本没起来——检查日志里有没有 `background poll not scheduled`（任务被拒绝）。

### 运行阶段（每轮一条，需 debug 级）

```
vrcc: background poll round finished  round=3 unread=1 toast_attempted=1 changed=true
vrcc: background poll skipping         round=4                     ← 未连接
vrcc: background poll sleeping         round=3 delay_ms=71234
```

### 拿到提醒时

```
vrcc: fetched recent notices      total=5 unread=2 read_cursor=1024
vrcc: desktop notification dispatched  notice_id=1026 body_len=42
```

### 通知没弹出去时

```
vrcc: desktop notification not delivered  notice_id=1026 error=...
```

| 错误内容            | 原因                                                |
| ------------------- | --------------------------------------------------- |
| 无错误但也没弹      | 窗口处于后台且系统通知被关闭（Windows 设置 → 通知） |
| `notification: ...` | 系统级失败，通常是应用未注册到通知中心              |

### 超过每轮上限时

```
vrcc: unread notices exceed per-poll toast limit  unread=8 limit=3
```

说明积压多，剩余的下轮再弹。**不是错误**，但连续出现说明轮询间隔太长或被中断过。

## 四、前端侧的对应证据

前端日志在同一个文件里，前缀是 `[vrcc]`：

| 日志                                         | 含义                                           |
| -------------------------------------------- | ---------------------------------------------- |
| `[vrcc] notices changed event subscribed`    | 事件订阅成功。**没有这行，界面就不会自动刷新** |
| `[vrcc] notices changed event received`      | 后端推来了信号                                 |
| `[vrcc] notices changed event unsubscribed`  | 页面切走，正常退订                             |
| `[vrcc] subscribe notices event failed: ...` | 订阅失败，界面只能手动刷新                     |
| `[vrcc] load failed: ...`                    | 首屏加载失败                                   |

**关键判断**：如果日志里有 `desktop notification dispatched`（后端弹了通知）但**没有** `notices changed event received`（前端没收到），问题在事件通道；如果两者都有但界面没变，问题在前端刷新逻辑。

## 五、常见症状对照

### 症状：通知完全不来

按顺序核对：

1. `starting background poll` 有没有出现？没有 → 轮询没起
2. `has_desktop_notifier` 是 `true` 吗？`false` → 通知桥没挂
3. `token_connected` 是 `true` 吗？`false` → 没连星遇同频
4. `round finished` 的 `unread` 是 0 吗？是 → 确实没有新提醒
5. `unread > 0` 但无 `dispatched` → 看有没有 `not delivered`
6. 以上都正常 → 检查 Windows 系统通知设置，以及「专注助手」是否开启

### 症状：日志里 `unread` 一直是 0，但网页上明明有新提醒

`read_cursor` 的值来自服务端。如果它比提醒的 id 大，说明**账号在别处已经读过**
（网页或手机 App），按设计不再弹。这不是 bug。

要看是不是这种情况：比较 `read_cursor` 和网页上那条提醒的 id。

### 症状：界面显示「未连接」但日志 `token_connected=true`

前端 `connection` 状态没刷到。看 `[vrcc] load failed`。

### 症状：`403 scope` / `scope_unreviewed`

`dev-` 标识只能要普通权限。`mutuals`、`today`、`crossings` 等敏感权限需要
应用登记并通过审核后，换成 `app_…` 标识才能用。**这是预期行为，不是缺陷。**

### 症状：`接口暂时不可用` / `error sending request`

网络层。检查：

1. 应用设置里的代理是否填对（本机需要代理才能访问 `vrcc.cc`）
2. 日志里有没有 `vrcc: no proxy configured, connecting directly`
3. 有没有 `vrcc: invalid proxy url, using direct`（代理串写错）

## 六、手工验证网络

不依赖应用，直接测接口：

```powershell
# 不走代理
curl.exe -sS -o NUL -w "%{http_code}`n" https://vrcc.cc/api/v1/status

# 走代理（按你实际的代理端口改）
curl.exe -sS -o NUL -w "%{http_code}`n" --ssl-no-revoke -x http://127.0.0.1:7890 https://vrcc.cc/api/v1/status
```

返回 `200` 说明网络通。两个都通说明代理不是必需的；只有走代理才通，就要在应用里配代理。

## 七、报告问题时请附上

1. `diagnostic-log.txt`（**建议先搜 `vrcc` 只贴相关行**，其余内容可能含个人信息）
2. 上面「手工验证网络」两条命令的输出
3. `has_desktop_notifier` / `token_connected` 两行的实际值
4. 系统版本（Win10 / Win11），以及是否开了专注助手

---

## 附：探针清单（给开发者）

| 位置                                   | 级别  | 内容                                   |
| -------------------------------------- | ----- | -------------------------------------- |
| `state.rs::start_vrcc_background_poll` | info  | 启动前提四要素                         |
| `vrcc.rs::spawn_background_poll`       | info  | 任务启动、停止请求                     |
| 同上                                   | debug | 每轮结果、休眠时长、跳过原因           |
| 同上                                   | warn  | 轮询失败、任务未被调度                 |
| `poll_once`                            | debug | 拉取到的总数/未读/游标                 |
| `poll_once`                            | info  | 每条通知投递成功（含 id 与正文字节数） |
| `poll_once`                            | warn  | 投递失败、超每轮上限                   |
| 前端 `useVrccController`               | info  | 订阅、收事件、退订                     |
| 前端 `useVrccController`               | error | 订阅失败、加载失败、跳转失败           |

**正文内容从不进日志**，只记 `body_len` 与 `notice_id`——提醒正文属于玩家数据。
