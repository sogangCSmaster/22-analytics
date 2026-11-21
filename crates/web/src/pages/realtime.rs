//! 실시간: 최근 5분·30분 활성 방문자, 분당 페이지뷰, 지금 많이 보는 페이지. 30초마다 갱신.

use crate::api;
use crate::components::chart::{BarList, ColumnChart};
use crate::components::ui::{Card, EmptyState, ErrorBox, PageHead, SkeletonBlock, SkeletonCards};
use crate::format;
use crate::logic::view::{Poller, scoped};
use crate::logic::{dates, query};
use crate::platform::{document_hidden, sleep_ms};
use analytics_core::dto::reports::RealtimeReport;
use dioxus::prelude::*;

pub const REFRESH_MS: u32 = 30_000;
const POLL_TICK_MS: u32 = 1_000;

#[component]
pub fn Realtime(pid: ReadSignal<i32>) -> Element {
    // 이전 결과를 유지한 채 다시 불러와 화면이 깜빡이지 않게 한다.
    // 결과는 프로젝트와 함께 기억해, 프로젝트를 바꾸면 이전 프로젝트 숫자를 보여주지 않는다.
    let mut last = use_signal(|| None::<(i32, RealtimeReport)>);
    let res = use_resource(move || {
        let p = pid();
        async move {
            let r = api::get::<RealtimeReport>(&query::realtime(p)).await;
            if let Ok(v) = &r {
                last.set(Some((p, v.clone())));
            }
            r
        }
    });
    // 1초마다 확인하되, 탭이 숨겨진 동안은 부르지 않고 다시 보이면 바로 새로 고친다.
    use_future(move || async move {
        let mut poller = Poller::new(REFRESH_MS);
        loop {
            sleep_ms(POLL_TICK_MS).await;
            if poller.tick(POLL_TICK_MS, document_hidden()) {
                let mut r = res;
                r.restart();
            }
        }
    });
    let err = match &*res.read() {
        Some(Err(e)) => Some(e.clone()),
        _ => None,
    };
    let data = scoped(&last(), &pid());

    rsx! {
        PageHead {
            title: "실시간",
            desc: "지금 사이트에 있는 방문자를 30초마다 새로 고쳐 보여줘요.",
            actions: rsx! {
                span { class: "live-pill", span { class: "live-dot" } "실시간" }
                if let Some(d) = &data {
                    span { class: "muted", "기준 {dates::datetime_kr(&d.now)}" }
                }
            },
        }
        if let Some(e) = err {
            ErrorBox { error: e, onretry: move |_| { let mut r = res; r.restart(); } }
        }
        match data {
            None => rsx! {
                SkeletonCards { n: 2 }
                Card { SkeletonBlock { chart: true } }
            },
            Some(d) => rsx! {
                div { class: "grid grid-2",
                    div { class: "card stat",
                        div { class: "stat-label", "최근 5분 활성 방문자" }
                        div { class: "stat-value xl", "{format::count(d.active_5m)}" }
                        div { class: "stat-sub", "지금 이 순간에 가까운 수치" }
                    }
                    div { class: "card stat",
                        div { class: "stat-label", "최근 30분 활성 방문자" }
                        div { class: "stat-value", "{format::count(d.active_30m)}" }
                        div { class: "stat-sub", "최근 30분 동안 한 번이라도 활동한 방문자" }
                    }
                }
                Card { title: "분당 페이지뷰 (최근 30분)",
                    if d.per_minute.iter().all(|m| m.pageviews == 0) {
                        EmptyState { icon: crate::components::icon::IconName::Realtime, title: "최근 30분 동안 조회가 없어요", desc: "설치를 확인하려면 사이트를 새 창으로 열어 보세요. 몇 초 안에 막대가 올라옵니다." }
                    } else {
                        ColumnChart {
                            values: d.per_minute.iter().map(|m| m.pageviews as f64).collect::<Vec<_>>(),
                            labels: d.per_minute.iter().map(|m| m.minute.clone()).collect::<Vec<_>>(),
                            highlight_last: true,
                            value_name: "페이지뷰",
                            aria_label: format!("최근 30분 분당 페이지뷰, 합계 {}", format::count(d.per_minute.iter().map(|m| m.pageviews).sum())),
                        }
                    }
                }
                Card { title: "지금 많이 보는 페이지",
                    if d.top_pages.is_empty() {
                        EmptyState { icon: crate::components::icon::IconName::Pages, title: "아직 없어요", desc: "최근 30분 동안 조회된 페이지가 없습니다." }
                    } else {
                        BarList { items: d.top_pages.iter().map(|p| (format::path(&p.path), p.pageviews as i64)).collect::<Vec<_>>(), show_share: true }
                    }
                }
            },
        }
    }
}
