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

export function LocaleProvider({ children, initialLocale = "en" }: { children: ReactNode; initialLocale?: Locale }): JSX.Element {
  const [locale, setLocale] = useState<Locale>(initialLocale);
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
