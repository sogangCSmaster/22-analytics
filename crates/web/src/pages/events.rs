//! 이벤트: 이벤트별 발생 수·방문자. 행을 누르면 오른쪽 패널에서 속성 값 분포를 본다.

use crate::api;
use crate::components::chart::BarList;
use crate::components::ui::{
    Badge, Card, CsvButton, Drawer, EmptyState, PageHead, SkeletonBlock, SortTh, load,
};
use crate::format;
use crate::logic::table::{Sort, cmp_text, sort_rows};
use crate::logic::view::{is_activation_key, scoped};
use crate::logic::{labels, query};
use crate::platform::sleep_ms;
use crate::state::use_app;
use analytics_core::dto::reports::{EventCountRow, PropertyValueRow};
use dioxus::prelude::*;

pub const LIMIT: u32 = 200;

/// 이벤트별 자주 쓰는 속성 이름 (추천 칩)
pub fn suggested_props(event: &str) -> &'static [&'static str] {
    match event {
        "page_view" => &["path", "title", "referrer"],
        "outbound_click" => &["url", "domain"],
        "file_download" => &["url", "file_ext"],
        "scroll" => &["percent", "path"],
        e if analytics_core::event::names::is_ecommerce(e) => {
            &["item_id", "item_name", "currency", "value"]
        }
        _ => &["value", "label", "category"],
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Col {
    Name,
    Count,
    Visitors,
}

#[component]
pub fn EventsTable(
    rows: Vec<(EventCountRow, Option<EventCountRow>)>,
    compare: bool,
    sort: Signal<Sort<Col>>,
    onselect: EventHandler<String>,
) -> Element {
    let _ = (rows, compare, sort, onselect);
    rsx! {}
}

#[component]
pub fn Events(pid: ReadSignal<i32>) -> Element {
    let ctx = use_app();
    let sort = use_signal(|| Sort::desc(Col::Count));
    // 고른 이벤트는 프로젝트에 묶는다 (프로젝트를 바꾸면 패널이 닫힌다)
    let mut selected = use_signal(|| None::<(i32, String)>);
    let res = use_resource(move || {
        let (p, r) = (pid(), ctx.range());
        async move { api::get::<Vec<EventCountRow>>(&query::events(p, r, LIMIT)).await }
    });
    let range = ctx.range();

    rsx! {
        PageHead { title: "이벤트", desc: format!("{} · 이벤트를 누르면 속성별 분포를 볼 수 있어요", range.label()) }
        Card {
            {load(res, rsx! { SkeletonBlock { rows: 8 } }, |mut rows| {
                if rows.is_empty() {
                    return rsx! { EmptyState { icon: crate::components::icon::IconName::Events, title: "이 기간에 받은 이벤트가 없어요", desc: "at('event', '이벤트_이름', {{ 속성 }}) 으로 원하는 행동을 기록할 수 있어요." } };
                }
                sort_rows(&mut rows, sort(), |a, b, k| match k {
                    Col::Name => cmp_text(&a.event_name, &b.event_name),
                    Col::Count => a.count.cmp(&b.count),
                    Col::Visitors => a.visitors.cmp(&b.visitors),
                });
                let csv: Vec<Vec<String>> = rows.iter().map(|r| vec![r.event_name.clone(), r.count.to_string(), r.visitors.to_string()]).collect();
                rsx! {
                    div { class: "toolbar",
                        span { class: "muted", "이벤트 {rows.len()}종" }
                        CsvButton { file_name: format!("이벤트_{}.csv", range.label()), headers: vec!["이벤트".into(), "발생 수".into(), "방문자".into()], rows: csv }
                    }
                    div { class: "table-wrap",
                        table { class: "table",
                            thead {
                                tr {
                                    SortTh { label: "이벤트", k: Col::Name, sort, right: false }
                                    SortTh { label: "발생 수", k: Col::Count, sort }
                                    SortTh { label: "방문자", k: Col::Visitors, sort }
                                    th { class: "r", scope: "col", "방문자당 횟수" }
                                }
                            }
                            tbody {
                                for r in rows.iter() {
                                    tr {
                                        key: "{r.event_name}",
                                        class: "clickable",
                                        tabindex: "0",
                                        onclick: {
                                            let n = r.event_name.clone();
                                            move |_| selected.set(Some((pid(), n.clone())))
                                        },
                                        onkeydown: {
                                            let n = r.event_name.clone();
                                            move |e: KeyboardEvent| if is_activation_key(&e.key()) {
                                                e.prevent_default();
                                                selected.set(Some((pid(), n.clone())));
                                            }
                                        },
                                        td {
                                            div { class: "cell-title mono", "{r.event_name}" }
                                            if let Some(h) = labels::event_hint(&r.event_name) {
                                                div { class: "cell-sub", "{h}" }
                                            }
                                        }
                                        td { class: "r", "{format::count(r.count)}" }
                                        td { class: "r", "{format::count(r.visitors)}" }
                                        td { class: "r muted", "{crate::pages::overview::per(r.count, r.visitors)}" }
                                    }
                                }
                            }
                        }
                    }
                }
            })}
        }
        if let Some(name) = scoped(&selected(), &pid()) {
            PropertyDrawer { pid: pid(), event: name, onclose: move |_| selected.set(None) }
        }
    }
}

#[component]
fn PropertyDrawer(
    pid: ReadSignal<i32>,
    event: ReadSignal<String>,
    onclose: EventHandler<()>,
) -> Element {
    let ctx = use_app();
    // 속성 이름은 이벤트에 묶는다: 다른 이벤트를 고르면 그 이벤트의 추천 속성으로 돌아간다.
    let mut key = use_signal(|| None::<(String, String)>);
    let event_sig = event;
    let current_key = move |ev: &str| {
        scoped(&key(), &ev.to_string()).unwrap_or_else(|| suggested_props(ev)[0].to_string())
    };
    let mut set_key = move |k: String| key.set(Some((event_sig(), k)));
    let res = use_resource(move || {
        let ev = event_sig();
        let k = current_key(&ev);
        let (p, r) = (pid(), ctx.range());
        async move {
            let k = k.trim().to_string();
            if k.is_empty() {
                return Ok(vec![]);
            }
            sleep_ms(250).await;
            api::get::<Vec<PropertyValueRow>>(&query::event_property(p, &ev, &k, r, 20)).await
        }
    });
    let event = event_sig();
    let cur_key = current_key(&event);
    rsx! {
        Drawer { open: true, title: event.clone(), onclose: move |_| onclose.call(()),
            if let Some(h) = labels::event_hint(&event) {
                div { Badge { "{h}" } }
            }
            div { class: "field",
                label {
                    "속성 이름"
                    input {
                        class: "input",
                        value: "{cur_key}",
                        placeholder: "예: item_id",
                        oninput: move |e| set_key(e.value()),
                    }
                }
                div { class: "chips",
                    for s in suggested_props(&event) {
                        button {
                            key: "{s}",
                            class: if cur_key == *s { "chip on" } else { "chip" },
                            aria_pressed: if cur_key == *s { "true" } else { "false" },
                            onclick: move |_| set_key(s.to_string()),
                            "{s}"
                        }
                    }
                }
            }
            {load(res, rsx! { SkeletonBlock { rows: 6 } }, |rows| {
                let total: u64 = rows.iter().map(|r| r.count).sum();
                rsx! {
                    if rows.is_empty() {
                        EmptyState { icon: crate::components::icon::IconName::Search, title: "값이 없어요", desc: "이 이벤트에 해당 속성이 기록되지 않았거나 이름이 다릅니다. 속성 이름을 확인해 보세요." }
                    } else {
                        div { class: "muted", "상위 {rows.len()}개 값 · 합계 {format::count(total)}회" }
                        BarList { items: rows.iter().map(|r| (if r.value.is_empty() { "(빈 값)".to_string() } else { r.value.clone() }, r.count as i64)).collect::<Vec<_>>(), show_share: true }
                        div { class: "table-wrap",
                            table { class: "table",
                                thead { tr { th { scope: "col", "값" } th { class: "r", scope: "col", "발생 수" } th { class: "r", scope: "col", "방문자" } } }
                                tbody {
                                    for (i, r) in rows.iter().enumerate() {
                                        tr { key: "{i}",
                                            td { class: "path-cell", title: "{r.value}", "{r.value}" }
                                            td { class: "r", "{format::count(r.count)}" }
                                            td { class: "r", "{format::count(r.visitors)}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            })}
        }
    }
}
