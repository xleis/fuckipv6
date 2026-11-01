"use strict";

(() => {
  const i18n = window.FuckIPV6I18n;
  if (!i18n) throw new Error("recovery-contract: shared i18n runtime is missing");

  const ERROR_KEYS = Object.freeze({
    forbidden: "owner_auth.recovery.error.forbidden",
    invalid_ticket: "owner_auth.recovery.error.invalid_ticket",
    query_not_allowed: "owner_auth.recovery.error.query_not_allowed",
    recovery_conflict: "owner_auth.recovery.error.state_changed",
    already_running: "owner_auth.recovery.error.already_running",
    complete: "owner_auth.recovery.error.complete",
    cancelled: "owner_auth.recovery.error.cancelled",
    registration_missing: "owner_auth.recovery.error.registration_missing",
    registration_failed: "owner_auth.recovery.error.registration_failed",
    internal_error: "owner_auth.recovery.error.internal",
    recovery_completed_audit_failed: "owner_auth.recovery.error.committed_audit_failed",
    state_changed: "owner_auth.recovery.error.state_changed",
    save_not_confirmed: "owner_auth.recovery.error.save_not_confirmed",
    no_pending_result: "owner_auth.recovery.error.no_pending_result",
  });

  const $ = (id) => document.getElementById(id);
  const ticket = document.body.dataset.recoveryTicket || "";
  const expectedOrigin = document.body.dataset.recoveryOrigin || "";
  const introStep = $("step-intro");
  const recoveryStep = $("step-recovery");
  const completeStep = $("step-complete");
  const completeMessage = $("complete-message");
  const beginButton = $("begin-recovery");
  const cancelButton = $("cancel-recovery");
  const cancelAfterRegistrationButton = $("cancel-after-registration");
  const recoveryCodeElement = $("recovery-code");
  const copyButton = $("copy-recovery");
  const savedCheckbox = $("recovery-saved");
  const confirmButton = $("confirm-recovery");
  const closeButton = $("close-window");
  const statusBox = $("status");
  const languageSelect = $("language-preference");

  let currentRecoveryCode = "";
  let languagePreference = "system";
  let locale = i18n.resolveLocale(languagePreference);
  let statusState = null;
  let completeKey = "";
  let beginActionKey = "owner_auth.recovery.begin";
  let confirmSubmitting = false;

  function hasKey(key) {
    const messages = i18n.catalog && i18n.catalog.messages && i18n.catalog.messages[locale];
    return Boolean(key && messages && Object.prototype.hasOwnProperty.call(messages, key));
  }

  function t(key) {
    return i18n.t(locale, key);
  }

  function tr(key, params) {
    return i18n.tr(locale, key, params);
  }

  function localizedError(key, params, payload) {
    const error = new Error(key);
    error.key = key;
    error.params = params || {};
    error.payload = payload || null;
    return error;
  }

  function payloadErrorKey(payload) {
    const explicit = payload && typeof payload.message_key === "string" ? payload.message_key : "";
    if (hasKey(explicit)) return explicit;
    const code = payload && typeof payload.code === "string" ? payload.code : "";
    return ERROR_KEYS[code] || "";
  }

  function errorPresentation(error, fallbackKey) {
    const explicitKey = error && error.key && hasKey(error.key) ? error.key : "";
    const responseKey = error && error.payload ? payloadErrorKey(error.payload) : "";
    return {
      key: explicitKey || responseKey || fallbackKey,
      params: (error && error.params) || {},
    };
  }

  function showStatusKey(key, kind = "", params = {}) {
    statusState = { key, kind, params };
    renderStatus();
  }

  function showError(error, fallbackKey) {
    const presentation = errorPresentation(error, fallbackKey);
    showStatusKey(presentation.key, "error", presentation.params);
  }

  function renderStatus() {
    if (!statusState || !hasKey(statusState.key)) {
      statusBox.textContent = "";
      statusBox.className = "status";
      return;
    }
    statusBox.textContent = tr(statusState.key, statusState.params);
    statusBox.className = `status show ${statusState.kind}`.trim();
  }

  function clearStatus() {
    statusState = null;
    renderStatus();
  }

  function showStep(step) {
    for (const candidate of [introStep, recoveryStep, completeStep]) {
      candidate.classList.toggle("active", candidate === step);
    }
  }

  function renderDynamicText() {
    document.title = `fuckipv6 · ${t("owner_auth.recovery.page_title")}`;
    beginButton.textContent = tr(beginActionKey);
    confirmButton.textContent = tr(
      confirmSubmitting
        ? "owner_auth.recovery.status.submitting"
        : "owner_auth.recovery.complete_button",
    );
    completeMessage.textContent = completeKey && hasKey(completeKey) ? t(completeKey) : "";
  }

  function applyLanguage() {
    locale = i18n.resolveLocale(languagePreference);
    languageSelect.value = languagePreference;
    i18n.applyDocument(document, locale);
    renderDynamicText();
    renderStatus();
  }

  function webAuthnIssue(expected) {
    if (!window.isSecureContext) return { key: "owner_auth.issue.secure_context", params: {} };
    if (!window.PublicKeyCredential || !navigator.credentials) {
      return { key: "owner_auth.issue.passkey_unsupported", params: {} };
    }
    if (expected && window.origin !== expected) {
      return {
        key: "owner_auth.issue.origin_mismatch",
        params: { expected, actual: window.origin },
      };
    }
    return null;
  }

  function showWebAuthnIssue(expected) {
    const issue = webAuthnIssue(expected);
    if (!issue) return false;
    showStatusKey(issue.key, "error", issue.params);
    return true;
  }

  function base64urlToBuffer(value) {
    if (typeof value !== "string" || value.length === 0) {
      throw localizedError("owner_auth.recovery.status.invalid_challenge");
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
      throw localizedError("owner_auth.recovery.status.invalid_server_options");
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
        throw localizedError("owner_auth.request.invalid_json", { status: response.status });
      }
    }
    if (!response.ok) {
      const key = payloadErrorKey(payload) || "owner_auth.request.failed";
      throw localizedError(key, { status: response.status }, payload);
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
    if (showWebAuthnIssue(expectedOrigin)) return;

    clearStatus();
    beginButton.disabled = true;
    cancelButton.disabled = true;
    beginActionKey = "owner_auth.recovery.status.waiting";
    renderDynamicText();
    try {
      const challenge = await requestJson("/api/recovery/start", {
        method: "POST",
        body: JSON.stringify({ ticket }),
      });
      beginActionKey = "owner_auth.recovery.status.waiting";
      renderDynamicText();
      const credential = await navigator.credentials.create(decodeCreationOptions(challenge));
      if (!credential) {
        throw localizedError("owner_auth.recovery.status.cancelled_registration");
      }
      beginActionKey = "owner_auth.recovery.status.verifying";
      renderDynamicText();
      const result = await requestJson("/api/recovery/finish", {
        method: "POST",
        body: JSON.stringify({
          ticket,
          credential: encodeRegistrationCredential(credential),
        }),
      });
      if (!result || typeof result.recovery_code !== "string" || result.recovery_code.length < 20) {
        throw localizedError("owner_auth.recovery.status.missing_new_code");
      }
      currentRecoveryCode = result.recovery_code;
      recoveryCodeElement.textContent = currentRecoveryCode;
      showStep(recoveryStep);
      showStatusKey("owner_auth.recovery.status.verified", "ok");
      recoveryCodeElement.focus();
    } catch (error) {
      showError(error, "owner_auth.recovery.status.registration_failed");
      beginButton.disabled = false;
      cancelButton.disabled = false;
    } finally {
      beginActionKey = "owner_auth.recovery.begin";
      renderDynamicText();
    }
  }

  async function copyRecoveryCode() {
    if (!currentRecoveryCode) return;
    try {
      await navigator.clipboard.writeText(currentRecoveryCode);
      showStatusKey("owner_auth.recovery.status.copied", "ok");
    } catch (_error) {
      showStatusKey("owner_auth.recovery.status.clipboard_denied", "warn");
    }
  }

  async function confirmRecovery() {
    if (!savedCheckbox.checked || !currentRecoveryCode) return;
    clearStatus();
    setRecoveryButtonsDisabled(true);
    confirmSubmitting = true;
    renderDynamicText();
    try {
      await requestJson("/api/recovery/confirm", {
        method: "POST",
        body: JSON.stringify({ ticket, confirmed: true }),
      });
      currentRecoveryCode = "";
      recoveryCodeElement.textContent = "";
      completeKey = "owner_auth.recovery.status.completed";
      showStep(completeStep);
      renderDynamicText();
      closeButton.focus();
    } catch (error) {
      showError(error, "owner_auth.recovery.status.submit_failed");
      setRecoveryButtonsDisabled(false);
    } finally {
      confirmSubmitting = false;
      renderDynamicText();
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
      completeKey = "owner_auth.recovery.status.cancelled";
      showStep(completeStep);
      renderDynamicText();
      closeButton.focus();
    } catch (error) {
      showError(error, "owner_auth.recovery.status.cancel_failed");
      setRecoveryButtonsDisabled(false);
    }
  }

  languageSelect.addEventListener("change", () => {
    languagePreference = languageSelect.value;
    applyLanguage();
  });
  beginButton.addEventListener("click", beginRecovery);
  cancelButton.addEventListener("click", cancelRecovery);
  cancelAfterRegistrationButton.addEventListener("click", cancelRecovery);
  copyButton.addEventListener("click", copyRecoveryCode);
  savedCheckbox.addEventListener("change", () => {
    confirmButton.disabled = !savedCheckbox.checked || !currentRecoveryCode;
  });
  confirmButton.addEventListener("click", confirmRecovery);
  closeButton.addEventListener("click", () => window.close());

  applyLanguage();
  if (showWebAuthnIssue(expectedOrigin) || !ticket || !expectedOrigin) {
    if (!ticket || !expectedOrigin) {
      showStatusKey("owner_auth.recovery.status.missing_context", "error");
    }
    setRecoveryButtonsDisabled(true);
  }
})();
