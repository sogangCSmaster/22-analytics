//! 리포트 엔드포인트 (`GET /v1/projects/{project_id}/reports/...`).
//!
//! - 권한: 프로젝트 멤버면 역할과 무관하게 읽는다. 접근할 수 없는 프로젝트는 404.
//! - 기간: `from`·`to` (`YYYY-MM-DD`, Asia/Seoul, 양 끝 포함, 최대 400일). 둘 다 없으면
//!   오늘(Asia/Seoul)로 끝나는 최근 7일. 하나만 주면 400.
//! - 응답 본문은 `analytics_core::dto::reports`의 DTO 그대로다 (목록형은 JSON 배열).
//! - 캐시: 기간이 오늘 이전에 끝나면(닫힌 날) `private, max-age=60`, 오늘·미래를 포함하면
//!   `no-store`. 실시간·주문 상세·오류 응답은 전역 기본값 `no-store`.
//! - ClickHouse 오류는 503 `storage_unavailable` (원인은 로그에만).
//! - 사용자 입력(경로 필터, 이벤트 이름, 속성 키, 주문 번호)은 길이만 검사하고 그대로
//!   ClickHouse 바인딩으로 넘긴다 (SQL에 이어 붙이지 않는다).

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiQuery, ProjectAccess};
use analytics_ch::reports::{self as ch, DateRange, MAX_LIMIT};
use analytics_core::attribution::Model;
use analytics_core::event::MAX_PROP_KEY_LEN;
use analytics_core::orders::MAX_ORDER_ID_CHARS;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use time::Duration;

/// 기본 기간 일수 (오늘 포함).
pub const DEFAULT_RANGE_DAYS: i64 = 7;
/// 페이지 경로 필터 최대 글자 수.
pub const MAX_QUERY_CHARS: usize = 200;
/// 이벤트 이름 최대 글자 수 (수집 규칙과 같다).
pub const MAX_EVENT_NAME_CHARS: usize = 64;
/// 주문 목록 offset 상한 (깊은 페이지 스캔 방지).
pub const MAX_OFFSET: u64 = 100_000;
/// 닫힌 기간 리포트의 캐시 헤더.
pub const CACHE_CLOSED: &str = "private, max-age=60";

const DEFAULT_TOP_LIMIT: u32 = 50;
const DEFAULT_OVERLAP_LIMIT: u32 = 20;

/// 모든 리포트가 공유하는 쿼리 파라미터 (라우트에 없는 값은 무시한다).
#[derive(Debug, Default, Deserialize)]
pub struct ReportQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub q: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u64>,
    pub model: Option<String>,
    pub lookback: Option<u16>,
}

// ---------------------------------------------------------------------------
// 파라미터 검증
// ---------------------------------------------------------------------------

fn range(state: &AppState, q: &ReportQuery) -> ApiResult<DateRange> {
    match (q.from.as_deref(), q.to.as_deref()) {
        (None, None) => {
            let today = state.clock.today_kst();
            DateRange::new(today - Duration::days(DEFAULT_RANGE_DAYS - 1), today)
                .map_err(|e| ApiError::internal(e.into()))
        }
        (Some(from), Some(to)) => {
            DateRange::parse(from, to).map_err(|e| ApiError::invalid(e.to_string()))
        }
        _ => Err(ApiError::invalid("from과 to는 함께 지정해야 합니다")),
    }
}

fn limit(v: Option<u32>, default: u32) -> ApiResult<u32> {
    match v {
        None => Ok(default),
        Some(n) if (1..=MAX_LIMIT).contains(&n) => Ok(n),
        Some(_) => Err(ApiError::invalid(format!(
            "limit: 1~{MAX_LIMIT} 사이의 정수여야 합니다"
        ))),
    }
}

fn offset(v: Option<u64>) -> ApiResult<u64> {
    match v.unwrap_or(0) {
        n if n <= MAX_OFFSET => Ok(n),
        _ => Err(ApiError::invalid(format!(
            "offset: 0~{MAX_OFFSET} 사이의 정수여야 합니다"
        ))),
    }
}

/// 길이만 검사한 문자열 (빈 값 금지).
fn text<'a>(field: &str, s: &'a str, max: usize) -> ApiResult<&'a str> {
    let n = s.chars().count();
    if n == 0 || n > max {
        return Err(ApiError::invalid(format!("{field}: 1~{max}자")));
    }
    Ok(s)
}

fn path_param<'a>(params: &'a HashMap<String, String>, name: &str) -> &'a str {
    params.get(name).map(String::as_str).unwrap_or("")
}

/// 경로 필터: 빈 값이면 필터 없음.
fn path_filter(q: &ReportQuery) -> ApiResult<Option<&str>> {
    match q.q.as_deref() {
        None | Some("") => Ok(None),
        Some(s) if s.chars().count() > MAX_QUERY_CHARS => {
            Err(ApiError::invalid(format!("q: 최대 {MAX_QUERY_CHARS}자")))
        }
        Some(s) => Ok(Some(s)),
    }
}

fn parse_model(s: &str) -> Option<Model> {
    Model::ALL.into_iter().find(|m| m.as_str() == s)
}

/// 기여 결과를 읽는 리포트의 설정값 (모델·lookback).
struct Attribution {
    model: Model,
    lookback_days: u8,
}

/// `model`은 매출 보존 모델만 (없으면 프로젝트 기본 모델), `lookback`은 프로젝트에 설정된 값만
/// (없으면 가장 짧은 값) 허용한다. 설정에 없는 lookback은 연산 결과가 없으므로 거부한다.
async fn attribution(
    state: &AppState,
    access: &ProjectAccess,
    q: &ReportQuery,
) -> ApiResult<Attribution> {
    let requested_model = match q.model.as_deref() {
        None => None,
        Some(s) => match parse_model(s).filter(|m| m.conserves_revenue()) {
            Some(m) => Some(m),
            None => {
                return Err(ApiError::invalid(
                    "model: last_click, first_click, linear, time_decay 중 하나여야 합니다",
                ));
            }
        },
    };
    let settings = analytics_pg::attribution::get(&state.pg, access.id()).await?;
    let lookback = match q.lookback {
        None => settings.lookback_days.first().copied(),
        Some(d) => settings
            .lookback_days
            .iter()
            .copied()
            .find(|c| i32::from(*c) == i32::from(d)),
    };
    let Some(lookback) = lookback else {
        let list: Vec<String> = settings
            .lookback_days
            .iter()
            .map(|d| d.to_string())
            .collect();
        return Err(ApiError::invalid(format!(
            "lookback: 프로젝트에 설정된 값({}일) 중 하나여야 합니다",
            list.join(", ")
        )));
    };
    let lookback_days = u8::try_from(lookback)
        .map_err(|_| ApiError::internal(anyhow::anyhow!("잘못 저장된 lookback: {lookback}")))?;
    let model = requested_model
        .or_else(|| parse_model(&settings.default_model).filter(|m| m.conserves_revenue()))
        .unwrap_or(Model::LastClick);
    Ok(Attribution {
        model,
        lookback_days,
    })
}

// ---------------------------------------------------------------------------
// 응답
// ---------------------------------------------------------------------------

/// ClickHouse 결과 → 503 `storage_unavailable`.
fn storage<T>(r: anyhow::Result<T>) -> ApiResult<T> {
    r.map_err(ApiError::storage_unavailable)
}

/// 기간 리포트 응답. 닫힌 기간이면 짧게 캐시해도 된다 (늦게 도착한 데이터·재연산은 1분 안에 반영).
fn ranged<T: Serialize>(state: &AppState, range: DateRange, body: T) -> Response {
    let mut res = Json(body).into_response();
    if range.to() < state.clock.today_kst() {
        let h = res.headers_mut();
        h.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(CACHE_CLOSED),
        );
        // 세션(쿠키)마다 다른 응답
        h.append(header::VARY, HeaderValue::from_static("Cookie"));
    }
    res
}

// ---------------------------------------------------------------------------
// 핸들러
// ---------------------------------------------------------------------------

pub async fn overview(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiQuery(q): ApiQuery<ReportQuery>,
) -> ApiResult<Response> {
    let r = range(&state, &q)?;
    let body = storage(ch::overview(&state.ch, access.scope, r).await)?;
    Ok(ranged(&state, r, body))
}

pub async fn timeseries(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiQuery(q): ApiQuery<ReportQuery>,
) -> ApiResult<Response> {
    let r = range(&state, &q)?;
    let body = storage(ch::timeseries(&state.ch, access.scope, r).await)?;
    Ok(ranged(&state, r, body))
}

pub async fn channels(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiQuery(q): ApiQuery<ReportQuery>,
) -> ApiResult<Response> {
    let r = range(&state, &q)?;
    let body = storage(ch::channels(&state.ch, access.scope, r).await)?;
    Ok(ranged(&state, r, body))
}

pub async fn pages(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiQuery(q): ApiQuery<ReportQuery>,
) -> ApiResult<Response> {
    let r = range(&state, &q)?;
    let n = limit(q.limit, DEFAULT_TOP_LIMIT)?;
    let filter = path_filter(&q)?;
    let body = storage(ch::pages(&state.ch, access.scope, r, filter, n).await)?;
    Ok(ranged(&state, r, body))
}

pub async fn events(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiQuery(q): ApiQuery<ReportQuery>,
) -> ApiResult<Response> {
    let r = range(&state, &q)?;
    let n = limit(q.limit, DEFAULT_TOP_LIMIT)?;
    let body = storage(ch::events(&state.ch, access.scope, r, n).await)?;
    Ok(ranged(&state, r, body))
}

pub async fn event_properties(
    State(state): State<AppState>,
    access: ProjectAccess,
    Path(params): Path<HashMap<String, String>>,
    ApiQuery(q): ApiQuery<ReportQuery>,
) -> ApiResult<Response> {
    let r = range(&state, &q)?;
    let n = limit(q.limit, DEFAULT_TOP_LIMIT)?;
    let event_name = text(
        "event_name",
        path_param(&params, "event_name"),
        MAX_EVENT_NAME_CHARS,
    )?;
    let prop_key = text(
        "prop_key",
        path_param(&params, "prop_key"),
        MAX_PROP_KEY_LEN,
    )?;
    let body =
        storage(ch::event_properties(&state.ch, access.scope, r, event_name, prop_key, n).await)?;
    Ok(ranged(&state, r, body))
}

pub async fn realtime(State(state): State<AppState>, access: ProjectAccess) -> ApiResult<Response> {
    let body = storage(ch::realtime(&state.ch, access.scope, state.clock.now()).await)?;
    // 항상 no-store (전역 기본값)
    Ok(Json(body).into_response())
}

pub async fn funnel(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiQuery(q): ApiQuery<ReportQuery>,
) -> ApiResult<Response> {
    let r = range(&state, &q)?;
    let body = storage(ch::ecommerce_funnel(&state.ch, access.scope, r).await)?;
    Ok(ranged(&state, r, body))
}

pub async fn revenue(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiQuery(q): ApiQuery<ReportQuery>,
) -> ApiResult<Response> {
    let r = range(&state, &q)?;
    let a = attribution(&state, &access, &q).await?;
    let spend =
        analytics_pg::ad_spend::sum_by_channel(&state.pg, access.id(), r.from(), r.to()).await?;
    let body = storage(
        ch::revenue_report(&state.ch, access.scope, r, a.model, a.lookback_days, &spend).await,
    )?;
    Ok(ranged(&state, r, body))
}

pub async fn overlap(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiQuery(q): ApiQuery<ReportQuery>,
) -> ApiResult<Response> {
    let r = range(&state, &q)?;
    let n = limit(q.limit, DEFAULT_OVERLAP_LIMIT)?;
    let a = attribution(&state, &access, &q).await?;
    let body = storage(ch::overlap_paths(&state.ch, access.scope, r, a.lookback_days, n).await)?;
    Ok(ranged(&state, r, body))
}

pub async fn orders(
    State(state): State<AppState>,
    access: ProjectAccess,
    ApiQuery(q): ApiQuery<ReportQuery>,
) -> ApiResult<Response> {
    let r = range(&state, &q)?;
    let n = limit(q.limit, DEFAULT_TOP_LIMIT)?;
    let off = offset(q.offset)?;
    let a = attribution(&state, &access, &q).await?;
    let body =
        storage(ch::recent_orders(&state.ch, access.scope, r, a.lookback_days, n, off).await)?;
    Ok(ranged(&state, r, body))
}

/// 주문 상세. 주문은 언제든 바뀔 수 있어(환불·취소·재연산) 캐시하지 않는다.
pub async fn order_detail(
    State(state): State<AppState>,
    access: ProjectAccess,
    Path(params): Path<HashMap<String, String>>,
    ApiQuery(q): ApiQuery<ReportQuery>,
) -> ApiResult<Response> {
    let order_id = text(
        "order_id",
        path_param(&params, "order_id"),
        MAX_ORDER_ID_CHARS,
    )?;
    let a = attribution(&state, &access, &q).await?;
    let detail =
        storage(ch::order_detail(&state.ch, access.scope, order_id, a.lookback_days).await)?;
    match detail {
        Some(d) => Ok(Json(d).into_response()),
        None => Err(ApiError::not_found()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_bounds() {
        assert_eq!(limit(None, 50).unwrap(), 50);
        assert_eq!(limit(Some(1), 50).unwrap(), 1);
        assert_eq!(limit(Some(MAX_LIMIT), 50).unwrap(), MAX_LIMIT);
        assert!(limit(Some(0), 50).is_err());
        assert!(limit(Some(MAX_LIMIT + 1), 50).is_err());
        assert_eq!(offset(None).unwrap(), 0);
        assert_eq!(offset(Some(MAX_OFFSET)).unwrap(), MAX_OFFSET);
        assert!(offset(Some(MAX_OFFSET + 1)).is_err());
    }

    #[test]
    fn text_counts_chars_not_bytes() {
        assert!(text("k", &"가".repeat(64), 64).is_ok());
        assert!(text("k", &"가".repeat(65), 64).is_err());
        assert!(text("k", "", 64).is_err());
    }

    #[test]
    fn model_names() {
        assert_eq!(parse_model("linear"), Some(Model::Linear));
        assert_eq!(parse_model("raw_sum"), Some(Model::RawSum));
        assert_eq!(parse_model("Linear"), None);
        assert_eq!(parse_model(""), None);
    }
}
