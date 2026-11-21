# Verify 工作流的依赖链与失败模式

本文记录 `.github/workflows/verify.yml` 在 2026-11-21 逐个修通时学到的依赖顺序。
它的用途是让下一个人不必再从"第一个 crate 就失败"开始重走一遍。

## 一、每个 job 真正需要的系统依赖

| job | 需要的包 | 为什么 |
|---|---|---|
| `docs` | 无（仅 Python） | 只跑 `check-docs.py` 与单二进制边界检查 |
| `contracts` | `libfontconfig1-dev` `libfreetype-dev` `libxcb1-dev` `libxkbcommon-dev` `libxkbcommon-x11-dev` `pkg-config` | 它跑 `cargo test -p fuckipv6-client --lib node::contract`，客户端的测试二进制经 GPUI 平台层链接 `xcb`、`xkbcommon` |
| `governance` | 同上 | 12 个 check 都是 `cargo test` 域测试，同样链接客户端 |
| `api-security` | 同上 | 同上 |
| `gpui-smoke` | 同上 + `xvfb` `libvulkan1` `mesa-vulkan-drivers` `xdotool` | 要在 Xvfb 里开真实窗口；`gpui-smoke.sh` 在启动前就检查 `xdotool` |
| `visual-regression` | 同上 + `fonts-inter` + Chrome | 同一张页面在 GPUI 与 WebUI 两端截图比对 |
| `linux` | 同上 + `mingw-w64` + `musl-tools` | 交叉检查 windows-gnu 与 musl，后者需要 `x86_64-linux-musl-gcc` |
| `android` | JDK 21 + Android SDK 36 + Gradle 缓存 | `assembleDebug`、`testDebugUnitTest`、`lintDebug` |
| `windows` / `macos` | 平台自带 | 只做 `release-gate.py run --platform …` |

## 二、六个真实踩过的坑

1. **pkg-config 阶段**：`yeslogic-fontconfig-sys` 的 `build.rs` 直接 `unwrap()`，
   runner 没有 `fontconfig.pc` 时整棵树在依赖解析就失败，断言一步都没跑到。
2. **链接阶段**：补上 fontconfig/freetype 之后，客户端测试二进制仍在链 `-lxcb`、
   `-lxkbcommon`、`-lxkbcommon-x11`。**运行时包（`libxcb1`）不够，要 `-dev`**。
3. **脚本前置检查**：`gpui-smoke.sh` 先在构建之后才检查 `xdotool`，于是 job 花了
   4 分钟编译再退出——依赖检查放在最前面能省一轮。
4. **交叉工具名**：`aws-lc-sys` 按名字探测 `x86_64-linux-musl-gcc`，而 `musl-tools`
   只装 `musl-gcc`，需要 `ln -sf /usr/bin/musl-gcc /usr/local/bin/x86_64-linux-musl-gcc`。
5. **时限**：全 workspace 的 `check + test + clippy + release-build + 两条 cross-check`
   在三核 runner 上超过 2 小时；Linux 150 分钟、Windows/macOS 240 分钟、visual 120 分钟
   是按实测给的预算，不是估算。
6. **阻塞读**：测试基建里把握手失败的 piped stderr 读到 EOF，而 control 会**故意 detach**
   一个 node 进程、那个孙进程握着管道写端，EOF 永不到来——`test` 步骤因此挂 147 分钟，
   表现为"卡住"而不是"红"。stderr 落文件、诊断读文件即可。

## 三、定位顺序（先看什么）

1. `[gate] <step>: FAIL/PASS` 行给出第一步失败的位置，比 grep `error` 快；
2. 若失败在 `check` 之前，几乎总是系统依赖（pkg-config 或链接器）；
3. 若失败在 `test` 且耗时异常长，先怀疑阻塞读或超时预算，而不是断言；
4. 若 job 是 `cancelled` 而非 `failure`，先看 `timeout-minutes`，不要当成内容失败。

## 四、一次成功的基线

run `44879084747`（commit `ec596ab`）的 Linux gate 全绿，可作为对照：

```
format 4.9s · helper-tests 0.2s · single-binary 0.07s · docs 0.17s
check 194.5s · test 667.5s · lifecycle-cleanup 134.6s · clippy 63.8s
release-build 818.1s · cross-check musl 169.9s · cross-check windows-gnu 185.9s
lifecycle-cleanup-route/dns/tun/exit-rules · benchmark 39.3s
```

同一提交上 `docs`、`contracts`、`governance`、`supply-chain`、`api-security`、
`gpui-smoke`、`android` 七个 job 也为 success。