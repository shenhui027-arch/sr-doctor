import { createContext, useCallback, useContext, useEffect, useMemo, useState, type JSX, type ReactNode } from "react";

export type Locale = "en" | "zh";
export type LocalizedText = (english: string, chinese: string) => string;

type LocaleContextValue = {
  locale: Locale;
  setLocale(locale: Locale): void;
  text: LocalizedText;
};

const defaultContext: LocaleContextValue = {
  locale: "en",
  setLocale: () => undefined,
  text: (english) => english,
};

const LocaleContext = createContext<LocaleContextValue>(defaultContext);

const LOCALE_KEY = "sr-doctor.locale";

/**
 * Task 68: the language the viewer chose last (a per-browser convenience), else
 * the browser's own language. Storage may be blocked; then it is simply not kept.
 */
export function rememberedLocale(): Locale {
  try {
    const stored = window.localStorage.getItem(LOCALE_KEY);
    if (stored === "zh" || stored === "en") return stored;
  } catch { /* storage unavailable */ }
  return typeof navigator !== "undefined" && navigator.language.toLowerCase().startsWith("zh") ? "zh" : "en";
}

export function LocaleProvider({ children, initialLocale = "en", remember = false }: { children: ReactNode; initialLocale?: Locale; remember?: boolean }): JSX.Element {
  const [locale, setLocale] = useState<Locale>(initialLocale);
  useEffect(() => {
    if (!remember) return;
    try { window.localStorage.setItem(LOCALE_KEY, locale); } catch { /* storage unavailable */ }
  }, [locale, remember]);
  const text = useCallback<LocalizedText>((english, chinese) => locale === "zh" ? chinese : english, [locale]);
  const value = useMemo(() => ({ locale, setLocale, text }), [locale, text]);

  useEffect(() => {
    document.documentElement.lang = locale === "zh" ? "zh-CN" : "en";
  }, [locale]);

  return <LocaleContext.Provider value={value}>{children}</LocaleContext.Provider>;
}

export function useLocale(): LocaleContextValue {
  return useContext(LocaleContext);
}
