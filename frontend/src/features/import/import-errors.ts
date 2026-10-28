import type { ApiFailure } from "../../lib/api";

type Localize = (english: string, chinese: string) => string;

const BUDGET_CODES = new Set([
  "input_too_large",
  "too_many_lines",
  "line_too_long",
  "too_deep",
  "too_many_nodes",
  "too_many_metrics",
]);

function isApiFailure(error: unknown): error is ApiFailure {
  return typeof error === "object" && error !== null
    && "code" in error && typeof error.code === "string"
    && "message_id" in error && typeof error.message_id === "string";
}

export function importFailureMessage(error: unknown, text: Localize): string {
  if (error instanceof Error) return error.message;
  if (!isApiFailure(error)) return text("Import request failed.", "导入请求失败。");

  if (BUDGET_CODES.has(error.code) || error.stage === "limits") {
    return text(
      "This Profile exceeds the local analysis limit. Reduce the file size or import a smaller capture.",
      "该 Profile 超出本地分析限制，请缩小文件或导入更小的采集结果。",
    );
  }
  if (error.code === "kind_mismatch" || error.message_id === "requested_json_mismatch" || error.message_id === "requested_text_mismatch") {
    return text(
      "The selected Profile format does not match the file content. Choose Auto or the matching format.",
      "所选 Profile 格式与文件内容不匹配，请选择“自动”或正确格式。",
    );
  }
  if (error.message_id === "input_empty") {
    return text("The Profile is empty. Choose a non-empty file.", "Profile 为空，请选择非空文件。");
  }
  if (error.message_id === "no_trusted_facts") {
    return text(
      "No supported StarRocks Profile facts were found. Check that this is a complete Profile capture.",
      "没有识别到受支持的 StarRocks Profile 事实，请确认这是完整的 Profile 采集结果。",
    );
  }
  if (error.code === "parse_failed") {
    return text(
      "The Profile could not be parsed. Check the file content and selected format.",
      "Profile 解析失败，请检查文件内容和所选格式。",
    );
  }
  return text("Import request failed. Review the input and try again.", "导入请求失败，请检查输入后重试。");
}
