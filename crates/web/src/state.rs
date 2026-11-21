//! 앱 전역 상태: 로그인 사용자, 화면 설정(기간·비교·선택 프로젝트), 알림.

use crate::api::ApiResult;
use crate::logic::dates::{DateRange, Day};
use crate::logic::errors::ApiError;
use crate::logic::prefs::{self, Prefs, Selection};
use crate::logic::view::ToastState;
use crate::platform;
use analytics_core::dto::admin::{MeResponse, Role};
use dioxus::prelude::*;

#[derive(Clone, Copy)]
pub struct AppCtx {
    pub me: Signal<Option<MeResponse>>,
    pub prefs: Signal<Prefs>,
    pub toast: Signal<ToastState>,
    /// 기간·비교는 설정 전체가 아니라 각자 값이 바뀔 때만 알린다.
    /// (비교 토글·최근 프로젝트 기록이 모든 리포트를 다시 부르지 않게)
    range: Memo<DateRange>,
    compare: Memo<bool>,
}

impl AppCtx {
    pub fn provide() -> AppCtx {
        let prefs =
            use_signal(|| Prefs::decode(platform::storage_get(prefs::STORAGE_KEY).as_deref()));
        let range = use_memo(move || prefs.read().range(Self::today()));
        let compare = use_memo(move || prefs.read().compare);
        let me = use_signal(|| None);
        let toast = use_signal(ToastState::default);
        use_context_provider(|| AppCtx {
            me,
            prefs,
            toast,
            range,
            compare,
        })
    }

    pub fn today() -> Day {
        Day::seoul_today(platform::now_ms())
    }

    /// 현재 기간 (구독: 기간이 바뀔 때만 다시 읽는 곳이 갱신된다).
    pub fn range(&self) -> DateRange {
        (self.range)()
    }

    pub fn compare(&self) -> bool {
        (self.compare)()
    }

    pub fn update_prefs(&mut self, f: impl FnOnce(&mut Prefs)) {
        let mut p = self.prefs.write();
        f(&mut p);
        platform::storage_set(prefs::STORAGE_KEY, &p.encode());
    }

    pub fn selection(&self, pid: i32) -> Option<Selection> {
        let me = self.me.read();
        prefs::resolve(me.as_ref()?, Some(pid)).filter(|s| s.project_id == pid)
    }

    /// 프로젝트에서의 내 역할 (슈퍼 관리자는 admin)
    pub fn role(&self, pid: i32) -> Option<Role> {
        self.selection(pid).map(|s| s.role)
    }

    pub fn can_edit(&self, pid: i32) -> bool {
        self.role(pid).is_some_and(|r| r >= Role::Analyst)
    }

    pub fn is_super_admin(&self) -> bool {
        self.me
            .read()
            .as_ref()
            .is_some_and(|m| m.user.is_super_admin)
    }

    pub fn notify(&mut self, msg: impl Into<String>) {
        self.toast.with_mut(|t| {
            t.show(msg.into());
        });
    }

    /// 변경 요청 결과를 알림으로 보여주고 성공 여부를 돌려준다.
    pub fn report<T>(&mut self, r: &ApiResult<T>, ok_msg: &str) -> bool {
        match r {
            Ok(_) => {
                self.notify(ok_msg);
                true
            }
            Err(ApiError::Unauthorized) => false,
            Err(e) => {
                self.notify(e.message());
                false
            }
        }
    }
}

pub fn use_app() -> AppCtx {
    use_context::<AppCtx>()
}
