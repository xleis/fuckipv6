# 科目 08 — 临时防火墙规则清理

## 1. 科目内容

验证所有临时实验资源在测试结束后都能自动/手动清理干净：

1. 客户端 nft 阻断表；
2. TTL 定时器；
3. TUN `lxtrescue0`；
4. client/server sidecar；
5. 实验端口 39090/39091；
6. 服务端没有遗留 iptables/firewalld/默认路由改动。

这是安全验收项，不是功能项；不通过则不能把实验留在机器上。

## 2. 代码位置与入口

- [`../../code/lxt_public_synack_v4/client_drop_nft.sh`](../../code/lxt_public_synack_v4/client_drop_nft.sh)
  - `start SERVER_IP NIC [TTL]`：建 `table inet lxt_v4` + input 链 + 单条 drop，带 TTL。
  - `stop`：删表、杀 TTL 定时器。
  - `status`：查看表/规则/计数。
- [`../../code/lxt_public_synack_v4/phase4_client.sh`](../../code/lxt_public_synack_v4/phase4_client.sh)
  - `trap cleanup EXIT INT TERM`：退出时停 sidecar、删 nft 表、杀定时器。
- 原始脚本：[`../../code/lxt_public_synack_v4/client_drop_once.sh`](../../code/lxt_public_synack_v4/client_drop_once.sh)
- 完整报告：[`../../reports/LXT_V4_PUBLIC_TEST_REPORT.md`](../../reports/LXT_V4_PUBLIC_TEST_REPORT.md)

## 3. 实现细节

### 3.1 nft 表生命周期

- 表名固定 `lxt_v4`，只包含 `input` 链和一条匹配指定服务端 IP/端口/标志的规则。
- `TTL`（秒）通过后台 `sleep` 进程实现；到点执行 `nft delete table inet lxt_v4`。
- `stop` 会先删表，再 kill 定时器；重复执行是幂等的。
- 不使用 `iptables -F`、不清空默认策略、不动 SSH/默认路由。

### 3.2 phase4_client.sh 的 trap

- 从 stdin 读 `LXT_LAB_KEY`，不把 key 放进 argv。
- 启动客户端 sidecar 前先记录 PID；退出时 `kill`，等待 TUN 消失。
- `cleanup()` 调用 `client_drop_nft.sh stop`，并删除临时日志/PCAP（可选）。
- `full` 模式跑对照 + 救援；`rescue` 模式只跑救援；两者都走同一 trap。

### 3.3 手工兜底

```bash
./client_drop_nft.sh stop
pkill -f '[l]xt_public_lab.py client-sidecar' || true
ip link show lxtrescue0 2>&1 || true
nft list table inet lxt_v4 2>&1 || true
ss -lntup | grep -E '39090|39091' || true
```

## 4. V4 清理结果

| 检查项 | 结果 |
|---|---|
| `nft list table inet lxt_v4` | 表不存在 |
| `lxtrescue0` | 不存在 |
| client-sidecar | 无进程 |
| TTL 定时器 | 无 |
| 服务端 39090/39091 | 已关闭 |
| 服务端 iptables | 仍是 `-P INPUT ACCEPT`、`-N FKV6F-99E15423`、`-A FORWARD -j FKV6F-99E15423`、`-i/-o fuckipv6`；未改 |
| 服务端默认路由 | 未改 |
| 客户端既有 nginx/sing-box | 正常 |

**结果：PASS。**

## 5. V7/V8 的清理

- V7：删除 `lxt_v7_a` / `lxt_v7_b` 表、netns、veth、TUN、sidecar、key、SSH ControlMaster；服务端 sidecar/proxy/bulk 停止；iptables/默认路由/SSH/SELinux 未改。
- V8：删除 `lxt_v8_obs` / `lxt_v8_ct` / `lxt_v8_a` / `lxt_v8_b` / `lxt_v8_c`、三个 netns、veth、`ip_forward=0`、`/tmp/lxt8/key`、`/root/lxt_v8`；宿主 nft 为空、39090/39091 无监听、既有 nginx/sing-box 正常。

详细清理记录见：

- [`../../reports/LXT_V7_MIDDLEBOX_REPORT.md`](../../reports/LXT_V7_MIDDLEBOX_REPORT.md)
- [`../../reports/LXT_V8_NO_NAT_REPORT.md`](../../reports/LXT_V8_NO_NAT_REPORT.md)

## 6. 边界与风险

- 如果实验进程被 `SIGKILL`，trap 不会执行；TTL 定时器仍会兜底删表，但 sidecar/TUN 需要手工清理。
- 如果机器重启，nft 表默认不持久化；但仍应执行 `stop` 检查。
- 不要在朋友的服务器上做任何清理动作；本实验的阻断只允许在客户端。
- 旧 PSK 曾在会话记录中暴露，实验后应轮换；本目录不包含 key。
