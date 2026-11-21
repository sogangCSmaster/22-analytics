-- 22-analytics 메타데이터 스키마 (docs/ARCHITECTURE.md §6.3)

CREATE TABLE users (
    id              BIGSERIAL PRIMARY KEY,
    email           TEXT NOT NULL UNIQUE,
    password_hash   TEXT NOT NULL,
    name            TEXT NOT NULL DEFAULT '',
    is_super_admin  BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at   TIMESTAMPTZ
);

-- 광고주
CREATE TABLE organizations (
    id          BIGSERIAL PRIMARY KEY,
    name        TEXT NOT NULL,
    memo        TEXT NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE memberships (
    user_id  BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    org_id   BIGINT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    role     TEXT NOT NULL CHECK (role IN ('admin', 'analyst', 'viewer')),
    PRIMARY KEY (user_id, org_id)
);

-- id는 ClickHouse project_id(UInt32)와 같은 값
CREATE TABLE projects (
    id                SERIAL PRIMARY KEY,
    org_id            BIGINT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name              TEXT NOT NULL,
    public_key        TEXT NOT NULL UNIQUE,
    timezone          TEXT NOT NULL DEFAULT 'Asia/Seoul',
    currency          TEXT NOT NULL DEFAULT 'KRW',
    salt              TEXT NOT NULL,
    status            TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'paused', 'deleted')),
    settings          JSONB NOT NULL DEFAULT '{}'::jsonb,
    settings_version  INTEGER NOT NULL DEFAULT 1,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX projects_org_idx ON projects(org_id);

CREATE TABLE project_origins (
    project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    origin      TEXT NOT NULL,
    PRIMARY KEY (project_id, origin)
);

CREATE TABLE api_keys (
    id            BIGSERIAL PRIMARY KEY,
    project_id    INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    prefix        TEXT NOT NULL UNIQUE,
    key_hash      TEXT NOT NULL,
    scopes        TEXT[] NOT NULL DEFAULT ARRAY['orders:write'],
    created_by    BIGINT REFERENCES users(id),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at  TIMESTAMPTZ,
    revoked_at    TIMESTAMPTZ
);

CREATE TABLE custom_hostnames (
    project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    hostname        TEXT NOT NULL UNIQUE,
    cf_hostname_id  TEXT,
    status          TEXT NOT NULL DEFAULT 'pending',
    verified_at     TIMESTAMPTZ,
    PRIMARY KEY (project_id, hostname)
);

-- project_id NULL = 전역 기본 규칙
CREATE TABLE channel_rules (
    id          BIGSERIAL PRIMARY KEY,
    project_id  INTEGER REFERENCES projects(id) ON DELETE CASCADE,
    priority    INTEGER NOT NULL DEFAULT 100,
    channel     TEXT NOT NULL,
    match       JSONB NOT NULL,
    label       TEXT NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX channel_rules_project_idx ON channel_rules(project_id, priority);

CREATE TABLE attribution_settings (
    project_id        INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    lookback_days     SMALLINT[] NOT NULL DEFAULT ARRAY[7, 30]::SMALLINT[],
    default_model     TEXT NOT NULL DEFAULT 'last_click',
    half_life_hours   INTEGER NOT NULL DEFAULT 168,
    include_organic   BOOLEAN NOT NULL DEFAULT TRUE,
    include_referral  BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE TABLE ad_spend (
    project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    day         DATE NOT NULL,
    channel     TEXT NOT NULL,
    campaign    TEXT NOT NULL DEFAULT '',
    amount      BIGINT NOT NULL,
    PRIMARY KEY (project_id, day, channel, campaign)
);

CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL
);
CREATE INDEX sessions_user_idx ON sessions(user_id);

CREATE TABLE audit_log (
    id       BIGSERIAL PRIMARY KEY,
    org_id   BIGINT,
    user_id  BIGINT,
    action   TEXT NOT NULL,
    target   TEXT NOT NULL DEFAULT '',
    payload  JSONB NOT NULL DEFAULT '{}'::jsonb,
    at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE job_watermarks (
    job         TEXT NOT NULL,
    project_id  INTEGER NOT NULL DEFAULT 0,
    watermark   BIGINT NOT NULL DEFAULT 0,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (job, project_id)
);
