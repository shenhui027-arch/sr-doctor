// Design board A2 (相似 SQL): clusters of exact templates with their explained
// basis, and the selected cluster's members with a per-request split. Every
// fact is a server DTO field; nothing here groups, sums or ranks.
import { useState, type JSX } from "react";
import type {
  AuditSimilarSqlResponse,
  SimilarSqlCluster,
  SimilarSqlMember,
  SimilarSqlUnavailableReason,
} from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { ExpandableSqlLine } from "../shell/SqlText";
import { metricRawText, metricText, metricTone } from "./audit-display";

const unavailableLabels: Record<SimilarSqlUnavailableReason, [string, string]> = {
  template_unavailable: ["no reliable template", "无可靠模板"],
  not_select: ["not a SELECT", "非 SELECT"],
  common_table_expression: ["WITH clause", "含 WITH"],
  set_operation: ["UNION / EXCEPT / INTERSECT", "含集合运算"],
  subquery: ["subquery", "含子查询"],
  derived_table: ["derived table", "含派生表"],
  unsupported_from_clause: ["unsupported FROM", "FROM 形态不支持"],
};

/** The cluster's name is its basis: tables joined, then predicate columns. */
export function similarClusterName(cluster: SimilarSqlCluster, text: LocalizedText): string {
  const tables = cluster.basis.tables.length ? cluster.basis.tables.join(" ⋈ ") : text("any tables", "任意表");
  return cluster.basis.predicate_columns.length ? `${tables} · ${cluster.basis.predicate_columns.join(", ")}` : tables;
}

function metric(item: { aggregates: SimilarSqlMember["aggregates"] }, id: "query_time", aggregation: "sum" | "max") {
  return item.aggregates.find((fact) => fact.metric.metric_id === id && fact.aggregation === aggregation)?.metric ?? null;
}

function MetricCell({ item, aggregation, text }: { item: { aggregates: SimilarSqlMember["aggregates"] }; aggregation: "sum" | "max"; text: LocalizedText }): JSX.Element {
  const fact = metric(item, "query_time", aggregation);
  return <td className="num" title={metricRawText(fact)}>{metricTone(fact) === "missing"
    ? <span className="audit-missing-tag" title={text("No member provided this field; it is not counted as 0", "成员均未提供该字段，不按 0 计")}>{text("Missing", "缺失")}</span>
    : metricText(fact)}</td>;
}

function SourceTag({ member, text }: { member: SimilarSqlMember; text: LocalizedText }): JSX.Element {
  return member.source.kind === "official_digest"
    ? <span className="audit-similar-source official" title={member.source.native_digest}>{text("Official digest", "官方 digest")}</span>
    : <span className="audit-similar-source unofficial" title={member.source.fingerprint}>{text("Unofficial fingerprint", "非官方指纹")}</span>;
}

function sourceMix(cluster: SimilarSqlCluster, text: LocalizedText): string {
  const official = cluster.members.filter((member) => member.source.kind === "official_digest").length;
  const unofficial = cluster.members.length - official;
  return [official ? text(`${official} official`, `${official} 官方`) : "", unofficial ? text(`${unofficial} unofficial`, `${unofficial} 非官方`) : ""].filter(Boolean).join(" + ");
}

export function AuditSimilarSqlList({ response, selectedKey, onSelect, text }: {
  response: AuditSimilarSqlResponse;
  selectedKey: string | null;
  onSelect(cluster: SimilarSqlCluster): void;
  text: LocalizedText;
}): JSX.Element {
  const unavailable = response.unavailable.reduce<Record<string, number>>((counts, member) => {
    if (member.structure.state === "unavailable") counts[member.structure.reason] = (counts[member.structure.reason] ?? 0) + 1;
    return counts;
  }, {});
  return <>
    {response.clusters.length === 0
      ? <p className="audit-empty">{text("No two templates share the selected basis in this window.", "本窗口内没有按所选依据相同的两个模板。")}</p>
      : <div className="audit-family-list-wrap"><table aria-label={text("Similar SQL clusters", "相似 SQL 聚类")} className="audit-family-list audit-similar-list">
        <thead><tr>
          <th scope="col">{text("Cluster · basis", "聚类 · 归并依据")}</th>
          <th className="num" scope="col">{text("Member templates", "成员模板")}</th>
          <th className="num" scope="col">{text("Executions", "执行次数")}</th>
          <th className="num" scope="col">{text("Cumulative query time", "累计查询耗时")}</th>
          <th className="num last" scope="col">{text("Maximum query time", "最大查询耗时")}</th>
        </tr></thead>
        <tbody>{response.clusters.map((cluster) => {
          const selected = cluster.cluster_key === selectedKey;
          return <tr aria-selected={selected} className={selected ? "selected" : undefined} key={cluster.cluster_key}>
            <td className="audit-family-summary-cell">
              <button aria-pressed={selected} className="audit-similar-pick" onClick={() => onSelect(cluster)} type="button">
                <span className="audit-similar-name">{similarClusterName(cluster, text)}{cluster.refined ? ` · ${text("refined", "已细分")}` : ""}</span>
                <span className="audit-similar-basis">
                  {response.dimensions.tables && <span>{text("tables", "表")} {`{${cluster.basis.tables.join(", ")}}`}</span>}
                  {response.dimensions.join && <span>{cluster.basis.joins.length ? cluster.basis.joins.join("; ") : text("no join", "无 Join")}</span>}
                  {(response.dimensions.predicates || cluster.refined) && <span>{cluster.basis.predicate_columns.length ? `${text("predicates", "谓词")} {${cluster.basis.predicate_columns.join(", ")}}` : text("no predicates", "无谓词")}</span>}
                </span>
              </button>
            </td>
            <td className="num"><span>{cluster.members.length}</span><span className="audit-similar-mix">{sourceMix(cluster, text)}</span></td>
            <td className="num">{cluster.execution_count}</td>
            <MetricCell aggregation="sum" item={cluster} text={text} />
            <MetricCell aggregation="max" item={cluster} text={text} />
          </tr>;
        })}</tbody>
      </table></div>}
    <div className="audit-similar-foot">
      <span>{text(`${response.clusters.length} clusters · ${response.coverage.clustered_member_count} templates · ${response.coverage.independent_member_count} independent`, `${response.clusters.length} 个聚类 · ${response.coverage.clustered_member_count} 个模板 · ${response.coverage.independent_member_count} 个模板独立`)}</span>
      <span>{Object.keys(unavailable).length
        ? `${text("Not clustered", "未参与聚类")}: ${Object.entries(unavailable).map(([reason, count]) => `${text(...unavailableLabels[reason as SimilarSqlUnavailableReason])} ${count}`).join(" · ")}`
        : text("Truncated or non-normalizable statements never join a cluster", "截断或不可规范化的语句不参与聚类")}</span>
    </div>
    {response.coverage.exact_families.state === "partial" && <p className="audit-similar-note" role="note">{text(`Exact families: the first ${response.coverage.exact_families.read} of ${response.coverage.exact_families.in_window} by the selected ranking were read.`, `精确查询族：按当前排序只读取了 ${response.coverage.exact_families.in_window} 个中的前 ${response.coverage.exact_families.read} 个。`)}</p>}
  </>;
}

export function AuditSimilarAside({ cluster, dimensions, loading, splitCount, onClose, onDetach, onRefine, onUndo, text }: {
  cluster: SimilarSqlCluster;
  dimensions: AuditSimilarSqlResponse["dimensions"];
  loading: boolean;
  splitCount: number;
  onClose(): void;
  onDetach(memberKeys: string[]): void;
  onRefine(): void;
  onUndo(): void;
  text: LocalizedText;
}): JSX.Element {
  const [checkedOut, setUnchecked] = useState<Set<string>>(new Set());
  // A member split out by an earlier request is no longer here; forget it.
  const unchecked = new Set([...checkedOut].filter((key) => cluster.members.some((member) => member.member_key === key)));
  const keep = cluster.members.length - unchecked.size;
  return <aside aria-label={text("Selected cluster", "已选聚类")} className="audit-family-aside audit-similar-aside">
    <div className="audit-similar-aside-head">
      <div><span className="audit-similar-label">{text("Selected cluster", "已选聚类")}</span><h2>{similarClusterName(cluster, text)}</h2></div>
      <button aria-label={text("Close details", "关闭详情")} className="audit-similar-close" onClick={onClose} type="button">×</button>
    </div>
    <section className="audit-similar-why">
      <h3>{text("Why these are one cluster", "为什么归为一类")}</h3>
      <dl>
        <dt>{text("Tables", "表集合")}</dt><dd>{dimensions.tables ? `{${cluster.basis.tables.join(", ")}}` : text("not used", "未使用")}</dd>
        <dt>{text("Join shape", "Join 形态")}</dt><dd>{dimensions.join ? (cluster.basis.joins.join("; ") || text("no join", "无 Join")) : text("not used", "未使用")}</dd>
        <dt>{text("Predicate columns", "谓词列集合")}</dt><dd>{dimensions.predicates || cluster.refined ? (cluster.basis.predicate_columns.length ? `{${cluster.basis.predicate_columns.join(", ")}}` : text("no predicates", "无谓词")) : text("not used", "未使用")}</dd>
      </dl>
      <p>{text("Projection, grouping and ordering are not part of the basis.", "投影、分组与排序不参与归并。")}</p>
    </section>
    <div className="audit-similar-members-head"><h3>{text("Member templates", "成员模板")}</h3><span>{text("Uncheck to split out", "取消勾选的模板可拆出")}</span></div>
    <ul className="audit-similar-members">{cluster.members.map((member) => {
      const checked = !unchecked.has(member.member_key);
      return <li className={checked ? undefined : "unchecked"} key={member.member_key}>
        {/* The checkbox only splits; reading the template is its own Expand control, so a click on the text never changes the split. */}
        <div className="audit-similar-member">
          <input aria-label={`${text("Keep in this cluster", "保留在本聚类")} · ${member.template ?? text("No template", "无模板")}`} checked={checked} onChange={() => setUnchecked((current) => {
            const next = new Set(current);
            if (next.has(member.member_key)) next.delete(member.member_key); else next.add(member.member_key);
            return next;
          })} type="checkbox" />
          {member.template
            ? <ExpandableSqlLine copyLabel={text("Copy template", "复制模板")} id={`audit-similar-template-${member.member_key}`} label={text("Member template", "成员模板")} lineClassName="audit-similar-template" value={member.template} />
            : <span className="audit-similar-template">{text("No template", "无模板")}</span>}
          <span className="audit-similar-count">{text(`${member.execution_count} runs`, `${member.execution_count} 次`)}</span>
          <span />
          <SourceTag member={member} text={text} />
          <span className="audit-similar-total" title={metricRawText(metric(member, "query_time", "sum"))}>{metricText(metric(member, "query_time", "sum"))}</span>
        </div>
      </li>;
    })}</ul>
    {splitCount > 0 && <p className="audit-similar-split" role="status">{text(`${splitCount} template(s) split out (this view only).`, `已拆出 ${splitCount} 个模板为独立项（仅本视图）。`)} <button disabled={loading} onClick={onUndo} type="button">{text("Undo", "撤销")}</button></p>}
    <div className="audit-similar-actions">
      <div>
        <button className="button accent" disabled={loading || unchecked.size === 0 || keep < 1} onClick={() => onDetach([...unchecked])} type="button">{text("Split out unchecked templates", "拆出未勾选的模板")}</button>
        <button className="button secondary" disabled={loading || dimensions.predicates || cluster.refined} onClick={onRefine} title={dimensions.predicates ? text("Predicate columns already take part", "谓词列已参与归并") : undefined} type="button">{text("Refine by predicate columns", "按谓词列再细分")}</button>
      </div>
      <p>{text("Splits apply to this view only and are not saved; refreshing restores the default clusters.", "拆分仅对当前视图有效，不会保存；刷新后恢复默认归并。")}</p>
      <p className="audit-similar-planned"><span>Planned</span>{text("Keeping splits for later windows is not available in this version", "保存拆分结果、后续窗口沿用：当前版本暂不支持")}</p>
    </div>
  </aside>;
}
