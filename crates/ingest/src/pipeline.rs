//! 수신 → (아카이브 + ClickHouse) 쓰기 파이프라인.
//!
//! 이벤트는 받는 즉시 로컬 세그먼트 파일에 append하고(원본 보존), 주기 또는 N행마다
//! ClickHouse에 한 번에 insert한다. insert(재시도 대기 포함)는 별도 작업에서 한 번에 하나만
//! 돌리고, 수신 루프는 그동안에도 계속 받아 아카이브와 버퍼에 쌓는다 (insert가 수신을 막지 않는다).
//! insert가 계속 실패하면 버퍼 상한까지 보관했다가 넘치는 분(오래된 것부터)은 버린다.
//! 버려진 분도 세그먼트 파일에 남아 있으므로 재적재로 복구할 수 있다.

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

/// 진행 중인 insert 작업. 끝나면 (실패했으면 되돌려 받을) 행을 돌려준다.
type InFlight = JoinHandle<Option<Vec<Event>>>;

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
    let mut inflight: Option<InFlight> = None;

    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(events) => {
                    if let Err(e) = archive.append(&events) {
                        metrics.archive_errors.fetch_add(1, Relaxed);
                        tracing::error!(error = %e, "아카이브 쓰기 실패");
                    }
                    buf.extend(events);
                    if buf.len() >= cfg.flush_rows && inflight.is_none() {
                        inflight = Some(spawn_insert(&client, &mut buf, &metrics, cfg.flush_rows));
                    } else if inflight.is_some() {
                        // insert가 오래 걸리는 동안에도 메모리는 상한 안에 둔다
                        trim_overflow(&mut buf, max_buffer);
                    }
                }
                None => break,
            },
            done = async { inflight.as_mut().expect("is_some으로 확인").await },
                if inflight.is_some() =>
            {
                inflight = None;
                requeue(&mut buf, done, max_buffer);
                // 그동안 쌓인 분이 이미 한도를 넘었으면 바로 다음 insert를 시작한다
                if buf.len() >= cfg.flush_rows {
                    inflight = Some(spawn_insert(&client, &mut buf, &metrics, cfg.flush_rows));
                }
            }
            _ = tick.tick() => {
                if inflight.is_none() && !buf.is_empty() {
                    inflight = Some(spawn_insert(&client, &mut buf, &metrics, cfg.flush_rows));
                }
                if let Err(e) = archive.tick() {
                    metrics.archive_errors.fetch_add(1, Relaxed);
                    tracing::error!(error = %e, "아카이브 회전 실패");
                }
            }
        }
    }

    // 종료: 진행 중인 insert를 기다린 뒤 남은 것 모두 비우기
    if let Some(h) = inflight.take() {
        let done = h.await;
        requeue(&mut buf, done, max_buffer);
    }
    if !buf.is_empty() {
        let rows = std::mem::take(&mut buf);
        if let Some(failed) = insert_with_retry(&client, rows, &metrics).await {
            tracing::error!(
                rows = failed.len(),
                "종료 시 insert 실패: 아카이브에서 재적재 필요"
            );
        }
    }
    archive.close()?;
    Ok(())
}

/// 버퍼를 통째로 넘겨(`mem::take`) 별도 작업에서 insert한다.
fn spawn_insert(
    client: &analytics_ch::Client,
    buf: &mut Vec<Event>,
    metrics: &Arc<Metrics>,
    flush_rows: usize,
) -> InFlight {
    let rows = std::mem::replace(buf, Vec::with_capacity(flush_rows));
    let client = client.clone();
    let metrics = metrics.clone();
    tokio::spawn(async move { insert_with_retry(&client, rows, &metrics).await })
}

/// 실패한 insert의 행을 버퍼 앞(더 오래된 쪽)에 되돌리고 상한을 적용한다.
fn requeue(
    buf: &mut Vec<Event>,
    done: Result<Option<Vec<Event>>, tokio::task::JoinError>,
    max_buffer: usize,
) {
    match done {
        Ok(None) => {}
        Ok(Some(mut failed)) => {
            failed.append(buf);
            *buf = failed;
            trim_overflow(buf, max_buffer);
        }
        Err(e) => tracing::error!(error = %e, "insert 작업 실패 (아카이브에서 재적재 가능)"),
    }
}

/// 재시도까지 해서 insert한다. 성공하면 `None`, 끝내 실패하면 행을 돌려준다.
async fn insert_with_retry(
    client: &analytics_ch::Client,
    rows: Vec<Event>,
    metrics: &Metrics,
) -> Option<Vec<Event>> {
    for (attempt, backoff) in std::iter::once(Duration::ZERO)
        .chain(RETRY_BACKOFF)
        .enumerate()
    {
        if !backoff.is_zero() {
            tokio::time::sleep(backoff).await;
        }
        match analytics_ch::rows::insert_events(client, &rows).await {
            Ok(()) => {
                metrics.rows_inserted.fetch_add(rows.len() as u64, Relaxed);
                return None;
            }
            Err(e) => {
                metrics.insert_errors.fetch_add(1, Relaxed);
                tracing::warn!(attempt, rows = rows.len(), error = %e, "ClickHouse insert 실패");
            }
        }
    }
    Some(rows)
}

/// 버퍼가 `max_buffer`를 넘으면 오래된 것부터 버린다. 버린 개수.
fn trim_overflow(buf: &mut Vec<Event>, max_buffer: usize) -> usize {
    if buf.len() <= max_buffer {
        return 0;
    }
    let overflow = buf.len() - max_buffer;
    buf.drain(..overflow);
    tracing::error!(
        overflow,
        "버퍼 상한 초과: 오래된 이벤트를 버림 (아카이브에서 재적재 가능)"
    );
    overflow
}

#[cfg(test)]
mod tests {
    use super::*;
    use analytics_testkit::TestDb;
    use std::path::PathBuf;

    fn event(project_id: u32, i: u128) -> Event {
        let raw = serde_json::json!({
            "project_id": project_id, "event_id": uuid::Uuid::from_u128(i + 1),
            "ts_ms": 1_795_167_912_000i64, "received_at_ms": 1_795_167_913_000i64,
            "event_name": "page_view",
            "visitor_id": uuid::Uuid::from_u128(77), "session_id": uuid::Uuid::from_u128(3),
            "is_session_start": false, "member_hash": "", "url": "https://a.com/", "host": "a.com",
            "path": "/", "title": "", "referrer": "", "referrer_host": "", "utm_source": "",
            "utm_medium": "", "utm_campaign": "", "utm_content": "", "utm_term": "", "click_id": "",
            "click_id_type": "", "channel": "direct", "country": "KR", "region": "", "city": "",
            "device_type": "desktop", "browser": "", "os": "", "screen_w": 0, "props": []
        });
        serde_json::from_value(raw).unwrap()
    }

    fn temp_archive() -> PathBuf {
        std::env::temp_dir().join(format!("ingest-pipeline-{}", uuid::Uuid::new_v4()))
    }

    fn cfg(dir: &std::path::Path, flush_rows: usize, interval_ms: u64) -> Arc<Config> {
        Arc::new(Config {
            archive_dir: dir.to_path_buf(),
            flush_rows,
            flush_interval: Duration::from_millis(interval_ms),
            ..Config::for_tests(b"", true)
        })
    }

    async fn wait_writer(writer: JoinHandle<anyhow::Result<()>>) {
        tokio::time::timeout(Duration::from_secs(30), writer)
            .await
            .expect("종료가 끝나지 않음")
            .unwrap()
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dead_clickhouse_does_not_block_intake_and_shutdown_completes() {
        // insert가 재시도(최대 4.2초 대기)로 실패하는 동안에도 수신 루프는 계속 받아 아카이브·버퍼에 쌓는다
        let dir = temp_archive();
        let metrics = Arc::new(Metrics::default());
        let dead = analytics_ch::Client::default().with_url("http://127.0.0.1:1");
        let (p, writer) = start(dead, cfg(&dir, 1, 50), metrics.clone())
            .await
            .unwrap();
        let n = CHANNEL_CAPACITY * 2;
        for i in 0..n {
            assert!(
                p.try_send(vec![event(1, i as u128)]).is_ok(),
                "{i}번째 배치 거부: 수신 루프가 insert 재시도에 막혔다"
            );
            if i % 256 == 255 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while metrics.insert_errors.load(Relaxed) == 0 {
            assert!(std::time::Instant::now() < deadline, "insert가 시도되지 않음");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(metrics.rows_inserted.load(Relaxed), 0);
        drop(p);
        wait_writer(writer).await;
        assert_eq!(metrics.archive_errors.load(Relaxed), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn events_are_inserted_and_shutdown_flushes_rest() {
        let db = TestDb::new().await;
        let dir = temp_archive();
        let metrics = Arc::new(Metrics::default());
        // flush_rows 100, 주기 1시간: 앞 300건은 행 수로, 나머지 50건은 종료 시 플러시로 들어간다
        let (p, writer) = start(db.ch.clone(), cfg(&dir, 100, 3_600_000), metrics.clone())
            .await
            .unwrap();
        for i in 0..350u128 {
            p.try_send(vec![event(9, i)]).unwrap();
        }
        drop(p);
        wait_writer(writer).await;
        let n: u64 = db
            .ch
            .query("SELECT count() FROM events WHERE project_id = 9")
            .fetch_one()
            .await
            .unwrap();
        assert_eq!(n, 350);
        assert_eq!(metrics.rows_inserted.load(Relaxed), 350);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn overflow_drops_oldest_beyond_max_buffer() {
        let mut buf: Vec<Event> = (0..15).map(|i| event(1, i)).collect();
        assert_eq!(trim_overflow(&mut buf, 10), 5);
        assert_eq!(buf.len(), 10);
        assert_eq!(buf[0].event_id, uuid::Uuid::from_u128(6), "오래된 것부터 버린다");
        assert_eq!(trim_overflow(&mut buf, 10), 0);
    }

    #[test]
    fn failed_rows_go_back_in_front_and_are_capped() {
        let mut buf: Vec<Event> = (10..14).map(|i| event(1, i)).collect();
        let failed: Vec<Event> = (0..8).map(|i| event(1, i)).collect();
        requeue(&mut buf, Ok(Some(failed)), 10);
        let ids: Vec<u128> = buf.iter().map(|e| e.event_id.as_u128() - 1).collect();
        assert_eq!(ids, vec![2, 3, 4, 5, 6, 7, 10, 11, 12, 13]);
        requeue(&mut buf, Ok(None), 10);
        assert_eq!(buf.len(), 10);
    }
}
