//! 광고비 CSV 업로드·조회.
//!
//! - `POST /v1/projects/{project_id}/ad-spend` (analyst 이상, `Content-Type: text/csv`, 최대 5MB)
//!   형식: `day,channel,campaign,amount` (머리글 행은 선택, 비어 있지 않은 첫 줄). 같은
//!   (day, channel, campaign)은 덮어쓴다(upsert). 한 행이라도 잘못되면 아무것도 저장하지 않는다.
//!   amount는 0..=[`MAX_AMOUNT`] 정수, channel은 소문자로 저장한다 (채널 규칙 이름과 같게).
//! - `GET /v1/projects/{project_id}/ad-spend?from=YYYY-MM-DD&to=YYYY-MM-DD` (멤버, 최대 366일)

use super::audit;
use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiQuery, ProjectAccess};
use crate::validate;
use analytics_core::dto::admin::{AdSpendResponse, AdSpendRow, AdSpendUploadResponse, Role};
use analytics_pg::ad_spend::Spend;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::extract::rejection::BytesRejection;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use time::Date;
use time::macros::format_description;

pub const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_ROWS: usize = 200_000;
pub const MAX_RANGE_DAYS: i64 = 366;
const MAX_CAMPAIGN_CHARS: usize = 200;
/// 행당 광고비 상한 (통화 최소단위). 원화로 1천조: 현실적인 값은 모두 들어가고,
/// 행을 많이 합쳐도 집계가 BIGINT 근처에 가기 어렵다 (합계는 그래도 포화 처리한다).
pub const MAX_AMOUNT: i64 = 1_000_000_000_000_000;

pub fn parse_day(s: &str) -> Option<Date> {
    Date::parse(s.trim(), format_description!("[year]-[month]-[day]")).ok()
}

pub fn fmt_day(d: Date) -> String {
    d.format(format_description!("[year]-[month]-[day]"))
        .unwrap_or_default()
}

/// CSV 파싱·검증. 같은 키가 여러 번 나오면 마지막 값을 쓴다.
pub fn parse_csv(body: &[u8]) -> ApiResult<Vec<Spend>> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(body);
    let mut rows: BTreeMap<(Date, String, String), i64> = BTreeMap::new();
    let mut seen = 0usize;
    // 머리글은 비어 있지 않은 첫 줄에서만 알아본다 (앞에 빈 줄·공백 줄이 있어도).
    let mut first = true;
    for (i, rec) in rdr.records().enumerate() {
        let line = i + 1;
        let err = |msg: &str| ApiError::invalid(format!("{line}행: {msg}"));
        let rec = rec.map_err(|_| err("CSV 형식 오류 (UTF-8, 따옴표 확인)"))?;
        if rec.len() == 1 && rec.get(0).is_some_and(str::is_empty) {
            continue; // 빈 줄
        }
        if std::mem::take(&mut first) && rec.get(0).is_some_and(|c| c.eq_ignore_ascii_case("day")) {
            continue; // 머리글
        }
        if rec.len() != 4 {
            return Err(err("열은 day,channel,campaign,amount 4개여야 합니다"));
        }
        let day = parse_day(&rec[0]).ok_or_else(|| err("day는 YYYY-MM-DD 형식의 실제 날짜"))?;
        let channel = validate::name("channel", &rec[1], validate::MAX_CHANNEL_CHARS)
            .map_err(|_| err("channel은 1~100자"))?
            .to_lowercase();
        let campaign = validate::optional_name("campaign", &rec[2], MAX_CAMPAIGN_CHARS)
            .map_err(|_| err("campaign은 최대 200자"))?;
        let raw = &rec[3];
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(err("amount는 0 이상의 정수 (통화 최소단위)"));
        }
        let amount: i64 = raw
            .parse()
            .ok()
            .filter(|a| *a <= MAX_AMOUNT)
            .ok_or_else(|| err("amount가 너무 큽니다 (최대 1,000,000,000,000,000)"))?;
        seen += 1;
        if seen > MAX_ROWS {
            return Err(ApiError::invalid(format!("최대 {MAX_ROWS}행")));
        }
        rows.insert((day, channel, campaign), amount);
    }
    if rows.is_empty() {
        return Err(ApiError::invalid("데이터 행이 없습니다"));
    }
    Ok(rows
        .into_iter()
        .map(|((day, channel, campaign), amount)| Spend {
            day,
            channel,
            campaign,
            amount,
        })
        .collect())
}

pub async fn upload(
    State(state): State<AppState>,
    access: ProjectAccess,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Json<AdSpendUploadResponse>> {
    access.require(Role::Analyst)?;
    let body = body.map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::payload_too_large()
        } else {
            ApiError::invalid("본문을 읽을 수 없습니다")
        }
    })?;
    let rows = parse_csv(&body)?;
    analytics_pg::ad_spend::upsert(&state.pg, access.id(), &rows).await?;
    let (from, to) = (
        rows.iter().map(|r| r.day).min(),
        rows.iter().map(|r| r.day).max(),
    );
    audit(
        &state.pg,
        Some(access.project.org_id),
        access.user.id(),
        "ad_spend.upload",
        format!("project:{}", access.id()),
        json!({ "rows": rows.len(), "from": from.map(fmt_day), "to": to.map(fmt_day) }),
    )
    .await?;
    Ok(Json(AdSpendUploadResponse { rows: rows.len() }))
}

#[derive(Debug, Deserialize)]
pub struct RangeQuery {
    pub from: Option<String>,
    pub to: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiQuery(q): ApiQuery<RangeQuery>,
) -> ApiResult<Json<AdSpendResponse>> {
    let day = |v: &Option<String>, field: &str| {
        v.as_deref()
            .and_then(parse_day)
            .ok_or_else(|| ApiError::invalid(format!("{field}: YYYY-MM-DD 날짜가 필요합니다")))
    };
    let from = day(&q.from, "from")?;
    let to = day(&q.to, "to")?;
    if from > to {
        return Err(ApiError::invalid("from은 to보다 늦을 수 없습니다"));
    }
    if (to - from).whole_days() > MAX_RANGE_DAYS {
        return Err(ApiError::invalid(format!("기간은 최대 {MAX_RANGE_DAYS}일")));
    }
    let rows = analytics_pg::ad_spend::list(&state.pg, access.id(), from, to).await?;
    Ok(Json(AdSpendResponse {
        from: fmt_day(from),
        to: fmt_day(to),
        rows: rows
            .into_iter()
            .map(|r| AdSpendRow {
                day: fmt_day(r.day),
                channel: r.channel,
                campaign: r.campaign,
                amount: r.amount,
            })
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_fields_and_header() {
        let rows = parse_csv(
            "Day,Channel,Campaign,Amount\n2026-01-02,Meta,\"a, b\",10\n\n2026-01-01,naver,,0\n"
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].channel, "naver");
        assert_eq!(rows[1].campaign, "a, b");
        assert_eq!(rows[1].channel, "meta");
    }

    #[test]
    fn rejects_bad_rows_with_line_number() {
        let e = parse_csv(b"2026-01-01,meta,,1\n2026-01-01,meta,,-1\n").unwrap_err();
        assert!(e.message.starts_with("2행"), "{}", e.message);
        assert!(parse_csv(b"2026-02-29,meta,,1\n").is_err(), "윤년 아님");
        assert!(parse_csv(b"2028-02-29,meta,,1\n").is_ok());
        assert!(parse_csv(&[0xff, 0xfe, b'\n']).is_err());
    }
}
