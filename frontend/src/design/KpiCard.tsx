import type { JSX, ReactNode } from "react";
import { useLocale } from "../i18n/locale";
import { gridSpanClasses, type GridSpan } from "./Card";
import { InfoTip } from "./ui";

export function KpiCard(props: {
  label: ReactNode;
  value: ReactNode;
  foot?: ReactNode;
  info?: ReactNode;
  span?: GridSpan;
  tone?: "default" | "critical";
}): JSX.Element {
  const { label, value, foot, info, span, tone = "default" } = props;
  const { text } = useLocale();
  const classes = ["card", "card--kpi", gridSpanClasses(span)].filter(Boolean).join(" ");
  const infoLabel = typeof label === "string"
    ? text(`About ${label}`, `${label}口径说明`)
    : text("Metric definition", "指标口径说明");

  return (
    <article className={classes}>
      <div className="kpi-head">
        <span>{label}</span>
        {info !== undefined && info !== null ? <InfoTip label={infoLabel}>{info}</InfoTip> : null}
      </div>
      <div className="kpi-main">
        <div className={tone === "critical" ? "kpi-val kpi-val--critical" : "kpi-val"}>
          {value}
        </div>
      </div>
      {foot !== undefined && foot !== null ? <div className="kpi-foot">{foot}</div> : null}
    </article>
  );
}
