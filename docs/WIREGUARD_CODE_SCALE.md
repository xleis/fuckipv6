# WireGuard 上游与 fuckipv6 代码规模统计

- 统计日期：2026-10-31（Asia/Shanghai）
- 用途：记录三个固定 `git archive` 快照的可复现体量，并并列展示 `fuckipv6` 与 WireGuard 上游源码规模。
- 边界：本报告只统计文件和源码行数，不评价安全性、密码学强度、代码质量或性能，也不得出“超越 WireGuard”的结论。
- 主结果只读取固定提交的 `git archive` 快照；不得从活动工作树、未提交改动或动态 `git ls-files` 结果生成数字。

## 1. 版本固定

Linux 仓库没有可用的发布 tag，因此固定 `stable` 分支解析出的明确提交；Go 仓库固定到与数据面分析报告相同的 `master` 提交。`fuckipv6` 固定到本报告实际使用的已提交 HEAD，三者都先导出为独立 `git archive` 快照再统计。

| 仓库 | 克隆/来源 | 固定提交 | 提交日期 |
|---|---|---|---|
| `fuckipv6` | 本仓库 `git archive` | `39ca63730e4bad9302084272c3c6fc6c6eddb737` | 2026-10-31 |
| `wireguard-linux` | `https://git.zx2c4.com/wireguard-linux` | `2a73c9b2c0ca737696b1bacc7559fcaf619e302e` | 2026-10-08 |
| `wireguard-go` | `https://git.zx2c4.com/wireguard-go` | `b3bc704ef0ee21a36298601a9e1e5ff90377ac52` | 2026-10-25 |

复现时先检出完整提交，再把指定提交导出到新的 snapshot 目录：

```bash
set -euo pipefail
SNAPSHOT_ROOT=/tmp/fuckipv6-docs-snapshots
FUCKIPV6_REPO=/path/to/fuckipv6
FUCKIPV6_COMMIT=39ca63730e4bad9302084272c3c6fc6c6eddb737
WG_LINUX_COMMIT=2a73c9b2c0ca737696b1bacc7559fcaf619e302e
WG_GO_COMMIT=b3bc704ef0ee21a36298601a9e1e5ff90377ac52

rm -rf "$SNAPSHOT_ROOT"
mkdir -p "$SNAPSHOT_ROOT/fuckipv6" "$SNAPSHOT_ROOT/wireguard-linux" \
  "$SNAPSHOT_ROOT/wireguard-go"

git -C "$FUCKIPV6_REPO" archive --format=tar "$FUCKIPV6_COMMIT" \
  | tar -xf - -C "$SNAPSHOT_ROOT/fuckipv6"

git clone --filter=blob:none https://git.zx2c4.com/wireguard-linux \
  "$SNAPSHOT_ROOT/wireguard-linux-src"
git -C "$SNAPSHOT_ROOT/wireguard-linux-src" checkout --detach "$WG_LINUX_COMMIT"
git -C "$SNAPSHOT_ROOT/wireguard-linux-src" archive --format=tar "$WG_LINUX_COMMIT" \
  drivers/net/wireguard tools/testing/selftests/wireguard \
  | tar -xf - -C "$SNAPSHOT_ROOT/wireguard-linux"

git clone --filter=blob:none https://git.zx2c4.com/wireguard-go \
  "$SNAPSHOT_ROOT/wireguard-go-src"
git -C "$SNAPSHOT_ROOT/wireguard-go-src" checkout --detach "$WG_GO_COMMIT"
git -C "$SNAPSHOT_ROOT/wireguard-go-src" archive --format=tar "$WG_GO_COMMIT" \
  | tar -xf - -C "$SNAPSHOT_ROOT/wireguard-go"

git -C "$SNAPSHOT_ROOT/wireguard-linux-src" rev-parse HEAD
git -C "$SNAPSHOT_ROOT/wireguard-go-src" rev-parse HEAD
git -C "$FUCKIPV6_REPO" rev-parse "$FUCKIPV6_COMMIT"
```

## 2. 统计口径

### 2.1 通用口径

- “物理行”使用 Python `str.splitlines()` 统计；不区分代码、空行和注释行。
- “有效行”只统计非空且不是纯注释的行；行尾注释仍计入，块注释按词法状态扣除。
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
- 生成文件 `device/devicestate_string.go` 共 16 行，文件头标记为 `stringer` 生成；生产口径排除该文件。
- 非测试代码仍包含 test helper 包和 `tun/netstack/examples/` 示例，因此它是“非测试 Go 代码”，不应与最小协议核心逐行等同。

### 2.4 `fuckipv6`

- 主表只统计提交 `39ca63730e4bad9302084272c3c6fc6c6eddb737` 的 `git archive` 快照；不读取活动工作树。
- `git archive` 只包含该提交中 tracked 的文件，因此统计所有快照内的 `.rs` 文件。
- `crates/*/src` 包含内联 `#[cfg(test)]` 测试模块，不能直接视为纯生产代码。
- 生产口径排除文件末尾的顶层 `#[cfg(test)] mod tests` 块，以及由 `#[cfg(test)] mod tests;` 引用的 `tests.rs`。
- 集成测试、benchmark 和 fuzz target 单独列在 `src` 之外。

## 3. 可复现统计命令

### 3.1 `wireguard-linux`

```bash
cd /tmp/fuckipv6-docs-snapshots/wireguard-linux

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

### 3.2 `wireguard-go` 与 `fuckipv6` 的同口径计数

下面脚本只读取前一步导出的固定 `git archive` 快照目录，不调用 Git，也不读取任何活动工作树。`fuckipv6` 的 Rust 生产口径按 2.4 节规则截断末尾顶层测试模块。

```bash
python3 - \
  /tmp/fuckipv6-docs-snapshots/fuckipv6 \
  /tmp/fuckipv6-docs-snapshots/wireguard-go <<'PY'
from pathlib import Path
import sys

def count_texts(texts):
    physical = effective = 0
    for text in texts:
        lines = text.splitlines()
        physical += len(lines)
        block = False
        for line in lines:
            s = line.strip()
            if not s:
                continue
            if block:
                if "*/" not in s:
                    continue
                s = s.split("*/", 1)[1].strip()
                block = False
                if not s:
                    continue
            if s.startswith("//"):
                continue
            if s.startswith("/*"):
                if "*/" not in s:
                    block = True
                    continue
                s = s.split("*/", 1)[1].strip()
                if not s:
                    continue
            effective += 1
    return len(texts), physical, effective

def counts(paths):
    return count_texts([p.read_text(errors="replace") for p in paths])

def rust_production(text):
    lines = text.splitlines(keepends=True)
    cutoff = None
    for i, line in enumerate(lines):
        if line.rstrip() != "#[cfg(test)]":
            continue
        for following in lines[i + 1:i + 8]:
            s = following.strip()
            if not s or s.startswith(("//", "#")):
                continue
            if s.startswith("mod tests"):
                cutoff = i
            break
    return "".join(lines[:cutoff] if cutoff is not None else lines)

def row(label, result):
    print(f"{label:36} {result[0]:5} {result[1]:8} {result[2]:8}")

repo = Path(sys.argv[1])
rust = sorted(repo.rglob("*.rs"))
src = [p for p in rust
       if len(p.relative_to(repo).parts) >= 3
       and p.relative_to(repo).parts[0] == "crates"
       and p.relative_to(repo).parts[2] == "src"]
outside = [p for p in rust if p not in src]
prod_texts = []
for path in src:
    parent = path.parent / "mod.rs"
    parent_text = parent.read_text(errors="replace") if parent.exists() else ""
    if path.name == "tests.rs" and "#[cfg(test)]" in parent_text \
            and "mod tests;" in parent_text:
        continue
    prod_texts.append(rust_production(path.read_text(errors="replace")))

print("fuckipv6 Rust (39ca637 git archive)")
row("all tracked Rust", counts(rust))
row("crates/*/src (tests included)", counts(src))
row("crates/*/src (production)", count_texts(prod_texts))
row("outside src", counts(outside))

wg = Path(sys.argv[2])
go = sorted(wg.rglob("*.go"))
prod_go, tests, generated = [], [], []
for path in go:
    head = "\n".join(path.read_text(errors="replace").splitlines()[:5])
    if path.name.endswith("_test.go"):
        tests.append(path)
    elif "Code generated" in head and "DO NOT EDIT" in head:
        generated.append(path)
    else:
        prod_go.append(path)

print("wireguard-go")
row("all Go", counts(go))
row("production Go", counts(prod_go))
row("test Go", counts(tests))
row("generated Go", counts(generated))
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

### 4.2 `wireguard-go`，提交 `b3bc704`

| 分类 | 文件数 | 物理行 | 有效行 |
|---|---:|---:|---:|
| 非测试 Go（生产口径，排除生成文件） | 77 | 13,793 | 10,815 |
| 测试 Go（`*_test.go`） | 22 | 4,149 | 3,569 |
| 生成 Go（`devicestate_string.go`） | 1 | 16 | 10 |
| **全部 Go** | **100** | **17,958** | **14,394** |

### 4.3 `fuckipv6` Rust，固定提交 `39ca637`

数字来自 `39ca63730e4bad9302084272c3c6fc6c6eddb737` 的 `git archive` 快照，不包含 2026-10-31 活动工作树中的任何未提交变更。

| 分类 | 文件数 | 物理行 | 有效行 |
|---|---:|---:|---:|
| `crates/*/src`（含内联/外置测试） | 113 | 76,217 | 66,164 |
| `crates/*/src`（生产口径，排除测试） | 112 | 52,787 | 44,892 |
| `src` 外的 tests/benches/fuzz | 11 | 2,198 | 1,983 |
| **全部 tracked Rust** | **124** | **78,415** | **68,147** |

按同一有效行口径，生产口径约为 `wireguard-go` 非测试代码 `10,815` 行的 `4.15` 倍；把 `crates/*/src` 内测试计入后约为 `6.12` 倍。

`crates/*/src` 分 crate 生产口径结果：

| Crate | 文件数 | 物理行 | 有效行 | 占生产有效行 |
|---|---:|---:|---:|---:|
| `client` | 64 | 31,804 | 27,830 | 62.0% |
| `common` | 16 | 5,616 | 4,439 | 9.9% |
| `coordinator` | 11 | 6,506 | 5,668 | 12.6% |
| `secure-session` | 21 | 8,861 | 6,955 | 15.5% |
| **合计** | **112** | **52,787** | **44,892** | **100%** |

### 4.4 历史 `fuckipv6` 快照，提交 `a0dbd04`

下表保留 2026-10-30 的历史基线，同样来自提交 `a0dbd04d4188a81dfa8526ee7ff525e59d6317f1` 的 `git archive` 快照，不应与 4.3 的当前固定快照混用：

| 分类 | 文件数 | 物理行 | 有效行 |
|---|---:|---:|---:|
| `crates/*/src`（含测试） | 103 | 66,360 | 57,394 |
| `crates/*/src`（生产口径） | 102 | 47,069 | 39,939 |
| `src` 外测试资源 | 10 | 1,760 | 1,587 |
| **全部 tracked Rust** | **113** | **68,120** | **58,981** |

### 4.5 后续已提交快照 `4360aac`

为回答“当前已提交代码有多大”，本轮又对 HEAD `4360aac17a524d1ac02a5a78340d8896595399b4` 做了同样的 `git archive` 固定快照统计。该数字仍**不包含当前 dirty worktree**，因此会低估正在整合的生产连接层改动。

| 分类 | 文件数 | 物理行 | 有效行 |
|---|---:|---:|---:|
| `crates/*/src`（含内联/外置测试） | 116 | 80,339 | 69,650 |
| `crates/*/src`（生产口径，排除测试） | 115 | 55,406 | 47,027 |
| `src` 外的 tests/benches/fuzz | 11 | 2,272 | 2,047 |
| **全部 tracked Rust** | **127** | **82,611** | **71,697** |

相对 `39ca637`，当前已提交快照增加 3 个生产 Rust 文件、2,619 物理行、2,135 生产有效行。按同一口径，生产有效行约为 `wireguard-go` 非测试 Go 的 `4.35` 倍；计入 `crates/*/src` 内测试后约为 `6.44` 倍。

当前已提交 HEAD 的 per-crate 生产口径：

| Crate | 文件数 | 物理行 | 有效行 | 占生产有效行 |
|---|---:|---:|---:|---:|
| `client` | 66 | 33,330 | 29,063 | 61.8% |
| `common` | 16 | 5,732 | 4,516 | 9.6% |
| `coordinator` | 11 | 6,532 | 5,693 | 12.1% |
| `secure-session` | 22 | 9,812 | 7,755 | 16.5% |
| **合计** | **115** | **55,406** | **47,027** | **100%** |

这两组统计只能说明体量，不能说明代码质量或性能；当前 dirty worktree 中的 UDP 接线、有界队列和批量 secure-runtime 整合尚未计入。

## 5. 对比时应保留的解释

这三组数字只表示仓库体量和统计范围内的人工/生成前源码行数，不能直接等价：

- `wireguard-linux` 是 Linux 内核模块，依赖内核网络栈、加密 API、路由、socket、RCU 和设备模型；其体量不能代表独立用户态实现的完整成本。
- `wireguard-go` 是专注 WireGuard 协议和 TUN/UAPI 的最小用户态实现，其非测试 Go 代码中含平台适配、test helper 和示例。
- `fuckipv6` 是多 crate 产品代码，包含协调器、节点控制面、路由/出口节点、Secure Session、GPUI/WebUI/IPC、DHT/overlay、配置、测试和 fuzz 等更广功能。
- 文件数或物理行数不能说明加密等级、协议安全性、可审计性、每包性能、吞吐或延迟。安全等级必须由协议设计、正确性证明、独立审计和真实威胁模型支撑；性能必须由固定硬件与可复现基准支撑。

因此，本报告不据此宣称 `fuckipv6` 在加密等级或性能上超过 WireGuard。
