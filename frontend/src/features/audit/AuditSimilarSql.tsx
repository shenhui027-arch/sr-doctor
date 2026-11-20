// Design board 03 (查询 · 相似 SQL): clusters of exact templates with their explained
// basis, and the selected cluster's members with a per-request split. Every
// fact is a server DTO field; nothing here groups, sums or ranks.
import { useState, type JSX } from "react";
import type {
  AuditSimilarSqlResponse,
  SimilarSqlCluster,
  SimilarSqlMember,
  SimilarSqlUnavailableReason,
} from "../../../../contracts/generated/doctor-contracts";
import { Button, Dialog } from "../../design";
import { lookupEnumLabel } from "../../i18n/enum-label";
import type { LocalizedText } from "../../i18n/locale";
import { ExpandableSqlLine } from "../shell/SqlText";
import { AuditMetricValue } from "./AuditMetricValue";
import { metricRawText } from "./audit-display";

const unavailableLabels: Record<SimilarSqlUnavailableReason, readonly [string, string]> = {
  template_unavailable: ["no reliable template", "无可靠模板"],
  not_select: ["not a SELECT", "非 SELECT"],
  common_table_expression: ["WITH clause", "含 WITH"],
  set_operation: ["UNION / EXCEPT / INTERSECT", "含集合运算"],
  subquery: ["subquery", "含子查询"],
  derived_table: ["derived table", "含派生表"],
  unsupported_from_clause: ["unsupported FROM", "FROM 形态不支持"],
};

const SOURCE_LABELS: Record<SimilarSqlMember["source"]["kind"], readonly [string, string]> = {
  official_digest: ["Official digest", "官方 digest"],
  product_fingerprint: ["Unofficial fingerprint", "非官方指纹"],
};

/** The cluster's name is its basis: tables joined, then predicate columns. */
export function similarClusterName(cluster: SimilarSqlCluster, text: LocalizedText): string {
  const tables = cluster.basis.tables.length ? cluster.basis.tables.join(" ⋈ ") : text("any tables", "任意表");
  return cluster.basis.predicate_columns.length ? `${tables} · ${cluster.basis.predicate_columns.join(", ")}` : tables;
}

function metric(item: { aggregates: SimilarSqlMember["aggregates"] }, id: "query_time", aggregation: "sum" | "max") {
  return item.aggregates.find((fact) => fact.metric.metric_id === id && fact.aggregation === aggregation)?.metric ?? null;
}

function MetricCell({
  item,
  aggregation,
  text,
}: {
  item: { aggregates: SimilarSqlMember["aggregates"] };
  aggregation: "sum" | "max";
  text: LocalizedText;
}): JSX.Element {
  const fact = metric(item, "query_time", aggregation);
  return (
    <td className="num" title={metricRawText(fact)}>
      <AuditMetricValue text={text} value={fact} />
    </td>
  );
}

function SourceTag({ member, text }: { member: SimilarSqlMember; text: LocalizedText }): JSX.Element {
  const kind = member.source.kind;
  const label = lookupEnumLabel(SOURCE_LABELS, kind, text);
  const title = kind === "official_digest" ? member.source.native_digest : kind === "product_fingerprint" ? member.source.fingerprint : String(kind);
  return (
    <span
      className={kind === "official_digest" ? "badge-stamp" : "badge-stamp badge-stamp--unofficial"}
      title={title}
    >
      {label}
    </span>
  );
}

function sourceMix(cluster: SimilarSqlCluster, text: LocalizedText): string {
  const official = cluster.members.filter((member) => member.source.kind === "official_digest").length;
  const unofficial = cluster.members.length - official;
  return [
    official ? text(`${official} official`, `${official} 官方`) : "",
    unofficial ? text(`${unofficial} unofficial`, `${unofficial} 非官方`) : "",
  ]
    .filter(Boolean)
    .join(" + ");
}

export function AuditSimilarSqlList({
  response,
  selectedKey,
  onSelect,
  text,
}: {
  response: AuditSimilarSqlResponse;
  selectedKey: string | null;
  onSelect(cluster: SimilarSqlCluster): void;
  text: LocalizedText;
}): JSX.Element {
  const unavailable = response.unavailable.reduce<Record<string, number>>((counts, member) => {
    if (member.structure.state === "unavailable") counts[member.structure.reason] = (counts[member.structure.reason] ?? 0) + 1;
    return counts;
  }, {});

  return (
    <>
      {response.clusters.length === 0 ? (
        <div className="state-box">{text("No two templates share the selected basis in this window.", "本窗口内没有按所选依据相同的两个模板。")}</div>
      ) : (
        <div className="audit-family-list-wrap">
          <table aria-label={text("Similar SQL clusters", "相似 SQL 聚类")} className="data-table audit-family-list audit-similar-list">
            <thead>
              <tr>
                <th scope="col">{text("Cluster · basis", "聚类 · 归并依据")}</th>
                <th className="num" scope="col">{text("Member templates", "成员模板")}</th>
                <th className="num" scope="col">{text("Executions", "执行次数")}</th>
                <th className="num" scope="col">{text("Cumulative query time", "累计查询耗时")}</th>
                <th className="num last" scope="col">{text("Maximum query time", "最大查询耗时")}</th>
              </tr>
            </thead>
            <tbody>
              {response.clusters.map((cluster) => {
                const selected = cluster.cluster_key === selectedKey;
                const clusterLabel = `${similarClusterName(cluster, text)}${cluster.refined ? ` · ${text("refined", "已细分")}` : ""}`;
                return (
                  <tr className={selected ? "is-selected" : undefined} key={cluster.cluster_key}>
                    <td className="audit-family-summary-cell">
                      <button
                        aria-haspopup="dialog"
                        aria-label={`${text("Select cluster", "选择聚类")} ${clusterLabel}`}
                        className="row-pick-btn audit-similar-pick"
                        onClick={() => onSelect(cluster)}
                        type="button"
                      >
                        <span className="audit-similar-name mono">{clusterLabel}</span>
                        <span className="audit-similar-basis">
                          {response.dimensions.tables && <span>{text("tables", "表")} {`{${cluster.basis.tables.join(", ")}}`}</span>}
                          {response.dimensions.join && (
                            <span>{cluster.basis.joins.length ? cluster.basis.joins.join("; ") : text("no join", "无 Join")}</span>
                          )}
                          {(response.dimensions.predicates || cluster.refined) && (
                            <span>
                              {cluster.basis.predicate_columns.length
                                ? `${text("predicates", "谓词")} {${cluster.basis.predicate_columns.join(", ")}}`
                                : text("no predicates", "无谓词")}
                            </span>
                          )}
                        </span>
                      </button>
                    </td>
                    <td className="num">
                      <span>{cluster.members.length}</span>
                      <span className="audit-similar-mix">{sourceMix(cluster, text)}</span>
                    </td>
                    <td className="num">{cluster.execution_count}</td>
                    <MetricCell aggregation="sum" item={cluster} text={text} />
                    <MetricCell aggregation="max" item={cluster} text={text} />
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      <div className="audit-similar-foot">
        <span>
          {text(
            `${response.clusters.length} clusters · ${response.coverage.clustered_member_count} templates · ${response.coverage.independent_member_count} independent`,
            `${response.clusters.length} 个聚类 · ${response.coverage.clustered_member_count} 个模板 · ${response.coverage.independent_member_count} 个模板独立`,
          )}
        </span>
        <span>
          {Object.keys(unavailable).length
            ? `${text("Not clustered", "未参与聚类")}: ${Object.entries(unavailable)
                .map(([reason, count]) => `${lookupEnumLabel(unavailableLabels, reason, text)} ${count}`)
                .join(" · ")}`
            : text("Truncated or non-normalizable statements never join a cluster", "截断或不可规范化的语句不参与聚类")}
        </span>
      </div>
      {response.coverage.exact_families.state === "partial" && (
        <div className="notice-banner notice-banner--warn" role="note">
          {text(
            `Exact families: the first ${response.coverage.exact_families.read} of ${response.coverage.exact_families.in_window} by the selected ranking were read.`,
            `精确查询族：按当前排序只读取了 ${response.coverage.exact_families.in_window} 个中的前 ${response.coverage.exact_families.read} 个。`,
          )}
        </div>
      )}
    </>
  );
}

export function AuditSimilarAside({
  cluster,
  dimensions,
  loading,
  splitCount,
  open = cluster !== null,
  onClose,
  onDetach,
  onRefine,
  onUndo,
  text,
}: {
  cluster: SimilarSqlCluster | null;
  dimensions: AuditSimilarSqlResponse["dimensions"] | null;
  loading: boolean;
  splitCount: number;
  open?: boolean;
  onClose(): void;
  onDetach(memberKeys: string[]): void;
  onRefine(): void;
  onUndo(): void;
  text: LocalizedText;
}): JSX.Element {
  const [checkedState, setCheckedState] = useState<{ clusterKey: string | null; keys: Set<string> }>({
    clusterKey: null,
    keys: new Set(),
  });
  const clusterKey = cluster?.cluster_key ?? null;
  const checkedOut = checkedState.clusterKey === clusterKey ? checkedState.keys : new Set<string>();
  // A member split out by an earlier request is no longer here; forget it.
  const unchecked = new Set(
    cluster ? [...checkedOut].filter((key) => cluster.members.some((member) => member.member_key === key)) : [],
  );
  const keep = cluster ? cluster.members.length - unchecked.size : 0;

  return (
    <Dialog
      description={cluster ? text("Selected cluster · merge basis", "已选相似聚类 · 归并依据") : undefined}
      footer={
        cluster && dimensions ? (
          <div className="audit-similar-actions">
            <Button
              disabled={loading || unchecked.size === 0 || keep < 1}
              onClick={() => onDetach([...unchecked])}
              size="sm"
              variant="primary"
            >
              {text("Split out unchecked templates", "拆出未勾选的模板")}
            </Button>
            <Button
              disabled={loading || dimensions.predicates || cluster.refined}
              onClick={onRefine}
              size="sm"
              title={dimensions.predicates ? text("Predicate columns already take part", "谓词列已参与归并") : undefined}
              variant="secondary"
            >
              {text("Refine by predicate columns", "按谓词列再细分")}
            </Button>
          </div>
        ) : undefined
      }
      onOpenChange={(nextOpen) => {
        if (!nextOpen) onClose();
      }}
      open={open && cluster !== null}
      placement="right"
      title={cluster ? <span className="mono">{similarClusterName(cluster, text)}</span> : text("Selected cluster", "已选聚类")}
      width="lg"
    >
      {cluster && dimensions ? (
        <div className="audit-family-aside audit-similar-aside audit-family-aside-stack">
          <dl className="kv-grid">
            <dt>{text("Tables", "表集合")}</dt>
            <dd className="mono">{dimensions.tables ? `{${cluster.basis.tables.join(", ")}}` : text("not used", "未使用")}</dd>
            <dt>{text("Join shape", "Join 形态")}</dt>
            <dd className="mono">{dimensions.join ? cluster.basis.joins.join("; ") || text("no join", "无 Join") : text("not used", "未使用")}</dd>
            <dt>{text("Predicate columns", "谓词列集合")}</dt>
            <dd className="mono">
              {dimensions.predicates || cluster.refined
                ? cluster.basis.predicate_columns.length
                  ? `{${cluster.basis.predicate_columns.join(", ")}}`
                  : text("no predicates", "无谓词")
                : text("not used", "未使用")}
            </dd>
          </dl>
          <p className="muted">{text("Projection, grouping and ordering are not part of the basis.", "投影、分组与排序不参与归并。")}</p>

          <div className="audit-family-members-head">
            <span className="field-label">{text("Member templates", "成员模板")}</span>
            <span className="muted">{text("Uncheck to split out", "取消勾选的模板可拆出")}</span>
          </div>

          <ul className="audit-similar-members">
            {cluster.members.map((member) => {
              const checked = !unchecked.has(member.member_key);
              return (
                <li className={checked ? undefined : "unchecked"} key={member.member_key}>
                  {/* The checkbox only splits; reading the template is its own Expand control, so a click on the text never changes the split. */}
                  <div className="audit-similar-member">
                    <input
                      aria-label={`${text("Keep in this cluster", "保留在本聚类")} · ${member.template ?? text("No template", "无模板")}`}
                      checked={checked}
                      onChange={() =>
                        setCheckedState((current) => {
                          const base = current.clusterKey === cluster.cluster_key ? current.keys : new Set<string>();
                          const next = new Set(base);
                          if (next.has(member.member_key)) next.delete(member.member_key);
                          else next.add(member.member_key);
                          return { clusterKey: cluster.cluster_key, keys: next };
                        })
                      }
                      type="checkbox"
                    />
                    {member.template ? (
                      <ExpandableSqlLine
                        copyLabel={text("Copy template", "复制模板")}
                        id={`audit-similar-template-${member.member_key}`}
                        label={text("Member template", "成员模板")}
                        lineClassName="audit-similar-template"
                        value={member.template}
                      />
                    ) : (
                      <span className="audit-similar-template muted">{text("No template", "无模板")}</span>
                    )}
                    <span className="audit-similar-count mono">{text(`${member.execution_count} runs`, `${member.execution_count} 次`)}</span>
                    <SourceTag member={member} text={text} />
                    <span className="audit-similar-total mono" title={metricRawText(metric(member, "query_time", "sum"))}>
                      <AuditMetricValue text={text} value={metric(member, "query_time", "sum")} />
                    </span>
                  </div>
                </li>
              );
            })}
          </ul>

          {splitCount > 0 && (
            <div className="notice-banner" role="status">
              <span>
                {text(
                  `${splitCount} template(s) split out (this view only).`,
                  `已拆出 ${splitCount} 个模板为独立项（仅本视图）。`,
                )}
              </span>
              <Button disabled={loading} onClick={onUndo} size="sm" variant="ghost">
                {text("Undo", "撤销")}
              </Button>
            </div>
          )}

          <p className="audit-similar-planned muted">
            <span className="badge-stamp badge-stamp--planned">Planned</span>{" "}
            {text(
              "Keeping splits for later windows is not available in this version",
              "保存拆分结果、后续窗口沿用：当前版本暂不支持",
            )}
          </p>
        </div>
      ) : null}
    </Dialog>
  );
}
