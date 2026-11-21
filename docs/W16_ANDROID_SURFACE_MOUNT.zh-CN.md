# W16 Android 页面挂载清单

> 状态：**导航契约里的 14 个页面全部挂载真实数据面**（2026-11-21）。`FuckIpv6App`/`MainActivity` 接收
> `settings`/`fileBrowse`/`fileBrowseActions`/`transfers`/`members`/`joinRequests`/`invitations` 以及
> `peers`/`activity`/`traffic`/`logs`/`identity`/`networks`/`routing`/`localExit`/`exitSelections`/`topology`/`blacklist`，
> 由 `SurfaceResolver` 与各自的 bridge 切片驱动；首次启动走 `OnboardingProjection` + `OnboardingPage` 引导。
> 仍未完成的是**真机**项（SAF 选择器交互、真机上传下载、VpnService 包转发与前台服务、进程被杀后恢复），
> 它们属于 W16-02/W16-04 的剩余验收，本清单不声称覆盖。

## 1. 已就绪的制品

| 制品 | 作用 | 测试 |
|---|---|---|
| `bridge/NodeApiConfig`、`HttpSnapshotSource`、`SnapshotJsonParser` | 快照读取与切片（File/Relay/Coordinator/网络标签/peers 原始对象） | `NodeBridgeTest`、`SnapshotJsonParserTest`、`RelaySnapshotParseTest`、`CoordinatorSnapshotParseTest` |
| `bridge/CommandJson`、`NodeCommandClient`、`NodeCommandBridge` | 命令提交与操作终态跟随（只认终态 `succeeded`） | `CommandBridgeTest` |
| `bridge/EventStream`、`NodeEventStream` | SSE 事件触发权威刷新 | `EventFrameParserTest` |
| `bridge/AdminRecoverySurface` | 管理员恢复提交/状态/列表/撤销/执行 | `AdminRecoverySurfaceTest` |
| `bridge/MemberSurface`、`MemberLifecycle`、`JoinRequestSurface`、`InvitationSurface` | 成员目录、生命周期、加入申请（含批量审批）、邀请 | `MemberSurfaceTest`、`MemberLifecycleTest`、`JoinRequestSurfaceTest`、`InvitationSurfaceTest` |
| `bridge/BlacklistSurface` | 客户端黑名单查询/封禁/解除 | `BlacklistSurfaceTest` |
| `bridge/RoutingSurface` | 路由摘要与 `update_routing` 热更新 | `RoutingSurfaceTest` |
| `bridge/ExitSurface` | 本机 Exit 能力（两侧同意分离）与 `set_local_exit_capability`、Exit 选择 | `ExitSurfaceTest` |
| `bridge/PeersSurface`、`TrafficSurface`、`ActivitySurface`、`LogsSurface`、`IdentitySurface`、`NetworksSurface`、`TopologySurface` | 只读观察面；隐藏字段保持 `null` 并显示「已按隐私策略隐藏」 | `PeersSurfaceTest`、`TrafficActivitySurfaceTest`、`LogsIdentitySurfaceTest`、`NetworksSurfaceTest`、`TopologySurfaceTest` |
| `bridge/FileBrowse`、`ui/FileWorkspaceState`、`ui/FileBrowseController` | 只读 File 浏览（目标/目录/分页/过期守卫） | `FileBrowseTest`、`FileWorkspaceStateTest`、`FileBrowseControllerTest` |
| `bridge/FileTransfer`、`ui/FileTransferController`、`bridge/DownloadFetcher`、`ui/DownloadSaver`、`platform/SafDocuments` | 上传/下载启动、暂停/恢复、token 下载落盘、SAF 文档写入与失败删除 | `FileTransferTest`、`FileTransferControllerTest`、`DownloadSaverTest` |
| `ui/SettingsProjection`、`ui/SettingsPage`、`ui/OnboardingProjection`、`ui/OnboardingPage` | 节点连接设置与首次启动引导 | `SettingsProjectionTest`、`OnboardingProjectionTest` |
| `ui/FileBrowsePanel`、`ui/AppSurfaces`、`SurfaceResolver` | 面板渲染与 bridge→页面数据解析 | `FileProjectionTest`、`AppSurfacesTest` |
| `service/TunnelState`、`TunnelEngine`、`FuckIpv6VpnService` | 隧道状态机与 VpnService 外壳（未绑定 seam 时失败关闭） | `TunnelStateTest`、`TunnelEngineTest` |

## 2. 挂载顺序与完成情况

1. ~~设置页~~：已完成 — `pageId == "settings"` 渲染 `SettingsPageContent`，保存/清除写本机偏好后 `recreate()`。
2. ~~File 浏览面板~~：已完成 — 目标选择/进入目录/上级/加载更多/刷新接到 `FileBrowseController`。
3. ~~传输列表~~：已完成 — 上传走 `ActivityResultContracts.OpenDocument`，下载走 `CreateDocument` + `SafDownloadSink`，
   失败删除部分写入的文档；`Uri` 只在 `platform/SafDocuments` 里处理。
4. ~~成员页~~：已完成 — 数据来自 `MemberSurface.listCommand(networkId)`；未拿到结果前显示「未观察」。
5. ~~事件驱动~~：已完成 — `MainActivity` 跑 `NodeEventStream`，刷新走同一套权威读取，没有第二套轮询判定。
6. ~~只读观察面~~：已完成 — peers/traffic/activity/logs/identity/networks/topology 全部由快照切片投影。
7. ~~可写治理~~：已完成 — 成员生命周期、加入申请（单个 + 批量）、邀请、黑名单、路由热更新、本机 Exit 开关、
   管理员恢复全流程；每次写操作都以节点答复为准，必要时重读快照。

## 3. 挂载时的硬性要求

- 页面数据只能来自 `SurfaceResolver.resolve(...)` 或对应 bridge 切片；不得在 Compose 里推导「可执行/成功」。
- 任何 `null` 切片都必须落到「未观察/不可用」状态，不能填默认值冒充实时数据；**隐藏字段与缺失字段必须区分**。
- 动作按钮只在回调非空且后端可用时启用；提交成功后以服务端返回的投影/快照为准，不用表单意图改本地状态。
- 新增字符串必须同时加入 zh-CN 与 en；能复用 `file.page.*` / `state.*` / `identity.hidden` 的不要新造。
- 运行时拼出来的键（`activity.action.<x>`、`state.capability.<x>`）必须走 `translateOr`，否则缺键会把键名打到屏幕上。
- 提交前跑：`:app:testDebugUnitTest`、`:app:lintDebug`、`:app:assembleDebug`、`scripts/check-docs.py`、
  `python3 scripts/check-android-i18n.py`、`python3 scripts/check-surface-state-parity.py`。

## 4. 验证基线（2026-11-21）

在 HEAD（不含其他会话未提交的 gpui/Tauri 改动）实测：

- `:app:testDebugUnitTest` → **196 项通过 / 0 失败**
- `:app:lintDebug`、`:app:assembleDebug` → 通过
- `scripts/check-android-i18n.py` → passed（165 个字面量键，0 问题）
- `scripts/check-surface-state-parity.py` → passed（Android 14 / WebUI 13 / 目录 14）
- `scripts/check-docs.py` → passed

真机项（SAF 选择器交互、真机传输、VpnService 包转发与前台服务、进程被杀/重启后的恢复）不在本清单内，
仍是 W16-01/W16-02 的剩余验收。
