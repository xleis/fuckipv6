//! Local-only owner bootstrap page and browser WebAuthn adapter.

pub(super) const OWNER_BOOTSTRAP_HTML: &str = include_str!("owner_bootstrap.html");
pub(super) const OWNER_BOOTSTRAP_JS: &str = include_str!("owner_bootstrap.js");

pub(super) const OWNER_UNAVAILABLE_HTML: &str = r#"<!doctype html>
<html lang="zh-CN" data-theme="dark">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <meta name="referrer" content="no-referrer">
  <title>fuckipv6 · 安全状态异常</title>
  <style nonce="__CSP_NONCE__">
    :root{color-scheme:dark;--bg:#061018;--panel:#0c1c27;--line:#1b4356;--text:#d9eef7;--muted:#83a7b7;--bad:#fb7185}
    *{box-sizing:border-box}body{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px;background:radial-gradient(circle at 72% 0,#10374c 0,var(--bg) 44%);color:var(--text);font:14px/1.6 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}
    main{width:min(620px,100%);border:1px solid #71313d;border-radius:18px;background:linear-gradient(180deg,#23151c,#0a1720);box-shadow:0 24px 80px #0008;padding:30px}
    .brand{color:var(--bad);font-weight:800;letter-spacing:.11em}h1{font-size:24px;margin:14px 0 10px}p{color:var(--muted);margin:0 0 12px}code{color:var(--text)}
  </style>
</head>
<body>
  <main>
    <div class="brand">fuckipv6</div>
    <h1>安全状态异常，控制台已锁定</h1>
    <p>节点无法可靠读取所有者安全状态，因此不会回退到未初始化流程或管理界面。</p>
    <p>请在连接控制终端后重启程序，并检查配置目录中的 <code>owner/</code> 记录与权限。</p>
  </main>
</body>
</html>"#;
