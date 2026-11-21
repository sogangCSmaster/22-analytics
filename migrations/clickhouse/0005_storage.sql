-- 0005: 저장 비용 줄이기 (코덱, 컬럼 TTL).
--
-- 모두 메타데이터만 바꾸는 ALTER다. 기존 파트는 다시 쓰지 않는다.
--   - 코덱 변경: 새 파트(INSERT)와 이후 머지부터 적용된다.
--   - 컬럼 TTL: materialize_ttl_after_modify = 0으로 MATERIALIZE TTL mutation을 막는다.
--     만료 값은 이후 머지 때 컬럼 기본값으로 바뀐다 (행은 남는다).
-- 같은 정의로 다시 실행해도 아무것도 바뀌지 않는다 (재실행 안전).
-- 문장 구분자는 줄 끝의 세미콜론. 문장 안에는 세미콜론을 쓰지 않는다.

-- 시각 열: 단조 증가에 가까운 ms 값은 Delta + ZSTD가 가장 작다 (DoubleDelta보다 디코딩이 빠르고
-- 수집 순서가 섞여도 크기 차이가 작다). ts는 정렬·파티션 키지만 코덱 변경은 허용된다.
ALTER TABLE events MODIFY COLUMN ts DateTime64(3, 'UTC') CODEC(Delta, ZSTD(1));
ALTER TABLE events MODIFY COLUMN received_at DateTime64(3, 'UTC') CODEC(Delta, ZSTD(1));

-- 긴 문자열: 압축률을 높인다 (읽기 빈도가 낮은 열).
ALTER TABLE events MODIFY COLUMN title String CODEC(ZSTD(3));
ALTER TABLE events MODIFY COLUMN referrer String CODEC(ZSTD(3));

-- event_id는 수집 재전송 중복 확인에만 쓴다. 7일이 지나면 비운다 (행 TTL 25개월은 그대로).
ALTER TABLE events MODIFY COLUMN event_id UUID TTL toDateTime(ts) + INTERVAL 7 DAY
    SETTINGS materialize_ttl_after_modify = 0;

-- 주문 원본 JSON은 디버깅용이다 (투영·기여 연산은 읽지 않는다). 90일 뒤 비운다.
ALTER TABLE order_events MODIFY COLUMN raw String TTL toDateTime(received_at) + INTERVAL 90 DAY
    SETTINGS materialize_ttl_after_modify = 0;
