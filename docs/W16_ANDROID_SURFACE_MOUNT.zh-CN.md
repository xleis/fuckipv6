# W16 Android 页面挂载清单

> 状态：**设置页、File 浏览面板与传输摘要素材已挂载**（2026-11-21）。`FuckIpv6App`/`MainActivity` 已接受
> `settings`/`fileBrowse`/`fileBrowseActions`/`transfers`，`MainActivity` 用 `FileBrowseController` 与
> `FileTransferController` 驱动。挂载时把并行会话的 shell 样式改动（图标、技术详情、主题）一并提交，因为二者改的是
> 同一批文件；成员页与 SAF 选择器交互仍未挂载。

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

1. ~~**设置页**~~：已完成 — `pageId == "settings"` 渲染 `SettingsPageContent`，保存/清除写本机偏好后
   `recreate()` 重建桥接。
2. ~~**File 浏览面板**~~：已完成 — `pageId == "files"` 在能力卡片下方渲染 `FileBrowseContent`，
   目标选择/进入目录/上级/加载更多/刷新都接到 `FileBrowseController`。
3. **传输列表**：摘要已挂载（方向/状态/进度/错误码）；上传/下载按钮仍待接
   `FileTransferController.upload(...)` 与 `DownloadSaver.save(token, SafDownloadSink(...))`，
   选择器用 `ActivityResultContracts.OpenDocument` / `CreateDocument`，`Uri` 交给 `platform/SafDocuments`。
4. **成员页**：新增页面渲染，数据来自 `MemberSurface.listCommand(networkId)` 的 `network_members` 结果；
   未拿到结果前显示“未观察”。
5. **事件驱动**：`MainActivity` 已启动 `NodeEventStream`，挂载后不要再引入第二套轮询。

## 3. 挂载时的硬性要求

- 页面数据只能来自 `SurfaceResolver.resolve(...)`；不得在 Compose 里自行推导“可执行/成功”。
- 任何 `null` 切片都必须落到页面的 “未观察/不可用” 状态，不能填默认值冒充实时数据。
- 动作按钮只在对应回调非空且后端可用时启用（`backendAvailable` 已由 resolver 设置）。
- 新增字符串必须同时加入 zh-CN 与 en 并保持键集对齐；能复用 `file.page.*` / `state.*` 的不要新造。
- 提交前跑：`:app:testDebugUnitTest`、`:app:lintDebug`、`:app:assembleDebug`、`scripts/check-docs.py`、
  `python3 scripts/check-android-i18n.py`（字面量键必须同时存在于 zh-CN 与 en，插值键按运行时闭集由单测覆盖）。

## 4. 验证基线（2026-11-21）

在 HEAD + 本批文件（不含其他会话未提交改动）的独立 worktree 中实测：

- `:app:testDebugUnitTest` → **148 项通过 / 0 失败**
- `:app:lintDebug`、`:app:assembleDebug` → 通过
- `scripts/check-docs.py` → passed

真机项（SAF 选择器交互、真机传输、VpnService）不在本清单内，仍属 W16-02/W16-01 的剩余验收。
