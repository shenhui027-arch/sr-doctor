import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { TooltipProvider } from "./design";
import { AccessGate } from "./features/access/AccessGate";
import { LocaleProvider, rememberedLocale } from "./i18n/locale";
import "./design/tokens.css";
import "./design/base.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <LocaleProvider initialLocale={rememberedLocale()} remember><TooltipProvider><AccessGate><App /></AccessGate></TooltipProvider></LocaleProvider>
  </StrictMode>,
);
