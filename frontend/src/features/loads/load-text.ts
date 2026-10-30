// Words and number formats for the load workspace. The server decides every
// state, category and count; these only name and format them.
import type { LoadErrorCategory, LoadKindGroup, LoadStateClass, RoutineLoadStateClass } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { bytesText, compactCountText, countText, durationNanosecondsText } from "../../lib/duration-display";

export type Tone = "ok" | "bad" | "warn" | "off" | "info";

export function routineTone(state: RoutineLoadStateClass): Tone {
  return { running: "ok", scheduling: "warn", paused: "bad", cancelled: "off", stopped: "off", unknown: "off" }[state] as Tone;
}

export function loadTone(state: LoadStateClass): Tone {
  return { finished: "ok", cancelled: "bad", running: "info", unknown: "off" }[state] as Tone;
}

export function categoryText(category: LoadErrorCategory, text: LocalizedText): string {
  return {
    out_of_range: text("Value out of range", "字段超范围"),
    too_long: text("String too long", "字符串超长"),
    column_count: text("Column count mismatch", "列数不符"),
    null_in_non_nullable: text("NULL in a non-null column", "非空列为 NULL"),
    out_of_partition: text("Outside every partition", "超出分区"),
    json_cast: text("JSON type cast", "JSON 类型转换"),
    json_structure: text("JSON structure", "JSON 结构错误"),
    other: text("Other", "其他"),
  }[category];
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
  const seconds = Math.max(0, Math.round((nowMs - Number(epochMs)) / 1000));
  if (seconds < 60) return text(`${seconds} s ago`, `${seconds} 秒前`);
  if (seconds < 3600) return text(`${Math.round(seconds / 60)} min ago`, `${Math.round(seconds / 60)} 分钟前`);
  return text(`${Math.round(seconds / 3600)} h ago`, `${Math.round(seconds / 3600)} 小时前`);
}

/** Refused or failed reads, in words; unknown codes keep the code. */
export function loadFailureText(code: string, text: LocalizedText): string {
  return {
    invalid_load_request: text("The request was not valid.", "请求参数无效。"),
    invalid_load_error_log_request: text("That job id is not valid.", "任务编号无效。"),
    connection_expired: text("The connection expired; connect again.", "连接已过期，请重新连接。"),
  }[code] ?? text(`Request failed (${code})`, `请求失败（${code}）`);
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

export function groupText(group: LoadKindGroup, text: LocalizedText): string {
  return { stream_load: "Stream Load", insert: "INSERT", routine_load: text("Routine Load tasks", "Routine Load 任务"), broker: "Broker Load", other: text("Other", "其他") }[group];
}

/** Server reason keys: fixed Stream Load causes, or StarRocks' cancel type. */
export function failureReasonText(reason: string, text: LocalizedText): string {
  return {
    filtered_ratio: text("Filtered rows over max_filter_ratio", "过滤行超过 max_filter_ratio"),
    timeout: text("Timed out", "超时"),
    label_exists: text("Label already exists", "Label 已存在"),
    other: text("Other", "其他"),
    TIMEOUT: text("Timed out (TIMEOUT)", "超时（TIMEOUT）"),
    LOAD_RUN_FAIL: text("Run failed (LOAD_RUN_FAIL)", "执行失败（LOAD_RUN_FAIL）"),
    USER_CANCEL: text("Cancelled by a user (USER_CANCEL)", "用户取消（USER_CANCEL）"),
    ETL_RUN_FAIL: text("ETL failed (ETL_RUN_FAIL)", "ETL 失败（ETL_RUN_FAIL）"),
    ETL_SUBMIT_FAIL: text("ETL submit failed (ETL_SUBMIT_FAIL)", "ETL 提交失败（ETL_SUBMIT_FAIL）"),
    ETL_QUALITY_UNSATISFIED: text("Data quality not met (ETL_QUALITY_UNSATISFIED)", "数据质量不达标（ETL_QUALITY_UNSATISFIED）"),
    TXN_UNKNOWN: text("Transaction state unknown (TXN_UNKNOWN)", "事务状态未知（TXN_UNKNOWN）"),
    UNKNOWN: text("Unknown (UNKNOWN)", "未知（UNKNOWN）"),
  }[reason] ?? reason;
}

/** StarRocks' states of a load not yet finished. */
export function runningStateText(state: string, text: LocalizedText): string {
  return { LOADING: text("Loading", "执行中"), QUEUEING: text("Queueing", "排队中"), PREPARED: text("Committed, publishing", "已提交，发布中"), PENDING: text("Pending", "等待中"), ETL: "ETL", COMMITTED: text("Committed", "已提交") }[state] ?? state;
}

/** A phase histogram bin by its server edges, short enough for one line
 * under a bar: `10–50 ms`, `500 ms–1 s`, `1–5 s`. */
export function phaseBinText(lower: string, upper: string | null): string {
  if (upper === null) return `≥ ${edgeText(lower)}`;
  if (lower === "0") return `< ${edgeText(upper)}`;
  const [from, to] = [edgeText(lower), edgeText(upper)];
  const unit = (text: string) => text.split(" ")[1];
  return unit(from) === unit(to) ? `${from.split(" ")[0]}–${to}` : `${from}–${to}`;
}
