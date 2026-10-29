# Secure Session v2 crypto baseline: 2026-10-30

本目录保存 `fuckipv6-secure-session` v2 数据帧原语的 Criterion 开发基线。它衡量的是用户态内存中单包 `seal/open`、重放拒绝和 `EpochSession` 包装开销，**不是** TUN/UDP 端到端性能，也不能替代与 WireGuard 的同机对照。

## 结论

- 1420 字节载荷单包 `seal` 中位数约 `2.80 µs`，单包 `open` 约 `3.06 µs`；`EpochSession::seal` 约 `2.76 µs`。这些数字包含测试进程内构造、基准框架和宿主负载影响。
- 已认证包的重复接收在 Nonce/AEAD 前被重放窗口拒绝，耗时约 `4 ns`。
- 该基线只证明密码原语和当前分配式接口可被持续测量，不证明性能超过 WireGuard。端到端差距见 `../2026-10-30-dataplane-baseline/README.md`。
- 本轮使用 Criterion `--quick`，且宿主 `load average` 约 `6.8-7.5`；结果只能用于开发回归参考，不能用于发布门禁或对外排名。

## 结果

区间为 Criterion 给出的置信区间，原始输出见 `criterion-quick.txt`。

| 载荷 | seal | open | replay reject |
|---:|---:|---:|---:|
| 64 B | 1.561-1.593 µs | 1.813-1.884 µs | 4.160-4.308 ns |
| 512 B | 1.775-1.804 µs | 2.049-2.100 µs | 5.303-5.483 ns |
| 1280 B | 2.672-2.752 µs | 2.697-2.754 µs | 4.211-4.426 ns |
| 1420 B | 2.688-2.823 µs | 3.055-3.056 µs | 3.957-4.024 ns |

1420 字节 `EpochSession::seal` 为 `2.670-2.786 µs`，对应约 `486-507 MiB/s`。该吞吐按 AEAD 输入载荷计算，不含 IP/UDP/TUN 和调度开销。

## 复现

```bash
cargo bench -p fuckipv6-secure-session --bench dataplane -- --quick --noplot
```

正式采集应使用空闲机器、固定 CPU、关闭无关后台负载，并去掉 `--quick`：

```bash
taskset -c 2 cargo bench -p fuckipv6-secure-session --bench dataplane -- --noplot
```

## 运行环境

- CPU：AMD Ryzen 7 5800H with Radeon Graphics，8 核 / 16 逻辑 CPU。
- 内核：`7.2.11-zen1-1-zen`，x86_64。
- Rust：`rustc 1.99.1 (58cf8f9c3 2026-10-13)`，Cargo `1.99.1`。
- Criterion：`0.5.1`。
- 测试载荷：64、512、1280、1420 字节全零外载荷（实际填充 `0x42`）。
- 未启用 CPU 固定；测试时宿主 `load average` 为 `7.54/6.97/6.77`。
- 基准源码对应基础提交 `71d5909`；运行时工作树包含本目录和基准源码的未提交变更。

## 文件

- `criterion-quick.txt`：Criterion 原始快速采集输出。
- `metadata.json`：机器、工具链、提交、命令和统计口径。
