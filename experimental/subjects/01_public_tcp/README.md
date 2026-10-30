# 科目 01 — 公网正常 TCP

## 1. 科目内容

在没有 SYN-ACK 阻断、没有客户端 sidecar 的公网路径上，验证：

1. 客户端能正常解析/路由到服务端；
2. TCP 39090 能正常三次握手；
3. echo 消息能双向收发；
4. 服务端按 `--close-after` 主动关闭后，客户端能重新建连。

这是后续所有阻断/救援实验的基线。若这一项不通过，不应继续做阻断测试。

## 2. 代码位置与入口

- 核心代码：[`../../code/lxt_public_synack_v4/lxt_public_lab.py`](../../code/lxt_public_synack_v4/lxt_public_lab.py)
  - `run_echo_server()`：TCP echo 服务，读取一行，回复 `ACK:<line>`；`--close-after N` 控制每 N 条后关闭。
  - `run_probe()`：客户端探测，发送 `ping-i`，校验 `ACK:ping-i`，支持超时/重试/自动重连。
- 启动脚本参考：[`../../code/lxt_public_synack_v4/README.md`](../../code/lxt_public_synack_v4/README.md)
- 完整报告：[`../../reports/LXT_V4_PUBLIC_TEST_REPORT.md`](../../reports/LXT_V4_PUBLIC_TEST_REPORT.md)

## 3. 实现细节

- 服务端：`echo-server --bind 0.0.0.0 --tcp-port 39090 --close-after 2`。
- 客户端：`probe --server <SERVER_IP> --count 6 --timeout 4`。
- `probe` 每次连接使用新的客户端源端口；服务端每 2 条消息关闭连接，客户端在收到 EOF 后重新 `connect()`。
- 本阶段不启动 server-sidecar / client-sidecar，UDP 39091 不参与。
- echo 服务端只做普通 TCP socket；没有 RAW/TUN 权限要求。

## 4. 复现命令

```bash
# 服务端
python3 lxt_public_lab.py echo-server --bind 0.0.0.0 --tcp-port 39090 --close-after 2

# 客户端
python3 lxt_public_lab.py probe --server <SERVER_IP> --count 6 --timeout 4
```

## 5. V4 结果与证据

- 6/6 条消息成功，`established=3`。
- 客户端源端口：42006 / 42014 / 42024。
- 服务端 echo 日志：`accepted ('111.91.22.14', ...)` 三条。
- 输出含 `[probe] ESTABLISHED ('172.31.0.173', 42006) -> ('185.217.110.15', 39090)`、`[PASS] completed=6, established=3`。
- **结果：PASS。**

## 6. 关联报告

- V4：第 3 节验收表、4.1 节 Phase 3。
- V5：正常 18 MiB 基线（同一原生 TCP 路径）。
- V7：正常基线（netns 版）。
- V8：三 netns 正常基线。

## 7. 边界

- 本项只证明“无阻断时 TCP 正常”，不证明 UDP 救援。
- 真实公网路径可能受代理、NAT、MTU、云安全组影响；V4 最初在 Mac/Parallels + 家庭代理路径上失败，后来迁移到客户端 VPS 才得到干净路径。
