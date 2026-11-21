-- WP3: 인증·관리 API 보조 인덱스

-- 이메일은 대소문자 구분 없이 유일 (API는 소문자로 저장하지만 DB에서도 강제)
CREATE UNIQUE INDEX users_email_lower_idx ON users (lower(email));

-- 광고주별 멤버 목록
CREATE INDEX memberships_org_idx ON memberships (org_id);

-- 만료 세션 정리
CREATE INDEX sessions_expires_idx ON sessions (expires_at);

-- 광고주별 감사 로그 조회
CREATE INDEX audit_log_org_at_idx ON audit_log (org_id, at DESC);
