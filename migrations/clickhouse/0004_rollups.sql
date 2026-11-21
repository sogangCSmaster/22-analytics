-- 0004: daily_stats를 용도별 좁은 롤업 3개로 바꾼다.
--
-- daily_stats는 키(project, day, channel, host, path, device, country)가 넓어 원본 events와
-- 거의 같은 크기였다. 리포트가 실제로 쓰는 축만 남긴다.
--   daily_channel: 개요·추이·채널 (project, day, channel)
--   daily_pages:   페이지 (project, day, host, path)
--   daily_events:  이벤트 이름 (project, day, event_name)
-- 순 방문자·세션은 uniqCombined(14) 상태로 저장한다 (읽을 때 uniqCombinedMerge(14)).
--
-- 순서: 표 생성 → 기존 events 백필 → MV 생성. 백필은 표가 비어 있을 때만 넣으므로 중간에 실패해
-- 다시 실행해도 두 번 쌓이지 않는다. 백필과 MV 생성 사이에 들어온 이벤트는 빠지므로 마이그레이션은
-- 수집(ingest)을 멈춘 상태에서 실행한다.
-- 문장 구분자는 줄 끝의 세미콜론. 문장 안에는 세미콜론을 쓰지 않는다.

CREATE TABLE IF NOT EXISTS daily_channel
(
    project_id UInt32,
    day        Date,
    channel    LowCardinality(String),
    pageviews  SimpleAggregateFunction(sum, UInt64),
    events     SimpleAggregateFunction(sum, UInt64),
    entrances  SimpleAggregateFunction(sum, UInt64),
    visitors   AggregateFunction(uniqCombined(14), UUID),
    sessions   AggregateFunction(uniqCombined(14), UUID)
)
ENGINE = AggregatingMergeTree
PARTITION BY toYYYYMM(day)
ORDER BY (project_id, day, channel);

-- 페이지뷰와 세션 진입(페이지뷰가 아닌 이벤트로 시작한 세션 포함)만 담는다.
-- visitors는 페이지뷰를 낸 방문자만 센다.
CREATE TABLE IF NOT EXISTS daily_pages
(
    project_id UInt32,
    day        Date,
    host       LowCardinality(String),
    path       String CODEC(ZSTD(1)),
    pageviews  SimpleAggregateFunction(sum, UInt64),
    entrances  SimpleAggregateFunction(sum, UInt64),
    visitors   AggregateFunction(uniqCombined(14), UUID)
)
ENGINE = AggregatingMergeTree
PARTITION BY toYYYYMM(day)
ORDER BY (project_id, day, host, path);

CREATE TABLE IF NOT EXISTS daily_events
(
    project_id UInt32,
    day        Date,
    event_name LowCardinality(String),
    events     SimpleAggregateFunction(sum, UInt64),
    visitors   AggregateFunction(uniqCombined(14), UUID)
)
ENGINE = AggregatingMergeTree
PARTITION BY toYYYYMM(day)
ORDER BY (project_id, day, event_name);

-- 백필 (1차는 Asia/Seoul 기준 일자, 0001의 daily_stats와 같은 기준)
INSERT INTO daily_channel
SELECT project_id,
       toDate(ts, 'Asia/Seoul') AS day,
       channel,
       countIf(event_name = 'page_view') AS pageviews,
       count() AS events,
       countIf(is_session_start = 1) AS entrances,
       uniqCombinedState(14)(visitor_id) AS visitors,
       uniqCombinedState(14)(session_id) AS sessions
FROM events
WHERE (SELECT count() FROM daily_channel) = 0
GROUP BY project_id, day, channel;

INSERT INTO daily_pages
SELECT project_id,
       toDate(ts, 'Asia/Seoul') AS day,
       host, path,
       countIf(event_name = 'page_view') AS pageviews,
       countIf(is_session_start = 1) AS entrances,
       uniqCombinedStateIf(14)(visitor_id, event_name = 'page_view') AS visitors
FROM events
WHERE (event_name = 'page_view' OR is_session_start = 1)
  AND (SELECT count() FROM daily_pages) = 0
GROUP BY project_id, day, host, path;

INSERT INTO daily_events
SELECT project_id,
       toDate(ts, 'Asia/Seoul') AS day,
       event_name,
       count() AS events,
       uniqCombinedState(14)(visitor_id) AS visitors
FROM events
WHERE (SELECT count() FROM daily_events) = 0
GROUP BY project_id, day, event_name;

CREATE MATERIALIZED VIEW IF NOT EXISTS mv_daily_channel TO daily_channel AS
SELECT project_id,
       toDate(ts, 'Asia/Seoul') AS day,
       channel,
       countIf(event_name = 'page_view') AS pageviews,
       count() AS events,
       countIf(is_session_start = 1) AS entrances,
       uniqCombinedState(14)(visitor_id) AS visitors,
       uniqCombinedState(14)(session_id) AS sessions
FROM events
GROUP BY project_id, day, channel;

CREATE MATERIALIZED VIEW IF NOT EXISTS mv_daily_pages TO daily_pages AS
SELECT project_id,
       toDate(ts, 'Asia/Seoul') AS day,
       host, path,
       countIf(event_name = 'page_view') AS pageviews,
       countIf(is_session_start = 1) AS entrances,
       uniqCombinedStateIf(14)(visitor_id, event_name = 'page_view') AS visitors
FROM events
WHERE event_name = 'page_view' OR is_session_start = 1
GROUP BY project_id, day, host, path;

CREATE MATERIALIZED VIEW IF NOT EXISTS mv_daily_events TO daily_events AS
SELECT project_id,
       toDate(ts, 'Asia/Seoul') AS day,
       event_name,
       count() AS events,
       uniqCombinedState(14)(visitor_id) AS visitors
FROM events
GROUP BY project_id, day, event_name;

DROP VIEW IF EXISTS mv_daily_stats;

DROP TABLE IF EXISTS daily_stats;

-- 원본 events 압축 (메타데이터만 바뀌고 기존 파트는 병합 때 새 코덱으로 다시 쓰인다)
ALTER TABLE events MODIFY COLUMN ts CODEC(DoubleDelta, ZSTD(1));

ALTER TABLE events MODIFY COLUMN received_at CODEC(DoubleDelta, ZSTD(1));

ALTER TABLE events MODIFY COLUMN url CODEC(ZSTD(1));

ALTER TABLE events MODIFY COLUMN path CODEC(ZSTD(1));

ALTER TABLE events MODIFY COLUMN title CODEC(ZSTD(1));

ALTER TABLE events MODIFY COLUMN referrer CODEC(ZSTD(1));
