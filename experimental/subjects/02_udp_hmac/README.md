# 科目 02 — UDP HMAC 认证

## 1. 科目内容

验证 sidecar 之间的 UDP 控制报文具备：

1. 预共享密钥认证；
2. 时间戳新鲜度检查；
3. nonce 重放防护；
4. 类型/长度校验；
5. 认证失败时静默丢弃。

V4 只做了**正向认证**（合法 HELLO/HELLO_ACK/SYNACK 能通过）；伪造、篡改、重放等负向测试未在 V4 验收中执行，这是明确的测试缺口。

## 2. 代码位置与入口

- [`../../code/lxt_public_synack_v4/lxt_public_lab.py`](../../code/lxt_public_synack_v4/lxt_public_lab.py)
  - `MAGIC = b"LXTPUB4"`、`HELLO, HELLO_ACK, SYNACK = 1, 2, 3`（第 31–32 行）
  - `load_key()`（第 37 行）：从 `LXT_LAB_KEY` 读取 hex，要求 ≥32 字节。
  - `mac_msg()`（第 48 行）：构造报文并追加 HMAC-SHA256 前 16 字节。
  - `check_msg()`（第 55 行）：校验 MAGIC、长度、时间戳 ±30 秒、HMAC、nonce 重放。
- 服务端认证入口：`run_server_sidecar()` 的 UDP 分支。
- 客户端认证入口：`run_client_sidecar()` 的 UDP 分支。

## 3. 报文格式

```text
MAGIC (7B "LXTPUB4")
type (1B: 1=HELLO, 2=HELLO_ACK, 3=SYNACK)
timestamp (u64, big-endian, Unix 秒)
nonce (12B, secrets.token_bytes)
body_len (u16, big-endian)
body (0..2000B)
tag (16B, HMAC-SHA256(key, 前面所有字节) 的前 16 字节)
```

实现细节：

- `body` 对 `SYNACK` 来说就是完整的 IPv4 包（不含 Ethernet 头）。
- `check_msg()` 先检查 `len(data) >= header + 16`、MAGIC、时间戳；再检查长度字段与实际长度一致；最后 `hmac.compare_digest()` 比较 tag。
- nonce 存在 `collections.OrderedDict` 中，重复 nonce 直接拒绝；表上限 4096，超出后按插入顺序淘汰。
- 服务端只接受 `type == HELLO`，把 `(addr, monotonic)` 存入 `peers[addr[0]]`，有效期 8 秒；随后回复 `HELLO_ACK`。
- 客户端每 `--hello-interval`（默认 1.5 秒）发一次 HELLO；收到 `HELLO_ACK` 打印 `HELLO_ACK verified`。
- 服务端/客户端对认证失败的包不回复、不记录敏感内容，直接 `continue`。

## 4. 复现与验证

```bash
# 两端使用同一个 64-hex key；只在实验进程环境里存在
export LXT_LAB_KEY="$(openssl rand -hex 32)"   # 示例；真实测试不要把值发到聊天/日志

# 服务端 sidecar
sudo env LXT_LAB_KEY="$LXT_LAB_KEY" python3 lxt_public_lab.py server-sidecar \
  --interface eth0 --tcp-port 39090 --udp-port 39091

# 客户端 sidecar
sudo env LXT_LAB_KEY="$LXT_LAB_KEY" python3 lxt_public_lab.py client-sidecar \
  --interface eth0 --server <SERVER_IP> --tcp-port 39090 --udp-port 39091
```

正向证据：

- 服务端：`[server-sidecar] HELLO verified from <CLIENT_IP>:<PORT>`
- 客户端：`[client-sidecar] HELLO_ACK verified`
- 每次救援的 SYNACK 都必须先通过 `check_msg()` 才会进入注入分支；V4 的 4 个救援 SYNACK 均通过。

## 5. V4 结果

- 正向认证：PASS。
- 服务端/客户端均能稳定完成 HELLO / HELLO_ACK。
- 4 个救援 SYNACK 均通过 HMAC 后才注入 TUN。
- 负向（错误 key、篡改、过期时间戳、重放 nonce）未执行；代码具备这些检查，但没有对应的验收数据。

## 6. 关联报告

- V4：第 3 节“UDP HMAC 认证”、第 7 节限制。
- V7/V8：同一套 sidecar 在隔离 netns 中重复认证（`HELLO verified` / `HELLO_ACK verified`）。

## 7. 安全注意

- 不要提交 key；不要把 key 放进 argv（会出现在 `ps`/日志）；本目录中的命令统一用 `env LXT_LAB_KEY=...` 从环境读取。
- V4 旧 PSK 在会话记录中曾暴露，已建议轮换；V8 使用的新临时 key 已删除。
