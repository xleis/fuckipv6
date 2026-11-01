"use strict";

(() => {
  const authCopy = Object.assign({
    bootstrapDescription: "这个节点只属于一位所有者。请使用节点所有者可访问终端中显示的一次性初始化码注册 Passkey，并离线保存随后生成的恢复码。",
    bootstrapCodeHelp: "初始化码只通过节点所有者可访问的终端显示，不会写入 URL、日志或磁盘明文。",
    secureContextRequirements: "当前页面不是受信任的 WebAuthn 安全上下文。请使用配置的 HTTPS 公网地址或 http://localhost；证书或域名错误必须先修复，不能跳过校验。",
    passkeyUnsupported: "当前浏览器或平台不支持所需的 Passkey 能力。请使用支持 WebAuthn 的现代浏览器和系统认证器。",
    originMismatchTemplate: "WebAuthn 地址不匹配。预期 {expected}，当前 {actual}。请使用 {expected} 打开；若浏览器报告证书错误，必须先修复证书，不能跳过校验。",
    recoveryCodeWarning: "恢复码只显示这一次。全部 Passkey 丢失时，它是本机恢复的唯一凭据，请立即保存到离线安全位置。",
    recoveryCodeConfirmation: "我已将恢复码保存到离线安全位置，并确认页面关闭后无法再次查看。",
  }, window.FuckIPV6OwnerAuthCopy || {});
  const $ = (id) => document.getElementById(id);
  const codeStep = $("step-code");
  const recoveryStep = $("step-recovery");
  const codeInput = $("bootstrap-code");
  const startButton = $("start-passkey");
  const recoveryCode = $("recovery-code");
  const copyButton = $("copy-recovery");
  const savedCheckbox = $("recovery-saved");
  const confirmButton = $("confirm-recovery");
  const statusBox = $("status");
  const pageTitle = $("page-title");
  const footer = $("bootstrap-footer");

  let currentRecoveryCode = "";
  let currentExpectedOrigin = "";

  function applyAuthCopy() {
    $("bootstrap-description").textContent = authCopy.bootstrapDescription;
    $("bootstrap-code-help").textContent = authCopy.bootstrapCodeHelp;
    $("recovery-warning").textContent = authCopy.recoveryCodeWarning;
    $("recovery-confirmation").textContent = authCopy.recoveryCodeConfirmation;
  }

  function formatCopy(template, values) {
    return Object.entries(values).reduce(
      (text, [key, value]) => text.replaceAll(`{${key}}`, String(value)),
      String(template || ""),
    );
  }

  function webAuthnIssue(expectedOrigin) {
    if (!window.isSecureContext) return authCopy.secureContextRequirements;
    if (!window.PublicKeyCredential || !navigator.credentials) return authCopy.passkeyUnsupported;
    if (expectedOrigin && window.origin !== expectedOrigin) {
      return formatCopy(authCopy.originMismatchTemplate, { expected: expectedOrigin, actual: window.origin });
    }
    return "";
  }

  function showStatus(message, kind = "") {
    statusBox.textContent = message;
    statusBox.className = `status show ${kind}`.trim();
  }

  function clearStatus() {
    statusBox.textContent = "";
    statusBox.className = "status";
  }

  function base64urlToBuffer(value) {
    if (typeof value !== "string" || value.length === 0) {
      throw new Error("WebAuthn challenge 缺少有效的 base64url 数据");
    }
    const normalized = value.replace(/-/g, "+").replace(/_/g, "/");
    const padding = "=".repeat((4 - (normalized.length % 4)) % 4);
    const binary = atob(normalized + padding);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) {
      bytes[index] = binary.charCodeAt(index);
    }
    return bytes.buffer;
  }

  function bufferToBase64url(value) {
    const bytes = new Uint8Array(value);
    let binary = "";
    for (let offset = 0; offset < bytes.length; offset += 0x8000) {
      binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
    }
    return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
  }

  function decodeCreationOptions(challenge) {
    const options = challenge && challenge.publicKey;
    if (!options || typeof options !== "object") {
      throw new Error("服务端返回的 Passkey 注册参数无效");
    }
    const decoded = {
      ...options,
      challenge: base64urlToBuffer(options.challenge),
      user: {
        ...options.user,
        id: base64urlToBuffer(options.user && options.user.id),
      },
    };
    if (Array.isArray(options.excludeCredentials)) {
      decoded.excludeCredentials = options.excludeCredentials.map((credential) => ({
        ...credential,
        id: base64urlToBuffer(credential.id),
      }));
    }
    return { publicKey: decoded };
  }

  function encodeRegistrationCredential(credential) {
    const response = credential.response;
    const transports = typeof response.getTransports === "function" ? response.getTransports() : undefined;
    return {
      id: credential.id,
      rawId: bufferToBase64url(credential.rawId),
      type: credential.type,
      response: {
        attestationObject: bufferToBase64url(response.attestationObject),
        clientDataJSON: bufferToBase64url(response.clientDataJSON),
        ...(Array.isArray(transports) ? { transports } : {}),
      },
      clientExtensionResults: credential.getClientExtensionResults
        ? credential.getClientExtensionResults()
        : {},
    };
  }

  async function requestJson(path, options) {
    const response = await fetch(path, {
      cache: "no-store",
      credentials: "same-origin",
      ...options,
      headers: {
        "Content-Type": "application/json",
        ...(options && options.headers ? options.headers : {}),
      },
    });
    const text = await response.text();
    let payload = null;
    if (text) {
      try {
        payload = JSON.parse(text);
      } catch (_error) {
        throw new Error(`服务端返回了无效响应（HTTP ${response.status}）`);
      }
    }
    if (!response.ok) {
      throw new Error((payload && payload.error) || `请求失败（HTTP ${response.status}）`);
    }
    return payload;
  }

  function applySurfaceMode(origin) {
    const publicClaim = window.location.protocol === "https:";
    const title = publicClaim ? "公网所有者认领" : "本机所有者初始化";
    document.title = `fuckipv6 · ${title}`;
    pageTitle.textContent = title;
    footer.textContent = publicClaim
      ? `HTTPS 公网认领 · Origin ${origin} · 来源校验、限流与审计已启用`
      : `本机回环初始化 · WebAuthn origin ${origin}`;
  }

  async function verifyBootstrapSurface() {
    applyAuthCopy();
    try {
      const status = await requestJson("/api/v1/auth/bootstrap", { method: "GET" });
      if (status.initialized) {
        window.location.replace("/");
        return false;
      }
      currentExpectedOrigin = status.origin || window.origin;
      applySurfaceMode(currentExpectedOrigin);
      const issue = webAuthnIssue(currentExpectedOrigin);
      if (issue) {
        showStatus(issue, "error");
        startButton.disabled = true;
        return false;
      }
      return true;
    } catch (error) {
      showStatus(error && error.message ? error.message : "无法读取所有者初始化状态", "error");
      startButton.disabled = true;
      return false;
    }
  }

  async function beginRegistration() {
    const code = codeInput.value.trim();
    if (code.length < 20) {
      showStatus("请输入终端中完整的一次性初始化码。", "error");
      codeInput.focus();
      return;
    }
    const issue = webAuthnIssue(currentExpectedOrigin);
    if (issue) {
      showStatus(issue, "error");
      return;
    }

    clearStatus();
    startButton.disabled = true;
    startButton.textContent = "正在验证...";
    try {
      await requestJson("/api/v1/auth/bootstrap/verify", {
        method: "POST",
        body: JSON.stringify({ code }),
      });
      startButton.textContent = "等待 Passkey 授权...";
      const challenge = await requestJson("/api/v1/auth/webauthn/registration/start", {
        method: "POST",
        body: JSON.stringify({ code }),
      });
      const credential = await navigator.credentials.create(decodeCreationOptions(challenge));
      if (!credential) {
        throw new Error("Passkey 注册已取消");
      }
      startButton.textContent = "正在完成注册...";
      const result = await requestJson("/api/v1/auth/webauthn/registration/finish", {
        method: "POST",
        body: JSON.stringify({ credential: encodeRegistrationCredential(credential) }),
      });
      if (!result || typeof result.recovery_code !== "string" || result.recovery_code.length < 20) {
        throw new Error("服务端未返回有效恢复码");
      }
      codeInput.value = "";
      currentRecoveryCode = result.recovery_code;
      recoveryCode.textContent = currentRecoveryCode;
      codeStep.classList.remove("active");
      recoveryStep.classList.add("active");
      showStatus("Passkey 已注册。请先离线保存恢复码，再完成初始化。", "ok");
      recoveryCode.focus();
    } catch (error) {
      showStatus(error && error.message ? error.message : "Passkey 注册失败", "error");
    } finally {
      startButton.disabled = Boolean(webAuthnIssue(currentExpectedOrigin));
      startButton.textContent = "验证并注册 Passkey";
    }
  }

  async function copyRecoveryCode() {
    if (!currentRecoveryCode) {
      return;
    }
    try {
      await navigator.clipboard.writeText(currentRecoveryCode);
      showStatus("恢复码已复制。请立即存入离线安全位置。", "ok");
    } catch (_error) {
      showStatus("浏览器拒绝剪贴板访问，请手动复制恢复码。", "warn");
    }
  }

  async function confirmRecovery() {
    if (!savedCheckbox.checked || !currentRecoveryCode) {
      return;
    }
    confirmButton.disabled = true;
    confirmButton.textContent = "正在提交...";
    try {
      await requestJson("/api/v1/auth/bootstrap/recovery-confirm", {
        method: "POST",
        body: JSON.stringify({ confirmed: true }),
      });
      currentRecoveryCode = "";
      recoveryCode.textContent = "";
      showStatus("所有者初始化完成，正在进入控制台...", "ok");
      window.setTimeout(() => window.location.replace("/"), 400);
    } catch (error) {
      showStatus(error && error.message ? error.message : "初始化提交失败", "error");
      confirmButton.disabled = false;
      confirmButton.textContent = "完成所有者初始化";
    }
  }

  startButton.addEventListener("click", beginRegistration);
  verifyBootstrapSurface();
  copyButton.addEventListener("click", copyRecoveryCode);
  savedCheckbox.addEventListener("change", () => {
    confirmButton.disabled = !savedCheckbox.checked;
  });
  confirmButton.addEventListener("click", confirmRecovery);
  codeInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      beginRegistration();
    }
  });
})();
