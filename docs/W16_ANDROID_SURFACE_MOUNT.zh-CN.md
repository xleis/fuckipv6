# W16 Android 页面挂载清单

> 状态：挂载契约与回调就绪（2026-11-21）。本文只记录“把已实现的能力挂到 shell 上”需要做的事，
> 不重复各模块的验收证据。当前阻塞：`android/.../ui/FuckIpv6App.kt` 与 `MainActivity.kt` 带有其他并行
> 会话的未提交改动，挂载提交必须等这两个文件回到可提交状态，避免把他人未完成的工作混入。

## 1. 已就绪的制品

| 制品 | 作用 | 测试 |
|---|---|---|
| `bridge/NodeApiConfig`、`HttpSnapshotSource`、`SnapshotJsonParser` | 快照读取与切片（File/Relay/Coordinator/网络标签） | `NodeBridgeTest`、`SnapshotJsonParserTest`、`RelaySnapshotParseTest`、`CoordinatorSnapshotParseTest` |
| `bridge/CommandJson`、`NodeCommandClient`、`NodeCommandBridge` | 命令提交与操作终态跟随 | `CommandBridgeTest` |
| `bridge/EventStream`、`NodeEventStream` | SSE 事件触发权威刷新 | `EventFrameParserTest` |
| `bridge/AdminRecoverySurface` | 管理员恢复命令编码与结果解析 | `AdminRecoverySurfaceTest` |
| `bridge/FileBrowse`、`ui/FileWorkspaceState`、`ui/FileBrowseController` | 只读 File 浏览（目标/目录/分页/过期守卫） | `FileBrowseTest`、`FileWorkspaceStateTest`、`FileBrowseControllerTest` |
| `bridge/FileTransfer`、`ui/FileTransferController` | 上传/下载启动、暂停/恢复、列表刷新 | `FileTransferTest`、`FileTransferControllerTest` |
| `bridge/DownloadFetcher`、`ui/DownloadSaver`、`platform/SafDocuments` | token 下载落盘、SAF 文档写入与失败删除 | `DownloadSaverTest` |
| `bridge/MemberSurface` | 成员目录解析 | `MemberSurfaceTest` |
| `ui/SettingsProjection`、`ui/SettingsPage` | 节点连接设置 | `SettingsProjectionTest` |
| `ui/FileBrowsePanel` | File 浏览面板 | 编译/lint/assemble |
| `ui/AppSurfaces`、`SurfaceResolver` | 把 bridge 状态解析成 shell 需要的全部页面数据 | `AppSurfacesTest` |

## 2. 挂载步骤（按顺序，每步可单独提交）

1. **设置页**：`FuckIpv6App(..., settings = surfaces.settings)`，`PageContent` 增加
   `pageId == "settings" && settings != null -> SettingsPageContent(...)`。
2. **File 浏览面板**：`FuckIpv6App(..., fileBrowse = surfaces.fileBrowse)`，
   `pageId == "files"` 时优先渲染 `FileBrowseContent`，其 `files != null` 的能力卡片保留在同一页顶部。
3. **传输列表**：在 File 面板下方加 `transfers` 区块；上传/下载按钮分别调用
   `FileTransferController.upload(...)` 与 `DownloadSaver.save(token, SafDownloadSink(...))`，
   选择器用 `ActivityResultContracts.OpenDocument` / `CreateDocument`，拿到的 `Uri` 交给
   `platform/SafDocuments`。
4. **成员页**：新增页面渲染，数据来自 `MemberSurface.listCommand(networkId)` 的 `network_members` 结果；
   未拿到结果前显示“未观察”。
5. **事件驱动**：`MainActivity` 已启动 `NodeEventStream`，挂载后不要再引入第二套轮询。

## 3. 挂载时的硬性要求

- 页面数据只能来自 `SurfaceResolver.resolve(...)`；不得在 Compose 里自行推导“可执行/成功”。
- 任何 `null` 切片都必须落到页面的 “未观察/不可用” 状态，不能填默认值冒充实时数据。
- 动作按钮只在对应回调非空且后端可用时启用（`backendAvailable` 已由 resolver 设置）。
- 新增字符串必须同时加入 zh-CN 与 en 并保持键集对齐；能复用 `file.page.*` / `state.*` 的不要新造。
- 提交前跑：`:app:testDebugUnitTest`、`:app:lintDebug`、`:app:assembleDebug`、`scripts/check-docs.py`。

## 4. 验证基线（2026-11-21）

在 HEAD + 本批文件（不含其他会话未提交改动）的独立 worktree 中实测：

- `:app:testDebugUnitTest` → **108 项通过 / 0 失败**
- `:app:lintDebug`、`:app:assembleDebug` → 通过
- `scripts/check-docs.py` → passed

真机项（SAF 选择器交互、真机传输、VpnService）不在本清单内，仍属 W16-02/W16-01 的剩余验收。
