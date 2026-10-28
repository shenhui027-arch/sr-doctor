//! Concurrent analysis guard (v0.6-04), release builds only.
//!
//! The largest operator-heavy and metric-dense Profiles the default budget
//! admits (doctor-core `topology_scale.rs`) are posted to `/api/v1/analyses`
//! 2 and 4 at a time. With the default two work slots:
//!
//! * two concurrent analyses both finish inside the 10 s `analysis_wall`;
//! * of four, exactly two are admitted and finish in time; the other two are
//!   refused at once with the `analysis_busy` envelope, never a partial report.
//!
//! With a 3 s `analysis_wall` both shapes run past the deadline two at a
//! time. Both slots are free again within `SLOT_RELEASE` of the last answer
//! (v0.6-08); with stage-boundary cancellation alone that took 2.5-2.9 s.
//!
//! Debug timings say nothing about the API budget, so the test is ignored there.

#[path = "../../doctor-core/tests/support/scale_profile.rs"]
mod scale_profile;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use doctor_api::{ApiConfig, ApiState, WorkKind, router};
use scale_profile::{BULK_PIPELINES, Shape, metric_dense};
use tokio::sync::Barrier;
use tower::ServiceExt;

const ANALYSIS_WALL: Duration = Duration::from_secs(10);
/// Measured 0.1-0.3 s; generous for a loaded machine, far below the 2.5 s
/// that stage-boundary polling needed.
const SLOT_RELEASE: Duration = Duration::from_secs(1);

struct Outcome {
    status: StatusCode,
    elapsed: Duration,
    bytes: axum::body::Bytes,
}

/// A success body is a complete report; its 50-90 MiB are skipped, not built.
#[derive(serde::Deserialize)]
struct Report {
    #[allow(dead_code)]
    outcome: ReportOutcome,
}

#[derive(serde::Deserialize)]
struct ReportOutcome {
    #[allow(dead_code)]
    report: serde::de::IgnoredAny,
}

async fn post_concurrently(state: &ApiState, body: &str, requests: usize) -> Vec<Outcome> {
    let app = router(state.clone());
    let barrier = Arc::new(Barrier::new(requests));
    let mut tasks = Vec::with_capacity(requests);
    for index in 0..requests {
        let app = app.clone();
        let barrier = Arc::clone(&barrier);
        let mut request = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/analyses")
            .header("host", "127.0.0.1:8000")
            .header("content-type", "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((
                [127, 0, 0, 1],
                41000 + index as u16,
            ))));
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            let started = Instant::now();
            let response = app.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            Outcome {
                status,
                elapsed: started.elapsed(),
                bytes,
            }
        }));
    }
    let mut outcomes = Vec::with_capacity(requests);
    for task in tasks {
        outcomes.push(task.await.unwrap());
    }
    outcomes
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(debug_assertions, ignore = "timing guard for release builds")]
async fn default_budget_maximum_profiles_analyse_two_at_a_time_and_refuse_the_rest() {
    for (name, fragments, dense) in [("operator-heavy", 590, false), ("metric-dense", 170, true)] {
        let body = profile_body(fragments, dense);
        for requests in [2, 4] {
            let state = ApiState::new(ApiConfig::default()).unwrap();
            let outcomes = post_concurrently(&state, &body, requests).await;
            let mut admitted = 0;
            for outcome in &outcomes {
                eprintln!(
                    "{name} x{requests}: {} in {:?}",
                    outcome.status, outcome.elapsed
                );
                match outcome.status {
                    StatusCode::OK => {
                        admitted += 1;
                        serde_json::from_slice::<Report>(&outcome.bytes).unwrap();
                        assert!(
                            outcome.elapsed < ANALYSIS_WALL,
                            "{name} x{requests}: {:?} exceeds the {ANALYSIS_WALL:?} analysis wall",
                            outcome.elapsed
                        );
                    }
                    StatusCode::TOO_MANY_REQUESTS => {
                        let body: serde_json::Value =
                            serde_json::from_slice(&outcome.bytes).unwrap();
                        assert_eq!(body["code"], "analysis_busy");
                        assert_eq!(
                            body["details"]["reason"],
                            "concurrent_request_limit_reached"
                        );
                        assert!(body.get("outcome").is_none());
                    }
                    status => panic!(
                        "{name} x{requests}: unexpected {status}: {}",
                        String::from_utf8_lossy(&outcome.bytes)
                    ),
                }
            }
            assert_eq!(admitted, 2, "{name} x{requests}");
        }
    }
}

fn profile_body(fragments: usize, dense: bool) -> String {
    let (text, _, _) = Shape {
        fragments,
        bulk_pipelines: BULK_PIPELINES,
    }
    .profile();
    let text = if dense { metric_dense(&text) } else { text };
    serde_json::json!({ "source_kind": "text_profile", "content": text }).to_string()
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(debug_assertions, ignore = "timing guard for release builds")]
async fn analyses_past_the_deadline_free_their_slots_soon_after_answering() {
    for (name, fragments, dense) in [("operator-heavy", 590, false), ("metric-dense", 170, true)] {
        let body = profile_body(fragments, dense);
        let mut config = ApiConfig::default();
        config.budgets.analysis_wall = Duration::from_secs(3);
        let state = ApiState::new(config).unwrap();
        let outcomes = post_concurrently(&state, &body, 2).await;
        let answered = Instant::now();
        for outcome in &outcomes {
            eprintln!(
                "{name} x2, 3 s wall: {} in {:?}",
                outcome.status, outcome.elapsed
            );
            match outcome.status {
                StatusCode::OK => {
                    serde_json::from_slice::<Report>(&outcome.bytes).unwrap();
                }
                StatusCode::REQUEST_TIMEOUT => {
                    let body: serde_json::Value = serde_json::from_slice(&outcome.bytes).unwrap();
                    assert_eq!(body["code"], "analysis_timeout");
                    assert!(body.get("outcome").is_none());
                }
                status => panic!("{name}: unexpected {status}"),
            }
        }
        // Both slots free: two leases can be held at once.
        loop {
            let first = state.budgets().begin(WorkKind::Analysis);
            let second = state.budgets().begin(WorkKind::Analysis);
            if first.is_ok() && second.is_ok() {
                break;
            }
            drop((first, second));
            assert!(
                answered.elapsed() < SLOT_RELEASE,
                "{name}: slots still busy {:?} after the last answer",
                answered.elapsed()
            );
            tokio::task::yield_now().await;
        }
        eprintln!(
            "{name} x2, 3 s wall: slots free {:?} after the last answer",
            answered.elapsed()
        );
    }
}
