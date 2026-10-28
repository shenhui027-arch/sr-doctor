// Development-only pixel-parity harness (v0.7). Renders the real App, in
// Chinese, with a fetch double that answers the product API from Demo
// fixtures for one design board, drives the same clicks a user would, then
// raises `window.__visualReady` for tests/visual/pixel-parity.mjs. The Demo
// data exists only here, to compare pixels with the design; it never reaches
// the production bundle (vite build follows index.html only).
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "../App";
import { LocaleProvider } from "../i18n/locale";
import { BOARDS, type BoardName } from "./boards";
import "../styles.css";

declare global {
  interface Window { __visualReady?: boolean; __visualError?: string }
}

const name = (new URLSearchParams(location.search).get("board") ?? "Connect") as BoardName;
const board = BOARDS[name];

async function run(): Promise<void> {
  if (!board) throw new Error(`Unknown board ${name}`);
  const nativeFetch = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const url = new URL(input instanceof Request ? input.url : String(input), location.href);
    if (!url.pathname.startsWith("/api/v1/")) return nativeFetch(input, init);
    const answer = board.respond(url.pathname.slice("/api/v1".length), init?.method ?? "GET", typeof init?.body === "string" ? init.body : null);
    if (answer === undefined) return new Response(JSON.stringify({ error: { code: "visual_fixture_missing", message_id: url.pathname } }), { status: 404 });
    return new Response(JSON.stringify(answer.body), { status: answer.status ?? 200, headers: { "Content-Type": "application/json" } });
  };
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <LocaleProvider initialLocale="zh"><App /></LocaleProvider>
    </StrictMode>,
  );
  await board.drive(document);
  await document.fonts.ready;
  // Two frames so layout, fonts and the graph worker settle before capture.
  await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  window.__visualReady = true;
}

run().catch((error: unknown) => { window.__visualError = error instanceof Error ? error.message : String(error); });
