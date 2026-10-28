import type { JSX } from "react";
import type { GraphNode } from "../../../../contracts/generated/doctor-contracts";
import { useLocale } from "../../i18n/locale";
import { SqlText } from "../shell/SqlText";

/** Render the backend's bound excerpt verbatim; never consult the SQL draft. */
export function NodeSqlLocation({ node }: { node: GraphNode }): JSX.Element {
  const { text } = useLocale();
  const location = node.sql_location;
  return <section className="node-sql-location" aria-label={text("Corresponding SQL", "对应 SQL")}>
    <h4>{text("Corresponding SQL", "对应 SQL")}</h4>
    {location?.state === "located"
      ? <SqlText copyLabel={text("Copy excerpt", "复制片段")} label={text("Corresponding SQL excerpt", "对应 SQL 片段")} value={`${location.before}${location.highlight}${location.after}`}>{location.before}<mark>{location.highlight}</mark>{location.after}</SqlText>
      : <p>{location?.state === "ambiguous" ? text("Multiple matches; not located yet", "存在多个匹配，尚未定位") : text("Not located yet", "尚未定位")}</p>}
  </section>;
}
