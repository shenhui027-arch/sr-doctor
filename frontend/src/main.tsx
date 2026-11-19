import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { AccessGate } from "./features/access/AccessGate";
import { LocaleProvider, rememberedLocale } from "./i18n/locale";
import "./styles.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <LocaleProvider initialLocale={rememberedLocale()} remember><AccessGate><App /></AccessGate></LocaleProvider>
  </StrictMode>,
);
