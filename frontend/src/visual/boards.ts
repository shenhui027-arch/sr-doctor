// Demo fixtures per design board (v0.7 pixel parity). Each board answers the
// product API paths it needs and drives the UI into the state the design shows.
import type { AuditOccurrenceReadRequest, AuditOccurrenceResponse, ConnectedProfileComparisonRequest, AuditOverviewReadRequest, Capabilities, ProfileFetchRequest } from "../../../contracts/generated/doctor-contracts";
import { DEMO_CONNECTION_ID, DEMO_SESSION, mainMembers, mainOverview } from "./demo-session";
import { CANDIDATE_QUERY_ID, demoComparison, membersWithCandidate } from "./demo-compare";
import { doctorFetch } from "./demo-doctor";

export type BoardName = "Connect" | "Main" | "Doctor" | "Recommendations" | "Compare";
export type Answer = { status?: number; body: unknown };
export type Board = {
  respond(path: string, method: string, body: string | null): Answer | undefined;
  drive(document: Document): Promise<void>;
};

export const DEMO_CAPABILITIES: Capabilities = {
  api_version: "v0.1",
  schema_version: "1.0",
  supported_inputs: ["text_profile", "json_profile"],
  version_packs: [],
  rule_sets: [],
  // The real backend budget (ResourceBudgets::single_input_bytes), not the design copy's 32 MB.
  limits: { single_input_bytes: 16 * 1024 * 1024 },
  csrf: { nonce: "visual-harness", required_when_origin_present: true },
  build: { git_commit: "visual", candidate_label: "visual" },
};

const base = (extra: (path: string, method: string, body: string | null) => Answer | undefined = () => undefined) =>
  (path: string, method: string, body: string | null): Answer | undefined =>
    path === "/capabilities" ? { body: DEMO_CAPABILITIES } : extra(path, method, body);

export async function waitFor(document: Document, selector: string, timeoutMs = 10_000): Promise<Element> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const element = document.querySelector(selector);
    if (element) return element;
    if (Date.now() > deadline) throw new Error(`visual harness: ${selector} did not appear`);
    await new Promise((done) => setTimeout(done, 50));
  }
}

export async function waitForText(document: Document, selector: string, needle: string, timeoutMs = 10_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!(document.querySelector(selector)?.textContent ?? "").includes(needle)) {
    if (Date.now() > deadline) throw new Error(`visual harness: "${needle}" did not appear in ${selector}`);
    await new Promise((done) => setTimeout(done, 50));
  }
}

/** Set a React-controlled input the way typing does. */
export function setValue(element: HTMLInputElement | HTMLTextAreaElement, value: string): void {
  const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(prototype, "value")?.set?.call(element, value);
  element.dispatchEvent(new Event("input", { bubbles: true }));
}

/** Connect the first-run form to the Demo / Mock cluster (answered by connected()). */
async function connect(document: Document): Promise<void> {
  const form = await waitFor(document, ".first-run .connection-form") as HTMLFormElement;
  const inputs = form.querySelectorAll("input");
  setValue(inputs[2] as HTMLInputElement, "analyst");
  setValue(inputs[3] as HTMLInputElement, "demo-password");
  await new Promise((done) => setTimeout(done, 50));
  form.requestSubmit();
  await waitFor(document, ".queries-toolbar");
}

const connections = "/connections/" + DEMO_CONNECTION_ID + "/";
const connected = (extra: (path: string, body: string | null) => unknown | undefined) => base((path, method, body) => {
  if (path === "/connections" && method === "POST") return { body: DEMO_SESSION };
  if (!path.startsWith(connections)) return undefined;
  const answer = extra(path.slice(connections.length), body);
  return answer === undefined ? undefined : { body: answer };
});

// The member ranking the Doctor board last read; its rows are what "fetch Profile" may name.
let members: AuditOccurrenceResponse | null = null;

export const BOARDS: Record<BoardName, Board> = {
  // 05 首次进入 · 连接与导入: the product's first-run page, nothing connected.
  Connect: { respond: base(), drive: async (document) => { await waitFor(document, "main, .first-run"); } },
  // 01 查询 · 审计历史: repeated SQL, first family selected with its member ranking.
  Main: {
    respond: connected((path, body) => {
      if (path === "audit/overview") return mainOverview((JSON.parse(body ?? "{}") as AuditOverviewReadRequest).request.scope);
      if (path === "audit/occurrences") return mainMembers((JSON.parse(body ?? "{}") as AuditOccurrenceReadRequest).request.scope);
      return undefined;
    }),
    drive: async (document) => {
      await connect(document);
      await waitFor(document, ".audit-family-list tbody tr");
      setValue(document.querySelector(".audit-filter-database") as HTMLInputElement, "demo");
      await new Promise((done) => setTimeout(done, 50));
      (document.querySelector(".audit-filter-bar") as HTMLFormElement).requestSubmit();
      await new Promise((done) => setTimeout(done, 300));
      const row = await waitFor(document, ".audit-family-list tbody tr") as HTMLElement;
      row.click();
      await waitFor(document, ".audit-family-members li");
    },
  },
  // 02 诊断 · 证据工作台: the slowest member of the Main family, pulled from the Demo cluster.
  Doctor: {
    respond: connected((path, body) => {
      if (path === "audit/overview") return mainOverview((JSON.parse(body ?? "{}") as AuditOverviewReadRequest).request.scope);
      if (path === "audit/occurrences") return (members = mainMembers((JSON.parse(body ?? "{}") as AuditOccurrenceReadRequest).request.scope));
      // The Official Advisor answers without guidance (board 03 shows its collapsed status).
      if (path === "advisor-guides") return { capability: "available", recommendations: [], limitations: [] };
      if (path === "profiles/fetch" && members) {
        const request = JSON.parse(body ?? "{}") as ProfileFetchRequest;
        const member = members.rows.find((row) => row.query.query_id === request.query_id);
        return member ? doctorFetch(request, member.query) : undefined;
      }
      return undefined;
    }),
    drive: async (document) => {
      await BOARDS.Main.drive(document);
      (await waitFor(document, ".audit-family-aside .button.accent.primary:not(:disabled)") as HTMLButtonElement).click();
      await waitFor(document, ".workspace-doctor .doctor-summary");
      await waitFor(document, ".workspace-doctor .execution-graph-canvas[data-layout-state=\"ready\"]");
      // The data-flow tree settles at 1:1 once the canvas is measured.
      await waitFor(document, ".workspace-doctor .react-flow__viewport[style*=\"scale(1)\"]:not([style*=\"translate(0px, 0px)\"])");
    },
  },
  // 03 优化建议: the same pulled Profile, opened from the navigation; first recommendation selected.
  Recommendations: {
    respond: (path, method, body) => BOARDS.Doctor.respond(path, method, body),
    drive: async (document) => {
      await BOARDS.Doctor.drive(document);
      (document.querySelector(".workspace-nav button[data-step=\"3\"]") as HTMLButtonElement).click();
      await waitFor(document, ".workspace-recommendations .recommendation-card.selected");
    },
  },
  // 04 对比: the board-02 member as baseline, compared with a later execution of the same query.
  Compare: {
    respond: connected((path, body) => {
      if (path === "audit/overview") return mainOverview((JSON.parse(body ?? "{}") as AuditOverviewReadRequest).request.scope);
      if (path === "audit/occurrences") return (members = membersWithCandidate(mainMembers((JSON.parse(body ?? "{}") as AuditOccurrenceReadRequest).request.scope)));
      if (path === "advisor-guides") return { capability: "available", recommendations: [], limitations: [] };
      if (path === "profiles/fetch" && members) {
        const request = JSON.parse(body ?? "{}") as ProfileFetchRequest;
        const member = members.rows.find((row) => row.query.query_id === request.query_id);
        return member ? doctorFetch(request, member.query) : undefined;
      }
      if (path === "profiles/compare" && members) {
        const request = JSON.parse(body ?? "{}") as ConnectedProfileComparisonRequest;
        const find = (id: string) => members!.rows.find((row) => row.query.query_id === id)?.query;
        const baseline = find(request.baseline_query_id);
        const candidate = find(request.candidate_query_id);
        return baseline && candidate ? demoComparison(request, baseline, candidate) : undefined;
      }
      return undefined;
    }),
    drive: async (document) => {
      await BOARDS.Main.drive(document);
      const button = (label: string) => Array.from(document.querySelectorAll<HTMLButtonElement>(".audit-family-aside-actions button")).find((item) => item.textContent?.trim() === label && !item.disabled);
      // The slowest member (7e8c3a91) is selected first: set it as baseline.
      await waitFor(document, ".audit-family-aside-actions button:not(:disabled)");
      button("设为基线")?.click();
      await waitForText(document, ".audit-family-aside-actions", "已设为基线");
      (document.querySelector(`.audit-family-members button[title="${CANDIDATE_QUERY_ID}"]`) as HTMLButtonElement).click();
      await waitForText(document, ".audit-family-aside-actions", "与基线对比");
      button("与基线对比")?.click();
      await waitFor(document, ".workspace-compare .compare-structure-table");
    },
  },
};
