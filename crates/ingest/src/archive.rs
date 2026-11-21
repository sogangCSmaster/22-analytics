//! 원본 세그먼트 아카이브.
//!
//! `{dir}/dt=YYYY-MM-DD/hour=HH/{시작ms}-{uuid}.ndjson`에 append하고, 회전 주기마다 닫아서
//! zstd로 압축한다(`.ndjson.zst`). 운영에서는 압축 파일을 S3로 올린다 (Phase 1, 업로더 추가 예정).

use analytics_core::Event;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use time::OffsetDateTime;

const MAX_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;

struct Segment {
    path: PathBuf,
    out: BufWriter<File>,
    opened: Instant,
    bytes: u64,
}

pub struct SegmentWriter {
    dir: PathBuf,
    rotate: Duration,
    current: Option<Segment>,
}

impl SegmentWriter {
    pub fn open(dir: PathBuf, rotate: Duration) -> anyhow::Result<Self> {
        fs::create_dir_all(&dir)?;
        // 비정상 종료로 남은 미압축 세그먼트를 정리한다.
        for path in leftover_segments(&dir) {
            tracing::info!(path = %path.display(), "남은 세그먼트 압축");
            compress(&path)?;
        }
        Ok(Self {
            dir,
            rotate,
            current: None,
        })
    }

    pub fn append(&mut self, events: &[Event]) -> anyhow::Result<()> {
        if self.current.is_none() {
            self.current = Some(self.new_segment()?);
        }
        let seg = self.current.as_mut().expect("방금 열었다");
        for e in events {
            let line = serde_json::to_vec(e)?;
            seg.out.write_all(&line)?;
            seg.out.write_all(b"\n")?;
            seg.bytes += line.len() as u64 + 1;
        }
        if seg.bytes >= MAX_SEGMENT_BYTES {
            self.rotate_now()?;
        }
        Ok(())
    }

    /// 주기적으로 호출: 버퍼를 디스크에 내리고, 회전 시간이 지났으면 회전한다.
    pub fn tick(&mut self) -> anyhow::Result<()> {
        let due = match self.current.as_mut() {
            Some(seg) => {
                seg.out.flush()?;
                seg.opened.elapsed() >= self.rotate
            }
            None => false,
        };
        if due {
            self.rotate_now()?;
        }
        Ok(())
    }

    pub fn close(&mut self) -> anyhow::Result<()> {
        self.rotate_now()
    }

    fn rotate_now(&mut self) -> anyhow::Result<()> {
        if let Some(mut seg) = self.current.take() {
            seg.out.flush()?;
            drop(seg.out);
            let path = seg.path;
            // 압축은 블로킹 작업이라 별도 스레드에서 한다.
            std::thread::spawn(move || {
                if let Err(e) = compress(&path) {
                    tracing::error!(path = %path.display(), error = %e, "세그먼트 압축 실패");
                }
            });
        }
        Ok(())
    }

    fn new_segment(&self) -> anyhow::Result<Segment> {
        let now = OffsetDateTime::now_utc();
        let dir = self.dir.join(format!(
            "dt={:04}-{:02}-{:02}/hour={:02}",
            now.year(),
            now.month() as u8,
            now.day(),
            now.hour()
        ));
        fs::create_dir_all(&dir)?;
        let name = format!(
            "{}-{}.ndjson",
            now.unix_timestamp_nanos() / 1_000_000,
            uuid::Uuid::new_v4()
        );
        let path = dir.join(name);
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)?;
        Ok(Segment {
            path,
            out: BufWriter::with_capacity(256 * 1024, file),
            opened: Instant::now(),
            bytes: 0,
        })
    }
}

fn compress(path: &Path) -> anyhow::Result<()> {
    let mut target = path.as_os_str().to_owned();
    target.push(".zst");
    let input = File::open(path)?;
    let output = File::create(&target)?;
    zstd::stream::copy_encode(input, output, 9)?;
    fs::remove_file(path)?;
    Ok(())
}

fn leftover_segments(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "ndjson") {
                out.push(p);
            }
        }
    }
    out
}
