//! 설치·수집 설정: 설치(스니펫·비밀 키), 수집 항목, 허용 도메인, 채널 규칙, 기여 설정, 광고비.
//!
//! 보기 권한(viewer)이면 읽기만, 분석·설정(analyst) 이상이면 편집, 비밀 키는 관리자(admin)만.

use crate::Route;
use crate::api::{self, Method};
use crate::components::ui::{
    Badge, Button, Card, CodeBlock, ConfirmButton, Drawer, EmptyState, SegmentedControl, Size,
    SkeletonBlock, TextArea, TextField, ToggleRow, Tone, Variant, copy_to_clipboard, load,
};
use crate::format;
use crate::logic::validate::{self, RuleForm};
use crate::logic::{dates, labels, query};
use crate::state::use_app;
use analytics_core::dto::admin::{
    AdSpendResponse, AdSpendUploadResponse, AddOriginRequest, ApiKeyDto, AttributionSettingsDto,
    ChannelRuleDto, CreatedApiKeyResponse, OriginsResponse, ProjectSettingsResponse, Role,
    SnippetResponse, UpdateChannelRuleRequest,
};
use analytics_core::settings::TrackingSettings;
use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tab {
    Install,
    Tracking,
    Origins,
    Rules,
    Attribution,
    Spend,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::Install,
        Tab::Tracking,
        Tab::Origins,
        Tab::Rules,
        Tab::Attribution,
        Tab::Spend,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Tab::Install => "install",
            Tab::Tracking => "tracking",
            Tab::Origins => "origins",
            Tab::Rules => "rules",
            Tab::Attribution => "attribution",
            Tab::Spend => "spend",
        }
    }

    pub fn parse(s: &str) -> Tab {
        Tab::ALL
            .into_iter()
            .find(|t| t.slug() == s)
            .unwrap_or(Tab::Install)
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Install => "설치",
            Tab::Tracking => "수집 항목",
            Tab::Origins => "허용 도메인",
            Tab::Rules => "채널 규칙",
            Tab::Attribution => "기여 설정",
            Tab::Spend => "광고비",
        }
    }
}

#[component]
pub fn Settings(pid: ReadSignal<i32>, tab: ReadSignal<String>) -> Element {
    let ctx = use_app();
    let t = Tab::parse(&tab());
    let p = pid();
    let role = ctx.role(p);
    let can_edit = ctx.can_edit(p);
    rsx! {
        crate::components::ui::PageHead {
            title: "설치·수집 설정",
            desc: "사이트에 스크립트를 넣고, 무엇을 어떻게 셀지 정합니다.",
            actions: rsx! {
                if let Some(r) = role {
                    Badge { tone: if can_edit { Tone::Primary } else { Tone::Neutral }, "내 권한: {labels::role(r)}" }
                }
            },
        }
        div { class: "tabs", role: "tablist",
            for x in Tab::ALL {
                Link {
                    key: "{x.slug()}",
                    class: if x == t { "tab on" } else { "tab" },
                    to: Route::Settings { pid: p, tab: x.slug().into() },
                    "{x.label()}"
                }
            }
        }
        if !can_edit && t != Tab::Install {
            div { class: "callout", "보기 권한이라 설정을 바꿀 수 없어요. 변경이 필요하면 대행사 담당자에게 요청하세요." }
        }
        match t {
            Tab::Install => rsx! { InstallTab { pid: p, is_admin: role == Some(Role::Admin) } },
            Tab::Tracking => rsx! { TrackingTab { pid: p, can_edit } },
            Tab::Origins => rsx! { OriginsTab { pid: p, can_edit } },
            Tab::Rules => rsx! { RulesTab { pid: p, can_edit } },
            Tab::Attribution => rsx! { AttributionTab { pid: p, can_edit } },
            Tab::Spend => rsx! { SpendTab { pid: p, can_edit } },
        }
    }
}

// ------------------------------------------------------------------ 설치

const S2S_EXAMPLE: &str = "POST {TRACKER}/v1/orders\nAuthorization: Bearer sk_live_...\nContent-Type: application/json\n\n{\n  \"event_type\": \"PURCHASE_CONFIRMED\",\n  \"order_id\": \"ORD-20261120-009901\",\n  \"visitor_id\": \"<_at_vid 쿠키 값>\",\n  \"net_amount\": 100000,\n  \"ordered_at\": \"2026-11-20T18:45:12+09:00\"\n}";

#[component]
fn InstallTab(pid: i32, is_admin: bool) -> Element {
    let snippet = use_resource(move || async move {
        api::get::<SnippetResponse>(&format!("/v1/projects/{pid}/snippet")).await
    });
    rsx! {
        Card { title: "1. 스크립트 넣기",
            p { class: "page-desc", style: "margin: 0 0 16px;",
                "모든 페이지의 "
                code { "<head>" }
                " 안에 아래 코드를 붙여 넣으세요. 설치 후 실시간 화면에서 바로 확인할 수 있어요."
            }
            {load(snippet, rsx! { SkeletonBlock { rows: 3 } }, |s| rsx! {
                CodeBlock { code: s.html.clone() }
                div { class: "kv", style: "margin-top: 12px",
                    span { class: "muted", "공개 키" }
                    code { "{s.public_key}" }
                }
            })}
        }
        Card { title: "2. 주문은 서버에서 보내기",
            actions: rsx! { Badge { tone: Tone::Primary, "권장" } },
            p { class: "page-desc", style: "margin: 0 0 16px;",
                "브라우저는 결제 직후 창을 닫으면 기록을 놓치지만, 주문 서버는 결제 사실을 반드시 압니다. 실제 매출은 이 주문을 기준으로 계산해요."
            }
            {load(snippet, rsx! { SkeletonBlock { rows: 3 } }, |s| rsx! {
                CodeBlock { code: S2S_EXAMPLE.replace("{TRACKER}", &s.tracker_host) }
            })}
        }
        ApiKeys { pid, is_admin }
    }
}

#[component]
fn ApiKeys(pid: i32, is_admin: bool) -> Element {
    let mut ctx = use_app();
    let mut keys = use_resource(move || async move {
        api::get::<Vec<ApiKeyDto>>(&format!("/v1/projects/{pid}/api-keys")).await
    });
    let mut revealed = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    rsx! {
        Card {
            title: "3. 서버용 비밀 키",
            actions: rsx! {
                if is_admin {
                    Button {
                        variant: Variant::Secondary,
                        size: Size::Sm,
                        disabled: busy(),
                        onclick: move |_| {
                            busy.set(true);
                            spawn(async move {
                                let r = api::send::<_, CreatedApiKeyResponse>(Method::Post, &format!("/v1/projects/{pid}/api-keys"), &serde_json::json!({})).await;
                                match r {
                                    Ok(c) => {
                                        revealed.set(Some(c.secret));
                                        keys.restart();
                                    }
                                    Err(e) => ctx.notify(e.message()),
                                }
                                busy.set(false);
                            });
                        },
                        "새 키 만들기"
                    }
                }
            },
            p { class: "page-desc", style: "margin: 0 0 16px;", "주문 서버가 주문 API를 부를 때 씁니다. 브라우저 코드에 넣지 마세요." }
            if let Some(secret) = revealed() {
                div { class: "reveal", role: "alert",
                    div { class: "reveal-title", "새 비밀 키가 만들어졌어요. 지금 한 번만 보여요." }
                    div { class: "reveal-key mono", "{secret}" }
                    div { class: "row",
                        Button { size: Size::Sm, onclick: { let s = secret.clone(); move |_| { copy_to_clipboard(&s); ctx.notify("비밀 키를 복사했어요"); } }, "복사" }
                        Button { variant: Variant::Ghost, size: Size::Sm, onclick: move |_| revealed.set(None), "안전하게 보관했어요" }
                    }
                }
            }
            {load(keys, rsx! { SkeletonBlock { rows: 2 } }, |list| rsx! {
                if list.is_empty() {
                    EmptyState { icon: crate::components::icon::IconName::Lock, title: "아직 만든 키가 없어요", desc: if is_admin { "‘새 키 만들기’를 눌러 주문 서버용 키를 발급하세요." } else { "광고주 관리자가 키를 발급할 수 있어요." } }
                } else {
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr {
                                th { scope: "col", "키" }
                                th { scope: "col", "만든 날" }
                                th { scope: "col", "마지막 사용" }
                                th { scope: "col", "상태" }
                                th { class: "r", scope: "col", "" }
                            } }
                            tbody {
                                for k in list.iter() {
                                    tr { key: "{k.id}",
                                        td { class: "mono", "{k.prefix}••••••••" }
                                        td { "{dates::datetime_kr(&k.created_at)}" }
                                        td { class: "muted", {k.last_used_at.as_deref().map(dates::datetime_kr).unwrap_or_else(|| "사용 전".into())} }
                                        td {
                                            if k.revoked_at.is_some() {
                                                Badge { tone: Tone::Neutral, "폐기됨" }
                                            } else {
                                                Badge { tone: Tone::Up, "사용 중" }
                                            }
                                        }
                                        td { class: "r",
                                            if is_admin && k.revoked_at.is_none() {
                                                ConfirmButton {
                                                    label: "폐기",
                                                    confirm: "폐기하면 이 키로 보낸 주문이 거부돼요",
                                                    onconfirm: {
                                                        let id = k.id;
                                                        move |_| {
                                                            spawn(async move {
                                                                let r = api::send_empty(Method::Delete, &format!("/v1/projects/{pid}/api-keys/{id}")).await;
                                                                if ctx.report(&r, "키를 폐기했어요") {
                                                                    keys.restart();
                                                                }
                                                            });
                                                        }
                                                    },
                                                }
                                            }
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

// ------------------------------------------------------------------ 수집 항목

#[component]
fn TrackingTab(pid: i32, can_edit: bool) -> Element {
    let mut ctx = use_app();
    let mut draft = use_signal(|| None::<TrackingSettings>);
    let mut version = use_signal(|| 0i32);
    let mut lists = use_signal(|| (String::new(), String::new(), String::new()));
    let mut err = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);
    let res = use_resource(move || async move {
        let r = api::get::<ProjectSettingsResponse>(&format!("/v1/projects/{pid}/settings")).await;
        if let Ok(v) = &r {
            lists.set((
                v.settings.click_id_params.join(", "),
                v.settings.capture_query_params.join(", "),
                v.settings.strip_query_params.join(", "),
            ));
            version.set(v.settings_version);
            draft.set(Some(v.settings.clone()));
        }
        r
    });
    let Some(s) = draft() else {
        return load(
            res,
            rsx! { Card { SkeletonBlock { rows: 6 } } },
            |_| rsx! {},
        );
    };
    let toggle = move |f: fn(&mut TrackingSettings, bool)| {
        move |v: bool| {
            if let Some(d) = draft.write().as_mut() {
                f(d, v)
            }
        }
    };
    let save = move |_| {
        let Some(mut s) = draft() else { return };
        let (a, b, c) = lists();
        match (
            validate::param_list(&a),
            validate::param_list(&b),
            validate::param_list(&c),
        ) {
            (Ok(a), Ok(b), Ok(c)) => {
                s.click_id_params = a;
                s.capture_query_params = b;
                s.strip_query_params = c;
            }
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                err.set(Some(e));
                return;
            }
        }
        err.set(None);
        saving.set(true);
        spawn(async move {
            let r = api::send::<_, ProjectSettingsResponse>(
                Method::Put,
                &format!("/v1/projects/{pid}/settings"),
                &s,
            )
            .await;
            if let Ok(v) = &r {
                version.set(v.settings_version);
                draft.set(Some(v.settings.clone()));
            }
            ctx.report(&r, "수집 설정을 저장했어요. 사이트에는 몇 분 안에 반영돼요");
            saving.set(false);
        });
    };
    rsx! {
        div { class: "grid grid-2",
            Card { title: "자동 수집",
                ToggleRow { title: "페이지뷰 자동 수집", desc: "페이지를 열 때마다 기록합니다.", on: s.auto_pageview, disabled: !can_edit, onchange: toggle(|d, v| d.auto_pageview = v) }
                ToggleRow { title: "SPA 페이지 이동", desc: "새로고침 없이 주소가 바뀌는 사이트용입니다.", on: s.spa_mode, disabled: !can_edit, onchange: toggle(|d, v| d.spa_mode = v) }
                ToggleRow { title: "외부 링크 클릭", desc: "다른 사이트로 나가는 링크 클릭을 셉니다.", on: s.outbound_links, disabled: !can_edit, onchange: toggle(|d, v| d.outbound_links = v) }
                ToggleRow { title: "파일 다운로드", desc: "pdf, zip, xlsx, hwp 등", on: s.file_downloads, disabled: !can_edit, onchange: toggle(|d, v| d.file_downloads = v) }
                ToggleRow { title: "스크롤 깊이", desc: "25 · 50 · 75 · 90%", on: s.scroll_depth, disabled: !can_edit, onchange: toggle(|d, v| d.scroll_depth = v) }
                ToggleRow { title: "추적 거부(DNT) 존중", desc: "브라우저가 추적 거부를 보내면 수집하지 않습니다.", on: s.respect_dnt, disabled: !can_edit, onchange: toggle(|d, v| d.respect_dnt = v) }
            }
            Card { title: "전자상거래",
                ToggleRow { title: "전자상거래 이벤트", desc: "상품 조회, 장바구니, 결제 시작, 구매 이벤트를 받습니다.", on: s.ecommerce, disabled: !can_edit, onchange: toggle(|d, v| d.ecommerce = v) }
                ToggleRow { title: "브라우저 구매를 주문으로 인정", desc: "서버 주문 연동 전 임시로만 쓰세요. 창을 닫으면 놓칠 수 있어요.", on: s.client_purchase_as_order, disabled: !can_edit, onchange: toggle(|d, v| d.client_purchase_as_order = v) }
                div { class: "stack", style: "margin-top: 16px",
                    TextArea { label: "광고 클릭 ID 파라미터", value: lists().0, disabled: !can_edit, hint: "쉼표로 구분 (예: gclid, fbclid)", oninput: move |v| lists.write().0 = v }
                    TextArea { label: "추가로 남길 쿼리 파라미터", value: lists().1, disabled: !can_edit, oninput: move |v| lists.write().1 = v }
                    TextArea { label: "항상 지울 쿼리 파라미터", value: lists().2, disabled: !can_edit, hint: "이메일·전화번호 같은 개인정보 파라미터는 자동으로 지워요.", oninput: move |v| lists.write().2 = v }
                }
            }
        }
        div { class: "savebar",
            span { class: "muted", "설정 버전 {version}" }
            if let Some(e) = err() {
                span { class: "form-error", role: "alert", "{e}" }
            }
            Button { disabled: !can_edit || saving(), onclick: save, if saving() { "저장 중…" } else { "저장" } }
        }
    }
}

// ------------------------------------------------------------------ 허용 도메인

#[component]
fn OriginsTab(pid: i32, can_edit: bool) -> Element {
    let mut ctx = use_app();
    let mut input = use_signal(String::new);
    let mut err = use_signal(|| None::<String>);
    let mut res = use_resource(move || async move {
        api::get::<OriginsResponse>(&format!("/v1/projects/{pid}/origins")).await
    });
    let mut add = move || match validate::origin(&input()) {
        Ok(o) => {
            err.set(None);
            spawn(async move {
                let r = api::send::<_, OriginsResponse>(
                    Method::Post,
                    &format!("/v1/projects/{pid}/origins"),
                    &AddOriginRequest { origin: o.clone() },
                )
                .await;
                if ctx.report(&r, &format!("{o}을(를) 추가했어요")) {
                    input.set(String::new());
                    res.restart();
                }
            });
        }
        Err(e) => err.set(Some(e.into())),
    };
    rsx! {
        Card { title: "수집 허용 도메인",
            p { class: "page-desc", style: "margin: 0 0 16px", "여기 있는 도메인에서 온 이벤트만 받아요. client.com을 넣으면 하위 도메인(www, m 등)도 포함돼요." }
            if can_edit {
                form {
                    class: "inline-form",
                    onsubmit: move |e| { e.prevent_default(); add(); },
                    TextField { label: "도메인 추가", value: input(), placeholder: "shop.client.com", oninput: move |v| { input.set(v); err.set(None); }, error: err() }
                    Button { "추가" }
                }
            }
            {load(res, rsx! { SkeletonBlock { rows: 3 } }, |o| rsx! {
                if o.origins.is_empty() {
                    EmptyState { icon: crate::components::icon::IconName::Acquisition, title: "허용 도메인이 없어요", desc: "도메인을 추가해야 사이트에서 보낸 이벤트를 받을 수 있어요." }
                } else {
                    ul { class: "list",
                        for d in o.origins.iter() {
                            li { key: "{d}", class: "list-row",
                                span { class: "mono", "{d}" }
                                if can_edit {
                                    ConfirmButton {
                                        label: "삭제",
                                        onconfirm: {
                                            let d = d.clone();
                                            move |_| {
                                                let d = d.clone();
                                                spawn(async move {
                                                    let r = api::send_empty(Method::Delete, &query::origin(pid, &d)).await;
                                                    if ctx.report(&r, "삭제했어요") {
                                                        res.restart();
                                                    }
                                                });
                                            }
                                        },
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

// ------------------------------------------------------------------ 채널 규칙

fn rule_summary(r: &ChannelRuleDto) -> String {
    let m = &r.when;
    let mut parts = vec![];
    let mut push = |name: &str, v: &[String]| {
        if !v.is_empty() {
            parts.push(format!("{name} = {}", v.join(" 또는 ")));
        }
    };
    push("utm_source", &m.utm_source);
    push("utm_medium", &m.utm_medium);
    push("캠페인 접두어", &m.utm_campaign_prefix);
    push("클릭 ID", &m.click_id_type);
    push("리퍼러", &m.referrer_domain);
    parts.join(" 그리고 ")
}

fn rule_form(r: &ChannelRuleDto) -> RuleForm {
    RuleForm {
        channel: r.channel.clone(),
        utm_source: r.when.utm_source.join(", "),
        utm_medium: r.when.utm_medium.join(", "),
        utm_campaign_prefix: r.when.utm_campaign_prefix.join(", "),
        click_id_type: r.when.click_id_type.join(", "),
        referrer_domain: r.when.referrer_domain.join(", "),
        priority: r.priority.to_string(),
        label: r.label.clone(),
    }
}

#[component]
fn RulesTab(pid: i32, can_edit: bool) -> Element {
    let mut ctx = use_app();
    let mut res = use_resource(move || async move {
        api::get::<Vec<ChannelRuleDto>>(&format!("/v1/projects/{pid}/channel-rules")).await
    });
    // (편집 중인 규칙 id, 폼). id None = 새 규칙
    let mut editing = use_signal(|| None::<(Option<i64>, RuleForm)>);
    rsx! {
        Card {
            title: "채널 규칙",
            actions: rsx! {
                if can_edit {
                    Button { variant: Variant::Secondary, size: Size::Sm, onclick: move |_| editing.set(Some((None, RuleForm::default()))), "규칙 추가" }
                }
            },
            p { class: "page-desc", style: "margin: 0 0 16px", "기본 분류(네이버·메타·구글 등)보다 먼저 적용돼요. 우선순위 숫자가 작을수록 먼저 봅니다. 바꾸면 과거 데이터에도 다시 적용돼요." }
            {load(res, rsx! { SkeletonBlock { rows: 3 } }, |rules| rsx! {
                if rules.is_empty() {
                    EmptyState { icon: crate::components::icon::IconName::Settings, title: "직접 만든 규칙이 없어요", desc: "예: utm_source=naver 이고 캠페인이 shop_으로 시작하면 ‘네이버 쇼핑’으로 분류" }
                } else {
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr {
                                th { class: "r", scope: "col", "우선순위" }
                                th { scope: "col", "채널" }
                                th { scope: "col", "조건" }
                                th { class: "r", scope: "col", "" }
                            } }
                            tbody {
                                for r in rules.iter() {
                                    tr { key: "{r.id}",
                                        td { class: "r", "{r.priority}" }
                                        td {
                                            div { class: "cell-title", "{labels::channel(&r.channel)}" }
                                            if !r.label.is_empty() { div { class: "cell-sub", "{r.label}" } }
                                        }
                                        td { class: "wrap-cell", "{rule_summary(r)}" }
                                        td { class: "r",
                                            if can_edit {
                                                div { class: "row", style: "justify-content: flex-end",
                                                    Button { variant: Variant::Ghost, size: Size::Sm, onclick: { let r = r.clone(); move |_| editing.set(Some((Some(r.id), rule_form(&r)))) }, "편집" }
                                                    ConfirmButton {
                                                        label: "삭제",
                                                        onconfirm: {
                                                            let id = r.id;
                                                            move |_| { spawn(async move {
                                                                let rr = api::send_empty(Method::Delete, &format!("/v1/projects/{pid}/channel-rules/{id}")).await;
                                                                if ctx.report(&rr, "규칙을 삭제했어요") { res.restart(); }
                                                            }); }
                                                        },
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
            })}
        }
        if let Some((id, form)) = editing() {
            RuleDrawer { pid, id, form, onclose: move |saved: bool| { editing.set(None); if saved { res.restart(); } } }
        }
    }
}

#[component]
fn RuleDrawer(pid: i32, id: Option<i64>, form: RuleForm, onclose: EventHandler<bool>) -> Element {
    let mut ctx = use_app();
    let mut f = use_signal(|| form);
    let mut err = use_signal(|| None::<String>);
    let field = move |label: &'static str,
                      get: fn(&RuleForm) -> String,
                      set: fn(&mut RuleForm, String),
                      hint: &'static str| {
        rsx! {
            TextField { label, value: get(&f()), hint, oninput: move |v| set(&mut f.write(), v) }
        }
    };
    let save = move |_| match validate::rule(&f()) {
        Err(e) => err.set(Some(e.into())),
        Ok(req) => {
            err.set(None);
            spawn(async move {
                let ok = match id {
                    None => {
                        let r = api::send::<_, ChannelRuleDto>(
                            Method::Post,
                            &format!("/v1/projects/{pid}/channel-rules"),
                            &req,
                        )
                        .await;
                        ctx.report(&r, "규칙을 추가했어요")
                    }
                    Some(rid) => {
                        let patch = UpdateChannelRuleRequest {
                            channel: Some(req.channel),
                            when: Some(req.when),
                            priority: req.priority,
                            label: Some(req.label),
                        };
                        let r = api::send::<_, ChannelRuleDto>(
                            Method::Patch,
                            &format!("/v1/projects/{pid}/channel-rules/{rid}"),
                            &patch,
                        )
                        .await;
                        ctx.report(&r, "규칙을 저장했어요")
                    }
                };
                if ok {
                    onclose.call(true);
                }
            });
        }
    };
    rsx! {
        Drawer { open: true, title: if id.is_some() { "채널 규칙 편집" } else { "채널 규칙 추가" }, onclose: move |_| onclose.call(false),
            div { class: "stack",
                {field("채널 이름", |r| r.channel.clone(), |r, v| r.channel = v, "리포트에 보일 채널 ID (예: naver_shopping)")}
                div { class: "section-label", "조건 (여러 값은 쉼표로, 입력한 조건을 모두 만족해야 해요)" }
                {field("utm_source", |r| r.utm_source.clone(), |r, v| r.utm_source = v, "예: naver, NAVER")}
                {field("utm_medium", |r| r.utm_medium.clone(), |r, v| r.utm_medium = v, "예: cpc")}
                {field("캠페인 이름 접두어", |r| r.utm_campaign_prefix.clone(), |r, v| r.utm_campaign_prefix = v, "예: shop_")}
                {field("클릭 ID 종류", |r| r.click_id_type.clone(), |r, v| r.click_id_type = v, "예: gclid, n_keyword")}
                {field("리퍼러 도메인", |r| r.referrer_domain.clone(), |r, v| r.referrer_domain = v, "예: blog.naver.com")}
                {field("우선순위", |r| r.priority.clone(), |r, v| r.priority = v, "0~100000, 비우면 100")}
                {field("메모", |r| r.label.clone(), |r, v| r.label = v, "팀이 알아볼 설명 (선택)")}
                if let Some(e) = err() {
                    div { class: "form-error", role: "alert", "{e}" }
                }
                div { class: "row", style: "justify-content: flex-end",
                    Button { variant: Variant::Ghost, onclick: move |_| onclose.call(false), "취소" }
                    Button { onclick: save, "저장" }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ 기여 설정

#[component]
fn AttributionTab(pid: i32, can_edit: bool) -> Element {
    let mut ctx = use_app();
    let mut draft = use_signal(|| None::<AttributionSettingsDto>);
    let mut lookbacks = use_signal(String::new);
    let mut half = use_signal(String::new);
    let mut err = use_signal(|| None::<String>);
    let res = use_resource(move || async move {
        let r =
            api::get::<AttributionSettingsDto>(&format!("/v1/projects/{pid}/attribution-settings"))
                .await;
        if let Ok(v) = &r {
            lookbacks.set(
                v.lookback_days
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            half.set(v.half_life_hours.to_string());
            draft.set(Some(v.clone()));
        }
        r
    });
    let Some(s) = draft() else {
        return load(
            res,
            rsx! { Card { SkeletonBlock { rows: 5 } } },
            |_| rsx! {},
        );
    };
    let save = move |_| {
        let Some(mut d) = draft() else { return };
        match (
            validate::lookbacks(&lookbacks()),
            validate::half_life(&half()),
        ) {
            (Ok(l), Ok(h)) => {
                d.lookback_days = l;
                d.half_life_hours = h;
            }
            (Err(e), _) | (_, Err(e)) => {
                err.set(Some(e.into()));
                return;
            }
        }
        err.set(None);
        spawn(async move {
            let r = api::send::<_, AttributionSettingsDto>(
                Method::Put,
                &format!("/v1/projects/{pid}/attribution-settings"),
                &d,
            )
            .await;
            if let Ok(v) = &r {
                draft.set(Some(v.clone()));
            }
            ctx.report(&r, "기여 설정을 저장했어요. 다음 연산부터 반영돼요");
        });
    };
    rsx! {
        Card { title: "기본 기여 방식",
            SegmentedControl {
                options: labels::MODELS.iter().map(|m| (*m, labels::model(*m).to_string())).collect::<Vec<_>>(),
                value: s.default_model,
                onchange: move |m| { if can_edit && let Some(d) = draft.write().as_mut() { d.default_model = m } },
            }
            p { class: "callout", style: "margin-top: 16px", "{labels::model_help(s.default_model)}" }
        }
        div { class: "grid grid-2",
            Card { title: "클릭 인정 기간",
                div { class: "stack",
                    TextField { label: "기간(일)", value: lookbacks(), disabled: !can_edit, hint: "쉼표로 여러 개 (예: 7, 30). 실제 매출 화면에서 고를 수 있어요.", oninput: move |v| lookbacks.set(v) }
                    TextField { label: "시간 가중 반감기(시간)", value: half(), kind: "number", disabled: !can_edit, hint: "구매 몇 시간 전 클릭의 가중치를 절반으로 볼지 (기본 168 = 7일)", oninput: move |v| half.set(v) }
                }
            }
            Card { title: "기여 대상",
                ToggleRow { title: "자연 검색 포함", desc: "광고가 아닌 검색 유입도 매출을 나눠 받습니다.", on: s.include_organic, disabled: !can_edit, onchange: move |v| { if let Some(d) = draft.write().as_mut() { d.include_organic = v } } }
                ToggleRow { title: "외부 사이트 포함", desc: "블로그·커뮤니티 등 링크 유입도 포함합니다.", on: s.include_referral, disabled: !can_edit, onchange: move |v| { if let Some(d) = draft.write().as_mut() { d.include_referral = v } } }
            }
        }
        div { class: "savebar",
            if let Some(e) = err() {
                span { class: "form-error", role: "alert", "{e}" }
            }
            Button { disabled: !can_edit, onclick: save, "저장" }
        }
    }
}

// ------------------------------------------------------------------ 광고비

/// 채널별 합계 (내림차순)
pub fn spend_by_channel(rows: &[analytics_core::dto::admin::AdSpendRow]) -> Vec<(String, i64)> {
    let mut m: std::collections::BTreeMap<String, i64> = Default::default();
    for r in rows {
        *m.entry(r.channel.clone()).or_default() += r.amount;
    }
    let mut v: Vec<(String, i64)> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

#[component]
fn SpendTab(pid: i32, can_edit: bool) -> Element {
    let mut ctx = use_app();
    let mut err = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut res = use_resource(move || {
        let r = ctx.range();
        async move { api::get::<AdSpendResponse>(&query::ad_spend(pid, r)).await }
    });
    let range = ctx.range();
    rsx! {
        if can_edit {
            Card { title: "광고비 CSV 올리기",
                p { class: "page-desc", style: "margin: 0 0 12px", "머리글 day,channel,campaign,amount 형식. 같은 날·채널·캠페인은 새 값으로 덮어써요. 한 행이라도 틀리면 아무것도 저장하지 않아요." }
                CodeBlock { code: "day,channel,campaign,amount\n2026-11-01,naver_sa,brand,150000\n2026-11-01,meta,retargeting,98000" }
                div { class: "row", style: "margin-top: 16px",
                    label { class: "btn btn-secondary file-btn",
                        if busy() { "올리는 중…" } else { "CSV 파일 선택" }
                        input {
                            r#type: "file",
                            accept: ".csv,text/csv",
                            disabled: busy(),
                            onchange: move |e| {
                                let Some(file) = e.files().into_iter().next() else { return };
                                busy.set(true);
                                err.set(None);
                                spawn(async move {
                                    let text = file.read_string().await.unwrap_or_default();
                                    match validate::ad_spend_csv(&file.name(), file.size(), &text) {
                                        Err(e) => err.set(Some(e.into())),
                                        Ok(_) => {
                                            let r = api::post_csv::<AdSpendUploadResponse>(&format!("/v1/projects/{pid}/ad-spend"), text).await;
                                            match r {
                                                Ok(u) => { ctx.notify(format!("{}행을 반영했어요", format::count(u.rows as u64))); res.restart(); }
                                                Err(e) => err.set(Some(e.message())),
                                            }
                                        }
                                    }
                                    busy.set(false);
                                });
                            },
                        }
                    }
                    if let Some(e) = err() {
                        span { class: "form-error", role: "alert", "{e}" }
                    }
                }
            }
        }
        Card { title: "올린 광고비 · {range.label()}",
            {load(res, rsx! { SkeletonBlock { rows: 4 } }, |s| {
                let total: i64 = s.rows.iter().map(|r| r.amount).sum();
                let by = spend_by_channel(&s.rows);
                rsx! {
                    if s.rows.is_empty() {
                        EmptyState { icon: crate::components::icon::IconName::Revenue, title: "이 기간 광고비가 없어요", desc: "CSV를 올리면 실제 매출 화면에서 ROAS를 볼 수 있어요." }
                    } else {
                        div { class: "kv", style: "margin-bottom: 16px", span { class: "muted", "합계" } strong { "{format::won(total)}" } }
                        crate::components::chart::BarList { items: by.iter().map(|(c, v)| (labels::channel(c), *v)).collect::<Vec<_>>(), money: true, show_share: true }
                        div { class: "table-wrap", style: "margin-top: 16px",
                            table { class: "table",
                                thead { tr { th { scope: "col", "날짜" } th { scope: "col", "채널" } th { scope: "col", "캠페인" } th { class: "r", scope: "col", "광고비" } } }
                                tbody {
                                    for (i, r) in s.rows.iter().take(200).enumerate() {
                                        tr { key: "{i}",
                                            td { "{r.day}" }
                                            td { "{labels::channel(&r.channel)}" }
                                            td { class: "path-cell", title: "{r.campaign}", "{r.campaign}" }
                                            td { class: "r", "{format::won(r.amount)}" }
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

#[cfg(test)]
mod tests {
    use super::*;
    use analytics_core::channel::RuleMatch;
    use analytics_core::dto::admin::AdSpendRow;

    #[test]
    fn tab_slugs_roundtrip() {
        for t in Tab::ALL {
            assert_eq!(Tab::parse(t.slug()), t);
        }
        assert_eq!(Tab::parse(""), Tab::Install);
        assert_eq!(Tab::parse("<script>"), Tab::Install);
    }

    #[test]
    fn summarizes_rules() {
        let r = ChannelRuleDto {
            id: 1,
            priority: 10,
            channel: "naver_shopping".into(),
            when: RuleMatch {
                utm_source: vec!["naver".into(), "nv".into()],
                utm_campaign_prefix: vec!["shop_".into()],
                ..Default::default()
            },
            label: String::new(),
        };
        assert_eq!(
            rule_summary(&r),
            "utm_source = naver 또는 nv 그리고 캠페인 접두어 = shop_"
        );
        let f = rule_form(&r);
        assert_eq!(f.utm_source, "naver, nv");
        assert_eq!(validate::rule(&f).unwrap().when, r.when);
    }

    #[test]
    fn sums_spend_by_channel() {
        let row = |c: &str, a| AdSpendRow {
            day: "2026-11-01".into(),
            channel: c.into(),
            campaign: "x".into(),
            amount: a,
        };
        let v = spend_by_channel(&[row("meta", 5), row("naver_sa", 7), row("meta", 4)]);
        assert_eq!(v, vec![("meta".into(), 9), ("naver_sa".into(), 7)]);
        assert!(spend_by_channel(&[]).is_empty());
    }
}
