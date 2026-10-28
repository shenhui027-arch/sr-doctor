import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { AccessGate } from "./features/access/AccessGate";
import { LocaleProvider } from "./i18n/locale";
import "./styles.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <LocaleProvider><AccessGate><App /></AccessGate></LocaleProvider>
  </StrictMode>,
);
