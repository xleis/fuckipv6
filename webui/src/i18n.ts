export function t(locale: Locale, key: string): string {
  const runtime = window.FuckIPV6I18n;
  if (!runtime) return key;
  const text = runtime.t(locale, key);
  return text && text !== key ? text : key;
}

export function tr(locale: Locale, key: string, params: Record<string, string | number>): string {
  const runtime = window.FuckIPV6I18n;
  if (!runtime) return key;
  const text = runtime.tr(locale, key, params);
  return text && text !== key ? text : key;
}
