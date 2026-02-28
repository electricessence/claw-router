//! Prometheus-compatible `/metrics` endpoint.
//!
//! All metrics are derived from the in-memory ring-buffer window. Because the
//! buffer has a fixed capacity, values represent a **sliding window** of recent
//! requests rather than lifetime counters. Use `TYPE gauge` throughout for
//! semantic accuracy — values may decrease as old entries rotate out.
//!
//! Metric families:
//! - `lmg_window_size`             — entries currently in the ring buffer
//! - `lmg_requests`                — per-tier/backend/outcome request counts
//! - `lmg_latency_ms_sum`          — sum of latencies per tier/backend (for avg)
//! - `lmg_latency_ms_count`        — denominator matching the sum above
//! - `lmg_escalations_total`       — requests that were escalated
//! - `lmg_errors_total`            — requests that returned an error

use std::{
    collections::HashMap,
    sync::Arc,
};

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
};

use crate::router::RouterState;

/// `GET /metrics` — renders Prometheus text format.
pub async fn metrics(State(state): State<Arc<RouterState>>) -> impl IntoResponse {
    // Grab the full ring-buffer window in one lock acquisition.
    let entries = state.traffic.recent(usize::MAX).await;

    // --- aggregate ---
    let window_size = entries.len();
    let mut escalations: u64 = 0;
    let mut errors: u64 = 0;

    // (tier, backend, success) → count
    let mut request_counts: HashMap<(String, String, bool), u64> = HashMap::new();
    // (tier, backend) → (latency_sum_ms, count)
    let mut latency: HashMap<(String, String), (u64, u64)> = HashMap::new();

    for e in &entries {
        if e.escalated { escalations += 1; }
        if !e.success { errors += 1; }

        *request_counts
            .entry((e.tier.clone(), e.backend.clone(), e.success))
            .or_default() += 1;

        let lat = latency.entry((e.tier.clone(), e.backend.clone())).or_default();
        lat.0 += e.latency_ms;
        lat.1 += 1;
    }

    // --- render ---
    let mut out = String::with_capacity(1024);

    // window_size
    out.push_str("# HELP lmg_window_size Number of requests currently held in the ring-buffer window.\n");
    out.push_str("# TYPE lmg_window_size gauge\n");
    out.push_str(&format!("lmg_window_size {window_size}\n\n"));

    // request counts
    out.push_str("# HELP lmg_requests Request count in the current window, labelled by tier, backend, and outcome.\n");
    out.push_str("# TYPE lmg_requests gauge\n");
    let mut req_rows: Vec<_> = request_counts.iter().collect();
    req_rows.sort_by(|a, b| a.0.cmp(b.0));
    for ((tier, backend, success), count) in req_rows {
        let success_str = if *success { "true" } else { "false" };
        out.push_str(&format!(
            "lmg_requests{{tier=\"{tier}\",backend=\"{backend}\",success=\"{success_str}\"}} {count}\n"
        ));
    }
    out.push('\n');

    // latency sum + count
    out.push_str("# HELP lmg_latency_ms_sum Sum of request latency (ms) in the current window, grouped by tier and backend.\n");
    out.push_str("# TYPE lmg_latency_ms_sum gauge\n");
    out.push_str("# HELP lmg_latency_ms_count Number of observations for the latency sum above.\n");
    out.push_str("# TYPE lmg_latency_ms_count gauge\n");
    let mut lat_rows: Vec<_> = latency.iter().collect();
    lat_rows.sort_by(|a, b| a.0.cmp(b.0));
    for ((tier, backend), (sum, count)) in lat_rows {
        out.push_str(&format!(
            "lmg_latency_ms_sum{{tier=\"{tier}\",backend=\"{backend}\"}} {sum}\n"
        ));
        out.push_str(&format!(
            "lmg_latency_ms_count{{tier=\"{tier}\",backend=\"{backend}\"}} {count}\n"
        ));
    }
    out.push('\n');

    // escalations
    out.push_str("# HELP lmg_escalations_total Requests escalated to a higher tier in the current window.\n");
    out.push_str("# TYPE lmg_escalations_total gauge\n");
    out.push_str(&format!("lmg_escalations_total {escalations}\n\n"));

    // errors
    out.push_str("# HELP lmg_errors_total Requests that returned an error in the current window.\n");
    out.push_str("# TYPE lmg_errors_total gauge\n");
    out.push_str(&format!("lmg_errors_total {errors}\n"));

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        out,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::traffic::{TrafficEntry, TrafficLog};

    fn mock_log() -> Arc<TrafficLog> {
        let log = Arc::new(TrafficLog::new(100));
        log.push(
            TrafficEntry::new("fast".into(), "openai-prod".into(), 120, true)
                .with_requested_model("gpt-4o"),
        );
        log.push(
            TrafficEntry::new("fast".into(), "openai-prod".into(), 95, true)
                .with_requested_model("gpt-4o"),
        );
        log.push(
            TrafficEntry::new("economy".into(), "ollama-local".into(), 430, true),
        );
        log.push(
            TrafficEntry::new("fast".into(), "openai-prod".into(), 80, false)
                .with_error("upstream 500"),
        );
        log
    }

    #[tokio::test]
    async fn window_size_equals_entry_count() {
        let log = mock_log();
        let entries = log.recent(usize::MAX).await;
        assert_eq!(entries.len(), 4);
    }

    #[tokio::test]
    async fn error_count_is_accurate() {
        let log = mock_log();
        let entries = log.recent(usize::MAX).await;
        let errors = entries.iter().filter(|e| !e.success).count();
        assert_eq!(errors, 1);
    }

    #[tokio::test]
    async fn latency_sum_is_accurate() {
        let log = mock_log();
        let entries = log.recent(usize::MAX).await;
        let sum: u64 = entries
            .iter()
            .filter(|e| e.tier == "fast" && e.backend == "openai-prod")
            .map(|e| e.latency_ms)
            .sum();
        // 120 + 95 + 80 = 295
        assert_eq!(sum, 295);
    }
}
