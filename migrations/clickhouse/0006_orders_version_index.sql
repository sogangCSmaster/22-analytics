-- 0006: 기여 연산 증분 조회(changed_orders: `version > watermark`)용 스킵 인덱스.
--
-- version은 쓴 시각(unix ms)이라 파트·그래뉼마다 범위가 좁다: minmax 인덱스로 watermark 이전에
-- 쓰인 그래뉼을 건너뛴다 (정렬 키 (project_id, order_id)로는 version 조건을 거를 수 없다).
--
-- 메타데이터만 바꾼다. MATERIALIZE INDEX는 하지 않는다(MVP): 새 파트와 이후 머지부터 인덱스가 생기고,
-- 인덱스가 없는 기존 파트는 지금처럼 전부 읽는다. 운영에서 바로 효과를 보려면 한가할 때
-- `ALTER TABLE orders MATERIALIZE INDEX idx_version`(파트 재작성 mutation)을 따로 실행한다.
-- IF NOT EXISTS: 다시 실행해도 안전하다.
-- 문장 구분자는 줄 끝의 세미콜론. 문장 안에는 세미콜론을 쓰지 않는다.

ALTER TABLE orders ADD INDEX IF NOT EXISTS idx_version version TYPE minmax GRANULARITY 1;
