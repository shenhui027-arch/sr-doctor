import type { JSX } from "react";
import type { AuditMetricFact } from "../../../../contracts/generated/doctor-contracts";
import { StateStamp } from "../../design";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { metricText, metricTone } from "./audit-display";

export function AuditMetricValue({
  value,
  text,
}: {
  value: AuditMetricFact | null;
  text?: LocalizedText;
}): JSX.Element {
  const { text: contextText } = useLocale();
  const t = text ?? contextText;
  const tone = metricTone(value);

  switch (tone) {
    case "exact":
      return <>{metricText(value)}</>;
    case "qualified":
      return (
        <span
          className="state-stamp state-stamp--warn"
          data-honest-state="precision_unsafe"
          title={t(
            "Server-qualified integer above 2^53; shown without IEEE-754 float rounding.",
            "超过 2^53 的服务端限定整数；不做 IEEE-754 浮点舍入。",
          )}
        >
          <span aria-hidden="true" className="state-stamp-sym">~</span>
          <span>{metricText(value)}</span>
        </span>
      );
    case "missing":
      return <StateStamp state="missing" />;
    case "invalid":
      return <StateStamp state="invalid" />;
    case "overflow":
      return <StateStamp state="overflow" />;
    case "negative_sentinel":
      return (
        <span
          className="state-stamp state-stamp--warn"
          data-honest-state="negative_sentinel"
          title={t(
            "Audit sentinel (-1); not a real measurement and never counted as 0.",
            "审计哨兵值 (-1)，不代表真实测量值，不按 0 计。",
          )}
        >
          <span aria-hidden="true" className="state-stamp-sym">!</span>
          <span>{t("Sentinel (-1)", "哨兵值 (-1)")}</span>
        </span>
      );
    case "unavailable":
      return (
        <span
          className="state-stamp"
          data-honest-state="unavailable"
          title={t(
            "Source column unavailable or rows disagree; never counted as 0.",
            "源列缺失或重复行冲突，不按 0 计。",
          )}
        >
          <span aria-hidden="true" className="state-stamp-sym">—</span>
          <span>{t("Unavailable", "不可用")}</span>
        </span>
      );
  }
}
