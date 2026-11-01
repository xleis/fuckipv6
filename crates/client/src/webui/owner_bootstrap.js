"use strict";

(() => {
  const i18n = window.FuckIPV6I18n;
  if (!i18n) throw new Error("owner-bootstrap-contract: shared i18n runtime is missing");
  const LANGUAGE_STORAGE_KEY = "fuckipv6.language-preference";
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
  const languageSelect = $("language-preference");

  let currentRecoveryCode = "";
  let currentExpectedOrigin = "";
  let languagePreference = readLanguagePreference();
  let locale = i18n.resolveLocale(languagePreference);
  let statusState = null;
  let primaryAction = { key: "owner_auth.bootstrap.verify_button", params: {} };

  function readLanguagePreference() {
    try {
      const stored = window.localStorage.getItem(LANGUAGE_STORAGE_KEY);
      if (stored === "system" || stored === "zh-CN" || stored === "en") return stored;
    } catch (_error) {
      // Storage may be unavailable in hardened browsers; system detection still works.
    }
    return "system";
  }

  function saveLanguagePreference(preference) {
    try {
      window.localStorage.setItem(LANGUAGE_STORAGE_KEY, preference);
    } catch (_error) {
      // The current page still switches even when persistence is unavailable.
    }
  }

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

  function apiErrorKey(payload, fallbackKey) {
    const candidate = payload && typeof payload.message_key === "string" ? payload.message_key : "";
    return hasKey(candidate) ? candidate : fallbackKey;
  }

  function errorPresentation(error, fallbackKey) {
    const explicitKey = error && error.key && hasKey(error.key) ? error.key : "";
    const payloadKey = error && error.payload ? apiErrorKey(error.payload, "") : "";
    const key = explicitKey || payloadKey || fallbackKey;
    return { key, params: (error && error.params) || {} };
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

  function setPrimaryAction(key, params = {}) {
    primaryAction = { key, params };
    startButton.textContent = tr(key, params);
  }

  function applySurfaceMode(origin) {
    const publicClaim = window.location.protocol === "https:";
    const titleKey = publicClaim
      ? "owner_auth.bootstrap.public_title"
      : "owner_auth.bootstrap.local_title";
    const title = t(titleKey);
    document.title = `fuckipv6 · ${title}`;
    pageTitle.textContent = title;
    footer.textContent = tr(
      publicClaim
        ? "owner_auth.bootstrap.footer_public"
        : "owner_auth.bootstrap.footer_local",
      { origin: origin || window.origin },
    );
  }

  function applyLanguage() {
    locale = i18n.resolveLocale(languagePreference);
    languageSelect.value = languagePreference;
    i18n.applyDocument(document, locale);
    applySurfaceMode(currentExpectedOrigin);
    renderStatus();
    startButton.textContent = tr(primaryAction.key, primaryAction.params);
    if (confirmButton.disabled) {
      confirmButton.textContent = tr("owner_auth.bootstrap.complete_button");
    }
  }

  function webAuthnIssue(expectedOrigin) {
    if (!window.isSecureContext) return t("owner_auth.issue.secure_context");
    if (!window.PublicKeyCredential || !navigator.credentials) {
      return t("owner_auth.issue.passkey_unsupported");
    }
    if (expectedOrigin && window.origin !== expectedOrigin) {
      return tr("owner_auth.issue.origin_mismatch", {
        expected: expectedOrigin,
        actual: window.origin,
      });
    }
    return "";
  }

  function base64urlToBuffer(value) {
    if (typeof value !== "string" || value.length === 0) {
      throw localizedError("owner_auth.request.invalid_challenge");
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
      throw localizedError("owner_auth.request.invalid_creation_options");
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
      throw localizedError("owner_auth.request.failed", { status: response.status }, payload);
    }
    return payload;
  }

  async function verifyBootstrapSurface() {
    applyLanguage();
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
        showStatusKey("owner_auth.issue.secure_context", "error");
        startButton.disabled = true;
        return false;
      }
      return true;
    } catch (error) {
      showError(error, "owner_auth.bootstrap.status.read_failed");
      startButton.disabled = true;
      return false;
    }
  }

  async function beginRegistration() {
    const code = codeInput.value.trim();
    if (code.length < 20) {
      showStatusKey("owner_auth.bootstrap.status.code_required", "error");
      codeInput.focus();
      return;
    }
    const issue = webAuthnIssue(currentExpectedOrigin);
    if (issue) {
      showStatusKey("owner_auth.issue.secure_context", "error");
      return;
    }

    clearStatus();
    startButton.disabled = true;
    setPrimaryAction("owner_auth.bootstrap.status.verifying");
    try {
      await requestJson("/api/v1/auth/bootstrap/verify", {
        method: "POST",
        body: JSON.stringify({ code }),
      });
      setPrimaryAction("owner_auth.bootstrap.status.waiting_passkey");
      const challenge = await requestJson("/api/v1/auth/webauthn/registration/start", {
        method: "POST",
        body: JSON.stringify({ code }),
      });
      const credential = await navigator.credentials.create(decodeCreationOptions(challenge));
      if (!credential) {
        throw localizedError("owner_auth.request.registration_cancelled");
      }
      setPrimaryAction("owner_auth.bootstrap.status.finishing");
      const result = await requestJson("/api/v1/auth/webauthn/registration/finish", {
        method: "POST",
        body: JSON.stringify({ credential: encodeRegistrationCredential(credential) }),
      });
      if (!result || typeof result.recovery_code !== "string" || result.recovery_code.length < 20) {
        throw localizedError("owner_auth.request.missing_recovery_code");
      }
      codeInput.value = "";
      currentRecoveryCode = result.recovery_code;
      recoveryCode.textContent = currentRecoveryCode;
      codeStep.classList.remove("active");
      recoveryStep.classList.add("active");
      showStatusKey("owner_auth.bootstrap.status.registered", "ok");
      recoveryCode.focus();
    } catch (error) {
      showError(error, "owner_auth.bootstrap.status.registration_failed");
    } finally {
      startButton.disabled = Boolean(webAuthnIssue(currentExpectedOrigin));
      setPrimaryAction("owner_auth.bootstrap.verify_button");
    }
  }

  async function copyRecoveryCode() {
    if (!currentRecoveryCode) return;
    try {
      await navigator.clipboard.writeText(currentRecoveryCode);
      showStatusKey("owner_auth.recovery.code_copied", "ok");
    } catch (_error) {
      showStatusKey("owner_auth.recovery.clipboard_denied", "warn");
    }
  }

  async function confirmRecovery() {
    if (!savedCheckbox.checked || !currentRecoveryCode) return;
    confirmButton.disabled = true;
    confirmButton.textContent = tr("owner_auth.bootstrap.status.submitting");
    try {
      await requestJson("/api/v1/auth/bootstrap/recovery-confirm", {
        method: "POST",
        body: JSON.stringify({ confirmed: true }),
      });
      currentRecoveryCode = "";
      recoveryCode.textContent = "";
      showStatusKey("owner_auth.bootstrap.status.complete", "ok");
      window.setTimeout(() => window.location.replace("/"), 400);
    } catch (error) {
      showError(error, "owner_auth.bootstrap.status.submit_failed");
      confirmButton.disabled = false;
      confirmButton.textContent = tr("owner_auth.bootstrap.complete_button");
    }
  }

  languageSelect.addEventListener("change", () => {
    languagePreference = languageSelect.value;
    saveLanguagePreference(languagePreference);
    applyLanguage();
  });
  startButton.addEventListener("click", beginRegistration);
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
  verifyBootstrapSurface();
})();
