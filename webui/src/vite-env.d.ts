/// <reference types="vite/client" />

declare global {
  type Locale = "zh-CN" | "en";

  interface I18nRuntime {
    locales: Locale[];
    defaultLocale: Locale;
    normalizeLocale(value: string): Locale;
    detectLocale(tags?: string[]): Locale;
    resolveLocale(preference: string, systemLocale?: string): Locale;
    t(locale: Locale | string, key: string): string;
    tr(locale: Locale | string, key: string, params?: Record<string, string | number>): string;
    formatInteger(locale: Locale | string, value: number): string;
    formatBytes(locale: Locale | string, value: number): string;
    formatDuration(locale: Locale | string, seconds: number): string;
    formatTimestamp(locale: Locale | string, unixMs: number, offsetMinutes?: number): string;
    applyDocument(root: Document, locale: Locale | string): void;
  }

  interface Window {
    FuckIPV6I18n?: I18nRuntime;
  }
}

export {};
