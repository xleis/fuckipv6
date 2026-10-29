# Dataplane baseline: 2026-10-30

本目录保存 `fuckipv6`、Linux 内核 WireGuard 与 `wireguard-go` 的同机 network namespace 端到端基线。原始结果、汇总和运行环境均在此目录，避免只保留截图或口述结论。

## 结论

本基线**不支持**“安全等级和性能已经超过 WireGuard”的结论。当前数据面明显落后于两个 WireGuard 实现：

- 单流 TCP 中位数约 `0.055 Gbit/s`，是内核 WireGuard 的 `3.7%`、`wireguard-go` 的 `2.1%`。
- 受控 1420 字节 UDP 中位数约 `0.040 Gbit/s`，是两种 WireGuard 实现的 `81.0%`，同时出现 `15.0%` 丢包。
- 受控 64 字节 UDP 中位数约 `0.0035 Gbit/s`，是两种 WireGuard 实现的 `70.5%`，同时出现 `27.8%` 丢包。
- `/proc/stat` 记录的是系统级 CPU 时间，本次运行还受到其他桌面和编译进程干扰，CPU/byte 只能作为噪声较大的诊断指标，不能归因于单一进程。

因此后续优先级应是先修正 TUN/UDP 数据面热路径和调度模型，再讨论性能目标；仅替换 AEAD 或重做 UI 不足以弥补数量级差距。

## 运行环境

- CPU：AMD Ryzen 7 5800H with Radeon Graphics，16 个逻辑 CPU。
- 内核：`7.2.11-zen1-1-zen`，x86_64。
- 虚拟网络：`fd00::/64`，MTU `1420`。
- 每个用例运行 3 次，每次 10 秒；未启用 omit。
- 受控 UDP：1420 字节 `50 Mbit/s`，64 字节 `5 Mbit/s`。
- 饱和 UDP：发送端使用不限速 UDP，记录接收吞吐、发送吞吐和丢包。
- 运行期间宿主机的 `load average` 约为 `6.5-9.6`，不是空闲基准机。
- `fuckipv6` 构建对应提交 `b49be00f890a46e01653f6155774816e43d98ea7`；工作树只因两个未提交 release 二进制被标记为 dirty。

## 中位数结果

`throughput_gbps` 使用接收端实际传输字节计算；比率以 `fuckipv6 / WireGuard` 表示。

| 用例 | fuckipv6 | 内核 WireGuard | wireguard-go | 对内核比率 | fuckipv6 丢包 |
|---|---:|---:|---:|---:|---:|
| TCP 1 flow | 0.0551 Gbit/s | 1.4788 Gbit/s | 2.6436 Gbit/s | 3.7% | 0% |
| TCP 8 flows | 0.0497 Gbit/s | 1.4799 Gbit/s | 2.4687 Gbit/s | 3.4% | 0% |
| UDP 1420 controlled | 0.0405 Gbit/s | 0.0500 Gbit/s | 0.0500 Gbit/s | 81.0% | 15.0% |
| UDP 64 controlled | 0.00352 Gbit/s | 0.00500 Gbit/s | 0.00500 Gbit/s | 70.5% | 27.8% |
| UDP 1420 saturated | 0.0382 Gbit/s | 1.0381 Gbit/s | 0 Gbit/s | 3.7% | 98.3% |
| UDP 64 saturated | 0.00358 Gbit/s | 0.0870 Gbit/s | 0.0411 Gbit/s | 4.1% | 97.4% |

饱和 UDP 受发送端过载和主机负载影响很大；`wireguard-go` 的 1420 字节接收结果为 0，说明该组数据只能用于暴露问题，不能用于精确排名。受控 UDP 也出现明显丢包，表示当前测试环境尚不足以作为发布门禁。

## 复现

```bash
sudo env WG_GO_BIN=/path/to/wireguard-go \
  scripts/bench-dataplane.sh \
  --impls all --runs 3 --duration 10 \
  --output /tmp/fuckipv6-bench
python3 scripts/bench-summarize.py \
  --input /tmp/fuckipv6-bench/results.jsonl \
  --csv /tmp/fuckipv6-bench/summary.csv
```

正式性能结论必须在空闲机器上固定 CPU、关闭无关后台负载并重复运行。脚本现已支持
`--control-cpus`、`--client-cpus` 和 `--server-cpus`，例如
`--control-cpus 0 --client-cpus 2 --server-cpus 3`；本历史数据集未启用 CPU 绑定，
因此不能用它证明正式性能。

## 文件

- `summary.csv`：3 次运行的中位数汇总。
- `results.jsonl`：54 条逐次运行指标。
- `metadata.json`：提交、二进制哈希、内核、工具版本和测试参数。
- `lscpu.txt`：CPU 拓扑与特性。
- `raw/`：54 份 iperf3 JSON 和 54 份 `perf stat` CSV。
