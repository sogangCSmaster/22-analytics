//! 수신 → (아카이브 + ClickHouse) 쓰기 파이프라인.
//!
//! 이벤트는 받는 즉시 로컬 세그먼트 파일에 append하고(원본 보존), 1초 또는 N행마다
//! ClickHouse에 한 번에 insert한다. insert가 계속 실패하면 버퍼 상한까지 보관했다가
//! 넘치는 분은 버린다. 버려진 분도 세그먼트 파일에 남아 있으므로 재적재로 복구할 수 있다.

use crate::archive::SegmentWriter;
use crate::config::Config;
use crate::metrics::Metrics;
use analytics_core::Event;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const CHANNEL_CAPACITY: usize = 4096;
const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_millis(200),
    Duration::from_secs(1),
    Duration::from_secs(3),
];

#[derive(Clone)]
pub struct Pipeline {
    tx: mpsc::Sender<Vec<Event>>,
}

#[derive(Debug)]
pub struct Overloaded;

impl Pipeline {
    /// 대기열이 가득 차면 `Overloaded`. 호출자는 503을 돌려주고, 엣지는 Queue로 재시도한다.
    pub fn try_send(&self, events: Vec<Event>) -> Result<(), Overloaded> {
        if events.is_empty() {
            return Ok(());
        }
        self.tx.try_send(events).map_err(|_| Overloaded)
    }
}

pub async fn start(
    client: analytics_ch::Client,
    cfg: Arc<Config>,
    metrics: Arc<Metrics>,
) -> anyhow::Result<(Pipeline, JoinHandle<anyhow::Result<()>>)> {
    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    let archive = SegmentWriter::open(cfg.archive_dir.join("events"), cfg.archive_rotate)?;
    let handle = tokio::spawn(writer(rx, client, cfg, metrics, archive));
    Ok((Pipeline { tx }, handle))
}

async fn writer(
    mut rx: mpsc::Receiver<Vec<Event>>,
    client: analytics_ch::Client,
    cfg: Arc<Config>,
    metrics: Arc<Metrics>,
    mut archive: SegmentWriter,
) -> anyhow::Result<()> {
    let mut buf: Vec<Event> = Vec::with_capacity(cfg.flush_rows);
    let mut tick = tokio::time::interval(cfg.flush_interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let max_buffer = cfg.flush_rows * 10;

    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(events) => {
                    if let Err(e) = archive.append(&events) {
                        metrics.archive_errors.fetch_add(1, Relaxed);
                        tracing::error!(error = %e, "아카이브 쓰기 실패");
                    }
                    buf.extend(events);
                    if buf.len() >= cfg.flush_rows {
                        flush(&client, &mut buf, &metrics, max_buffer).await;
                    }
                }
                None => break,
            },
            _ = tick.tick() => {
                flush(&client, &mut buf, &metrics, max_buffer).await;
                if let Err(e) = archive.tick() {
                    metrics.archive_errors.fetch_add(1, Relaxed);
                    tracing::error!(error = %e, "아카이브 회전 실패");
                }
            }
        }
    }

    // 종료: 남은 것 모두 비우기
    flush(&client, &mut buf, &metrics, max_buffer).await;
    archive.close()?;
    Ok(())
}

async fn flush(
    client: &analytics_ch::Client,
    buf: &mut Vec<Event>,
    metrics: &Metrics,
    max_buffer: usize,
) {
    if buf.is_empty() {
        return;
    }
    for (attempt, backoff) in std::iter::once(Duration::ZERO)
        .chain(RETRY_BACKOFF)
        .enumerate()
    {
        if !backoff.is_zero() {
            tokio::time::sleep(backoff).await;
        }
        match analytics_ch::rows::insert_events(client, buf).await {
            Ok(()) => {
                metrics.rows_inserted.fetch_add(buf.len() as u64, Relaxed);
                buf.clear();
                return;
            }
            Err(e) => {
                metrics.insert_errors.fetch_add(1, Relaxed);
                tracing::warn!(attempt, rows = buf.len(), error = %e, "ClickHouse insert 실패");
            }
        }
    }
    if buf.len() > max_buffer {
        let overflow = buf.len() - max_buffer;
        buf.drain(..overflow);
        tracing::error!(
            overflow,
            "버퍼 상한 초과: 오래된 이벤트를 버림 (아카이브에서 재적재 가능)"
        );
    }
}
