//! What a notification says. A batch holds the facts of the alerts raised (or
//! resolved) by one evaluation of one cluster; each channel renders it in its
//! own format. Only alert facts appear: fingerprints, severities and median
//! changes, plus the literal-free template when the user opts in; for load
//! alerts (task 59) the rule, the job or table and the counted values. No
//! error log text and no Routine Load state-change reason ever appear.
use doctor_contracts::{NotifyEvent, NotifyLanguage};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AlertFact {
    pub alert_id: i64,
    /// `high`, `medium` or `low`.
    pub severity: String,
    pub confidence: String,
    pub evidence: String,
    /// `official_digest` or the unofficial fingerprint kind.
    pub fingerprint_kind: String,
    pub template_key: String,
    /// Literal-free template; only when the user opted in.
    pub template: Option<String>,
    /// Audit source units (ms).
    pub baseline_median_ms: Option<u64>,
    pub current_median_ms: Option<u64>,
    pub relative_permille: Option<i64>,
    pub bucket_start: i64,
    pub engine_version: String,
    pub rule_version: String,
}

/// One load alert (task 59): counted facts only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LoadAlertFact {
    pub alert_id: i64,
    /// `high`, `medium` or `low`.
    pub severity: String,
    /// `routine_paused`, `routine_stopped`, `lag_growing`, `failure_rate`
    /// or `filter_rate`.
    pub rule: String,
    /// The job (`name · db.table`) or the table and load kind.
    pub subject: String,
    /// `STOPPED` or `CANCELLED` for `routine_stopped`.
    pub state: Option<String>,
    /// Rates in parts per million (`failure_rate`, `filter_rate`).
    pub baseline_ppm: Option<u32>,
    pub current_ppm: Option<u32>,
    /// Loads (`failure_rate`) or input rows (`filter_rate`) in the window.
    /// Decimal text, never narrowed: counts can exceed 64 bits.
    pub window_count: Option<String>,
    /// Largest partition lag at the window's first and last hour.
    pub first_lag: Option<String>,
    pub latest_lag: Option<String>,
    pub window_start: i64,
    pub window_end: i64,
    pub engine_version: String,
    pub rule_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AlertBatch {
    /// The collection target, "host:port".
    pub cluster: String,
    pub event: NotifyEvent,
    pub alerts: Vec<AlertFact>,
    /// Task 59; absent in payloads queued before it.
    #[serde(default)]
    pub load_alerts: Vec<LoadAlertFact>,
}

impl AlertBatch {
    pub fn test() -> Self {
        Self {
            cluster: String::new(),
            event: NotifyEvent::Test,
            alerts: Vec::new(),
            load_alerts: Vec::new(),
        }
    }

    /// SQL and load alerts together.
    pub fn count(&self) -> usize {
        self.alerts.len() + self.load_alerts.len()
    }

    pub fn highest_severity(&self) -> &str {
        ["high", "medium", "low"]
            .into_iter()
            .find(|level| {
                self.alerts.iter().any(|alert| alert.severity == *level)
                    || self
                        .load_alerts
                        .iter()
                        .any(|alert| alert.severity == *level)
            })
            .unwrap_or("low")
    }
}

pub fn severity_rank(severity: &str) -> u8 {
    match severity {
        "high" => 3,
        "medium" => 2,
        _ => 1,
    }
}

pub struct Context<'a> {
    pub language: NotifyLanguage,
    pub link: Option<&'a str>,
    /// Thresholds are on trial until calibrated on real workload (G2).
    pub calibrated: bool,
}

impl Context<'_> {
    fn t<'s>(&self, en: &'s str, zh: &'s str) -> &'s str {
        match self.language {
            NotifyLanguage::En => en,
            NotifyLanguage::Zh => zh,
        }
    }

    pub fn level(&self, value: &str) -> &'static str {
        match (self.language, value) {
            (NotifyLanguage::Zh, "high") => "高",
            (NotifyLanguage::Zh, "medium") => "中",
            (NotifyLanguage::Zh, _) => "低",
            (NotifyLanguage::En, "high") => "high",
            (NotifyLanguage::En, "medium") => "medium",
            (NotifyLanguage::En, _) => "low",
        }
    }

    fn evidence(&self, value: &str) -> &'static str {
        match (self.language, value) {
            (NotifyLanguage::Zh, "complete") => "完整",
            (NotifyLanguage::Zh, "partial") => "部分",
            (NotifyLanguage::Zh, _) => "不可用",
            (NotifyLanguage::En, "complete") => "complete",
            (NotifyLanguage::En, "partial") => "partial",
            (NotifyLanguage::En, _) => "unavailable",
        }
    }

    fn fingerprint(&self, kind: &str) -> &'static str {
        match (self.language, kind == "official_digest") {
            (NotifyLanguage::Zh, true) => "官方 digest",
            (NotifyLanguage::Zh, false) => "非官方指纹",
            (NotifyLanguage::En, true) => "official digest",
            (NotifyLanguage::En, false) => "unofficial fingerprint",
        }
    }
}

pub fn duration_text(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1_000.0)
    } else {
        format!("{:.1} min", ms as f64 / 60_000.0)
    }
}

fn short_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 12 {
        key.to_owned()
    } else {
        format!(
            "{}…{}",
            chars[..4].iter().collect::<String>(),
            chars[chars.len() - 4..].iter().collect::<String>()
        )
    }
}

/// A rate in parts per million as a percent with enough digits.
fn ppm_text(ppm: u32) -> String {
    let percent = f64::from(ppm) / 10_000.0;
    if ppm == 0 {
        "0%".to_owned()
    } else if percent >= 1.0 {
        format!("{percent:.1}%")
    } else if percent >= 0.01 {
        format!("{percent:.2}%")
    } else {
        format!("{percent:.3}%")
    }
}

/// Thousands-grouped decimal text; anything else is shown as unknown.
fn grouped(value: &str) -> String {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return "—".to_owned();
    }
    let digits = value;
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `MM-DD HH:00–HH:00 UTC` for a window of whole hours.
fn window_text(start: i64, end: i64) -> String {
    let at = |seconds: i64| {
        time::OffsetDateTime::from_unix_timestamp(seconds)
            .map(|at| (at.month() as u8, at.day(), at.hour()))
            .unwrap_or((1, 1, 0))
    };
    let (month, day, hour) = at(start);
    let (_, _, end_hour) = at(end);
    format!("{month:02}-{day:02} {hour:02}:00–{end_hour:02}:00 UTC")
}

fn hour_text(start: i64) -> String {
    let format = |seconds: i64| {
        time::OffsetDateTime::from_unix_timestamp(seconds)
            .map(|at| (at.month() as u8, at.day(), at.hour()))
            .unwrap_or((1, 1, 0))
    };
    let (month, day, hour) = format(start);
    let (_, _, next) = format(start + 3_600);
    format!("{month:02}-{day:02} {hour:02}:00–{next:02}:00 UTC")
}

/// One alert, one line of facts: severity, median change, fingerprint.
pub struct AlertLine {
    pub severity: &'static str,
    pub change: String,
    pub fingerprint: String,
    pub detail: String,
    pub template: Option<String>,
}

impl Context<'_> {
    pub fn title(&self, batch: &AlertBatch) -> String {
        let count = batch.count();
        match batch.event {
            NotifyEvent::Test => self
                .t("SR Doctor test message", "SR Doctor 测试消息")
                .to_owned(),
            NotifyEvent::NewAlerts => match self.language {
                NotifyLanguage::Zh => format!(
                    "SR Doctor · {}：{count} 条新告警（最高严重度 {}）",
                    batch.cluster,
                    self.level(batch.highest_severity())
                ),
                NotifyLanguage::En => format!(
                    "SR Doctor · {}: {count} new alert{} (highest severity {})",
                    batch.cluster,
                    if count == 1 { "" } else { "s" },
                    self.level(batch.highest_severity())
                ),
            },
            NotifyEvent::Resolved => match self.language {
                NotifyLanguage::Zh => {
                    format!("SR Doctor · {}：{count} 条告警已恢复", batch.cluster)
                }
                NotifyLanguage::En => format!(
                    "SR Doctor · {}: {count} alert{} resolved",
                    batch.cluster,
                    if count == 1 { "" } else { "s" }
                ),
            },
        }
    }

    fn load_line(&self, alert: &LoadAlertFact) -> AlertLine {
        let prefix = self.t("Load", "导入");
        let change = match alert.rule.as_str() {
            "routine_paused" => format!(
                "{prefix} · {}",
                self.t("Routine Load paused", "Routine Load 暂停")
            ),
            "routine_stopped" => format!(
                "{prefix} · {} ({})",
                self.t("Routine Load stopped", "Routine Load 已结束"),
                alert.state.as_deref().unwrap_or("—")
            ),
            "lag_growing" => format!(
                "{prefix} · {} {} → {} {}",
                self.t("Lag keeps growing", "积压持续增长"),
                alert
                    .first_lag
                    .as_deref()
                    .map_or_else(|| "—".to_owned(), grouped),
                alert
                    .latest_lag
                    .as_deref()
                    .map_or_else(|| "—".to_owned(), grouped),
                self.t("messages", "条")
            ),
            rule => format!(
                "{prefix} · {} {} → {}",
                if rule == "filter_rate" {
                    self.t("Filter rate jump", "过滤率突增")
                } else {
                    self.t("Failure rate jump", "失败率突增")
                },
                alert.baseline_ppm.map_or_else(|| "—".to_owned(), ppm_text),
                alert.current_ppm.map_or_else(|| "—".to_owned(), ppm_text)
            ),
        };
        let window = match alert.rule.as_str() {
            "routine_paused" | "routine_stopped" => match self.language {
                NotifyLanguage::Zh => format!("{} 的快照", hour_text(alert.window_start)),
                NotifyLanguage::En => format!("snapshot in {}", hour_text(alert.window_start)),
            },
            "failure_rate" => match self.language {
                NotifyLanguage::Zh => format!(
                    "最近 3 小时 {} 次导入 · {}",
                    alert
                        .window_count
                        .as_deref()
                        .map_or_else(|| "—".to_owned(), grouped),
                    window_text(alert.window_start, alert.window_end)
                ),
                NotifyLanguage::En => format!(
                    "{} loads in the last 3 hours · {}",
                    alert
                        .window_count
                        .as_deref()
                        .map_or_else(|| "—".to_owned(), grouped),
                    window_text(alert.window_start, alert.window_end)
                ),
            },
            "filter_rate" => match self.language {
                NotifyLanguage::Zh => format!(
                    "最近 3 小时输入 {} 行 · {}",
                    alert
                        .window_count
                        .as_deref()
                        .map_or_else(|| "—".to_owned(), grouped),
                    window_text(alert.window_start, alert.window_end)
                ),
                NotifyLanguage::En => format!(
                    "{} input rows in the last 3 hours · {}",
                    alert
                        .window_count
                        .as_deref()
                        .map_or_else(|| "—".to_owned(), grouped),
                    window_text(alert.window_start, alert.window_end)
                ),
            },
            _ => window_text(alert.window_start, alert.window_end),
        };
        AlertLine {
            severity: self.level(&alert.severity),
            change,
            fingerprint: alert.subject.clone(),
            detail: if alert.engine_version == "unknown" {
                format!("{window} · {}", alert.rule_version)
            } else {
                format!(
                    "{window} · StarRocks {} · {}",
                    alert.engine_version, alert.rule_version
                )
            },
            template: None,
        }
    }

    pub fn lines(&self, batch: &AlertBatch) -> Vec<AlertLine> {
        let mut ranked: Vec<(u8, AlertLine)> = batch
            .load_alerts
            .iter()
            .map(|alert| (severity_rank(&alert.severity), self.load_line(alert)))
            .collect();
        ranked.extend(
            self.sql_lines(batch)
                .into_iter()
                .zip(
                    batch
                        .alerts
                        .iter()
                        .map(|alert| severity_rank(&alert.severity)),
                )
                .map(|(line, rank)| (rank, line)),
        );
        // Stable: within a severity, load alerts first, then SQL alerts.
        ranked.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
        ranked.into_iter().map(|(_, line)| line).collect()
    }

    fn sql_lines(&self, batch: &AlertBatch) -> Vec<AlertLine> {
        batch
            .alerts
            .iter()
            .map(|alert| {
                let medians = match (alert.baseline_median_ms, alert.current_median_ms) {
                    (Some(before), Some(after)) => {
                        let relative = alert.relative_permille.map_or_else(
                            || {
                                self.t(" (baseline 0, no percentage)", "（基线为 0，百分比不定义）")
                                    .to_owned()
                            },
                            |permille| format!("（{:+}%）", permille / 10),
                        );
                        let relative = if self.language == NotifyLanguage::En {
                            relative.replace('（', " (").replace('）', ")")
                        } else {
                            relative
                        };
                        format!(
                            "{} → {}{relative}",
                            duration_text(before),
                            duration_text(after)
                        )
                    }
                    _ => "—".to_owned(),
                };
                AlertLine {
                    severity: self.level(&alert.severity),
                    change: format!("{} {medians}", self.t("Median query time", "耗时中位数")),
                    fingerprint: format!(
                        "{} · {}",
                        short_key(&alert.template_key),
                        self.fingerprint(&alert.fingerprint_kind)
                    ),
                    detail: format!(
                        "{} {} · {} {} · {} · StarRocks {} · {}",
                        self.t("Confidence", "置信度"),
                        self.level(&alert.confidence),
                        self.t("evidence", "证据"),
                        self.evidence(&alert.evidence),
                        hour_text(alert.bucket_start),
                        alert.engine_version,
                        alert.rule_version
                    ),
                    template: alert.template.clone(),
                }
            })
            .collect()
    }

    pub fn footer(&self) -> String {
        let mut footer = String::new();
        if !self.calibrated {
            footer.push_str(self.t(
                "Thresholds are on trial until calibrated on real workload. ",
                "阈值尚未用真实业务分布校准，处于试运行。",
            ));
        }
        footer.push_str(self.t(
            "Sent automatically by SR Doctor; no SQL text or error log text is included.",
            "此消息由 SR Doctor 自动发送，不含 SQL 原文与错误日志原文。",
        ));
        footer
    }

    pub fn test_body(&self) -> &'static str {
        self.t(
            "This channel and target can deliver SR Doctor alert notifications.",
            "这个渠道与发送对象可以收到 SR Doctor 的告警通知。",
        )
    }

    pub fn link_label(&self) -> &'static str {
        self.t("View alerts in SR Doctor", "在 SR Doctor 中查看告警")
    }
}

pub struct Email {
    pub subject: String,
    pub text: String,
    pub html: String,
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(c),
        }
    }
    escaped
}

pub fn email(context: &Context<'_>, batch: &AlertBatch) -> Email {
    let subject = format!(
        "[SR Doctor] {}",
        context
            .title(batch)
            .trim_start_matches("SR Doctor · ")
            .trim_start_matches("SR Doctor ")
    );
    let mut text = String::new();
    let mut html = String::from(
        "<div style=\"font-family:sans-serif;font-size:14px;line-height:1.5;color:#151a22\">",
    );
    html.push_str(&format!(
        "<h2 style=\"font-size:16px;margin:0 0 12px\">{}</h2>",
        escape_html(&context.title(batch))
    ));
    if batch.event == NotifyEvent::Test {
        text.push_str(context.test_body());
        text.push('\n');
        html.push_str(&format!("<p>{}</p>", escape_html(context.test_body())));
    }
    for line in context.lines(batch) {
        text.push_str(&format!(
            "- [{}] {}\n  {}\n  {}\n",
            line.severity, line.change, line.fingerprint, line.detail
        ));
        html.push_str(&format!(
            "<p style=\"margin:0 0 10px\"><b>{}</b> · {}<br><span style=\"font-family:monospace;color:#5d6675\">{}</span><br><span style=\"color:#5d6675;font-size:12px\">{}</span>",
            escape_html(line.severity), escape_html(&line.change), escape_html(&line.fingerprint), escape_html(&line.detail)
        ));
        if let Some(template) = &line.template {
            text.push_str(&format!("  {template}\n"));
            html.push_str(&format!(
                "<br><code style=\"font-size:12px\">{}</code>",
                escape_html(template)
            ));
        }
        html.push_str("</p>");
    }
    if let Some(link) = context.link {
        text.push_str(&format!("\n{}: {link}\n", context.link_label()));
        html.push_str(&format!(
            "<p><a href=\"{}\">{}</a></p>",
            escape_html(link),
            escape_html(context.link_label())
        ));
    }
    text.push_str(&format!("\n{}\n", context.footer()));
    html.push_str(&format!(
        "<p style=\"color:#5d6675;font-size:12px\">{}</p></div>",
        escape_html(&context.footer())
    ));
    Email {
        subject,
        text,
        html,
    }
}

pub fn markdown(context: &Context<'_>, batch: &AlertBatch) -> String {
    let mut text = String::new();
    if batch.event == NotifyEvent::Test {
        text.push_str(context.test_body());
        text.push_str("\n\n");
    }
    for line in context.lines(batch) {
        text.push_str(&format!(
            "- **{}** {}\n  `{}`\n  {}\n",
            line.severity, line.change, line.fingerprint, line.detail
        ));
        if let Some(template) = &line.template {
            text.push_str(&format!("  `{}`\n", template.replace('`', "'")));
        }
    }
    if let Some(link) = context.link {
        text.push_str(&format!("\n[{}]({link})\n", context.link_label()));
    }
    text.push_str(&format!("\n{}", context.footer()));
    text
}

/// SMS template variables: cluster, count, highest severity. Clusters longer
/// than the provider's variable limit are cut and marked.
pub fn sms_params(context: &Context<'_>, batch: &AlertBatch) -> [String; 3] {
    let (cluster, count, level) = match batch.event {
        NotifyEvent::Test => (
            context.t("test", "测试").to_owned(),
            "0".to_owned(),
            context.t("none", "无").to_owned(),
        ),
        _ => (
            batch.cluster.clone(),
            batch.count().to_string(),
            context.level(batch.highest_severity()).to_owned(),
        ),
    };
    let cluster: String = if cluster.chars().count() > 20 {
        cluster
            .chars()
            .take(19)
            .chain(std::iter::once('…'))
            .collect()
    } else {
        cluster
    };
    [cluster, count, level]
}

#[cfg(test)]
pub(crate) fn sample_batch() -> AlertBatch {
    AlertBatch {
        cluster: "10.0.0.5:9030".into(),
        event: NotifyEvent::NewAlerts,
        alerts: vec![
            AlertFact {
                alert_id: 2,
                severity: "medium".into(),
                confidence: "medium".into(),
                evidence: "partial".into(),
                fingerprint_kind: "unofficial".into(),
                template_key: "5d22aaaabbbbe810".into(),
                template: None,
                baseline_median_ms: Some(88),
                current_median_ms: Some(160),
                relative_permille: Some(818),
                bucket_start: 1_793_264_400,
                engine_version: "3.5.14".into(),
                rule_version: "doctor_v0_1".into(),
            },
            AlertFact {
                alert_id: 1,
                severity: "high".into(),
                confidence: "high".into(),
                evidence: "complete".into(),
                fingerprint_kind: "official_digest".into(),
                template_key: "a91f00001111ccc3c07".into(),
                template: Some("SELECT * FROM t WHERE id = ? <x>".into()),
                baseline_median_ms: Some(420),
                current_median_ms: Some(1_900),
                relative_permille: Some(3_523),
                bucket_start: 1_793_264_400,
                engine_version: "3.5.14".into(),
                rule_version: "doctor_v0_1".into(),
            },
        ],
        load_alerts: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zh() -> Context<'static> {
        Context {
            language: NotifyLanguage::Zh,
            link: Some("http://10.0.0.5:8080"),
            calibrated: false,
        }
    }

    #[test]
    fn titles_and_lines_put_the_highest_severity_first() {
        let batch = sample_batch();
        assert_eq!(
            zh().title(&batch),
            "SR Doctor · 10.0.0.5:9030：2 条新告警（最高严重度 高）"
        );
        let lines = zh().lines(&batch);
        assert_eq!(lines[0].severity, "高");
        assert_eq!(lines[0].change, "耗时中位数 420 ms → 1.9 s（+352%）");
        assert_eq!(lines[0].fingerprint, "a91f…3c07 · 官方 digest");
        assert!(
            lines[0].detail.contains("10-29 09:00–10:00 UTC"),
            "{}",
            lines[0].detail
        );
        assert_eq!(lines[1].fingerprint, "5d22…e810 · 非官方指纹");
        let en = Context {
            language: NotifyLanguage::En,
            link: None,
            calibrated: true,
        };
        assert_eq!(
            en.lines(&batch)[0].change,
            "Median query time 420 ms → 1.9 s (+352%)"
        );
        assert_eq!(
            en.title(&batch),
            "SR Doctor · 10.0.0.5:9030: 2 new alerts (highest severity high)"
        );
    }

    #[test]
    fn email_html_escapes_everything_it_shows() {
        let mail = email(&zh(), &sample_batch());
        assert_eq!(
            mail.subject,
            "[SR Doctor] 10.0.0.5:9030：2 条新告警（最高严重度 高）"
        );
        assert_eq!(
            email(&zh(), &AlertBatch::test()).subject,
            "[SR Doctor] 测试消息"
        );
        assert!(mail.html.contains("&lt;x&gt;"));
        assert!(!mail.html.contains("<x>"));
        assert!(
            mail.text
                .contains("在 SR Doctor 中查看告警: http://10.0.0.5:8080")
        );
        assert!(mail.text.contains("不含 SQL 原文"));
    }

    #[test]
    fn sms_params_are_short_and_never_carry_templates() {
        let mut batch = sample_batch();
        assert_eq!(
            sms_params(&zh(), &batch),
            ["10.0.0.5:9030".to_owned(), "2".into(), "高".into()]
        );
        batch.cluster = "a-very-long-cluster-name.example.com:9030".into();
        let [cluster, ..] = sms_params(&zh(), &batch);
        assert_eq!(cluster.chars().count(), 20);
        assert!(cluster.ends_with('…'));
        assert_eq!(sms_params(&zh(), &AlertBatch::test())[1], "0");
    }

    fn load_alert(rule: &str, severity: &str) -> LoadAlertFact {
        LoadAlertFact {
            alert_id: 9,
            severity: severity.into(),
            rule: rule.into(),
            subject: "ods.orders · Stream Load".into(),
            state: None,
            baseline_ppm: Some(4_100),
            current_ppm: Some(118_000),
            window_count: Some("186".into()),
            first_lag: None,
            latest_lag: None,
            window_start: 1_793_437_200,
            window_end: 1_793_448_000,
            engine_version: "3.5.14".into(),
            rule_version: "r".into(),
        }
    }

    #[test]
    fn audit_la_03_load_alerts_render_facts_and_count_with_sql_alerts() {
        let mut batch = sample_batch();
        batch.load_alerts = vec![load_alert("failure_rate", "high"), {
            let mut lag = load_alert("lag_growing", "medium");
            lag.subject = "clicks_rl · ods.clicks_rt".into();
            lag.first_lag = Some("18200".into());
            lag.latest_lag = Some("62410".into());
            lag
        }];
        let context = zh();
        assert!(context.title(&batch).contains("4 条新告警"));
        let lines = context.lines(&batch);
        assert_eq!(lines.len(), 4);
        // Highest severity first; within it the load alert comes first.
        assert_eq!(lines[0].change, "导入 · 失败率突增 0.41% → 11.8%");
        assert!(lines[0].detail.starts_with("最近 3 小时 186 次导入 · "));
        assert_eq!(lines[0].fingerprint, "ods.orders · Stream Load");
        assert!(
            lines
                .iter()
                .any(|line| line.change == "导入 · 积压持续增长 18,200 → 62,410 条")
        );
        assert_eq!(sms_params(&context, &batch)[1], "4");
        assert!(context.footer().contains("错误日志原文"));
        // Payloads queued before task 59 still parse.
        let old = serde_json::json!({ "cluster": "c", "event": "new_alerts", "alerts": [] });
        let parsed: AlertBatch = serde_json::from_value(old).unwrap();
        assert!(parsed.load_alerts.is_empty());
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration_text(999), "999 ms");
        assert_eq!(duration_text(1_900), "1.9 s");
        assert_eq!(duration_text(150_000), "2.5 min");
    }
}
