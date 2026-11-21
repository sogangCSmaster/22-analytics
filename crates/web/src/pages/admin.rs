//! 관리: 광고주(정보·멤버·프로젝트)와 사용자. 사용자·광고주 만들기는 대행사 관리자만.

use crate::api::{self, Method};
use crate::components::ui::{
    Badge, Button, Card, ConfirmButton, Drawer, EmptyState, PageHead, SegmentedControl, Size,
    SkeletonBlock, TextArea, TextField, Tone, Variant, load,
};
use crate::logic::table::contains_ci;
use crate::logic::{dates, labels, query, validate};
use crate::state::use_app;
use analytics_core::dto::admin::{
    AddMemberRequest, CreateOrgRequest, CreateProjectRequest, CreateUserRequest, MeResponse,
    MemberDto, OrgDto, Page, ProjectDto, ProjectStatus, Role, UpdateOrgRequest,
    UpdateProjectRequest, UserDto,
};
use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tab {
    Orgs,
    Users,
}

/// 프로젝트·광고주가 바뀌면 사이드바 전환기도 갱신한다.
async fn refresh_me(mut ctx: crate::state::AppCtx) {
    if let Ok(m) = api::get::<MeResponse>("/v1/auth/me").await {
        ctx.me.set(Some(m));
    }
}

#[component]
pub fn Admin() -> Element {
    let ctx = use_app();
    let sup = ctx.is_super_admin();
    let mut tab = use_signal(|| Tab::Orgs);
    rsx! {
        PageHead {
            title: "광고주·사용자 관리",
            desc: "광고주를 만들고 프로젝트와 담당자를 연결해요.",
            actions: rsx! {
                if sup {
                    SegmentedControl { options: vec![(Tab::Orgs, "광고주".to_string()), (Tab::Users, "사용자".to_string())], value: tab(), onchange: move |t| tab.set(t) }
                }
            },
        }
        match tab() {
            Tab::Orgs => rsx! { OrgsTab { sup } },
            Tab::Users => rsx! { UsersTab {} },
        }
    }
}

#[component]
fn OrgsTab(sup: bool) -> Element {
    let mut ctx = use_app();
    let mut orgs = use_resource(|| async { api::get::<Vec<OrgDto>>("/v1/orgs").await });
    let mut q = use_signal(String::new);
    let mut open = use_signal(|| None::<OrgDto>);
    let mut name = use_signal(String::new);
    let mut err = use_signal(|| None::<String>);
    let create = move |_| match validate::name(&name()) {
        Err(e) => err.set(Some(e.into())),
        Ok(n) => {
            err.set(None);
            spawn(async move {
                let r = api::send::<_, OrgDto>(
                    Method::Post,
                    "/v1/orgs",
                    &CreateOrgRequest {
                        name: n,
                        memo: String::new(),
                    },
                )
                .await;
                if ctx.report(&r, "광고주를 만들었어요") {
                    name.set(String::new());
                    orgs.restart();
                    refresh_me(ctx).await;
                    if let Ok(o) = r {
                        open.set(Some(o));
                    }
                }
            });
        }
    };
    rsx! {
        if sup {
            Card { title: "새 광고주",
                div { class: "inline-form",
                    TextField { label: "광고주 이름", value: name(), placeholder: "예: 한빛몰", oninput: move |v| { name.set(v); err.set(None); }, error: err() }
                    Button { onclick: create, "만들기" }
                }
            }
        }
        Card { title: "광고주",
            div { class: "toolbar",
                input { class: "input search", r#type: "search", aria_label: "광고주 검색", placeholder: "광고주 검색", value: "{q}", oninput: move |e| q.set(e.value()) }
            }
            {load(orgs, rsx! { SkeletonBlock { rows: 5 } }, |list| {
                let rows: Vec<OrgDto> = list.into_iter().filter(|o| contains_ci(&o.name, &q()) || contains_ci(&o.memo, &q())).collect();
                rsx! {
                    if rows.is_empty() {
                        EmptyState { icon: crate::components::icon::IconName::Admin, title: "광고주가 없어요", desc: if sup { "위에서 첫 광고주를 만들어 보세요." } else { "관리자로 있는 광고주가 없습니다." } }
                    } else {
                        div { class: "table-wrap",
                            table { class: "table",
                                thead { tr { th { scope: "col", "광고주" } th { scope: "col", "내 역할" } th { scope: "col", "만든 날" } } }
                                tbody {
                                    for o in rows.iter() {
                                        tr {
                                            key: "{o.id}",
                                            class: "clickable",
                                            tabindex: "0",
                                            onclick: { let o = o.clone(); move |_| open.set(Some(o.clone())) },
                                            td {
                                                div { class: "cell-title", "{o.name}" }
                                                if !o.memo.is_empty() { div { class: "cell-sub", "{o.memo}" } }
                                            }
                                            td { Badge { "{labels::role(o.role)}" } }
                                            td { class: "muted", "{dates::datetime_kr(&o.created_at)}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            })}
        }
        if let Some(o) = open() {
            OrgDrawer { org: o, onclose: move |changed: bool| { open.set(None); if changed { orgs.restart(); } } }
        }
    }
}

#[component]
fn OrgDrawer(org: OrgDto, onclose: EventHandler<bool>) -> Element {
    let mut ctx = use_app();
    let id = org.id;
    let is_admin = org.role == Role::Admin;
    let mut name = use_signal(|| org.name.clone());
    let mut memo = use_signal(|| org.memo.clone());
    let mut changed = use_signal(|| false);
    let mut members = use_resource(move || async move {
        api::get::<Vec<MemberDto>>(&format!("/v1/orgs/{id}/members")).await
    });
    let mut projects = use_resource(move || async move {
        api::get::<Vec<ProjectDto>>(&format!("/v1/orgs/{id}/projects")).await
    });
    let mut m_email = use_signal(String::new);
    let mut m_role = use_signal(|| Role::Viewer);
    let mut p_name = use_signal(String::new);
    let mut ferr = use_signal(|| None::<String>);

    let save = move |_| {
        let n = match validate::name(&name()) {
            Ok(n) => n,
            Err(e) => return ferr.set(Some(e.into())),
        };
        ferr.set(None);
        spawn(async move {
            let r = api::send::<_, OrgDto>(
                Method::Patch,
                &format!("/v1/orgs/{id}"),
                &UpdateOrgRequest {
                    name: Some(n),
                    memo: Some(memo()),
                },
            )
            .await;
            if ctx.report(&r, "저장했어요") {
                changed.set(true);
                refresh_me(ctx).await;
            }
        });
    };
    let add_member = move |_| {
        let e = match validate::email(&m_email()) {
            Ok(e) => e,
            Err(e) => return ferr.set(Some(e.into())),
        };
        ferr.set(None);
        spawn(async move {
            let r = api::send::<_, MemberDto>(
                Method::Post,
                &format!("/v1/orgs/{id}/members"),
                &AddMemberRequest {
                    email: e,
                    role: m_role(),
                },
            )
            .await;
            if ctx.report(&r, "멤버를 추가했어요") {
                m_email.set(String::new());
                members.restart();
            }
        });
    };
    let add_project = move |_| {
        let n = match validate::name(&p_name()) {
            Ok(n) => n,
            Err(e) => return ferr.set(Some(e.into())),
        };
        ferr.set(None);
        spawn(async move {
            let r = api::send::<_, ProjectDto>(
                Method::Post,
                &format!("/v1/orgs/{id}/projects"),
                &CreateProjectRequest {
                    name: n,
                    timezone: None,
                    currency: None,
                },
            )
            .await;
            if ctx.report(&r, "프로젝트를 만들었어요. 설치 코드는 설정에서 확인하세요")
            {
                p_name.set(String::new());
                projects.restart();
                refresh_me(ctx).await;
            }
        });
    };
    let roles = vec![
        (Role::Viewer, labels::role(Role::Viewer).to_string()),
        (Role::Analyst, labels::role(Role::Analyst).to_string()),
        (Role::Admin, labels::role(Role::Admin).to_string()),
    ];

    rsx! {
        Drawer { open: true, title: org.name.clone(), onclose: move |_| onclose.call(changed()),
            if let Some(e) = ferr() {
                div { class: "form-error", role: "alert", "{e}" }
            }
            if is_admin {
                div { class: "stack",
                    div { class: "section-label", "광고주 정보" }
                    TextField { label: "이름", value: name(), oninput: move |v| name.set(v) }
                    TextArea { label: "메모", value: memo(), oninput: move |v| memo.set(v) }
                    div { class: "row", style: "justify-content: flex-end", Button { size: Size::Sm, onclick: save, "저장" } }
                }
            }
            div { class: "stack",
                div { class: "section-label", "프로젝트(사이트)" }
                {load(projects, rsx! { SkeletonBlock { rows: 2 } }, |list| rsx! {
                    if list.is_empty() {
                        div { class: "callout", "프로젝트가 없어요. 사이트마다 하나씩 만드세요." }
                    }
                    ul { class: "list",
                        for p in list.iter() {
                            li { key: "{p.id}", class: "list-row",
                                div {
                                    div { class: "cell-title", "{p.name}" }
                                    div { class: "cell-sub mono", "{p.public_key} · {p.timezone} · {p.currency}" }
                                }
                                div { class: "row",
                                    if p.status == ProjectStatus::Paused {
                                        Badge { tone: Tone::Warn, "일시정지" }
                                    }
                                    if is_admin {
                                        Button {
                                            variant: Variant::Ghost,
                                            size: Size::Sm,
                                            onclick: {
                                                let (pid, next) = (p.id, if p.status == ProjectStatus::Active { ProjectStatus::Paused } else { ProjectStatus::Active });
                                                move |_| { spawn(async move {
                                                    let r = api::send::<_, ProjectDto>(Method::Patch, &format!("/v1/projects/{pid}"), &UpdateProjectRequest { status: Some(next), ..Default::default() }).await;
                                                    if ctx.report(&r, if next == ProjectStatus::Paused { "수집을 멈췄어요" } else { "수집을 다시 시작했어요" }) {
                                                        projects.restart();
                                                        refresh_me(ctx).await;
                                                    }
                                                }); }
                                            },
                                            if p.status == ProjectStatus::Active { "일시정지" } else { "다시 시작" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                })}
                if is_admin {
                    div { class: "inline-form",
                        TextField { label: "새 프로젝트", value: p_name(), placeholder: "예: 한빛몰 PC·모바일", oninput: move |v| p_name.set(v) }
                        Button { variant: Variant::Secondary, onclick: add_project, "추가" }
                    }
                }
            }
            div { class: "stack",
                div { class: "section-label", "멤버" }
                {load(members, rsx! { SkeletonBlock { rows: 3 } }, |list| rsx! {
                    if list.is_empty() {
                        div { class: "callout", "멤버가 없어요." }
                    }
                    ul { class: "list",
                        for m in list.iter() {
                            li { key: "{m.user_id}", class: "list-row",
                                div {
                                    div { class: "cell-title", {if m.name.is_empty() { m.email.clone() } else { m.name.clone() }} }
                                    div { class: "cell-sub", "{m.email}" }
                                }
                                div { class: "row",
                                    Badge { "{labels::role(m.role)}" }
                                    if is_admin {
                                        ConfirmButton {
                                            label: "내보내기",
                                            confirm: "이 광고주에서 뺄까요?",
                                            onconfirm: {
                                                let uid = m.user_id;
                                                move |_| { spawn(async move {
                                                    let r = api::send_empty(Method::Delete, &format!("/v1/orgs/{id}/members/{uid}")).await;
                                                    if ctx.report(&r, "멤버를 뺐어요") { members.restart(); }
                                                }); }
                                            },
                                        }
                                    }
                                }
                            }
                        }
                    }
                })}
                if is_admin {
                    div { class: "stack",
                        TextField { label: "멤버 추가 (가입된 이메일)", value: m_email(), placeholder: "name@company.com", oninput: move |v| m_email.set(v) }
                        div { class: "row",
                            SegmentedControl { options: roles, value: m_role(), onchange: move |r| m_role.set(r) }
                            Button { variant: Variant::Secondary, size: Size::Sm, onclick: add_member, "추가" }
                        }
                        p { class: "hint", "이미 멤버면 역할만 바뀌어요. 보기: 리포트만 · 분석·설정: 수집 설정 편집 · 관리자: 멤버·프로젝트 관리" }
                    }
                }
            }
        }
    }
}

#[component]
fn UsersTab() -> Element {
    let mut ctx = use_app();
    let mut q = use_signal(String::new);
    let mut users = use_resource(move || {
        let q = q();
        async move {
            if !q.is_empty() {
                crate::platform::sleep_ms(300).await;
            }
            api::get::<Page<UserDto>>(&query::users(&q, 100, 0)).await
        }
    });
    let mut email = use_signal(String::new);
    let mut uname = use_signal(String::new);
    let mut pw = use_signal(String::new);
    let mut sup = use_signal(|| false);
    let mut err = use_signal(|| None::<String>);
    let create = move |_| {
        let e = match validate::email(&email()) {
            Ok(e) => e,
            Err(e) => return err.set(Some(e.into())),
        };
        if let Err(e) = validate::password(&pw()) {
            return err.set(Some(e.into()));
        }
        err.set(None);
        spawn(async move {
            let req = CreateUserRequest {
                email: e,
                password: pw(),
                name: uname().trim().to_owned(),
                is_super_admin: sup(),
            };
            let r = api::send::<_, UserDto>(Method::Post, "/v1/users", &req).await;
            if ctx.report(&r, "사용자를 만들었어요. 초기 비밀번호를 따로 전달하세요")
            {
                email.set(String::new());
                uname.set(String::new());
                pw.set(String::new());
                sup.set(false);
                users.restart();
            }
        });
    };
    rsx! {
        div { class: "grid grid-2 align-start",
            Card { title: "새 사용자",
                div { class: "stack",
                    TextField { label: "이메일", kind: "email", value: email(), oninput: move |v| email.set(v) }
                    TextField { label: "이름", value: uname(), oninput: move |v| uname.set(v) }
                    TextField { label: "초기 비밀번호", kind: "password", autocomplete: "new-password", value: pw(), hint: "10자 이상", oninput: move |v| pw.set(v) }
                    crate::components::ui::ToggleRow { title: "대행사 관리자", desc: "모든 광고주를 관리할 수 있어요. 꼭 필요한 사람만.", on: sup(), onchange: move |v| sup.set(v) }
                    if let Some(e) = err() {
                        div { class: "form-error", role: "alert", "{e}" }
                    }
                    Button { onclick: create, "사용자 만들기" }
                }
            }
            Card { title: "사용자",
                div { class: "toolbar",
                    input { class: "input search", r#type: "search", aria_label: "사용자 검색", placeholder: "이메일·이름 검색", value: "{q}", oninput: move |e| q.set(e.value()) }
                }
                {load(users, rsx! { SkeletonBlock { rows: 6 } }, |p| rsx! {
                    if p.items.is_empty() {
                        EmptyState { icon: crate::components::icon::IconName::Admin, title: "사용자가 없어요", desc: "검색어를 바꿔 보세요." }
                    } else {
                        div { class: "muted", style: "margin-bottom: 8px", "{p.total}명" }
                        ul { class: "list",
                            for u in p.items.iter() {
                                li { key: "{u.id}", class: "list-row",
                                    div {
                                        div { class: "cell-title", {if u.name.is_empty() { u.email.clone() } else { u.name.clone() }} }
                                        div { class: "cell-sub", "{u.email} · 마지막 로그인 {u.last_login_at.as_deref().map(dates::datetime_kr).unwrap_or_else(|| \"없음\".into())}" }
                                    }
                                    if u.is_super_admin {
                                        Badge { tone: Tone::Primary, "대행사 관리자" }
                                    }
                                }
                            }
                        }
                    }
                })}
            }
        }
    }
}
