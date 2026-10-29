# Secure Session v2 batch fast-path: 2026-10-30

本目录记录 `fuckipv6-secure-session` 的 32 包连续缓冲批量 `seal/open` 开发基准，并与同等工作量的逐包调用比较。时间均为 32 包总耗时，不包含 TUN、UDP、调度、缓冲池或端到端协议开销。

## 结论

- 1420 字节、32 包时，批量 seal 中位数为 `80.068 µs`（约 `2.502 µs/包`、`541.23 MiB/s`），逐包 seal 为 `82.361 µs`（约 `2.574 µs/包`、`526.16 MiB/s`）；本次样本中批量约快 2.8%。
- 1420 字节、32 包时，批量 open 中位数为 `82.441 µs`（约 `2.576 µs/包`、`525.65 MiB/s`），逐包 open 为 `86.229 µs`（约 `2.695 µs/包`、`502.56 MiB/s`）；本次样本中批量约快 4.4%。
- 1280 字节结果的信赖区间大量重叠，批量 seal 未显示更快，批量 open 仅显示小幅差异。不能据此声称稳定吞吐提升，更不能据此声称超过 WireGuard。
- 两种路径的执行闭包都不分配内存；open 基准的唯一预分配缓冲区、密文 fixture 和 receiver 均在计时区间外准备。

## 结果

单位为 32 包总耗时；区间为 Criterion 给出的 95% 信赖区间。

| 载荷 | 操作 | 实现 | 时间 | 吞吐 |
|---:|---|---|---:|---:|
| 1280 B | seal | 逐包 x32 | 71.047-72.166 µs | 541.29-549.81 MiB/s |
| 1280 B | seal | batch x32 | 71.605-74.210 µs | 526.37-545.53 MiB/s |
| 1280 B | open | 逐包 x32 | 74.883-80.521 µs | 485.12-521.65 MiB/s |
| 1280 B | open | batch x32 | 74.574-78.141 µs | 499.90-523.81 MiB/s |
| 1420 B | seal | 逐包 x32 | 81.361-83.277 µs | 520.37-532.63 MiB/s |
| 1420 B | seal | batch x32 | 79.762-80.360 µs | 539.26-543.31 MiB/s |
| 1420 B | open | 逐包 x32 | 82.616-91.362 µs | 474.32-524.54 MiB/s |
| 1420 B | open | batch x32 | 82.051-83.093 µs | 521.52-528.15 MiB/s |

中位数和原始观测分别见 `results.json` 与 `criterion.txt`。结果采集时宿主 load average 约为 `4.69/4.75/5.34`，不是发布性能门禁。

## 复现

```bash
taskset -c 2 cargo bench -p fuckipv6-secure-session --bench dataplane --   batch --sample-size 20 --warm-up-time 1 --measurement-time 3 --noplot
```

## 文件

- `criterion.txt`：Criterion 原始输出，包含编译记录和全部样本区间。
- `metadata.json`：机器、工具链、命令、源码/原始输出哈希和门禁状态。
- `results.json`：可机器读取的区间、中位数及每包派生值。
