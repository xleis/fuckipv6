"use strict";

(() => {
  const ticket = document.body.dataset.recoveryTicket || "";
  const expectedOrigin = document.body.dataset.recoveryOrigin || "";
  const introStep = document.getElementById("step-intro");
  const recoveryStep = document.getElementById("step-recovery");
  const completeStep = document.getElementById("step-complete");
  const completeMessage = document.getElementById("complete-message");
  const beginButton = document.getElementById("begin-recovery");
  const cancelButton = document.getElementById("cancel-recovery");
  const cancelAfterRegistrationButton = document.getElementById("cancel-after-registration");
  const recoveryCodeElement = document.getElementById("recovery-code");
  const copyButton = document.getElementById("copy-recovery");
  const savedCheckbox = document.getElementById("recovery-saved");
  const confirmButton = document.getElementById("confirm-recovery");
  const closeButton = document.getElementById("close-window");
  const statusBox = document.getElementById("status");

  let currentRecoveryCode = "";

  function showStatus(message, kind = "") {
    statusBox.textContent = message;
    statusBox.className = `status show ${kind}`.trim();
  }

  function clearStatus() {
    statusBox.textContent = "";
    statusBox.className = "status";
  }

  function showStep(step) {
    for (const candidate of [introStep, recoveryStep, completeStep]) {
      candidate.classList.toggle("active", candidate === step);
    }
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

  function setRecoveryButtonsDisabled(disabled) {
    beginButton.disabled = disabled;
    cancelButton.disabled = disabled;
    cancelAfterRegistrationButton.disabled = disabled;
    copyButton.disabled = disabled;
    confirmButton.disabled = disabled || !savedCheckbox.checked || !currentRecoveryCode;
    savedCheckbox.disabled = disabled;
  }

  async function beginRecovery() {
    if (!window.isSecureContext || !window.PublicKeyCredential || !navigator.credentials) {
      showStatus("当前浏览器不支持安全 WebAuthn。请使用支持 Passkey 的现代浏览器打开本机恢复页。", "error");
      return;
    }

    clearStatus();
    beginButton.disabled = true;
    cancelButton.disabled = true;
    beginButton.textContent = "正在准备...";
    try {
      const challenge = await requestJson("/api/recovery/start", {
        method: "POST",
        body: JSON.stringify({ ticket }),
      });
      beginButton.textContent = "等待 Passkey 授权...";
      const credential = await navigator.credentials.create(decodeCreationOptions(challenge));
      if (!credential) {
        throw new Error("Passkey 注册已取消");
      }
      beginButton.textContent = "正在验证新 Passkey...";
      const result = await requestJson("/api/recovery/finish", {
        method: "POST",
        body: JSON.stringify({
          ticket,
          credential: encodeRegistrationCredential(credential),
        }),
      });
      if (!result || typeof result.recovery_code !== "string" || result.recovery_code.length < 20) {
        throw new Error("服务端未返回有效的新恢复码");
      }
      currentRecoveryCode = result.recovery_code;
      recoveryCodeElement.textContent = currentRecoveryCode;
      showStep(recoveryStep);
      showStatus("新 Passkey 已验证。提交前现有凭据尚未改变。", "ok");
      recoveryCodeElement.focus();
    } catch (error) {
      showStatus(error && error.message ? error.message : "Passkey 注册失败", "error");
      beginButton.disabled = false;
      cancelButton.disabled = false;
    } finally {
      beginButton.textContent = "注册新 Passkey";
    }
  }

  async function copyRecoveryCode() {
    if (!currentRecoveryCode) {
      return;
    }
    try {
      await navigator.clipboard.writeText(currentRecoveryCode);
      showStatus("新恢复码已复制。请立即存入离线安全位置。", "ok");
    } catch (_error) {
      showStatus("浏览器拒绝剪贴板访问，请手动复制新恢复码。", "warn");
    }
  }

  async function confirmRecovery() {
    if (!savedCheckbox.checked || !currentRecoveryCode) {
      return;
    }
    clearStatus();
    setRecoveryButtonsDisabled(true);
    confirmButton.textContent = "正在提交...";
    try {
      await requestJson("/api/recovery/confirm", {
        method: "POST",
        body: JSON.stringify({ ticket, confirmed: true }),
      });
      currentRecoveryCode = "";
      recoveryCodeElement.textContent = "";
      showStep(completeStep);
      completeMessage.textContent = "本地所有者恢复已完成：旧 Passkey 和旧登录会话均已失效。临时恢复服务已经关闭。";
      closeButton.focus();
    } catch (error) {
      showStatus(error && error.message ? error.message : "本地恢复提交失败", "error");
      setRecoveryButtonsDisabled(false);
      confirmButton.textContent = "完成本地恢复";
    }
  }

  async function cancelRecovery() {
    clearStatus();
    setRecoveryButtonsDisabled(true);
    try {
      await requestJson("/api/recovery/cancel", {
        method: "POST",
        body: JSON.stringify({ ticket }),
      });
      currentRecoveryCode = "";
      recoveryCodeElement.textContent = "";
      showStep(completeStep);
      completeMessage.textContent = "本地恢复已取消，现有 Passkey 和登录会话没有改变。临时恢复服务已经关闭。";
      closeButton.focus();
    } catch (error) {
      showStatus(error && error.message ? error.message : "取消本地恢复失败", "error");
      setRecoveryButtonsDisabled(false);
    }
  }

  if (!ticket || !expectedOrigin) {
    showStatus("恢复页面缺少一次性安全上下文。请关闭本页并从终端重新启动恢复。", "error");
    setRecoveryButtonsDisabled(true);
    return;
  }
  if (window.origin !== expectedOrigin) {
    showStatus(`WebAuthn 地址不匹配。请使用 ${expectedOrigin} 打开本页。`, "error");
    setRecoveryButtonsDisabled(true);
    return;
  }

  beginButton.addEventListener("click", beginRecovery);
  cancelButton.addEventListener("click", cancelRecovery);
  cancelAfterRegistrationButton.addEventListener("click", cancelRecovery);
  copyButton.addEventListener("click", copyRecoveryCode);
  savedCheckbox.addEventListener("change", () => {
    confirmButton.disabled = !savedCheckbox.checked || !currentRecoveryCode;
  });
  confirmButton.addEventListener("click", confirmRecovery);
  closeButton.addEventListener("click", () => window.close());
})();
