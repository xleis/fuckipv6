"use strict";

(() => {
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

  let currentRecoveryCode = "";

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

  async function verifyLocalSurface() {
    try {
      const status = await requestJson("/api/v1/auth/bootstrap", { method: "GET" });
      if (status.initialized) {
        window.location.replace("/");
        return false;
      }
      if (status.origin && window.origin !== status.origin) {
        showStatus(`WebAuthn 地址不匹配。请使用 ${status.origin} 打开本页。`, "error");
        startButton.disabled = true;
        return false;
      }
      return true;
    } catch (error) {
      showStatus(error && error.message ? error.message : "无法读取本机初始化状态", "error");
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
    if (!window.isSecureContext || !window.PublicKeyCredential || !navigator.credentials) {
      showStatus("当前浏览器不支持安全 WebAuthn。请使用 http://localhost 打开本页，并使用现代浏览器。", "error");
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
      startButton.disabled = false;
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
      showStatus("本机所有者初始化完成，正在进入控制台...", "ok");
      window.setTimeout(() => window.location.replace("/"), 400);
    } catch (error) {
      showStatus(error && error.message ? error.message : "初始化提交失败", "error");
      confirmButton.disabled = false;
      confirmButton.textContent = "完成本机初始化";
    }
  }

  startButton.addEventListener("click", beginRegistration);
  verifyLocalSurface();
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
