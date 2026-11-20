// Words and number formats for the load workspace (board 07-loads).
// The server decides every state, category and count; these only name and format them.
// Every backend enum lookup goes through `lookupEnumLabel` or an explicit `default`
// branch so an unknown future value never crashes or renders `undefined`.
import type {
  LoadErrorCategory,
  LoadKindGroup,
  LoadStateClass,
  RoutineLoadStateClass,
} from "../../../../contracts/generated/doctor-contracts";
import { lookupEnumLabel } from "../../i18n/enum-label";
import type { LocalizedText } from "../../i18n/locale";
import { bytesText, compactCountText, countText, durationNanosecondsText } from "../../lib/duration-display";

export type Tone = "ok" | "bad" | "warn" | "off" | "info";

const EXPIRED_CODES = new Set(["connection_expired", "connection_session_expired"]);

/** True when a live Loads API error indicates the StarRocks connection session expired. */
export function isLoadSessionExpired(error: unknown): boolean {
  if (typeof error === "string") return EXPIRED_CODES.has(error);
  if (typeof error === "object" && error !== null) {
    const record = error as { code?: unknown; message_id?: unknown };
    if (typeof record.code === "string" && EXPIRED_CODES.has(record.code)) return true;
    if (typeof record.message_id === "string" && EXPIRED_CODES.has(record.message_id)) return true;
  }
  return false;
}

export function routineTone(state: RoutineLoadStateClass): Tone {
  switch (state) {
    case "running":
      return "ok";
    case "scheduling":
      return "warn";
    case "paused":
      return "bad";
    case "cancelled":
    case "stopped":
    case "unknown":
    default:
      return "off";
  }
}

export function routineDotClass(state: RoutineLoadStateClass): string {
  switch (state) {
    case "running":
      return "status-dot status-dot--ok";
    case "scheduling":
      return "status-dot status-dot--warn";
    case "paused":
      return "status-dot status-dot--critical";
    case "cancelled":
    case "stopped":
    case "unknown":
    default:
      return "status-dot status-dot--hollow";
  }
}

const ROUTINE_STATE_LABELS: Record<RoutineLoadStateClass, readonly [string, string]> = {
  running: ["Running", "运行中"],
  scheduling: ["Waiting to schedule", "等待调度"],
  paused: ["Paused", "已暂停"],
  stopped: ["Stopped", "已停止"],
  cancelled: ["Cancelled", "已取消"],
  unknown: ["Unknown", "未知"],
};

export function routineStateLabel(state: RoutineLoadStateClass, text: LocalizedText): string {
  return lookupEnumLabel(ROUTINE_STATE_LABELS, state, text);
}

export function loadTone(state: LoadStateClass): Tone {
  switch (state) {
    case "finished":
      return "ok";
    case "cancelled":
      return "bad";
    case "running":
      return "info";
    case "unknown":
    default:
      return "off";
  }
}

export function loadDotClass(state: LoadStateClass): string {
  switch (state) {
    case "finished":
      return "status-dot status-dot--ok";
    case "cancelled":
      return "status-dot status-dot--critical";
    case "running":
      return "status-dot status-dot--warn";
    case "unknown":
    default:
      return "status-dot status-dot--hollow";
  }
}

const LOAD_STATE_LABELS: Record<LoadStateClass, readonly [string, string]> = {
  finished: ["Finished", "已完成"],
  cancelled: ["Cancelled", "已取消"],
  running: ["In progress", "进行中"],
  unknown: ["Unknown", "未知"],
};

export function loadStateLabel(state: LoadStateClass, text: LocalizedText): string {
  return lookupEnumLabel(LOAD_STATE_LABELS, state, text);
}

const LOAD_KIND_LABELS: Record<string, readonly [string, string]> = {
  STREAM_LOAD: ["Stream Load", "Stream Load"],
  ROUTINE_LOAD: ["Routine Load", "Routine Load"],
  BROKER: ["Broker Load", "Broker Load"],
  INSERT: ["INSERT", "INSERT"],
  SPARK: ["Spark Load", "Spark Load"],
  stream_load: ["Stream Load", "Stream Load"],
  routine_load: ["Routine Load", "Routine Load"],
  broker: ["Broker Load", "Broker Load"],
  insert: ["INSERT", "INSERT"],
};

export function loadKindLabel(kind: string, text: LocalizedText): string {
  return lookupEnumLabel(LOAD_KIND_LABELS, kind, text);
}

const CATEGORY_LABELS: Record<LoadErrorCategory, readonly [string, string]> = {
  out_of_range: ["Value out of range", "字段超范围"],
  too_long: ["Column value too long", "列值超长"],
  column_count: ["Column count mismatch", "列数不符"],
  null_in_non_nullable: ["NULL in a non-null column", "非空列为 NULL"],
  out_of_partition: ["Outside every partition", "超出分区"],
  json_cast: ["JSON type cast", "JSON 类型转换"],
  json_structure: ["JSON structure", "JSON 结构错误"],
  other: ["Other", "其他"],
};

export function categoryText(category: LoadErrorCategory, text: LocalizedText): string {
  return lookupEnumLabel(CATEGORY_LABELS, category, text);
}

export const count = (value: string | null | undefined): string => (value ? countText(value) ?? value : "—");
export const compact = (value: string | null | undefined): string => (value ? compactCountText(value) ?? value : "—");
export const bytes = (value: string | null | undefined): string => (value ? bytesText(value) ?? value : "—");

/** Milliseconds from StarRocks: whole milliseconds below a second (the
 * source has no finer precision), the shared duration format above. */
export function ms(value: string | null | undefined): string {
  if (!value || !/^\d+$/.test(value)) return "—";
  if (value.length < 4) return `${Number(value)} ms`;
  return durationNanosecondsText(`${value}000000`) ?? "—";
}

/** A StarRocks `HH:MM:SS`-precision time string, trimmed to month-day and time. */
export function feTime(value: string | null | undefined): string {
  return value ? value.slice(5, 19) : "—";
}

/** Epoch milliseconds of a consumed message, in the browser's clock only to say how long ago. */
export function ago(epochMs: string | null | undefined, nowMs: number, text: LocalizedText): string {
  if (!epochMs || !/^\d+$/.test(epochMs)) return "—";
  const secondsElapsed = Math.max(0, Math.round((nowMs - Number(epochMs)) / 1000));
  if (secondsElapsed < 60) return text(`${secondsElapsed} s ago`, `${secondsElapsed} 秒前`);
  if (secondsElapsed < 3600) return text(`${Math.round(secondsElapsed / 60)} min ago`, `${Math.round(secondsElapsed / 60)} 分钟前`);
  return text(`${Math.round(secondsElapsed / 3600)} h ago`, `${Math.round(secondsElapsed / 3600)} 小时前`);
}

/** Refused or failed reads, in words; unknown codes keep the code. */
export function loadFailureText(code: string, text: LocalizedText): string {
  switch (code) {
    case "invalid_load_request":
      return text("The request was not valid.", "请求参数无效。");
    case "invalid_load_error_log_request":
      return text("That job id is not valid.", "任务编号无效。");
    case "connection_expired":
    case "connection_session_expired":
      return text("The connection expired; connect again.", "连接已过期，请重新连接。");
    default:
      return text(`Request failed (${code || "unknown"})`, `请求失败（${code || "未知"}）`);
  }
}

/** A server share in 1/10000ths as a percent with two decimals. */
export function shareText(permyriad: number | null | undefined): string | null {
  if (permyriad === null || permyriad === undefined) return null;
  return `${Math.floor(permyriad / 100)}.${String(permyriad % 100).padStart(2, "0")}%`;
}

/** `YYYY-MM-DD HH` from the server's hourly buckets as `MM-DD HH:00`. */
export function hourText(hour: string): string {
  return `${hour.slice(5, 13)}:00`;
}

/** The same hour as a range, `MM-DD HH:00–HH+1:00`. */
export function hourRangeText(hour: string): string {
  const next = (Number(hour.slice(11, 13)) + 1) % 24;
  return `${hourText(hour)}–${String(next).padStart(2, "0")}:00`;
}

/** A histogram edge: whole seconds stay short (`1 s`, not `1.000 s`). */
function edgeText(value: string): string {
  return /^\d+000$/.test(value) ? `${value.slice(0, -3)} s` : ms(value);
}

/** Whole seconds from the server, with thousands grouped. */
export function seconds(value: string | null | undefined): string {
  return value && /^\d+$/.test(value) ? `${count(value)} s` : "—";
}

function secondEdge(value: string): string {
  const number = Number(value);
  return number >= 60 && number % 60 === 0 ? `${number / 60} min` : `${number} s`;
}

/** A seconds histogram bin: `< 1 s`, `1–10 s`, `1–5 min`, `≥ 30 min`. */
export function durationBinText(lower: string, upper: string | null): string {
  if (upper === null) return `≥ ${secondEdge(lower)}`;
  if (lower === "0") return `< ${secondEdge(upper)}`;
  const [from, to] = [secondEdge(lower), secondEdge(upper)];
  return from.split(" ")[1] === to.split(" ")[1] ? `${from.split(" ")[0]}–${to}` : `${from}–${to}`;
}

const GROUP_LABELS: Record<LoadKindGroup, readonly [string, string]> = {
  stream_load: ["Stream Load", "Stream Load"],
  insert: ["INSERT", "INSERT"],
  routine_load: ["Routine Load tasks", "Routine Load 任务"],
  broker: ["Broker Load", "Broker Load"],
  other: ["Other", "其他"],
};

export function groupText(group: LoadKindGroup, text: LocalizedText): string {
  return lookupEnumLabel(GROUP_LABELS, group, text);
}

const FAILURE_REASON_LABELS: Record<string, readonly [string, string]> = {
  filtered_ratio: ["Filtered rows over max_filter_ratio", "过滤行超过 max_filter_ratio"],
  timeout: ["Timed out", "导入超时"],
  label_exists: ["Label already exists", "Label 已存在"],
  other: ["Other", "其他"],
  TIMEOUT: ["Timed out", "导入超时"],
  LOAD_RUN_FAIL: ["Load execution failed", "导入执行失败"],
  USER_CANCEL: ["Cancelled by user", "用户取消"],
  ETL_RUN_FAIL: ["ETL failed", "ETL 失败"],
  ETL_SUBMIT_FAIL: ["ETL submit failed", "ETL 提交失败"],
  ETL_QUALITY_UNSATISFIED: ["Data quality not met", "数据质量未达标"],
  TXN_UNKNOWN: ["Transaction state unknown", "事务状态未知"],
  UNKNOWN: ["Unknown", "未知原因"],
};

/** Server reason keys: fixed Stream Load causes, or StarRocks' cancel type. */
export function failureReasonText(reason: string, text: LocalizedText): string {
  return lookupEnumLabel(FAILURE_REASON_LABELS, reason, text);
}

const RUNNING_STATE_LABELS: Record<string, readonly [string, string]> = {
  LOADING: ["Loading", "执行中"],
  QUEUEING: ["Queueing", "排队中"],
  PREPARED: ["Committed, publishing", "已提交，发布中"],
  PENDING: ["Pending", "等待中"],
  ETL: ["ETL", "ETL"],
  COMMITTED: ["Committed", "已提交"],
};

/** StarRocks' states of a load not yet finished. */
export function runningStateText(state: string, text: LocalizedText): string {
  return lookupEnumLabel(RUNNING_STATE_LABELS, state, text);
}

/** A phase histogram bin by its server edges, short enough for one line
 * under a bar: `10–50 ms`, `500 ms–1 s`, `1–5 s`. */
export function phaseBinText(lower: string, upper: string | null): string {
  if (upper === null) return `≥ ${edgeText(lower)}`;
  if (lower === "0") return `< ${edgeText(upper)}`;
  const [from, to] = [edgeText(lower), edgeText(upper)];
  const unit = (item: string) => item.split(" ")[1];
  return unit(from) === unit(to) ? `${from.split(" ")[0]}–${to}` : `${from}–${to}`;
}
