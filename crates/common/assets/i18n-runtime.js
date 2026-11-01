(function (root) {
  "use strict";

  const catalog = __FUCKIPV6_I18N_CATALOG__;
  const messages = catalog && catalog.messages;
  if (!messages || typeof messages !== "object") {
    throw new Error("i18n catalog is missing");
  }

  const locales = Object.freeze(Object.keys(messages));
  const defaultLocale = locales.includes(catalog.default_locale) ? catalog.default_locale : locales[0];

  function normalizeLocale(tag) {
    const normalized = String(tag || "").trim().replace(/_/g, "-").toLowerCase();
    return normalized === "zh" || normalized.startsWith("zh-") ? "zh-CN" : "en";
  }

  function detectLocale(tags) {
    const candidates = Array.isArray(tags) ? tags : (typeof navigator !== "undefined"
      ? (navigator.languages && navigator.languages.length ? navigator.languages : [navigator.language])
      : []);
    for (const candidate of candidates) {
      if (typeof candidate === "string" && candidate.trim()) {
        return normalizeLocale(candidate);
      }
    }
    return defaultLocale;
  }

  function resolveLocale(preference, systemLocale) {
    if (preference === "zh-CN" || preference === "en") return preference;
    return normalizeLocale(systemLocale || detectLocale());
  }

  function interpolate(template, params) {
    let text = String(template);
    if (!params || typeof params !== "object") return text;
    for (const [name, value] of Object.entries(params)) {
      text = text.split("{" + name + "}").join(String(value));
    }
    return text;
  }

  function t(locale, key) {
    const language = messages[normalizeLocale(locale)] || messages[defaultLocale] || {};
    return Object.prototype.hasOwnProperty.call(language, key) ? language[key] : key;
  }

  function tr(locale, key, params) {
    return interpolate(t(locale, key), params);
  }

  const groupSeparator = ",";
  const decimalSeparator = ".";

  function formatInteger(locale, value) {
    const number = Number(value);
    if (!Number.isSafeInteger(number)) return t(locale, "common.not_available");
    const sign = number < 0 ? "-" : "";
    const digits = String(Math.abs(number));
    let grouped = "";
    for (let index = 0; index < digits.length; index += 1) {
      if (index > 0 && (digits.length - index) % 3 === 0) grouped += groupSeparator;
      grouped += digits[index];
    }
    return sign + grouped;
  }

  function formatDecimal(locale, value, precision) {
    const number = Number(value);
    if (!Number.isFinite(number)) return t(locale, "common.not_available");
    const places = Number.isInteger(precision) && precision >= 0 ? precision : 1;
    const [integer, fraction = ""] = Math.abs(number).toFixed(places).split(".");
    const formatted = (number < 0 ? "-" : "") + formatInteger(locale, Number(integer));
    return places === 0 ? formatted : formatted + decimalSeparator + fraction;
  }

  function formatBytes(locale, bytes) {
    const units = [["B", 1], ["KiB", 1024], ["MiB", 1048576], ["GiB", 1073741824]];
    const value = Math.max(0, Number(bytes) || 0);
    let selected = units[0];
    for (let index = units.length - 1; index >= 0; index -= 1) {
      if (value >= units[index][1]) { selected = units[index]; break; }
    }
    const amount = selected[1] === 1 ? formatInteger(locale, value) : formatDecimal(locale, value / selected[1], 1);
    return amount + " " + selected[0];
  }

  function formatDuration(locale, secondsValue) {
    const seconds = Math.max(0, Math.floor(Number(secondsValue) || 0));
    if (seconds < 60) return locale === "zh-CN" ? seconds + " 秒" : seconds + "s";
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) {
      const remainder = seconds % 60;
      return locale === "zh-CN" ? minutes + " 分 " + remainder + " 秒" : minutes + "m " + remainder + "s";
    }
    const hours = Math.floor(minutes / 60);
    const remainder = minutes % 60;
    return locale === "zh-CN" ? hours + " 小时 " + remainder + " 分" : hours + "h " + remainder + "m";
  }

  function formatTimestamp(locale, unixMs) {
    const date = new Date(Number(unixMs));
    if (!Number.isFinite(date.getTime())) return t(locale, "common.not_available");
    const offsetMinutes = -date.getTimezoneOffset();
    const sign = offsetMinutes < 0 ? "-" : "+";
    const absolute = Math.abs(offsetMinutes);
    const zone = "UTC" + sign + String(Math.floor(absolute / 60)).padStart(2, "0") + ":" + String(absolute % 60).padStart(2, "0");
    if (locale === "zh-CN") {
      const pad = (value) => String(value).padStart(2, "0");
      return date.getFullYear() + "-" + pad(date.getMonth() + 1) + "-" + pad(date.getDate()) + " " +
        pad(date.getHours()) + ":" + pad(date.getMinutes()) + ":" + pad(date.getSeconds()) + " " + zone;
    }
    return new Intl.DateTimeFormat("en", {
      year: "numeric", month: "short", day: "numeric",
      hour: "numeric", minute: "2-digit", second: "2-digit",
    }).format(date) + " " + zone;
  }

  function formatAddress(address, port) {
    const text = String(address || "");
    if (port === undefined || port === null || port === "") return text;
    const bracketed = text.includes(":") && !text.startsWith("[") ? "[" + text + "]" : text;
    return bracketed + ":" + String(port);
  }

  function pseudoLocalize(text) {
    const expanded = String(text).replace(/[aeiouAEIOU]/g, (character) => ({
      a: "á", e: "é", i: "í", o: "ó", u: "ú",
      A: "Á", E: "É", I: "Í", O: "Ó", U: "Ú",
    })[character] || character);
    return "⟦" + expanded + "⟧";
  }

  function truncateUnicode(text, maxChars) {
    const limit = Math.max(0, Math.floor(Number(maxChars) || 0));
    const characters = Array.from(String(text));
    if (limit === 0) return "";
    if (characters.length <= limit) return characters.join("");
    if (limit === 1) return "…";
    return characters.slice(0, limit - 1).join("") + "…";
  }

  function applyDocument(documentRoot, locale) {
    if (!documentRoot || typeof documentRoot.querySelectorAll !== "function") return;
    const language = normalizeLocale(locale);
    if (documentRoot.documentElement) {
      documentRoot.documentElement.lang = language;
      documentRoot.documentElement.dataset.locale = language;
    }
    documentRoot.querySelectorAll("[data-i18n]").forEach((element) => {
      element.textContent = t(language, element.dataset.i18n);
    });
    documentRoot.querySelectorAll("[data-i18n-aria-label]").forEach((element) => {
      element.setAttribute("aria-label", t(language, element.dataset.i18nAriaLabel));
    });
    documentRoot.querySelectorAll("[data-i18n-placeholder]").forEach((element) => {
      element.setAttribute("placeholder", t(language, element.dataset.i18nPlaceholder));
    });
    documentRoot.querySelectorAll("[data-i18n-title]").forEach((element) => {
      element.setAttribute("title", t(language, element.dataset.i18nTitle));
    });
  }

  root.FuckIPV6I18n = Object.freeze({
    catalog,
    locales,
    defaultLocale,
    normalizeLocale,
    detectLocale,
    resolveLocale,
    t,
    tr,
    interpolate,
    formatInteger,
    formatDecimal,
    formatBytes,
    formatDuration,
    formatTimestamp,
    formatAddress,
    pseudoLocalize,
    truncateUnicode,
    applyDocument,
  });
})(typeof globalThis !== "undefined" ? globalThis : this);
