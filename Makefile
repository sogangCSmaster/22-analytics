# 로컬 개발 명령. 배포 관련 명령은 없다 (배포는 대행사 계정·도메인 수령 후).
COMPOSE := docker compose -f infra/compose/docker-compose.yml
export DATABASE_URL ?= postgres://analytics:analytics@localhost:5432/analytics
export CLICKHOUSE_URL ?= http://localhost:8123

.PHONY: up down reset migrate seed tracker ingest api web test lint fmt check

up:        ## DB 기동 (ClickHouse, Postgres)
	$(COMPOSE) up -d --wait

down:      ## DB 중지
	$(COMPOSE) down

reset:     ## DB 볼륨까지 삭제
	$(COMPOSE) down -v

migrate:   ## 스키마 적용
	cargo run -q -p analytics-jobs -- migrate

seed:      ## 개발용 광고주·프로젝트 생성
	cargo run -q -p analytics-jobs -- seed-dev

tracker:   ## t.js 빌드 + 크기 검사
	cd tracker && npm install --silent && npm run -s check

ingest: tracker  ## 수집 서버 (엣지 에뮬레이션) → http://localhost:8081/dev/
	EDGE_EMULATION=true cargo run -p analytics-ingest

api:       ## API 서버 → http://localhost:8080
	cargo run -p analytics-api

web:       ## 대시보드 개발 서버
	dx serve --platform web --package analytics-web

test:      ## 전체 테스트 (DB 필요: make up migrate)
	cargo test --workspace --exclude analytics-web --exclude analytics-edge
	cargo test -p analytics-edge
	cargo test -p analytics-web --no-default-features
	cd tracker && npm run -s check

lint:
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -D warnings
	cargo clippy -p analytics-web --target wasm32-unknown-unknown -- -D warnings
	cargo clippy -p analytics-edge --target wasm32-unknown-unknown -- -D warnings

fmt:
	cargo fmt --all

check: lint test
