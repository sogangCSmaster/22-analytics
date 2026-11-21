//! 데모 모드: 브라우저 안의 가짜 API 서버 (`--features demo`).
//!
//! - 모든 API 호출은 [`crate::api`]의 `raw()` 한 곳에서 [`DemoServer::handle`]로 바뀐다.
//! - 응답은 실제 API 계약(`analytics_core::dto`) 그대로의 JSON이고, 검증 오류도 같은 코드
//!   (`400 invalid_input`, `401 unauthorized`, `404 not_found`)로 돌려준다.
//! - 로그인은 어떤 이메일·비밀번호로도 성공한다 (데모 관리자로 들어간다).
//! - 리포트 숫자는 상태가 아니라 [`data`]의 결정적 생성 결과다 (프로젝트·날짜 시드, 지금 시각 주입).
//! - 설정 변경(수집 설정, 허용 도메인, 채널 규칙, 기여 설정, 비밀 키, 광고비, 광고주·멤버·프로젝트)은
//!   [`DemoState`]에 남는다. 브라우저 쪽 연결(`api.rs`)이 localStorage에 저장한다.
//! - 순수 Rust(web-sys 없음): `cargo test -p analytics-web --no-default-features`로 검증한다.

pub mod data;

use crate::api::Method;
use crate::logic::dates::Day;
use crate::logic::validate;
use analytics_core::attribution::{self as attr, DAY_MS, HOUR_MS, Model, Params, Touch};
use analytics_core::channel::UNATTRIBUTED;
use analytics_core::dto::admin::*;
use analytics_core::dto::reports::*;
use analytics_core::settings::TrackingSettings;
use data::{CHANNELS, Clock, EVENTS, Order, PAID};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// 데모 사용자
pub const DEMO_EMAIL: &str = "demo@binarycube.co.kr";
pub const DEMO_NAME: &str = "데모 관리자";
/// localStorage 키 (로그인 여부 + 변경한 설정)
#[cfg_attr(not(feature = "demo"), allow(dead_code))]
pub const STORAGE_KEY: &str = "at_demo_state_v1";

const DEMO_USER_ID: i64 = 1;
const ORG_NAME: &str = "바이너리큐브 데모";
const TRACKER_HOST: &str = "https://collect.binarycube.co.kr";
const MAX_RANGE_DAYS: i32 = 400;
const MAX_LIMIT: u32 = 1000;
const MAX_OFFSET: u64 = 100_000;
const MAX_QUERY_CHARS: usize = 200;
const MAX_ORDER_ID_CHARS: usize = 128;

// ---------------------------------------------------------------- 상태

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrgRec {
    pub id: i64,
    pub name: String,
    pub memo: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemberRec {
    pub org_id: i64,
    pub user_id: i64,
    pub role: Role,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectRec {
    pub id: i32,
    pub org_id: i64,
    pub name: String,
    pub public_key: String,
    pub timezone: String,
    pub currency: String,
    pub status: ProjectStatus,
    pub created_at: String,
    pub settings: TrackingSettings,
    pub settings_version: i32,
    pub origins: Vec<String>,
    pub rules: Vec<ChannelRuleDto>,
    pub attribution: AttributionSettingsDto,
    pub api_keys: Vec<ApiKeyDto>,
    /// 업로드한 광고비 (같은 day·channel·campaign의 생성값을 덮어쓴다)
    pub ad_spend: Vec<AdSpendRow>,
}

/// 데모 서버의 바뀌는 상태. 기본값 = 처음 연 데모 (로그인 전).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DemoState {
    pub logged_in: bool,
    pub next_id: i64,
    pub users: Vec<UserDto>,
    pub orgs: Vec<OrgRec>,
    pub members: Vec<MemberRec>,
    pub projects: Vec<ProjectRec>,
}

fn default_attribution() -> AttributionSettingsDto {
    AttributionSettingsDto {
        lookback_days: vec![7, 30],
        default_model: Model::LastClick,
        half_life_hours: 168,
        include_organic: true,
        include_referral: true,
    }
}

fn user(id: i64, email: &str, name: &str, admin: bool) -> UserDto {
    UserDto {
        id,
        email: email.into(),
        name: name.into(),
        is_super_admin: admin,
        created_at: "2026-07-01T01:00:00Z".into(),
        last_login_at: None,
    }
}

fn seed_project(id: i32, name: &str, key: &str, origin: &str) -> ProjectRec {
    ProjectRec {
        id,
        org_id: 1,
        name: name.into(),
        public_key: key.into(),
        timezone: "Asia/Seoul".into(),
        currency: "KRW".into(),
        status: ProjectStatus::Active,
        created_at: "2026-07-01T02:00:00Z".into(),
        settings: TrackingSettings {
            ecommerce: true,
            scroll_depth: true,
            outbound_links: true,
            ..TrackingSettings::default()
        },
        settings_version: 3,
        origins: vec![origin.into()],
        rules: vec![ChannelRuleDto {
            id: 10 + id as i64,
            priority: 10,
            channel: "naver_sa".into(),
            when: analytics_core::channel::RuleMatch {
                utm_source: vec!["naver".into()],
                utm_medium: vec!["cpc".into()],
                ..Default::default()
            },
            label: "네이버 검색광고 (UTM)".into(),
        }],
        attribution: default_attribution(),
        api_keys: vec![ApiKeyDto {
            id: 20 + id as i64,
            prefix: format!("sk_demo_{id}a7c3f9e"),
            scopes: vec!["orders:write".into()],
            created_at: "2026-07-02T03:00:00Z".into(),
            last_used_at: Some("2026-11-20T23:58:00Z".into()),
            revoked_at: None,
        }],
        ad_spend: vec![],
    }
}

impl Default for DemoState {
    fn default() -> Self {
        Self {
            logged_in: false,
            next_id: 100,
            users: vec![
                user(DEMO_USER_ID, DEMO_EMAIL, DEMO_NAME, true),
                user(2, "analyst@demo.binarycube.co.kr", "이분석", false),
                user(3, "client@demo.binarycube.co.kr", "박광고주", false),
            ],
            orgs: vec![OrgRec {
                id: 1,
                name: ORG_NAME.into(),
                memo: "데모용 예시 광고주입니다. 숫자는 모두 예시 데이터입니다.".into(),
                created_at: "2026-07-01T02:00:00Z".into(),
            }],
            members: vec![
                MemberRec {
                    org_id: 1,
                    user_id: DEMO_USER_ID,
                    role: Role::Admin,
                },
                MemberRec {
                    org_id: 1,
                    user_id: 2,
                    role: Role::Analyst,
                },
                MemberRec {
                    org_id: 1,
                    user_id: 3,
                    role: Role::Viewer,
                },
            ],
            projects: vec![
                seed_project(
                    1,
                    "데모 쇼핑몰",
                    "pk_demo_shop0001",
                    "shop.demo.binarycube.co.kr",
                ),
                seed_project(
                    2,
                    "데모 브랜드몰",
                    "pk_demo_brand002",
                    "brand.demo.binarycube.co.kr",
                ),
            ],
        }
    }
}

impl DemoState {
    /// localStorage 값 → 상태. 없거나 깨졌으면 기본값.
    pub fn decode(s: Option<&str>) -> Self {
        s.and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    #[cfg_attr(not(feature = "demo"), allow(dead_code))]
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    fn id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    fn project(&self, pid: i32) -> Option<&ProjectRec> {
        self.projects.iter().find(|p| p.id == pid)
    }
}

// ---------------------------------------------------------------- 응답

type Resp = (u16, String);
type R = Result<Resp, Resp>;

fn json<T: Serialize>(status: u16, body: &T) -> R {
    Ok((status, serde_json::to_string(body).unwrap_or_default()))
}

fn ok<T: Serialize>(body: &T) -> R {
    json(200, body)
}

fn no_content() -> R {
    Ok((204, String::new()))
}

fn fail(status: u16, code: &str, message: impl Into<String>) -> Resp {
    let b = ApiErrorBody {
        error: code.into(),
        message: message.into(),
    };
    (status, serde_json::to_string(&b).unwrap_or_default())
}

fn invalid(message: impl Into<String>) -> Resp {
    fail(400, "invalid_input", message)
}

fn not_found() -> Resp {
    fail(404, "not_found", "찾을 수 없습니다")
}

fn parse_body<T: DeserializeOwned>(body: Option<&str>) -> Result<T, Resp> {
    serde_json::from_str(body.unwrap_or(""))
        .map_err(|_| invalid("요청 본문(JSON)을 읽을 수 없습니다"))
}

fn ok_or_400<T>(r: Result<T, &str>) -> Result<T, Resp> {
    r.map_err(invalid)
}

// ---------------------------------------------------------------- 경로·쿼리

/// `%XX` 디코드 (잘못된 인코딩은 그대로 둔다)
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct Query(HashMap<String, String>);

impl Query {
    fn parse(s: &str) -> Self {
        Query(
            s.split('&')
                .filter(|x| !x.is_empty())
                .map(|kv| match kv.split_once('=') {
                    Some((k, v)) => (decode(k), decode(v)),
                    None => (decode(kv), String::new()),
                })
                .collect(),
        )
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.0.get(k).map(String::as_str)
    }

    fn num<T: std::str::FromStr>(&self, k: &str) -> Result<Option<T>, Resp> {
        match self.get(k) {
            None => Ok(None),
            Some(v) => v
                .parse()
                .map(Some)
                .map_err(|_| invalid(format!("{k}: 숫자가 아닙니다"))),
        }
    }

    /// 리포트 기간 (실제 API와 같은 규칙)
    fn range(&self, today: Day) -> Result<(Day, Day), Resp> {
        match (self.get("from"), self.get("to")) {
            (None, None) => Ok((today.add(-6), today)),
            (Some(f), Some(t)) => {
                let p = |s: &str| {
                    Day::parse(s).ok_or_else(|| {
                        invalid(format!(
                            "날짜 형식이 잘못되었습니다 (YYYY-MM-DD): {}",
                            s.chars().take(32).collect::<String>()
                        ))
                    })
                };
                let (f, t) = (p(f)?, p(t)?);
                if f > t {
                    return Err(invalid("시작일이 종료일보다 늦습니다"));
                }
                let days = t.0 - f.0 + 1;
                if days > MAX_RANGE_DAYS {
                    return Err(invalid(format!(
                        "조회 기간은 최대 {MAX_RANGE_DAYS}일입니다 (요청 {days}일)"
                    )));
                }
                Ok((f, t))
            }
            _ => Err(invalid("from과 to는 함께 지정해야 합니다")),
        }
    }

    fn limit(&self, default: u32) -> Result<usize, Resp> {
        match self.num::<u32>("limit")? {
            None => Ok(default as usize),
            Some(n) if (1..=MAX_LIMIT).contains(&n) => Ok(n as usize),
            Some(_) => Err(invalid(format!(
                "limit: 1~{MAX_LIMIT} 사이의 정수여야 합니다"
            ))),
        }
    }

    fn offset(&self) -> Result<usize, Resp> {
        match self.num::<u64>("offset")?.unwrap_or(0) {
            n if n <= MAX_OFFSET => Ok(n as usize),
            _ => Err(invalid(format!(
                "offset: 0~{MAX_OFFSET} 사이의 정수여야 합니다"
            ))),
        }
    }
}

fn text(field: &str, s: &str, max: usize) -> Result<(), Resp> {
    let n = s.chars().count();
    if n == 0 || n > max {
        return Err(invalid(format!("{field}: 1~{max}자")));
    }
    Ok(())
}

// ---------------------------------------------------------------- 시각

fn ymd_hms(ms: i64) -> (Day, i64, i64, i64) {
    let day = Day(ms.div_euclid(DAY_MS) as i32);
    let s = ms.rem_euclid(DAY_MS) / 1000;
    (day, s / 3600, s % 3600 / 60, s % 60)
}

/// `YYYY-MM-DDTHH:MM:SS+09:00`
pub fn kst_iso(ms: i64) -> String {
    let (d, h, m, s) = ymd_hms(ms + data::SEOUL_MS);
    format!("{}T{h:02}:{m:02}:{s:02}+09:00", d.iso())
}

/// `YYYY-MM-DDTHH:MM:SSZ`
fn utc_iso(ms: i64) -> String {
    let (d, h, m, s) = ymd_hms(ms);
    format!("{}T{h:02}:{m:02}:{s:02}Z", d.iso())
}

fn hhmm(ms: i64) -> String {
    let (_, h, m, _) = ymd_hms(ms + data::SEOUL_MS);
    format!("{h:02}:{m:02}")
}

fn ratio(a: f64, b: f64) -> f64 {
    if b > 0.0 { a / b } else { 0.0 }
}

// ---------------------------------------------------------------- 서버

pub struct DemoServer {
    state: DemoState,
    now_ms: i64,
}

impl DemoServer {
    pub fn new(now_ms: i64) -> Self {
        Self::with_state(DemoState::default(), now_ms)
    }

    pub fn with_state(state: DemoState, now_ms: i64) -> Self {
        Self { state, now_ms }
    }

    pub fn set_now(&mut self, now_ms: i64) {
        self.now_ms = now_ms;
    }

    pub fn state(&self) -> &DemoState {
        &self.state
    }

    fn clock(&self) -> Clock {
        Clock::new(self.now_ms)
    }

    /// (상태 코드, 본문). 204는 빈 본문. 경로는 `/v1/...?쿼리` (API 주소 없이).
    pub fn handle(&mut self, method: Method, path: &str, body: Option<&str>) -> (u16, String) {
        match self.route(method, path, body) {
            Ok(r) | Err(r) => r,
        }
    }

    fn route(&mut self, method: Method, path: &str, body: Option<&str>) -> R {
        use Method::*;
        let (p, q) = path.split_once('?').unwrap_or((path, ""));
        let q = Query::parse(q);
        let segs: Vec<String> = p.trim_matches('/').split('/').map(decode).collect();
        let s: Vec<&str> = segs.iter().map(String::as_str).collect();
        let Some((&"v1", s)) = s.split_first() else {
            return Err(not_found());
        };
        // 인증
        match (method, s) {
            (Post, ["auth", "login"]) => return self.login(),
            (Post, ["auth", "logout"]) => {
                self.state.logged_in = false;
                return no_content();
            }
            _ => {}
        }
        if !self.state.logged_in {
            return Err(fail(401, "unauthorized", "로그인이 필요합니다"));
        }
        match (method, s) {
            (Get, ["auth", "me"]) => ok(&self.me()),
            (Get, ["users"]) => self.users(&q),
            (Post, ["users"]) => self.create_user(body),
            (Get, ["orgs"]) => ok(&self.orgs()),
            (Post, ["orgs"]) => self.create_org(body),
            (_, ["orgs", id, rest @ ..]) => {
                let id: i64 = id.parse().map_err(|_| not_found())?;
                if !self.state.orgs.iter().any(|o| o.id == id) {
                    return Err(not_found());
                }
                self.org_route(method, id, rest, body)
            }
            (_, ["projects", pid, rest @ ..]) => {
                let pid: i32 = pid.parse().map_err(|_| not_found())?;
                if self.state.project(pid).is_none() {
                    return Err(not_found());
                }
                self.project_route(method, pid, rest, &q, body)
            }
            _ => Err(not_found()),
        }
    }

    // ------------------------------------------------------------ 인증·사용자

    fn login(&mut self) -> R {
        // 데모: 어떤 이메일·비밀번호(빈 값 포함)로도 데모 관리자로 들어간다.
        self.state.logged_in = true;
        let now = utc_iso(self.now_ms);
        if let Some(u) = self.state.users.iter_mut().find(|u| u.id == DEMO_USER_ID) {
            u.last_login_at = Some(now);
        }
        ok(&self.me())
    }

    fn demo_user(&self) -> UserDto {
        self.state
            .users
            .iter()
            .find(|u| u.id == DEMO_USER_ID)
            .cloned()
            .unwrap_or_else(|| user(DEMO_USER_ID, DEMO_EMAIL, DEMO_NAME, true))
    }

    fn me(&self) -> MeResponse {
        MeResponse {
            user: self.demo_user(),
            orgs: self
                .state
                .orgs
                .iter()
                .map(|o| OrgAccess {
                    id: o.id,
                    name: o.name.clone(),
                    role: Role::Admin,
                    projects: self
                        .state
                        .projects
                        .iter()
                        .filter(|p| p.org_id == o.id)
                        .map(|p| ProjectSummary {
                            id: p.id,
                            name: p.name.clone(),
                            status: p.status,
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn users(&self, q: &Query) -> R {
        let needle = q.get("q").unwrap_or("").trim().to_lowercase();
        let (limit, offset) = (q.limit(50)?, q.offset()?);
        let all: Vec<&UserDto> = self
            .state
            .users
            .iter()
            .filter(|u| {
                needle.is_empty()
                    || u.email.to_lowercase().contains(&needle)
                    || u.name.to_lowercase().contains(&needle)
            })
            .collect();
        ok(&Page {
            total: all.len() as i64,
            items: all
                .into_iter()
                .skip(offset)
                .take(limit)
                .cloned()
                .collect::<Vec<_>>(),
            limit: limit as i64,
            offset: offset as i64,
        })
    }

    fn create_user(&mut self, body: Option<&str>) -> R {
        let req: CreateUserRequest = parse_body(body)?;
        let email = ok_or_400(validate::email(&req.email))?;
        ok_or_400(validate::password(&req.password))?;
        if self.state.users.iter().any(|u| u.email == email) {
            return Err(fail(409, "conflict", "이미 같은 이메일의 계정이 있습니다"));
        }
        let mut u = user(self.state.id(), &email, req.name.trim(), req.is_super_admin);
        u.created_at = utc_iso(self.now_ms);
        self.state.users.push(u.clone());
        json(201, &u)
    }

    // ------------------------------------------------------------ 광고주

    fn org_dto(o: &OrgRec) -> OrgDto {
        OrgDto {
            id: o.id,
            name: o.name.clone(),
            memo: o.memo.clone(),
            created_at: o.created_at.clone(),
            role: Role::Admin,
        }
    }

    fn orgs(&self) -> Vec<OrgDto> {
        self.state.orgs.iter().map(Self::org_dto).collect()
    }

    fn create_org(&mut self, body: Option<&str>) -> R {
        let req: CreateOrgRequest = parse_body(body)?;
        let name = ok_or_400(validate::name(&req.name))?;
        let o = OrgRec {
            id: self.state.id(),
            name,
            memo: req.memo.trim().to_owned(),
            created_at: utc_iso(self.now_ms),
        };
        self.state.orgs.push(o.clone());
        json(201, &Self::org_dto(&o))
    }

    fn org_route(&mut self, method: Method, id: i64, rest: &[&str], body: Option<&str>) -> R {
        use Method::*;
        match (method, rest) {
            (Get, []) => ok(&self.orgs().into_iter().find(|o| o.id == id)),
            (Patch, []) => {
                let req: UpdateOrgRequest = parse_body(body)?;
                let name = req
                    .name
                    .map(|n| ok_or_400(validate::name(&n)))
                    .transpose()?;
                let o = self
                    .state
                    .orgs
                    .iter_mut()
                    .find(|o| o.id == id)
                    .ok_or_else(not_found)?;
                if let Some(n) = name {
                    o.name = n;
                }
                if let Some(m) = req.memo {
                    o.memo = m.trim().to_owned();
                }
                ok(&Self::org_dto(o))
            }
            (Get, ["members"]) => ok(&self.members(id)),
            (Post, ["members"]) => {
                let req: AddMemberRequest = parse_body(body)?;
                let email = req.email.trim().to_lowercase();
                let Some(u) = self.state.users.iter().find(|u| u.email == email).cloned() else {
                    return Err(fail(
                        400,
                        "user_not_found",
                        "해당 이메일의 계정이 없습니다. 먼저 계정을 만드세요",
                    ));
                };
                match self
                    .state
                    .members
                    .iter_mut()
                    .find(|m| m.org_id == id && m.user_id == u.id)
                {
                    Some(m) => m.role = req.role,
                    None => self.state.members.push(MemberRec {
                        org_id: id,
                        user_id: u.id,
                        role: req.role,
                    }),
                }
                ok(&MemberDto {
                    user_id: u.id,
                    email: u.email,
                    name: u.name,
                    role: req.role,
                })
            }
            (Delete, ["members", uid]) => {
                let uid: i64 = uid.parse().map_err(|_| not_found())?;
                self.state
                    .members
                    .retain(|m| !(m.org_id == id && m.user_id == uid));
                no_content()
            }
            (Get, ["projects"]) => ok(&self
                .state
                .projects
                .iter()
                .filter(|p| p.org_id == id)
                .map(project_dto)
                .collect::<Vec<_>>()),
            (Post, ["projects"]) => {
                let req: CreateProjectRequest = parse_body(body)?;
                let name = ok_or_400(validate::name(&req.name))?;
                let pid = self.state.projects.iter().map(|p| p.id).max().unwrap_or(0) + 1;
                let key = format!(
                    "pk_demo_{:08x}",
                    data::seed(&[pid as i64, self.now_ms]) as u32
                );
                let mut p = seed_project(pid, &name, &key, "");
                p.org_id = id;
                p.origins.clear();
                p.rules.clear();
                p.api_keys.clear();
                p.settings = TrackingSettings::default();
                p.settings_version = 1;
                p.created_at = utc_iso(self.now_ms);
                p.timezone = req.timezone.unwrap_or_else(|| "Asia/Seoul".into());
                p.currency = req.currency.unwrap_or_else(|| "KRW".into());
                self.state.projects.push(p.clone());
                json(201, &project_dto(&p))
            }
            _ => Err(not_found()),
        }
    }

    fn members(&self, org: i64) -> Vec<MemberDto> {
        self.state
            .members
            .iter()
            .filter(|m| m.org_id == org)
            .filter_map(|m| {
                let u = self.state.users.iter().find(|u| u.id == m.user_id)?;
                Some(MemberDto {
                    user_id: u.id,
                    email: u.email.clone(),
                    name: u.name.clone(),
                    role: m.role,
                })
            })
            .collect()
    }

    // ------------------------------------------------------------ 프로젝트

    fn proj_mut(&mut self, pid: i32) -> &mut ProjectRec {
        self.state
            .projects
            .iter_mut()
            .find(|p| p.id == pid)
            .expect("route checks the project")
    }

    fn proj(&self, pid: i32) -> &ProjectRec {
        self.state.project(pid).expect("route checks the project")
    }

    fn project_route(
        &mut self,
        method: Method,
        pid: i32,
        rest: &[&str],
        q: &Query,
        body: Option<&str>,
    ) -> R {
        use Method::*;
        let now = self.now_ms;
        match (method, rest) {
            (Get, []) => ok(&project_dto(self.proj(pid))),
            (Patch, []) => {
                let req: UpdateProjectRequest = parse_body(body)?;
                let name = req
                    .name
                    .map(|n| ok_or_400(validate::name(&n)))
                    .transpose()?;
                let p = self.proj_mut(pid);
                if let Some(n) = name {
                    p.name = n;
                }
                if let Some(s) = req.status {
                    p.status = s;
                }
                if let Some(tz) = req.timezone {
                    p.timezone = tz;
                }
                ok(&project_dto(p))
            }
            (Get, ["snippet"]) => {
                let pk = self.proj(pid).public_key.clone();
                ok(&SnippetResponse {
                    html: format!(
                        "<script>window.at=window.at||function(){{(at.q=at.q||[]).push(arguments)}};</script>\n\
                         <script async src=\"{TRACKER_HOST}/t.js\" data-key=\"{pk}\"></script>"
                    ),
                    public_key: pk,
                    tracker_host: TRACKER_HOST.into(),
                })
            }
            (Get, ["settings"]) => ok(&settings_resp(self.proj(pid))),
            (Put, ["settings"]) => {
                let mut s: TrackingSettings = parse_body(body)?;
                for list in [
                    &mut s.click_id_params,
                    &mut s.capture_query_params,
                    &mut s.strip_query_params,
                ] {
                    *list = param_list(list)?;
                }
                let p = self.proj_mut(pid);
                if p.settings != s {
                    p.settings = s;
                    p.settings_version += 1;
                }
                ok(&settings_resp(p))
            }
            (Get, ["origins"]) => ok(&OriginsResponse {
                origins: self.proj(pid).origins.clone(),
            }),
            (Post, ["origins"]) => {
                let req: AddOriginRequest = parse_body(body)?;
                let o = ok_or_400(validate::origin(&req.origin)).map_err(|_| {
                    invalid("origin: 스킴·경로·포트 없는 도메인만 입력하세요 (예: shop.client.com)")
                })?;
                let p = self.proj_mut(pid);
                if !p.origins.contains(&o) {
                    p.origins.push(o);
                    p.origins.sort();
                }
                json(
                    201,
                    &OriginsResponse {
                        origins: p.origins.clone(),
                    },
                )
            }
            (Delete, ["origins", o]) => {
                let o = o.to_lowercase();
                self.proj_mut(pid).origins.retain(|x| *x != o);
                no_content()
            }
            (Get, ["channel-rules"]) => ok(&self.proj(pid).rules),
            (Post, ["channel-rules"]) => {
                let req: CreateChannelRuleRequest = parse_body(body)?;
                let rule = ChannelRuleDto {
                    id: self.state.id(),
                    priority: priority(req.priority.unwrap_or(100))?,
                    channel: channel_name(&req.channel)?,
                    when: rule_match(req.when)?,
                    label: req.label.trim().to_owned(),
                };
                let p = self.proj_mut(pid);
                p.rules.push(rule.clone());
                sort_rules(&mut p.rules);
                json(201, &rule)
            }
            (Patch, ["channel-rules", rid]) => {
                let req: UpdateChannelRuleRequest = parse_body(body)?;
                let rid: i64 = rid.parse().map_err(|_| not_found())?;
                let channel = req.channel.as_deref().map(channel_name).transpose()?;
                let when = req.when.map(rule_match).transpose()?;
                let prio = req.priority.map(priority).transpose()?;
                let p = self.proj_mut(pid);
                let r = p
                    .rules
                    .iter_mut()
                    .find(|r| r.id == rid)
                    .ok_or_else(not_found)?;
                if let Some(c) = channel {
                    r.channel = c;
                }
                if let Some(w) = when {
                    r.when = w;
                }
                if let Some(x) = prio {
                    r.priority = x;
                }
                if let Some(l) = req.label {
                    r.label = l.trim().to_owned();
                }
                let out = r.clone();
                sort_rules(&mut p.rules);
                ok(&out)
            }
            (Delete, ["channel-rules", rid]) => {
                let rid: i64 = rid.parse().map_err(|_| not_found())?;
                let p = self.proj_mut(pid);
                let n = p.rules.len();
                p.rules.retain(|r| r.id != rid);
                if p.rules.len() == n {
                    return Err(not_found());
                }
                no_content()
            }
            (Get, ["attribution-settings"]) => ok(&self.proj(pid).attribution),
            (Put, ["attribution-settings"]) => {
                let mut a: AttributionSettingsDto = parse_body(body)?;
                if !a.default_model.conserves_revenue() {
                    return Err(invalid(
                        "default_model: 매출 보존 모델만 기본값으로 쓸 수 있습니다",
                    ));
                }
                if a.lookback_days.iter().any(|d| !(1..=90).contains(d)) {
                    return Err(invalid("lookback_days: 1~90일"));
                }
                a.lookback_days.sort_unstable();
                a.lookback_days.dedup();
                if a.lookback_days.is_empty() || a.lookback_days.len() > 10 {
                    return Err(invalid("lookback_days: 1~10개"));
                }
                if !(1..=2160).contains(&a.half_life_hours) {
                    return Err(invalid("half_life_hours: 1~2160"));
                }
                self.proj_mut(pid).attribution = a.clone();
                ok(&a)
            }
            (Get, ["ad-spend"]) => {
                let day = |k: &str| {
                    q.get(k)
                        .and_then(|v| Day::parse(v.trim()))
                        .ok_or_else(|| invalid(format!("{k}: YYYY-MM-DD 날짜가 필요합니다")))
                };
                let (from, to) = (day("from")?, day("to")?);
                if from > to {
                    return Err(invalid("from은 to보다 늦을 수 없습니다"));
                }
                if to.0 - from.0 > MAX_RANGE_DAYS {
                    return Err(invalid(format!("기간은 최대 {MAX_RANGE_DAYS}일")));
                }
                ok(&AdSpendResponse {
                    from: from.iso(),
                    to: to.iso(),
                    rows: self.spend_rows(pid, from, to),
                })
            }
            (Post, ["ad-spend"]) => {
                let rows = parse_csv(body.unwrap_or(""))?;
                let n = rows.len();
                let p = self.proj_mut(pid);
                for r in rows {
                    match p.ad_spend.iter_mut().find(|x| {
                        x.day == r.day && x.channel == r.channel && x.campaign == r.campaign
                    }) {
                        Some(x) => x.amount = r.amount,
                        None => p.ad_spend.push(r),
                    }
                }
                ok(&AdSpendUploadResponse { rows: n })
            }
            (Get, ["api-keys"]) => ok(&self.proj(pid).api_keys),
            (Post, ["api-keys"]) => {
                let id = self.state.id();
                let secret = format!(
                    "sk_demo_{:016x}{:016x}",
                    data::seed(&[id, now, pid as i64]),
                    data::seed(&[now, id])
                );
                let key = ApiKeyDto {
                    id,
                    prefix: secret.chars().take(16).collect(),
                    scopes: vec!["orders:write".into()],
                    created_at: utc_iso(now),
                    last_used_at: None,
                    revoked_at: None,
                };
                self.proj_mut(pid).api_keys.insert(0, key.clone());
                json(201, &CreatedApiKeyResponse { key, secret })
            }
            (Delete, ["api-keys", kid]) => {
                let kid: i64 = kid.parse().map_err(|_| not_found())?;
                let k = self
                    .proj_mut(pid)
                    .api_keys
                    .iter_mut()
                    .find(|k| k.id == kid)
                    .ok_or_else(not_found)?;
                if k.revoked_at.is_none() {
                    k.revoked_at = Some(utc_iso(now));
                }
                no_content()
            }
            (Get, ["reports", rest @ ..]) => self.report(pid, rest, q),
            _ => Err(not_found()),
        }
    }

    // ------------------------------------------------------------ 리포트

    fn report(&self, pid: i32, rest: &[&str], q: &Query) -> R {
        let c = self.clock();
        let reports = Reports {
            pid,
            c,
            attribution: &self.proj(pid).attribution,
        };
        match rest {
            ["realtime"] => ok(&reports.realtime()),
            ["orders", id] => {
                text("order_id", id, MAX_ORDER_ID_CHARS)?;
                let lb = reports.lookback(q)?;
                ok(&reports.order_detail(id, lb).ok_or_else(not_found)?)
            }
            _ => {
                let (from, to) = q.range(c.today)?;
                match rest {
                    ["overview"] => ok(&OverviewReport {
                        range: rr(from, to),
                        previous_range: {
                            let n = to.0 - from.0 + 1;
                            rr(from.add(-n), to.add(-n))
                        },
                        current: reports.metrics(from, to),
                        previous: {
                            let n = to.0 - from.0 + 1;
                            reports.metrics(from.add(-n), to.add(-n))
                        },
                    }),
                    ["timeseries"] => ok(&reports.timeseries(from, to)),
                    ["channels"] => ok(&reports.channels(from, to)),
                    ["pages"] => {
                        let needle = match q.get("q") {
                            None | Some("") => None,
                            Some(s) if s.chars().count() > MAX_QUERY_CHARS => {
                                return Err(invalid(format!("q: 최대 {MAX_QUERY_CHARS}자")));
                            }
                            Some(s) => Some(s),
                        };
                        ok(&reports.pages(from, to, needle, q.limit(50)?))
                    }
                    ["events"] => ok(&reports.events(from, to, q.limit(50)?)),
                    ["events", ev, "properties", key] => {
                        text("event_name", ev, 64)?;
                        text("prop_key", key, analytics_core::event::MAX_PROP_KEY_LEN)?;
                        ok(&reports.properties(from, to, ev, key, q.limit(50)?))
                    }
                    ["funnel"] => ok(&reports.funnel(from, to)),
                    ["revenue"] => {
                        let model = match q.get("model") {
                            None => self.proj(pid).attribution.default_model,
                            Some(s) => Model::ALL
                                .into_iter()
                                .find(|m| m.as_str() == s && m.conserves_revenue())
                                .ok_or_else(|| {
                                    invalid("model: last_click, first_click, linear, time_decay 중 하나여야 합니다")
                                })?,
                        };
                        let lb = reports.lookback(q)?;
                        let spend = self.spend_by_channel(pid, from, to);
                        ok(&reports.revenue(from, to, model, lb, &spend))
                    }
                    ["overlap"] => {
                        let lb = reports.lookback(q)?;
                        ok(&reports.overlap(from, to, lb, q.limit(20)?))
                    }
                    ["orders"] => {
                        let lb = reports.lookback(q)?;
                        ok(&reports.recent_orders(from, to, lb, q.limit(50)?, q.offset()?))
                    }
                    _ => Err(not_found()),
                }
            }
        }
    }

    /// 광고비: 생성값(유료 채널 클릭 × CPC) + 업로드 값(덮어쓰기)
    fn spend_rows(&self, pid: i32, from: Day, to: Day) -> Vec<AdSpendRow> {
        let c = self.clock();
        let mut rows: BTreeMap<(String, String, String), i64> = BTreeMap::new();
        if let Some(prof) = data::profile(pid) {
            let lo = from.max(c.today.add(-(data::HISTORY_DAYS - 1)));
            let mut d = lo;
            while d <= to.min(c.today) {
                let t = data::day_traffic(pid, d, c);
                for (i, ch) in CHANNELS.iter().take(PAID).enumerate() {
                    let noise = 0.9 + 0.2 * data::unit(&[pid as i64, d.0 as i64, 300 + i as i64]);
                    let total = (t.channel_sessions[i] as f64 * prof.cpc[i] * noise / 10.0).round()
                        as u64
                        * 10;
                    let camps = data::campaigns(ch);
                    for (camp, amt) in camps.iter().zip(data::split(total / 10, &[0.6, 0.4])) {
                        if amt > 0 {
                            rows.insert(
                                (d.iso(), ch.to_string(), camp.to_string()),
                                amt as i64 * 10,
                            );
                        }
                    }
                }
                d = d.add(1);
            }
        }
        let (f, t) = (from.iso(), to.iso());
        for r in &self.proj(pid).ad_spend {
            if r.day >= f && r.day <= t {
                rows.insert(
                    (r.day.clone(), r.channel.clone(), r.campaign.clone()),
                    r.amount,
                );
            }
        }
        rows.into_iter()
            .map(|((day, channel, campaign), amount)| AdSpendRow {
                day,
                channel,
                campaign,
                amount,
            })
            .collect()
    }

    fn spend_by_channel(&self, pid: i32, from: Day, to: Day) -> BTreeMap<String, i64> {
        let mut m = BTreeMap::new();
        for r in self.spend_rows(pid, from, to) {
            *m.entry(r.channel.to_lowercase()).or_insert(0) += r.amount;
        }
        m
    }
}

fn rr(from: Day, to: Day) -> ReportRange {
    ReportRange {
        from: from.iso(),
        to: to.iso(),
    }
}

fn project_dto(p: &ProjectRec) -> ProjectDto {
    ProjectDto {
        id: p.id,
        org_id: p.org_id,
        name: p.name.clone(),
        public_key: p.public_key.clone(),
        timezone: p.timezone.clone(),
        currency: p.currency.clone(),
        status: p.status,
        settings_version: p.settings_version,
        created_at: p.created_at.clone(),
        role: Role::Admin,
    }
}

fn settings_resp(p: &ProjectRec) -> ProjectSettingsResponse {
    ProjectSettingsResponse {
        settings: p.settings.clone(),
        settings_version: p.settings_version,
    }
}

fn param_list(list: &[String]) -> Result<Vec<String>, Resp> {
    let mut out: Vec<String> = vec![];
    for p in list {
        let p = p.trim();
        let ok = !p.is_empty()
            && p.len() <= 64
            && p.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'[' | b']')
            });
        if !ok {
            return Err(invalid("파라미터 이름은 1~64자 영문·숫자·_-.[]"));
        }
        if !out.iter().any(|x| x == p) {
            out.push(p.to_owned());
        }
    }
    Ok(out)
}

fn channel_name(s: &str) -> Result<String, Resp> {
    let t = s.trim();
    if t.is_empty() || t.chars().count() > 100 || t.chars().any(char::is_control) {
        return Err(invalid("channel: 1~100자"));
    }
    Ok(t.to_owned())
}

fn priority(p: i32) -> Result<i32, Resp> {
    if (0..=100_000).contains(&p) {
        Ok(p)
    } else {
        Err(invalid("priority: 0~100000"))
    }
}

fn rule_match(
    m: analytics_core::channel::RuleMatch,
) -> Result<analytics_core::channel::RuleMatch, Resp> {
    let clean = |v: Vec<String>| -> Vec<String> {
        v.into_iter()
            .map(|x| x.trim().to_owned())
            .filter(|x| !x.is_empty())
            .collect()
    };
    let m = analytics_core::channel::RuleMatch {
        utm_source: clean(m.utm_source),
        utm_medium: clean(m.utm_medium),
        utm_campaign_prefix: clean(m.utm_campaign_prefix),
        click_id_type: clean(m.click_id_type),
        referrer_domain: clean(m.referrer_domain),
    };
    let empty = m.utm_source.is_empty()
        && m.utm_medium.is_empty()
        && m.utm_campaign_prefix.is_empty()
        && m.click_id_type.is_empty()
        && m.referrer_domain.is_empty();
    if empty {
        return Err(invalid("match: 조건을 하나 이상 입력하세요"));
    }
    Ok(m)
}

fn sort_rules(r: &mut [ChannelRuleDto]) {
    r.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.id.cmp(&b.id)));
}

/// 광고비 CSV (`day,channel,campaign,amount`, 머리글 선택). 같은 키는 마지막 값.
fn parse_csv(body: &str) -> Result<Vec<AdSpendRow>, Resp> {
    let mut rows: BTreeMap<(String, String, String), i64> = BTreeMap::new();
    let mut first = true;
    for (i, line) in body.trim_start_matches('\u{feff}').lines().enumerate() {
        let n = i + 1;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split(',').map(str::trim).collect();
        if std::mem::take(&mut first) && cols[0].eq_ignore_ascii_case("day") {
            continue;
        }
        if cols.len() != 4 {
            return Err(invalid(format!(
                "{n}행: 열은 day,channel,campaign,amount 4개여야 합니다"
            )));
        }
        let day = Day::parse(cols[0])
            .ok_or_else(|| invalid(format!("{n}행: day는 YYYY-MM-DD 형식의 실제 날짜")))?;
        let channel = channel_name(cols[1])
            .map_err(|_| invalid(format!("{n}행: channel은 1~100자")))?
            .to_lowercase();
        if cols[2].chars().count() > 200 {
            return Err(invalid(format!("{n}행: campaign은 최대 200자")));
        }
        let amount: i64 = (!cols[3].is_empty() && cols[3].bytes().all(|b| b.is_ascii_digit()))
            .then(|| cols[3].parse().ok())
            .flatten()
            .ok_or_else(|| invalid(format!("{n}행: amount는 0 이상의 정수 (통화 최소단위)")))?;
        rows.insert((day.iso(), channel, cols[2].to_owned()), amount);
    }
    if rows.is_empty() {
        return Err(invalid("데이터 행이 없습니다"));
    }
    Ok(rows
        .into_iter()
        .map(|((day, channel, campaign), amount)| AdSpendRow {
            day,
            channel,
            campaign,
            amount,
        })
        .collect())
}

// ---------------------------------------------------------------- 리포트 계산

struct Reports<'a> {
    pid: i32,
    c: Clock,
    attribution: &'a AttributionSettingsDto,
}

/// 기간 트래픽 합계
struct Totals {
    days: usize,
    sessions: u64,
    visitors: u64,
    pageviews: u64,
    events: u64,
    channel_sessions: [u64; 9],
    channel_pv: [u64; 9],
    path_pv: Vec<u64>,
    path_entrances: Vec<u64>,
    event_counts: [u64; 10],
}

impl Reports<'_> {
    fn days(&self, from: Day, to: Day) -> impl Iterator<Item = Day> {
        let lo = from.max(self.c.today.add(-(data::HISTORY_DAYS - 1)));
        let hi = to.min(self.c.today);
        (lo.0..=hi.0).map(Day)
    }

    fn totals(&self, from: Day, to: Day) -> Totals {
        let paths = data::profile(self.pid).map_or(0, |p| p.paths.len());
        let mut t = Totals {
            days: 0,
            sessions: 0,
            visitors: 0,
            pageviews: 0,
            events: 0,
            channel_sessions: [0; 9],
            channel_pv: [0; 9],
            path_pv: vec![0; paths],
            path_entrances: vec![0; paths],
            event_counts: [0; 10],
        };
        for d in self.days(from, to) {
            let x = data::day_traffic(self.pid, d, self.c);
            if x.sessions == 0 {
                continue;
            }
            t.days += 1;
            t.sessions += x.sessions;
            t.visitors += x.visitors;
            t.pageviews += x.pageviews;
            t.events += x.total_events();
            for i in 0..9 {
                t.channel_sessions[i] += x.channel_sessions[i];
                t.channel_pv[i] += x.channel_pv[i];
            }
            for i in 0..paths {
                t.path_pv[i] += x.path_pv[i];
                t.path_entrances[i] += x.path_entrances[i];
            }
            for i in 0..10 {
                t.event_counts[i] += x.events[i];
            }
        }
        t
    }

    fn metrics(&self, from: Day, to: Day) -> OverviewMetrics {
        let t = self.totals(from, to);
        let orders: Vec<Order> = data::orders_in(self.pid, from, to, self.c)
            .into_iter()
            .filter(|o| !o.cancelled())
            .collect();
        OverviewMetrics {
            visitors: data::uniq(t.visitors, t.days),
            sessions: t.sessions,
            pageviews: t.pageviews,
            entrances: t.sessions,
            events: t.events,
            revenue: orders.iter().map(Order::effective).sum(),
            orders: orders.len() as u64,
        }
    }

    fn timeseries(&self, from: Day, to: Day) -> TimeseriesReport {
        TimeseriesReport {
            range: rr(from, to),
            points: (from.0..=to.0)
                .map(Day)
                .map(|d| {
                    let x = data::day_traffic(self.pid, d, self.c);
                    TimeseriesPoint {
                        date: d.iso(),
                        visitors: x.visitors,
                        sessions: x.sessions,
                        pageviews: x.pageviews,
                    }
                })
                .collect(),
        }
    }

    fn channels(&self, from: Day, to: Day) -> Vec<ChannelTrafficRow> {
        let t = self.totals(from, to);
        let mut rows: Vec<ChannelTrafficRow> = (0..9)
            .filter(|i| t.channel_sessions[*i] > 0)
            .map(|i| {
                let s = t.channel_sessions[i];
                ChannelTrafficRow {
                    channel: CHANNELS[i].into(),
                    sessions: s,
                    entrances: s,
                    visitors: data::uniq((s as f64 * 0.82).round() as u64, t.days),
                    pageviews: t.channel_pv[i],
                }
            })
            .collect();
        rows.sort_by(|a, b| b.sessions.cmp(&a.sessions).then(a.channel.cmp(&b.channel)));
        rows
    }

    fn pages(&self, from: Day, to: Day, needle: Option<&str>, limit: usize) -> Vec<PageRow> {
        let Some(p) = data::profile(self.pid) else {
            return vec![];
        };
        let t = self.totals(from, to);
        let mut rows: Vec<PageRow> = p
            .paths
            .iter()
            .enumerate()
            .filter(|(i, _)| t.path_pv[*i] > 0 || t.path_entrances[*i] > 0)
            .filter(|(_, x)| needle.is_none_or(|n| x.path.contains(n)))
            .map(|(i, x)| PageRow {
                path: x.path.into(),
                pageviews: t.path_pv[i],
                visitors: data::uniq((t.path_pv[i] as f64 * 0.62).round() as u64, t.days),
                entrances: t.path_entrances[i],
            })
            .collect();
        rows.sort_by(|a, b| b.pageviews.cmp(&a.pageviews).then(a.path.cmp(&b.path)));
        rows.truncate(limit);
        rows
    }

    fn events(&self, from: Day, to: Day, limit: usize) -> Vec<EventCountRow> {
        let t = self.totals(from, to);
        let mut rows: Vec<EventCountRow> = EVENTS
            .iter()
            .enumerate()
            .filter(|(i, _)| t.event_counts[*i] > 0)
            .map(|(i, name)| {
                let n = t.event_counts[i];
                EventCountRow {
                    event_name: (*name).into(),
                    count: n,
                    visitors: data::uniq(
                        (n as f64 * data::event_visitor_ratio(name)).round() as u64,
                        t.days,
                    )
                    .min(n),
                }
            })
            .collect();
        rows.sort_by(|a, b| b.count.cmp(&a.count).then(a.event_name.cmp(&b.event_name)));
        rows.truncate(limit);
        rows
    }

    /// 이벤트 속성 값 분포 (합 = 이벤트 수)
    fn properties(
        &self,
        from: Day,
        to: Day,
        event: &str,
        key: &str,
        limit: usize,
    ) -> Vec<PropertyValueRow> {
        let Some(p) = data::profile(self.pid) else {
            return vec![];
        };
        let Some(idx) = EVENTS.iter().position(|e| *e == event) else {
            return vec![];
        };
        let owned = |v: &[(&str, f64)]| -> Vec<(String, f64)> {
            v.iter().map(|(a, b)| (a.to_string(), *b)).collect()
        };
        let paths = || -> Vec<(String, f64)> {
            p.paths.iter().map(|x| (x.path.to_string(), x.pv)).collect()
        };
        let items = |f: &dyn Fn(&data::Item) -> String| -> Vec<(String, f64)> {
            p.items
                .iter()
                .enumerate()
                .map(|(i, it)| (f(it), 1.0 / (i as f64 + 1.0)))
                .collect()
        };
        let values: Vec<(String, f64)> = match (event, key) {
            ("page_view", "path") | ("scroll", "path") => paths(),
            ("page_view", "title") => p
                .paths
                .iter()
                .map(|x| (x.title.to_string(), x.pv))
                .collect(),
            ("page_view", "referrer") => owned(&[
                ("", 0.38),
                ("m.search.naver.com", 0.24),
                ("www.google.com", 0.13),
                ("m.facebook.com", 0.08),
                ("www.instagram.com", 0.07),
                ("search.daum.net", 0.05),
                ("blog.naver.com", 0.05),
            ]),
            ("scroll", "percent") => owned(&[("25", 0.4), ("50", 0.28), ("75", 0.2), ("90", 0.12)]),
            ("outbound_click", "url") => owned(&[
                ("https://www.instagram.com/binarycube_demo", 0.5),
                ("https://pf.kakao.com/_binarycube_demo", 0.3),
                ("https://blog.naver.com/binarycube_demo", 0.2),
            ]),
            ("outbound_click", "domain") => owned(&[
                ("www.instagram.com", 0.5),
                ("pf.kakao.com", 0.3),
                ("blog.naver.com", 0.2),
            ]),
            ("coupon_download", "value") => {
                owned(&[("10%", 0.45), ("5000", 0.35), ("무료배송", 0.2)])
            }
            ("coupon_download", "label") => owned(&[
                ("BF2026_10PCT", 0.45),
                ("WELCOME_5000", 0.35),
                ("FREESHIP", 0.2),
            ]),
            ("coupon_download", "category") => owned(&[("블랙프라이데이", 0.6), ("웰컴", 0.4)]),
            ("signup_complete", "label") => {
                owned(&[("kakao", 0.48), ("naver", 0.32), ("email", 0.2)])
            }
            ("signup_complete", "value") => owned(&[("1", 1.0)]),
            ("signup_complete", "category") => owned(&[("회원가입", 1.0)]),
            (e, k) if analytics_core::event::names::is_ecommerce(e) => match k {
                "item_id" => items(&|it| it.id.to_string()),
                "item_name" => items(&|it| it.name.to_string()),
                "value" => items(&|it| it.price.to_string()),
                "currency" => owned(&[("KRW", 1.0)]),
                _ => vec![],
            },
            _ => vec![],
        };
        if values.is_empty() {
            return vec![];
        }
        let t = self.totals(from, to);
        let total = t.event_counts[idx];
        let weights: Vec<f64> = values.iter().map(|(_, w)| *w).collect();
        let ratio = data::event_visitor_ratio(event);
        let mut rows: Vec<PropertyValueRow> = values
            .into_iter()
            .zip(data::split(total, &weights))
            .filter(|(_, n)| *n > 0)
            .map(|((value, _), n)| PropertyValueRow {
                value,
                count: n,
                visitors: data::uniq((n as f64 * ratio).round() as u64, t.days).min(n),
            })
            .collect();
        rows.sort_by(|a, b| b.count.cmp(&a.count).then(a.value.cmp(&b.value)));
        rows.truncate(limit);
        rows
    }

    fn funnel(&self, from: Day, to: Day) -> FunnelReport {
        let t = self.totals(from, to);
        let mut prev: Option<u64> = None;
        let mut first = 0u64;
        let steps = ["view_item", "add_to_cart", "begin_checkout", "purchase"]
            .iter()
            .map(|name| {
                let i = EVENTS.iter().position(|e| e == name).unwrap_or(0);
                let raw =
                    (t.event_counts[i] as f64 * data::event_visitor_ratio(name)).round() as u64;
                let v = prev.map_or(raw, |p| raw.min(p));
                if prev.is_none() {
                    first = v;
                }
                let step = FunnelStep {
                    event_name: (*name).into(),
                    visitors: v,
                    rate_from_prev: match prev {
                        None => 1.0,
                        Some(p) => ratio(v as f64, p as f64),
                    },
                    rate_from_first: if prev.is_none() {
                        1.0
                    } else {
                        ratio(v as f64, first as f64)
                    },
                };
                prev = Some(v);
                step
            })
            .collect();
        FunnelReport {
            range: rr(from, to),
            steps,
        }
    }

    fn realtime(&self) -> RealtimeReport {
        let now = self.c.now_ms;
        let cur = now.div_euclid(data::MINUTE_MS) * data::MINUTE_MS;
        let per_minute: Vec<RealtimeMinute> = (0..30)
            .map(|i| {
                let start = cur - (29 - i) * data::MINUTE_MS;
                RealtimeMinute {
                    minute: hhmm(start),
                    pageviews: data::minute_pageviews(self.pid, start, self.c),
                }
            })
            .collect();
        let pv30: u64 = per_minute.iter().map(|m| m.pageviews).sum();
        let pv5: u64 = per_minute[25..].iter().map(|m| m.pageviews).sum();
        let active_30m = (pv30 as f64 / 2.4).round() as u64;
        let active_5m = ((pv5 as f64 / 1.7).round() as u64).min(active_30m);
        let top_pages = match data::profile(self.pid) {
            None => vec![],
            Some(p) => {
                let w: Vec<f64> = p.paths.iter().map(|x| x.pv).collect();
                let mut rows: Vec<RealtimePage> = p
                    .paths
                    .iter()
                    .zip(data::split(pv30, &w))
                    .filter(|(_, n)| *n > 0)
                    .map(|(x, n)| RealtimePage {
                        path: x.path.into(),
                        pageviews: n,
                    })
                    .collect();
                rows.sort_by(|a, b| b.pageviews.cmp(&a.pageviews).then(a.path.cmp(&b.path)));
                rows.truncate(10);
                rows
            }
        };
        RealtimeReport {
            now: kst_iso(now),
            active_5m,
            active_30m,
            per_minute,
            top_pages,
        }
    }

    // ---------------------------------------------------------------- 기여

    fn lookback(&self, q: &Query) -> Result<u8, Resp> {
        let list = &self.attribution.lookback_days;
        let lb = match q.get("lookback") {
            None => list.first().copied(),
            Some(v) => v
                .parse::<i16>()
                .ok()
                .and_then(|d| list.iter().copied().find(|x| *x == d)),
        };
        lb.and_then(|d| u8::try_from(d).ok()).ok_or_else(|| {
            let l: Vec<String> = list.iter().map(|d| d.to_string()).collect();
            invalid(format!(
                "lookback: 프로젝트에 설정된 값({}일) 중 하나여야 합니다",
                l.join(", ")
            ))
        })
    }

    fn params(&self, lb: u8) -> Params {
        Params {
            lookback_ms: lb as i64 * DAY_MS,
            half_life_ms: self.attribution.half_life_hours as i64 * HOUR_MS,
        }
    }

    /// lookback 안의 접점 (기여 설정의 자연 유입·외부 사이트 포함 여부 반영)
    fn touches(&self, o: &Order, lb: u8) -> Vec<Touch> {
        let a = self.attribution;
        let kept: Vec<Touch> = o
            .touches
            .iter()
            .filter(|t| {
                let organic = t.channel == "organic_search" || t.channel == "organic_social";
                (a.include_organic || !organic) && (a.include_referral || t.channel != "referral")
            })
            .cloned()
            .collect();
        attr::within_lookback(kept, o.ts_ms, lb as i64 * DAY_MS)
    }

    fn shares(&self, o: &Order, lb: u8, m: Model) -> Vec<attr::Share> {
        let t = self.touches(o, lb);
        attr::attribute(m, o.effective(), o.ts_ms, &t, &self.params(lb))
    }

    fn revenue(
        &self,
        from: Day,
        to: Day,
        model: Model,
        lb: u8,
        spend: &BTreeMap<String, i64>,
    ) -> RevenueReport {
        // channel → (raw_claimed, attributed, assists)
        let mut acc: BTreeMap<String, (i64, i64, u64)> = BTreeMap::new();
        let mut orders = 0u64;
        for o in data::orders_in(self.pid, from, to, self.c) {
            if o.cancelled() {
                continue;
            }
            orders += 1;
            for s in self.shares(&o, lb, Model::RawSum) {
                acc.entry(s.channel).or_default().0 += s.amount;
            }
            for s in self.shares(&o, lb, model) {
                acc.entry(s.channel).or_default().1 += s.amount;
            }
            for s in self.shares(&o, lb, Model::LastClick) {
                if s.is_assist {
                    acc.entry(s.channel).or_default().2 += 1;
                }
            }
        }
        let raw_sum: i64 = acc.values().map(|v| v.0).sum();
        let deduplicated: i64 = acc.values().map(|v| v.1).sum();
        let roas = |rev: i64, cost: i64| (cost > 0).then(|| rev as f64 / cost as f64);
        let mut channels: Vec<ChannelRevenueRow> = acc
            .iter()
            .map(|(ch, (raw, att, assists))| {
                let s = spend.get(ch).copied().unwrap_or(0);
                ChannelRevenueRow {
                    channel: ch.clone(),
                    raw_claimed: *raw,
                    attributed: *att,
                    share: ratio(*att as f64, deduplicated as f64),
                    assists: *assists,
                    spend: s,
                    roas: roas(*att, s),
                }
            })
            .collect();
        for (ch, s) in spend {
            if !acc.contains_key(ch) {
                channels.push(ChannelRevenueRow {
                    channel: ch.clone(),
                    raw_claimed: 0,
                    attributed: 0,
                    share: 0.0,
                    assists: 0,
                    spend: *s,
                    roas: roas(0, *s),
                });
            }
        }
        channels.sort_by(|a, b| {
            b.attributed
                .cmp(&a.attributed)
                .then(b.raw_claimed.cmp(&a.raw_claimed))
                .then_with(|| a.channel.cmp(&b.channel))
        });
        let spend_total: i64 = channels.iter().map(|c| c.spend).sum();
        let duplicate_amount = (raw_sum - deduplicated).max(0);
        RevenueReport {
            range: rr(from, to),
            model,
            lookback_days: lb,
            raw_sum,
            deduplicated,
            duplicate_amount,
            duplicate_rate: ratio(duplicate_amount as f64, raw_sum as f64),
            orders,
            spend_total,
            roas: roas(deduplicated, spend_total),
            channels,
        }
    }

    fn overlap(&self, from: Day, to: Day, lb: u8, limit: usize) -> OverlapReport {
        let mut paths: HashMap<Vec<String>, u64> = HashMap::new();
        let mut multi = 0u64;
        for o in data::orders_in(self.pid, from, to, self.c) {
            if o.cancelled() {
                continue;
            }
            let mut chs: Vec<String> = vec![];
            for t in self.touches(&o, lb) {
                if chs.last() != Some(&t.channel) {
                    chs.push(t.channel);
                }
            }
            let mut d = chs.clone();
            d.sort();
            d.dedup();
            if d.len() >= 2 {
                multi += 1;
                *paths.entry(chs).or_default() += 1;
            }
        }
        let mut paths: Vec<OverlapPath> = paths
            .into_iter()
            .map(|(channels, orders)| OverlapPath { channels, orders })
            .collect();
        paths.sort_by(|a, b| b.orders.cmp(&a.orders).then(a.channels.cmp(&b.channels)));
        paths.truncate(limit);
        OverlapReport {
            range: rr(from, to),
            lookback_days: lb,
            multi_channel_orders: multi,
            paths,
        }
    }

    fn last_click(&self, o: &Order, lb: u8) -> Option<String> {
        if o.cancelled() {
            return None;
        }
        let t = self.touches(o, lb);
        Some(
            t.last()
                .map(|x| x.channel.clone())
                .unwrap_or_else(|| UNATTRIBUTED.into()),
        )
    }

    fn recent_orders(
        &self,
        from: Day,
        to: Day,
        lb: u8,
        limit: usize,
        offset: usize,
    ) -> RecentOrdersReport {
        let mut all = data::orders_in(self.pid, from, to, self.c);
        all.sort_by(|a, b| b.ts_ms.cmp(&a.ts_ms).then_with(|| a.id.cmp(&b.id)));
        RecentOrdersReport {
            range: rr(from, to),
            total: all.len() as u64,
            rows: all
                .iter()
                .skip(offset)
                .take(limit)
                .map(|o| RecentOrderRow {
                    order_id: o.id.clone(),
                    ordered_at: kst_iso(o.ts_ms),
                    effective_amount: o.effective(),
                    status: o.status.into(),
                    last_click_channel: self.last_click(o, lb),
                })
                .collect(),
        }
    }

    fn order_detail(&self, id: &str, lb: u8) -> Option<OrderDetail> {
        let (day, _) = data::parse_order_id(self.pid, id, self.c)?;
        let o = data::orders_on(self.pid, day, self.c)
            .into_iter()
            .find(|o| o.id == id)?;
        let touches = self.touches(&o, lb);
        let shares = if o.cancelled() {
            vec![]
        } else {
            Model::ALL
                .into_iter()
                .map(|m| ModelShares {
                    model: m,
                    rows: self
                        .shares(&o, lb, m)
                        .into_iter()
                        .map(|s| ShareRow {
                            touch_index: s.touch_index,
                            channel: s.channel,
                            campaign: s.campaign,
                            amount: s.amount,
                            is_assist: s.is_assist,
                        })
                        .collect(),
                })
                .collect()
        };
        Some(OrderDetail {
            order: OrderInfo {
                order_id: o.id.clone(),
                source: o.source.into(),
                status: o.status.into(),
                currency: "KRW".into(),
                gross_amount: o.gross,
                net_amount: o.gross,
                refunded_amount: o.refunded,
                effective_amount: o.effective(),
                ordered_at: kst_iso(o.ts_ms),
            },
            lookback_days: lb,
            touches: touches
                .iter()
                .enumerate()
                .map(|(i, t)| OrderTouch {
                    touch_index: (i + 1) as u16,
                    ts: kst_iso(t.ts_ms),
                    channel: t.channel.clone(),
                    campaign: t.campaign.clone(),
                })
                .collect(),
            shares,
        })
    }
}

#[cfg(test)]
mod tests;
