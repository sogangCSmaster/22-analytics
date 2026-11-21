//! 브라우저 localStorage에 남기는 화면 설정 (선택한 프로젝트, 기간, 비교 여부).
//!
//! 저장값은 사용자가 고칠 수 있으므로 읽을 때 항상 검증한다. 형식이 깨졌으면 기본값.

use super::dates::{DateRange, Day, Preset, custom_range, preset_range};
use analytics_core::dto::admin::{MeResponse, ProjectStatus, Role};
use serde::{Deserialize, Serialize};

pub const STORAGE_KEY: &str = "at.prefs.v1";

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub project_id: Option<i32>,
    pub preset: Preset,
    pub custom_from: Option<String>,
    pub custom_to: Option<String>,
    /// 이전 기간과 비교 표시
    pub compare: bool,
}

impl Prefs {
    pub fn decode(raw: Option<&str>) -> Prefs {
        let Some(serde_json::Value::Object(m)) = raw.and_then(|r| serde_json::from_str(r).ok())
        else {
            return Prefs::default();
        };
        // 필드마다 따로 읽어 하나가 깨져도 나머지는 살린다.
        fn field<T: serde::de::DeserializeOwned>(
            m: &serde_json::Map<String, serde_json::Value>,
            k: &str,
        ) -> Option<T> {
            m.get(k).and_then(|v| serde_json::from_value(v.clone()).ok())
        }
        Prefs {
            project_id: field(&m, "project_id"),
            preset: field(&m, "preset").unwrap_or_default(),
            custom_from: field(&m, "custom_from"),
            custom_to: field(&m, "custom_to"),
            compare: field(&m, "compare").unwrap_or(false),
        }
    }

    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// 현재 기간. 직접 선택 값이 잘못됐으면 최근 7일.
    pub fn range(&self, today: Day) -> DateRange {
        let fallback = || preset_range(Preset::Days7, today).expect("7일 프리셋");
        match self.preset {
            Preset::Custom => match (&self.custom_from, &self.custom_to) {
                (Some(f), Some(t)) => custom_range(f, t, today).unwrap_or_else(|_| fallback()),
                _ => fallback(),
            },
            p => preset_range(p, today).unwrap_or_else(fallback),
        }
    }
}

/// 프로젝트 하나와 그 광고주.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub org_id: i64,
    pub org_name: String,
    pub project_id: i32,
    pub project_name: String,
    pub role: Role,
}

/// 원하는 프로젝트가 접근 가능하면 그것, 아니면 첫 번째 운영 중 프로젝트, 없으면 아무 프로젝트.
pub fn resolve(me: &MeResponse, wanted: Option<i32>) -> Option<Selection> {
    let pick = |want: &dyn Fn(&analytics_core::dto::admin::ProjectSummary) -> bool| {
        me.orgs.iter().find_map(|o| {
            o.projects.iter().find(|p| want(p)).map(|p| Selection {
                org_id: o.id,
                org_name: o.name.clone(),
                project_id: p.id,
                project_name: p.name.clone(),
                role: o.role,
            })
        })
    };
    wanted
        .and_then(|id| pick(&|p| p.id == id))
        .or_else(|| pick(&|p| p.status == ProjectStatus::Active))
        .or_else(|| pick(&|_| true))
}

/// 사이드바 전환기용: (광고주, [(프로젝트 id, 이름, 일시정지 여부)])
pub fn project_tree(me: &MeResponse) -> Vec<(String, Vec<(i32, String, bool)>)> {
    me.orgs
        .iter()
        .map(|o| {
            (
                o.name.clone(),
                o.projects
                    .iter()
                    .map(|p| (p.id, p.name.clone(), p.status == ProjectStatus::Paused))
                    .collect(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use analytics_core::dto::admin::{OrgAccess, ProjectSummary, UserDto};

    fn me() -> MeResponse {
        let p = |id, name: &str, status| ProjectSummary { id, name: name.into(), status };
        MeResponse {
            user: UserDto {
                id: 1,
                email: "a@b.co".into(),
                name: "관리자".into(),
                is_super_admin: false,
                created_at: "2026-01-01T00:00:00Z".into(),
                last_login_at: None,
            },
            orgs: vec![
                OrgAccess {
                    id: 10,
                    name: "광고주A".into(),
                    role: Role::Viewer,
                    projects: vec![p(1, "정지됨", ProjectStatus::Paused), p(2, "운영", ProjectStatus::Active)],
                },
                OrgAccess {
                    id: 20,
                    name: "광고주B".into(),
                    role: Role::Analyst,
                    projects: vec![p(5, "B사이트", ProjectStatus::Active)],
                },
            ],
        }
    }

    #[test]
    fn roundtrip() {
        let p = Prefs {
            project_id: Some(5),
            preset: Preset::Custom,
            custom_from: Some("2026-11-01".into()),
            custom_to: Some("2026-11-10".into()),
            compare: true,
        };
        assert_eq!(Prefs::decode(Some(&p.encode())), p);
    }

    #[test]
    fn decode_is_tolerant() {
        assert_eq!(Prefs::decode(None), Prefs::default());
        assert_eq!(Prefs::decode(Some("")), Prefs::default());
        assert_eq!(Prefs::decode(Some("{not json")), Prefs::default());
        assert_eq!(Prefs::decode(Some("[]")), Prefs::default());
        // 모르는 필드·잘못된 값은 무시하고 나머지는 살린다
        let p = Prefs::decode(Some(r#"{"project_id":7,"preset":"days30","extra":1}"#));
        assert_eq!((p.project_id, p.preset), (Some(7), Preset::Days30));
        let p = Prefs::decode(Some(r#"{"project_id":"x","preset":"forever","compare":true}"#));
        assert_eq!(p, Prefs { compare: true, ..Prefs::default() });
    }

    #[test]
    fn range_falls_back_on_bad_custom() {
        let today = Day::parse("2026-11-21").unwrap();
        let mut p = Prefs { preset: Preset::Custom, ..Prefs::default() };
        assert_eq!(p.range(today).days(), 7);
        p.custom_from = Some("2026-11-01".into());
        p.custom_to = Some("2026-11-03".into());
        assert_eq!(p.range(today).days(), 3);
        p.custom_to = Some("2099-01-01".into());
        assert_eq!(p.range(today).days(), 7);
    }

    #[test]
    fn resolves_selection() {
        let m = me();
        let s = resolve(&m, Some(5)).unwrap();
        assert_eq!((s.org_id, s.project_id, s.role), (20, 5, Role::Analyst));
        // 접근 불가 → 첫 운영 중 프로젝트
        assert_eq!(resolve(&m, Some(999)).unwrap().project_id, 2);
        assert_eq!(resolve(&m, None).unwrap().project_id, 2);
        // 일시정지 프로젝트도 명시하면 선택 가능
        assert_eq!(resolve(&m, Some(1)).unwrap().project_name, "정지됨");
        let empty = MeResponse { orgs: vec![], ..m };
        assert_eq!(resolve(&empty, Some(1)), None);
    }

    #[test]
    fn only_paused_projects() {
        let mut m = me();
        m.orgs.truncate(1);
        m.orgs[0].projects.remove(1);
        assert_eq!(resolve(&m, None).unwrap().project_id, 1);
    }
}
