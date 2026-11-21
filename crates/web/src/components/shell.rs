//! 공통 레이아웃: 로그인 확인(AuthGate) → 왼쪽 사이드바 + 상단 바 + 본문(Shell).

use crate::Route;
use crate::api::{self, Method};
use crate::components::brand::{BrandLogo, BrandSymbol};
use crate::components::icon::{Icon, IconName};
use crate::components::ui::{Button, Size, Skeleton, Toggle, Variant};
use crate::logic::dates::{Preset, custom_range};
use crate::logic::view::{
    DEMO_TOAST, DEMO_TOAST_KEY, DEMO_TOAST_MS, is_escape, should_show_demo_toast,
};
use crate::logic::{errors::ApiError, prefs, table::contains_ci};
use crate::platform;
use crate::state::{AppCtx, use_app};
use analytics_core::dto::admin::{MeResponse, Role};
use dioxus::prelude::*;

/// 로그인 사용자(`/v1/auth/me`)를 불러온 뒤에만 자식 화면을 그린다. 401이면 api가 로그인 화면으로 보낸다.
#[component]
pub fn AuthGate() -> Element {
    let ctx = use_app();
    let mut me = ctx.me;
    let res = use_resource(move || async move {
        let r = api::get::<MeResponse>("/v1/auth/me").await;
        if let Ok(m) = &r {
            me.set(Some(m.clone()));
        }
        r
    });
    if ctx.me.read().is_some() {
        return rsx! { Outlet::<Route> {} };
    }
    let state = res.read().clone();
    match state {
        Some(Err(e)) if e != ApiError::Unauthorized => rsx! {
            div { class: "center-page",
                div { class: "card login-card",
                    h2 { class: "card-title", "대시보드를 열 수 없어요" }
                    p { class: "page-desc", "{e.message()}" }
                    div { style: "margin-top: 16px",
                        Button { onclick: move |_| { let mut r = res; r.restart(); }, "다시 시도" }
                    }
                }
            }
        },
        _ => rsx! {
            div { class: "center-page", aria_busy: "true",
                div { class: "boot",
                    BrandSymbol { size: 48 }
                    Skeleton { width: "160px", height: "12px" }
                }
            }
        },
    }
}

#[component]
pub fn Shell() -> Element {
    let mut ctx = use_app();
    let route = use_route::<Route>();
    let mut nav_open = use_signal(|| false);
    // 화면의 프로젝트를 "최근 선택"으로 기억한다.
    let pid = route.pid();
    use_effect(use_reactive!(|pid| {
        if let Some(p) = pid
            && ctx.prefs.peek().project_id != Some(p)
        {
            ctx.update_prefs(|s| s.project_id = Some(p));
        }
    }));
    let current_pid = pid.or(ctx.prefs.read().project_id);
    let selection = {
        let me = ctx.me.read();
        me.as_ref().and_then(|m| prefs::resolve(m, current_pid))
    };
    let nav_pid = selection.as_ref().map(|s| s.project_id);
    let show_admin = {
        let me = ctx.me.read();
        me.as_ref()
            .is_some_and(|m| m.user.is_super_admin || m.orgs.iter().any(|o| o.role == Role::Admin))
    };
    // 접근할 수 없는 프로젝트 주소
    let forbidden = pid.is_some() && ctx.selection(pid.unwrap_or_default()).is_none();
    let uses_range = route.uses_range();
    // 데모: 로그인 뒤(이 브라우저 세션에서 처음) 한 번만 안내 알림을 띄운다. 상단 바에 상시 배지는 두지 않는다.
    use_effect(move || {
        let shown = platform::session_get(DEMO_TOAST_KEY);
        if should_show_demo_toast(cfg!(feature = "demo"), shown.as_deref()) {
            platform::session_set(DEMO_TOAST_KEY, "1");
            ctx.toast.with_mut(|t| {
                t.show_for(DEMO_TOAST.into(), DEMO_TOAST_MS);
            });
        }
    });

    rsx! {
        div { class: "shell",
            a { class: "skip-link", href: "#main", "본문으로 건너뛰기" }
            if nav_open() {
                div { class: "nav-backdrop", onclick: move |_| nav_open.set(false) }
            }
            aside { class: if nav_open() { "sidebar open" } else { "sidebar" }, aria_label: "메뉴",
                BrandLogo {}
                ProjectSwitcher { current: nav_pid }
                nav { class: "nav", onclick: move |_| nav_open.set(false),
                    for sec in nav_sections(nav_pid, show_admin) {
                        div { key: "{sec.title}", class: "nav-section", "{sec.title}" }
                        for it in sec.items {
                            NavLink { key: "{it.label}", to: it.to, label: it.label, icon: it.icon }
                        }
                    }
                }
                UserBlock {}
            }
            div { class: "main",
                header { class: if uses_range { "topbar has-range" } else { "topbar" },
                    div { class: "topbar-head",
                        button {
                            class: "icon-btn menu-btn",
                            aria_label: "메뉴 열기",
                            onclick: move |_| nav_open.toggle(),
                            Icon { name: IconName::Menu }
                        }
                        // 기간 선택이 있는 화면에서는 모바일에서만 보인다 (데스크톱은 사이드바에 프로젝트가 있다)
                        div { class: "topbar-title",
                            if let Some(s) = &selection {
                                span { class: "muted topbar-org", "{s.org_name} · " }
                                span { class: "topbar-project", "{s.project_name}" }
                            }
                        }
                    }
                    if uses_range {
                        RangePicker {}
                    }
                }
                main { id: "main", class: "content",
                    if forbidden {
                        crate::components::ui::EmptyState {
                            icon: crate::components::icon::IconName::Lock,
                            title: "프로젝트를 열 수 없어요",
                            desc: "없는 프로젝트이거나 접근 권한이 없습니다. 담당 관리자에게 문의하세요.",
                            action: rsx! {
                                Link { to: Route::Home {}, class: "btn btn-secondary", "내 프로젝트로 이동" }
                            },
                        }
                    } else {
                        Outlet::<Route> {}
                    }
                }
            }
        }
    }
}

/// 사이드바 메뉴 한 구역.
#[derive(Clone, PartialEq, Debug)]
pub struct NavSection {
    pub title: &'static str,
    pub items: Vec<NavItem>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct NavItem {
    pub to: Route,
    pub label: &'static str,
    pub icon: IconName,
}

/// 사이드바 메뉴: 프로젝트가 있으면 리포트·설정, 관리자면 관리.
pub fn nav_sections(pid: Option<i32>, show_admin: bool) -> Vec<NavSection> {
    let item = |to, label, icon| NavItem { to, label, icon };
    let mut out = vec![];
    if let Some(p) = pid {
        out.push(NavSection {
            title: "리포트",
            items: vec![
                item(Route::Overview { pid: p }, "개요", IconName::Overview),
                item(Route::Realtime { pid: p }, "실시간", IconName::Realtime),
                item(Route::Acquisition { pid: p }, "유입", IconName::Acquisition),
                item(Route::Pages { pid: p }, "페이지", IconName::Pages),
                item(Route::Events { pid: p }, "이벤트", IconName::Events),
                item(Route::Funnel { pid: p }, "전자상거래", IconName::Funnel),
                item(Route::Revenue { pid: p }, "실제 매출", IconName::Revenue),
            ],
        });
        out.push(NavSection {
            title: "설정",
            items: vec![item(
                Route::Settings {
                    pid: p,
                    tab: String::new(),
                },
                "설치·수집 설정",
                IconName::Settings,
            )],
        });
    }
    if show_admin {
        out.push(NavSection {
            title: "관리",
            items: vec![item(Route::Admin {}, "광고주·사용자", IconName::Admin)],
        });
    }
    // 컴포넌트 모음(/dev/components)은 개발용이라 메뉴에 두지 않는다 (주소로만 연다).
    out
}

#[component]
fn NavLink(to: Route, label: String, icon: IconName) -> Element {
    rsx! {
        Link { to, active_class: "active",
            span { class: "nav-icon", Icon { name: icon } }
            "{label}"
        }
    }
}

/// 광고주·프로젝트 전환기.
#[component]
fn ProjectSwitcher(current: Option<i32>) -> Element {
    let ctx = use_app();
    let route = use_route::<Route>();
    let mut open = use_signal(|| false);
    let mut q = use_signal(String::new);
    let (tree, label) = {
        let me = ctx.me.read();
        let Some(m) = me.as_ref() else {
            return rsx! {};
        };
        let sel = prefs::resolve(m, current);
        (
            prefs::project_tree(m),
            sel.map(|s| (s.org_name, s.project_name)),
        )
    };
    let total: usize = tree.iter().map(|(_, ps)| ps.len()).sum();
    rsx! {
        // 바깥을 누르면 닫히도록 투명한 덮개를 깐다 (팝오버보다 아래, 나머지 화면보다 위).
        if open() {
            div { class: "pop-backdrop", onclick: move |_| open.set(false) }
        }
        div {
            class: "project-switch-wrap",
            onkeydown: move |e| {
                if open() && is_escape(&e.key()) {
                    e.stop_propagation();
                    open.set(false);
                }
            },
            button {
                class: "project-switch",
                aria_haspopup: "listbox",
                aria_expanded: if open() { "true" } else { "false" },
                onclick: move |_| open.toggle(),
                if let Some((org, name)) = label {
                    span { class: "org", "{org}" }
                    span { class: "name", "{name}" }
                } else {
                    span { class: "org", "프로젝트" }
                    span { class: "name", "선택된 프로젝트 없음" }
                }
                span { class: "chev", aria_hidden: "true", "⌄" }
            }
            if open() {
                div { class: "popover project-pop", role: "listbox",
                    if total > 6 {
                        input {
                            class: "input input-sm",
                            placeholder: "광고주·프로젝트 검색",
                            value: "{q}",
                            oninput: move |e| q.set(e.value()),
                        }
                    }
                    if total == 0 {
                        div { class: "muted pop-empty", "접근 가능한 프로젝트가 없습니다" }
                    }
                    for (org, projects) in tree.into_iter().filter(|(org, ps)| ps.iter().any(|(_, n, _)| contains_ci(n, &q()) || contains_ci(org, &q()))) {
                            div { key: "{org}", class: "pop-group",
                                div { class: "pop-org", "{org}" }
                                for (id, name, paused) in projects.into_iter().filter(|(_, n, _)| contains_ci(n, &q()) || contains_ci(&org, &q())) {
                                    button {
                                        key: "{id}",
                                        class: if Some(id) == current { "pop-item on" } else { "pop-item" },
                                        role: "option",
                                        aria_selected: Some(id) == current,
                                        onclick: {
                                            let route = route.clone();
                                            move |_| {
                                                open.set(false);
                                                navigator().push(route.with_pid(id));
                                            }
                                        },
                                        "{name}"
                                        if paused {
                                            span { class: "badge badge-neutral", "일시정지" }
                                        }
                                    }
                                }
                            }
                    }
                }
            }
        }
    }
}

/// 사용자 정보 + 로그아웃.
#[component]
fn UserBlock() -> Element {
    let ctx = use_app();
    let me = ctx.me.read().clone();
    let Some(m) = me else {
        return rsx! {};
    };
    let display = if m.user.name.trim().is_empty() {
        m.user.email.clone()
    } else {
        m.user.name.clone()
    };
    let initial: String = display
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    rsx! {
        div { class: "user-block",
            div { class: "avatar", aria_hidden: "true", "{initial}" }
            div { class: "user-meta",
                div { class: "user-name", "{display}" }
                div { class: "user-sub",
                    if m.user.is_super_admin { "대행사 관리자" } else { "{m.user.email}" }
                }
            }
            button {
                class: "icon-btn",
                title: "로그아웃",
                aria_label: "로그아웃",
                onclick: move |_| {
                    let mut ctx = ctx;
                    spawn(async move {
                        let _ = api::send_empty(Method::Post, "/v1/auth/logout").await;
                        ctx.me.set(None);
                        navigator().replace(Route::Login { next: String::new() });
                    });
                },
                Icon { name: IconName::Logout, size: 18 }
            }
        }
    }
}

/// 상단 바 기간 선택 + 비교 토글.
#[component]
fn RangePicker() -> Element {
    let mut ctx = use_app();
    let prefs = ctx.prefs.read().clone();
    let today = AppCtx::today();
    let range = prefs.range(today);
    let mut custom_open = use_signal(|| false);
    let mut from = use_signal(|| range.from.iso());
    let mut to = use_signal(|| range.to.iso());
    let mut err = use_signal(|| None::<&'static str>);
    rsx! {
        div { class: "range-picker",
            div { class: "segmented", role: "radiogroup", aria_label: "기간",
                for p in Preset::ALL {
                    button {
                        key: "{p.label()}",
                        r#type: "button",
                        role: "radio",
                        class: if prefs.preset == p { "on" } else { "" },
                        aria_checked: if prefs.preset == p { "true" } else { "false" },
                        onclick: move |_| {
                            if p == Preset::Custom {
                                let r = ctx.range();
                                from.set(r.from.iso());
                                to.set(r.to.iso());
                                err.set(None);
                                custom_open.toggle();
                            } else {
                                custom_open.set(false);
                                ctx.update_prefs(|s| s.preset = p);
                            }
                        },
                        "{p.label()}"
                    }
                }
            }
            span { class: "range-label", "{range.label()}" }
            label { class: "compare",
                Toggle {
                    on: prefs.compare,
                    label: "이전 기간과 비교",
                    onchange: move |v| ctx.update_prefs(|s| s.compare = v),
                }
                span { class: "compare-long", "이전 기간 비교" }
                span { class: "compare-short", aria_hidden: "true", "비교" }
            }
            if custom_open() {
                div { class: "popover range-pop",
                    div { class: "row",
                        input { class: "input input-sm", r#type: "date", aria_label: "시작일", value: "{from}", max: "{today.iso()}", oninput: move |e| from.set(e.value()) }
                        span { "–" }
                        input { class: "input input-sm", r#type: "date", aria_label: "종료일", value: "{to}", max: "{today.iso()}", oninput: move |e| to.set(e.value()) }
                    }
                    if let Some(e) = err() {
                        div { class: "error-text-sm", role: "alert", "{e}" }
                    }
                    div { class: "row", style: "justify-content: flex-end",
                        Button { variant: Variant::Ghost, size: Size::Sm, onclick: move |_| custom_open.set(false), "취소" }
                        Button {
                            size: Size::Sm,
                            onclick: move |_| match custom_range(&from(), &to(), AppCtx::today()) {
                                Ok(r) => {
                                    ctx.update_prefs(|s| {
                                        s.preset = Preset::Custom;
                                        s.custom_from = Some(r.from.iso());
                                        s.custom_to = Some(r.to.iso());
                                    });
                                    custom_open.set(false);
                                }
                                Err(e) => err.set(Some(e)),
                            },
                            "적용"
                        }
                    }
                }
            }
        }
    }
}
