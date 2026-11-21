-- 22-analytics ClickHouse 초기 스키마 (docs/ARCHITECTURE.md §6.1)
-- 문장 구분자는 줄 끝의 세미콜론. 문장 안에는 세미콜론을 쓰지 않는다.

CREATE TABLE IF NOT EXISTS events
(
    project_id       UInt32,
    event_id         UUID,
    ts               DateTime64(3, 'UTC'),
    received_at      DateTime64(3, 'UTC'),
    event_name       LowCardinality(String),
    visitor_id       UUID,
    session_id       UUID,
    is_session_start UInt8,
    member_hash      String,
    url              String,
    host             LowCardinality(String),
    path             String,
    title            String,
    referrer         String,
    referrer_host    LowCardinality(String),
    utm_source       String,
    utm_medium       String,
    utm_campaign     String,
    utm_content      String,
    utm_term         String,
    click_id         String,
    click_id_type    LowCardinality(String),
    channel          LowCardinality(String),
    country          LowCardinality(String),
    region           LowCardinality(String),
    city             LowCardinality(String),
    device_type      LowCardinality(String),
    browser          LowCardinality(String),
    os               LowCardinality(String),
    screen_w         UInt16,
    `props.key`      Array(LowCardinality(String)),
    `props.value`    Array(String),
    ec_order_id      String,
    ec_value         Int64,
    ec_currency      LowCardinality(String),
    `items.id`       Array(String),
    `items.name`     Array(String),
    `items.price`    Array(Int64),
    `items.qty`      Array(UInt32)
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (project_id, toDate(ts, 'Asia/Seoul'), event_name, visitor_id, ts)
TTL toDateTime(ts) + INTERVAL 25 MONTH DELETE;

CREATE TABLE IF NOT EXISTS touchpoints
(
    project_id    UInt32,
    visitor_id    UUID,
    member_hash   String,
    ts            DateTime64(3, 'UTC'),
    session_id    UUID,
    utm_source    String,
    utm_medium    String,
    utm_campaign  String,
    utm_content   String,
    utm_term      String,
    click_id      String,
    click_id_type LowCardinality(String),
    referrer_host LowCardinality(String),
    page_host     LowCardinality(String),
    landing_url   String,
    channel       LowCardinality(String)
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (project_id, visitor_id, ts);

-- 세션을 시작시킨 유입 중 direct와 내부 이동을 뺀 것만 터치포인트로 남긴다.
CREATE MATERIALIZED VIEW IF NOT EXISTS mv_touchpoints TO touchpoints AS
SELECT project_id, visitor_id, member_hash, ts, session_id,
       utm_source, utm_medium, utm_campaign, utm_content, utm_term,
       click_id, click_id_type, referrer_host, host AS page_host, url AS landing_url, channel
FROM events
WHERE is_session_start = 1 AND channel NOT IN ('direct', '(internal)');

CREATE TABLE IF NOT EXISTS order_events
(
    project_id      UInt32,
    order_id        String,
    event_type      LowCardinality(String),
    source          LowCardinality(String),
    idempotency_key String,
    visitor_id      Nullable(UUID),
    member_hash     String,
    currency        LowCardinality(String),
    gross_amount    Int64,
    net_amount      Int64,
    refund_amount   Int64,
    occurred_at     DateTime64(3, 'UTC'),
    received_at     DateTime64(3, 'UTC'),
    raw             String
)
ENGINE = ReplacingMergeTree(received_at)
ORDER BY (project_id, order_id, idempotency_key);

CREATE TABLE IF NOT EXISTS orders
(
    project_id       UInt32,
    order_id         String,
    source           LowCardinality(String),
    status           LowCardinality(String),
    visitor_id       Nullable(UUID),
    member_hash      String,
    currency         LowCardinality(String),
    gross_amount     Int64,
    net_amount       Int64,
    refunded_amount  Int64,
    effective_amount Int64,
    ordered_at       DateTime64(3, 'UTC'),
    version          UInt64
)
ENGINE = ReplacingMergeTree(version)
PARTITION BY toYYYYMM(ordered_at)
ORDER BY (project_id, order_id);

CREATE TABLE IF NOT EXISTS identity_links
(
    project_id  UInt32,
    member_hash String,
    visitor_id  UUID,
    first_seen  SimpleAggregateFunction(min, DateTime64(3, 'UTC')),
    last_seen   SimpleAggregateFunction(max, DateTime64(3, 'UTC'))
)
ENGINE = AggregatingMergeTree
ORDER BY (project_id, member_hash, visitor_id);

CREATE MATERIALIZED VIEW IF NOT EXISTS mv_identity_links TO identity_links AS
SELECT project_id, member_hash, visitor_id, min(ts) AS first_seen, max(ts) AS last_seen
FROM events
WHERE member_hash != ''
GROUP BY project_id, member_hash, visitor_id;

CREATE TABLE IF NOT EXISTS attributed_revenue
(
    project_id    UInt32,
    model         LowCardinality(String),
    lookback_days UInt8,
    order_id      String,
    touch_index   UInt16,
    channel       LowCardinality(String),
    utm_campaign  String,
    touch_ts      Nullable(DateTime64(3, 'UTC')),
    ordered_at    DateTime64(3, 'UTC'),
    amount        Int64,
    is_assist     UInt8,
    computed_at   DateTime64(3, 'UTC'),
    is_deleted    UInt8
)
ENGINE = ReplacingMergeTree(computed_at, is_deleted)
PARTITION BY toYYYYMM(ordered_at)
ORDER BY (project_id, model, lookback_days, order_id, touch_index);

CREATE TABLE IF NOT EXISTS daily_stats
(
    project_id  UInt32,
    day         Date,
    channel     LowCardinality(String),
    host        LowCardinality(String),
    path        String,
    device_type LowCardinality(String),
    country     LowCardinality(String),
    pageviews   SimpleAggregateFunction(sum, UInt64),
    events      SimpleAggregateFunction(sum, UInt64),
    entrances   SimpleAggregateFunction(sum, UInt64),
    visitors    AggregateFunction(uniq, UUID),
    sessions    AggregateFunction(uniq, UUID)
)
ENGINE = AggregatingMergeTree
PARTITION BY toYYYYMM(day)
ORDER BY (project_id, day, channel, host, path, device_type, country);

-- 1차는 Asia/Seoul 기준 일자. 다른 타임존 프로젝트는 Phase 3에서 dictionary로 처리한다.
CREATE MATERIALIZED VIEW IF NOT EXISTS mv_daily_stats TO daily_stats AS
SELECT project_id,
       toDate(ts, 'Asia/Seoul') AS day,
       channel, host, path, device_type, country,
       countIf(event_name = 'page_view') AS pageviews,
       count() AS events,
       countIf(is_session_start = 1) AS entrances,
       uniqState(visitor_id) AS visitors,
       uniqState(session_id) AS sessions
FROM events
GROUP BY project_id, day, channel, host, path, device_type, country;
