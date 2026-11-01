export const translated = (key: string): string => `translated:${key}`;

export function installTestI18nRuntime(): void {
  window.FuckIPV6I18n = {
    locales: ["zh-CN", "en"],
    defaultLocale: "zh-CN",
    normalizeLocale: (value) => (value.startsWith("en") ? "en" : "zh-CN"),
    detectLocale: (tags) => (tags?.[0]?.startsWith("en") ? "en" : "zh-CN"),
    resolveLocale: (preference, systemLocale) =>
      preference === "en" || (preference === "system" && systemLocale?.startsWith("en")) ? "en" : "zh-CN",
    t: (_locale, key) => translated(key),
    tr: (_locale, key) => translated(key),
    formatInteger: (_locale, value) => String(value),
    formatBytes: (_locale, value) => `${value} B`,
    formatDuration: (_locale, seconds) => `${seconds}s`,
    formatTimestamp: (_locale, unixMs) => new Date(unixMs).toISOString(),
    applyDocument: () => undefined,
  };
}
