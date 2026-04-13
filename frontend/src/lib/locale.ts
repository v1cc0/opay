export type Locale = 'zh' | 'en';

export function resolveLocale(lang: string | null | undefined): Locale {
  return lang?.trim().toLowerCase() === 'en' ? 'en' : 'zh';
}

export function isEnglish(locale: Locale): boolean {
  return locale === 'en';
}

export function pickLocaleText<T>(locale: Locale, zh: T, en: T): T {
  return locale === 'en' ? en : zh;
}

export function applyLocaleToSearchParams(params: URLSearchParams, locale: Locale): URLSearchParams {
  if (locale === 'en') {
    params.set('lang', 'en');
  }
  return params;
}
