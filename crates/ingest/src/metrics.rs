use axum::extract::State;
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[derive(Default)]
pub struct Metrics {
    pub events_received: AtomicU64,
    pub events_dropped: AtomicU64,
    pub batches_rejected: AtomicU64,
    pub rows_inserted: AtomicU64,
    pub insert_errors: AtomicU64,
    pub archive_errors: AtomicU64,
}

pub async fn handler(State(s): State<crate::AppState>) -> String {
    let m = &s.metrics;
    let mut out = String::new();
    for (name, v) in [
        ("ingest_events_received_total", &m.events_received),
        ("ingest_events_dropped_total", &m.events_dropped),
        ("ingest_batches_rejected_total", &m.batches_rejected),
        ("ingest_rows_inserted_total", &m.rows_inserted),
        ("ingest_insert_errors_total", &m.insert_errors),
        ("ingest_archive_errors_total", &m.archive_errors),
    ] {
        let _ = writeln!(out, "# TYPE {name} counter\n{name} {}", v.load(Relaxed));
    }
    out
}
