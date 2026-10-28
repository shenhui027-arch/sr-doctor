//! Synthetic Level 1 text Profiles shared by the scale guards: doctor-core
//! `topology_scale.rs` and doctor-api `concurrent_analysis.rs` (included there
//! by `#[path]`, so items one of them does not use are allowed to be dead).
#![allow(dead_code)]

use std::fmt::Write as _;

/// Subordinate LOCAL_EXCHANGE_SOURCE / CHUNK_ACCUMULATE / LOCAL_EXCHANGE_SINK pipelines per fragment.
pub const BULK_PIPELINES: usize = 25;

/// Fragment `f` holds AGGREGATION `3f` -> PROJECT `3f+1` -> EXCHANGE `3f+2`;
/// the last fragment's PROJECT reads OLAP_SCAN `3 * fragments - 1` instead.
pub struct Shape {
    pub fragments: usize,
    pub bulk_pipelines: usize,
}

impl Shape {
    pub fn aggregation(&self, fragment: usize) -> usize {
        3 * fragment
    }
    pub fn project(&self, fragment: usize) -> usize {
        3 * fragment + 1
    }
    pub fn exchange(&self, fragment: usize) -> usize {
        3 * fragment + 2
    }
    pub fn scan(&self) -> usize {
        3 * self.fragments - 1
    }
    pub fn plan_nodes(&self) -> usize {
        3 * self.fragments
    }

    pub fn topology(&self) -> String {
        let mut nodes = Vec::with_capacity(self.plan_nodes());
        for fragment in 0..self.fragments {
            let sinks = if fragment == 0 {
                String::new()
            } else {
                self.exchange(fragment - 1).to_string()
            };
            nodes.push(format!(
                r#"{{"id":{},"name":"AGGREGATION","properties":{{"sinkIds":[{sinks}],"displayMem":true}},"children":[{}]}}"#,
                self.aggregation(fragment),
                self.project(fragment)
            ));
            let last = fragment + 1 == self.fragments;
            let below = if last {
                self.scan()
            } else {
                self.exchange(fragment)
            };
            nodes.push(format!(
                r#"{{"id":{},"name":"PROJECT","properties":{{"displayMem":false}},"children":[{below}]}}"#,
                self.project(fragment)
            ));
            if !last {
                nodes.push(format!(
                    r#"{{"id":{},"name":"EXCHANGE","properties":{{"displayMem":true}},"children":[{}]}}"#,
                    self.exchange(fragment),
                    self.aggregation(fragment + 1)
                ));
            }
        }
        nodes.push(format!(
            r#"{{"id":{},"name":"OLAP_SCAN","properties":{{"displayMem":false}},"children":[]}}"#,
            self.scan()
        ));
        format!(r#"{{"rootId":0,"nodes":[{}]}}"#, nodes.join(","))
    }

    /// Returns the Profile text, its operator count and its Topology line length.
    pub fn profile(&self) -> (String, usize, usize) {
        let mut text = String::with_capacity(16 * 1024 * 1024);
        text.push_str("Query:\n  Summary:\n     - Query ID: 00000000-0000-0000-0000-000000000001\n     - Total: 1s\n     - Query Type: Query\n     - Query State: Finished\n     - StarRocks Version: 3.5.14-23a56ec\n  Execution:\n");
        let topology_line = format!("     - Topology: {}", self.topology());
        let _ = writeln!(
            text,
            "{topology_line}\n     - QueryCumulativeOperatorTime: 1s"
        );
        let mut operators = 0;
        for fragment in 0..self.fragments {
            let _ = writeln!(text, "    Fragment {fragment}:");
            // Pipeline 0: source -> PROJECT -> AGGREGATE_BLOCKING_SINK.
            text.push_str("      Pipeline (id=0):\n");
            if fragment + 1 == self.fragments {
                operator(&mut text, "OLAP_SCAN", self.scan() as i64, false);
            } else {
                operator(
                    &mut text,
                    "EXCHANGE_SOURCE",
                    self.exchange(fragment) as i64,
                    false,
                );
            }
            operator(&mut text, "PROJECT", self.project(fragment) as i64, false);
            let aggregation = self.aggregation(fragment) as i64;
            operator(&mut text, "AGGREGATE_BLOCKING_SINK", aggregation, false);
            // Pipeline 1: AGGREGATE_BLOCKING_SOURCE -> the fragment sink.
            text.push_str("      Pipeline (id=1):\n");
            operator(&mut text, "AGGREGATE_BLOCKING_SOURCE", aggregation, false);
            if fragment == 0 {
                operator(&mut text, "RESULT_SINK", -1, false);
            } else {
                let target = self.exchange(fragment - 1) as i64;
                operator(&mut text, "EXCHANGE_SINK", target, false);
            }
            operators += 5;
            for pipeline in 0..self.bulk_pipelines {
                let _ = writeln!(text, "      Pipeline (id={}):", pipeline + 2);
                for name in [
                    "LOCAL_EXCHANGE_SOURCE",
                    "CHUNK_ACCUMULATE",
                    "LOCAL_EXCHANGE_SINK",
                ] {
                    operator(&mut text, name, self.project(fragment) as i64, true);
                }
                operators += 3;
            }
        }
        (text, operators, topology_line.len())
    }
}

pub fn operator(out: &mut String, name: &str, plan_node_id: i64, subordinate: bool) {
    let _ = write!(
        out,
        "        {name} (plan_node_id={plan_node_id}):\n          CommonMetrics:\n"
    );
    if subordinate {
        out.push_str("             - IsSubordinate\n");
    }
    out.push_str("             - OperatorTotalTime: 1.000us\n             - PushRowNum: 10\n");
}

/// Adds the registered `__MAX_OF_` / `__MIN_OF_` companions and three more
/// operator counters to every operator: the most metrics per text line the
/// adapter admits for this shape.
pub fn metric_dense(text: &str) -> String {
    let mut block = String::new();
    for name in [
        "OperatorTotalTime",
        "PushRowNum",
        "PullRowNum",
        "PullChunkNum",
        "PullTotalTime",
    ] {
        let value = if name.ends_with("Time") {
            "1.000us"
        } else {
            "10"
        };
        for prefix in ["", "__MAX_OF_", "__MIN_OF_"] {
            if prefix.is_empty() && matches!(name, "OperatorTotalTime" | "PushRowNum") {
                continue;
            }
            let _ = writeln!(block, "             - {prefix}{name}: {value}");
        }
    }
    text.replace(
        "             - PushRowNum: 10\n",
        &format!("             - PushRowNum: 10\n{block}"),
    )
}
