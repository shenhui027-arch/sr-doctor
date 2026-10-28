import { layoutCompound, layoutErrorCode, type LayoutInput, type LayoutResult } from "./graph-layout";

type WorkerRequest = { id: number; input: LayoutInput };
type WorkerResponse = { id: number; result: LayoutResult } | { id: number; error: "layout_invalid_request" | ReturnType<typeof layoutErrorCode> };
type WorkerScope = { addEventListener(type: "message", listener: (event: MessageEvent<unknown>) => void): void; postMessage(message: WorkerResponse): void };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exactKeys(value: Record<string, unknown>, keys: string[]): boolean {
  const actual = Object.keys(value);
  return actual.length === keys.length && actual.every((key) => keys.includes(key));
}

function isGeometryInput(value: unknown): value is LayoutInput {
  const keys = ["nodes", "edges", "groups", "nodeWidth", "nodeHeight", ...["density", "edgeLabels"].filter((key) => isRecord(value) && Object.hasOwn(value, key))];
  if (!isRecord(value) || !exactKeys(value, keys)) return false;
  if (value.density !== undefined && value.density !== "standard" && value.density !== "dense") return false;
  if (value.edgeLabels !== undefined && typeof value.edgeLabels !== "boolean") return false;
  if (!Array.isArray(value.nodes) || !Array.isArray(value.edges) || !Array.isArray(value.groups)) return false;
  return value.nodes.every((node) => isRecord(node) && exactKeys(node, ["id"]))
    && value.edges.every((edge) => isRecord(edge) && exactKeys(edge, ["id", "source", "target", "reverseForLayout"]))
    && value.groups.every((group) => isRecord(group) && exactKeys(group, ["id", "nodeIds"]));
}

function isWorkerRequest(value: unknown): value is WorkerRequest {
  return isRecord(value) && exactKeys(value, ["id", "input"]) && Number.isInteger(value.id) && isGeometryInput(value.input);
}

const scope = self as unknown as WorkerScope;

scope.addEventListener("message", (event) => {
  const request = event.data;
  if (!isWorkerRequest(request)) {
    scope.postMessage({ id: -1, error: "layout_invalid_request" });
    return;
  }
  void layoutCompound(request.input).then(
    (result) => scope.postMessage({ id: request.id, result }),
    (error: unknown) => scope.postMessage({ id: request.id, error: layoutErrorCode(error) }),
  );
});
