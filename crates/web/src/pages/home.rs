//! `/`: 최근 선택한 프로젝트(없으면 첫 운영 중 프로젝트)의 개요로 보낸다.

use crate::Route;
use crate::components::ui::EmptyState;
use crate::logic::prefs;
use crate::state::use_app;
use dioxus::prelude::*;

#[component]
pub fn Home() -> Element {
    let ctx = use_app();
    let (target, is_admin) = {
        let me = ctx.me.read();
        let wanted = ctx.prefs.peek().project_id;
        (
            me.as_ref().and_then(|m| prefs::resolve(m, wanted)).map(|s| s.project_id),
            me.as_ref().is_some_and(|m| m.user.is_super_admin),
        )
    };
    use_effect(move || match (target, is_admin) {
        (Some(pid), _) => {
            navigator().replace(Route::Overview { pid });
        }
        (None, true) => {
            navigator().replace(Route::Admin {});
        }
        _ => {}
    });
    if target.is_some() || is_admin {
        return rsx! {};
    }
    rsx! {
        div { class: "center-page",
            div { class: "card login-card",
                EmptyState {
                    icon: "📂",
                    title: "아직 볼 수 있는 프로젝트가 없어요",
                    desc: "광고주 관리자에게 멤버 추가를 요청하세요. 추가되면 다시 로그인하면 됩니다.",
                }
            }
        }
    }
}
