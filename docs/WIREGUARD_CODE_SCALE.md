# WireGuard 上游代码规模统计

- 统计日期：2026-10-30（Asia/Shanghai）
- 用途：记录 WireGuard 官方上游源码的可复现体量快照，并与当前 `fuckipv6` Rust 代码规模并列展示。
- 边界：本报告只统计文件和物理行数，不评价安全性、密码学强度、代码质量或性能，也不得出“超越 WireGuard”的结论。

## 1. 版本固定

两个仓库分别克隆到 `/tmp/fuckipv6-wireguard-scale/` 下的独立目录。Linux 仓库没有可用的发布 tag，因此固定 `stable` 分支上的明确提交；Go 仓库固定到 annotated tag 及其解析后的提交。

| 仓库 | 克隆源 | 固定引用 | 解析后提交 | 提交日期 |
|---|---|---|---|---|
| `wireguard-linux` | `https://git.zx2c4.com/wireguard-linux` | `stable` 分支（无 tag） | `2a73c9b2c0ca737696b1bacc7559fcaf619e302e` | 2026-10-08 |
| `wireguard-go` | `https://git.zx2c4.com/wireguard-go` | tag `0.0.20250522`（tag object `90f39c9a9b1a63c360a5b7d69e26a5089fc4af1a`） | `f333402bd9cbe0f3eeb02507bd14e23d7d639280` | 2025-05-22 |

复现克隆：

```bash
mkdir -p /tmp/fuckipv6-wireguard-scale
git clone --depth=1 --single-branch --branch stable \
  https://git.zx2c4.com/wireguard-linux \
  /tmp/fuckipv6-wireguard-scale/wireguard-linux
git clone --depth=1 --single-branch --branch 0.0.20250522 \
  https://git.zx2c4.com/wireguard-go \
  /tmp/fuckipv6-wireguard-scale/wireguard-go

git -C /tmp/fuckipv6-wireguard-scale/wireguard-linux rev-parse HEAD
git -C /tmp/fuckipv6-wireguard-scale/wireguard-go rev-parse 0.0.20250522
git -C /tmp/fuckipv6-wireguard-scale/wireguard-go rev-parse '0.0.20250522^{}'
```

## 2. 统计口径

### 2.1 通用口径

- “物理行”使用 `wc -l` 统计；不区分代码、空行和注释行。
- “非空行”使用 `awk 'NF { n++ }'` 统计；整行注释仍计入非空行。
- 文件数按实际参与统计的文件计数，不把目录或符号链接计为文件。
- 排除 `.git` 和 `vendor`。
- 排除 `generated/` 路径，以及文件头明确标记 `Code generated ... DO NOT EDIT` 的生成文件。
- 不统计 shell、Makefile、Kconfig、配置、文档和二进制文件；因此 QEMU/netns 等非 C/Go 测试资产不在行数中。

### 2.2 `wireguard-linux`

该仓库是 Linux 内核稳定分支，包含整个内核树，而不是只包含 WireGuard 模块。本报告只统计以下 WireGuard 范围的 `.c`/`.h`：

- 生产代码：`drivers/net/wireguard/`，排除 `generated/` 和 `selftest/`。
- 测试代码：`drivers/net/wireguard/selftest/` 与 `tools/testing/selftests/wireguard/`。
- 已知生成文件：`drivers/net/wireguard/generated/netlink.c` 和 `generated/netlink.h`，共 2 个文件、103 行，已排除。

### 2.3 `wireguard-go`

- 统计仓库内全部 `.go` 文件。
- `*_test.go` 归为测试代码，其余归为非测试代码（生产和支撑代码）。
- 生成文件 `device/devicestate_string.go` 共 16 行，文件头标记为 `stringer` 生成，已排除。
- 非测试代码仍包含 test helper 包和 `tun/netstack/examples/` 示例，因此它是“非测试 Go 代码”，不应与最小协议核心逐行等同。

### 2.4 `fuckipv6`

- Rust 快照固定为提交 `a0dbd04d4188a81dfa8526ee7ff525e59d6317f1`，通过 `git archive` 导出，避免工作树中的外部未提交修改污染结果。
- 统计所有 tracked `.rs` 文件。
- `crates/*/src` 包含内联 `#[cfg(test)]` 测试模块，不能直接视为纯生产代码。
- 集成测试、benchmark 和 fuzz target 单独列在 `src` 之外。

## 3. 可复现统计命令

### 3.1 `wireguard-linux`

```bash
cd /tmp/fuckipv6-wireguard-scale/wireguard-linux

# 生产 C/H：27 个文件
find drivers/net/wireguard -type f \
  \( -name '*.c' -o -name '*.h' \) \
  ! -path '*/generated/*' ! -path '*/selftest/*' \
  -print0 | xargs -0 wc -l

# 测试 C/H：4 个文件（3 个模块 selftest + QEMU init.c）
find drivers/net/wireguard/selftest tools/testing/selftests/wireguard \
  -type f \( -name '*.c' -o -name '*.h' \) \
  -print0 | xargs -0 wc -l

# 非空行
find drivers/net/wireguard -type f \
  \( -name '*.c' -o -name '*.h' \) \
  ! -path '*/generated/*' ! -path '*/selftest/*' \
  -print0 | xargs -0 awk 'NF { n++ } END { print n+0 }'
find drivers/net/wireguard/selftest tools/testing/selftests/wireguard \
  -type f \( -name '*.c' -o -name '*.h' \) \
  -print0 | xargs -0 awk 'NF { n++ } END { print n+0 }'
```

### 3.2 `wireguard-go`

```bash
cd /tmp/fuckipv6-wireguard-scale/wireguard-go

# 非测试 Go；排除已知生成文件
find . -path './.git' -prune -o -path './vendor' -prune \
  -o -path '*/generated/*' -prune -o \
  -type f -name '*.go' ! -name '*_test.go' \
  ! -name 'devicestate_string.go' -print0 | xargs -0 wc -l

# 测试 Go
find . -path './.git' -prune -o -path './vendor' -prune \
  -o -path '*/generated/*' -prune -o \
  -type f -name '*_test.go' -print0 | xargs -0 wc -l

# 非空行
find . -path './.git' -prune -o -path './vendor' -prune \
  -o -path '*/generated/*' -prune -o \
  -type f -name '*.go' ! -name '*_test.go' \
  ! -name 'devicestate_string.go' -print0 |
  xargs -0 awk 'NF { n++ } END { print n+0 }'
find . -path './.git' -prune -o -path './vendor' -prune \
  -o -path '*/generated/*' -prune -o \
  -type f -name '*_test.go' -print0 |
  xargs -0 awk 'NF { n++ } END { print n+0 }'
```

### 3.3 `fuckipv6` Rust

```bash
cd /home/xlei/project/fuckipv6
head=a0dbd04d4188a81dfa8526ee7ff525e59d6317f1
snap=$(mktemp -d)
git archive "$head" | tar -x -C "$snap"

# 全部 tracked Rust
find "$snap" -type f -name '*.rs' -print0 | xargs -0 wc -l
find "$snap" -type f -name '*.rs' -print0 |
  xargs -0 awk 'NF { n++ } END { print n+0 }'
find "$snap" -type f -name '*.rs' | wc -l

# src 与 src 外部分开统计
find "$snap/crates" \( -path '*/src/*.rs' -o -path '*/src/**/*.rs' \) \
  -print0 | xargs -0 wc -l
find "$snap" -type f -name '*.rs' ! -path '*/src/*' \
  -print0 | xargs -0 wc -l
```

## 4. 原始结果

### 4.1 `wireguard-linux`，提交 `2a73c9b`（无发布 tag）

| 分类 | 扩展名 | 文件数 | 物理行 | 非空行 |
|---|---|---:|---:|---:|
| 生产代码 | `.c` | 14 | 5,159 | 未单列 |
| 生产代码 | `.h` | 13 | 910 | 未单列 |
| **生产代码小计** | `.c` + `.h` | **27** | **6,069** | **5,219** |
| 测试代码 | `.c` | 4 | 1,342 | 1,197 |
| 测试代码 | `.h` | 0 | 0 | 0 |
| **测试代码小计** | `.c` + `.h` | **4** | **1,342** | **1,197** |
| **合计** | `.c` + `.h` | **31** | **7,411** | **6,416** |

测试 C 文件为：

- `drivers/net/wireguard/selftest/allowedips.c`
- `drivers/net/wireguard/selftest/counter.c`
- `drivers/net/wireguard/selftest/ratelimiter.c`
- `tools/testing/selftests/wireguard/qemu/init.c`

### 4.2 `wireguard-go`，tag `0.0.20250522`

| 分类 | 文件数 | 物理行 | 非空行 |
|---|---:|---:|---:|
| 非测试 Go | 77 | 13,792 | 11,982 |
| 测试 Go | 22 | 4,101 | 3,692 |
| **合计** | **99** | **17,893** | **15,674** |

### 4.3 当前 `fuckipv6` Rust，提交 `a0dbd04`

| 分类 | 文件数 | 物理行 | 非空行 |
|---|---:|---:|---:|
| `crates/*/src`（含内联测试） | 103 | 66,360 | 60,053 |
| `src` 外的 tests/benches/fuzz | 10 | 1,760 | 1,592 |
| **全部 tracked Rust** | **113** | **68,120** | **61,645** |

`crates/*/src` 分 crate 结果：

| Crate | 文件数 | 物理行 | 非空行 |
|---|---:|---:|---:|
| `client` | 58 | 36,789 | 33,356 |
| `common` | 16 | 7,623 | 6,713 |
| `coordinator` | 11 | 9,789 | 8,832 |
| `secure-session` | 18 | 12,159 | 11,152 |

## 5. 对比时应保留的解释

这三组数字只表示仓库体量和统计范围内的人工/生成前源码行数，不能直接等价：

- `wireguard-linux` 是 Linux 内核模块，依赖内核网络栈、加密 API、路由、socket、RCU 和设备模型；其体量不能代表独立用户态实现的完整成本。
- `wireguard-go` 是专注 WireGuard 协议和 TUN/UAPI 的最小用户态实现，其非测试 Go 代码中含平台适配、test helper 和示例。
- `fuckipv6` 是多 crate 产品代码，包含协调器、节点控制面、路由/出口节点、Secure Session、GPUI/WebUI/IPC、DHT/overlay、配置、测试和 fuzz 等更广功能。
- 文件数或物理行数不能说明加密等级、协议安全性、可审计性、每包性能、吞吐或延迟。安全等级必须由协议设计、正确性证明、独立审计和真实威胁模型支撑；性能必须由固定硬件与可复现基准支撑。

因此，本报告不据此宣称 `fuckipv6` 在加密等级或性能上超过 WireGuard。
