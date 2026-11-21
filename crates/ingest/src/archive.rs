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

/// 쓴 바이트 수를 세는 래퍼 (이벤트마다 중간 `Vec`을 만들지 않고 바로 버퍼에 직렬화한다).
struct Counting<W> {
    inner: W,
    bytes: u64,
}

impl<W: Write> Write for Counting<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

struct Segment {
    path: PathBuf,
    out: Counting<BufWriter<File>>,
    opened: Instant,
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
            serde_json::to_writer(&mut seg.out, e)?;
            seg.out.write_all(b"\n")?;
        }
        if seg.out.bytes >= MAX_SEGMENT_BYTES {
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
            analytics_core::timeutil::dt_to_ms(now),
            uuid::Uuid::new_v4()
        );
        let path = dir.join(name);
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)?;
        Ok(Segment {
            path,
            out: Counting {
                inner: BufWriter::with_capacity(256 * 1024, file),
                bytes: 0,
            },
            opened: Instant::now(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_uses_fast_zstd_level() {
        // 레벨 9는 수집 서버 CPU를 많이 쓴다. 3이면 압축률 차이는 작고 몇 배 빠르다.
        assert_eq!(ZSTD_LEVEL, 3);
        let dir = std::env::temp_dir().join(format!("archive-zstd-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("seg.ndjson");
        fs::write(&path, b"{\"a\":1}\n{\"a\":2}\n").unwrap();
        compress(&path).unwrap();
        assert!(!path.exists());
        let out = zstd::decode_all(File::open(dir.join("seg.ndjson.zst")).unwrap()).unwrap();
        assert_eq!(out, b"{\"a\":1}\n{\"a\":2}\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn counting_writer_counts_bytes() {
        let mut w = Counting {
            inner: Vec::new(),
            bytes: 0,
        };
        serde_json::to_writer(&mut w, &serde_json::json!({"a": "한글"})).unwrap();
        w.write_all(b"\n").unwrap();
        assert_eq!(w.bytes, w.inner.len() as u64);
        assert_eq!(w.inner, "{\"a\":\"한글\"}\n".as_bytes());
    }
}
