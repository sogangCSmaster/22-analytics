//! 기본 UI 컴포넌트. 스타일은 assets/main.css, 값은 assets/tokens.css.

use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum Variant {
    #[default]
    Primary,
    Secondary,
    Ghost,
    Danger,
}

impl Variant {
    fn class(self) -> &'static str {
        match self {
            Variant::Primary => "btn-primary",
            Variant::Secondary => "btn-secondary",
            Variant::Ghost => "btn-ghost",
            Variant::Danger => "btn-danger",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum Size {
    Sm,
    #[default]
    Md,
    Lg,
}

#[component]
pub fn Button(
    #[props(default)] variant: Variant,
    #[props(default)] size: Size,
    #[props(default)] disabled: bool,
    onclick: Option<EventHandler<MouseEvent>>,
    children: Element,
) -> Element {
    let size = match size {
        Size::Sm => "btn-sm",
        Size::Md => "",
        Size::Lg => "btn-lg",
    };
    rsx! {
        button {
            class: "btn {variant.class()} {size}",
            disabled,
            onclick: move |e| {
                if let Some(h) = &onclick {
                    h.call(e);
                }
            },
            {children}
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum Tone {
    #[default]
    Neutral,
    Primary,
    Up,
    Down,
    Warn,
}

#[component]
pub fn Badge(#[props(default)] tone: Tone, children: Element) -> Element {
    let tone = match tone {
        Tone::Neutral => "badge-neutral",
        Tone::Primary => "badge-primary",
        Tone::Up => "badge-up",
        Tone::Down => "badge-down",
        Tone::Warn => "badge-warn",
    };
    rsx! { span { class: "badge {tone}", {children} } }
}

#[component]
pub fn Card(
    title: Option<String>,
    /// 제목 오른쪽 영역 (버튼, 세그먼트 등)
    actions: Option<Element>,
    #[props(default)] filled: bool,
    children: Element,
) -> Element {
    let filled = if filled { "filled" } else { "" };
    rsx! {
        section { class: "card {filled}",
            if title.is_some() || actions.is_some() {
                div { class: "card-head",
                    if let Some(t) = title {
                        h3 { class: "card-title", "{t}" }
                    }
                    if let Some(a) = actions {
                        div { class: "row", {a} }
                    }
                }
            }
            {children}
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum StatStyle {
    #[default]
    Plain,
    /// 강조 (primary 배경, 큰 숫자)
    Hero,
    /// 취소선 (중복 소거 전 수치)
    Struck,
}

/// 지표 카드: 라벨 + 큰 숫자 + 보조 정보.
#[component]
pub fn StatCard(
    label: String,
    value: String,
    sub: Option<String>,
    /// 증감 비율 (예: 0.12 = +12%)
    delta: Option<f64>,
    /// 증감을 보여줄지 (이전 값이 0이라 delta가 None이어도 `–`를 표시)
    #[props(default)]
    show_delta: bool,
    /// 작은 추이선
    spark: Option<Vec<f64>>,
    #[props(default)] style: StatStyle,
) -> Element {
    let (card, value_class) = match style {
        StatStyle::Plain => ("card stat", "stat-value"),
        StatStyle::Hero => ("card primary stat", "stat-value xl"),
        StatStyle::Struck => ("card filled stat", "stat-value strike"),
    };
    let show_delta = show_delta || delta.is_some();
    let (dtext, dir) = crate::format::delta_label(delta);
    let dclass = match dir {
        1 => "delta up",
        -1 => "delta down",
        _ => "delta flat",
    };
    rsx! {
        div { class: "{card}",
            div { class: "stat-top",
                div { class: "stat-label", "{label}" }
                if let Some(v) = spark {
                    crate::components::chart::Sparkline { values: v }
                }
            }
            div { class: "{value_class}", "{value}" }
            if sub.is_some() || show_delta {
                div { class: "stat-sub",
                    if show_delta {
                        span { class: "{dclass}", aria_label: "이전 기간 대비 {dtext}", "{dtext}" }
                    }
                    if let Some(s) = sub {
                        span { "{s}" }
                    }
                }
            }
        }
    }
}

/// 세그먼트 컨트롤. 옵션은 (값, 라벨).
#[component]
pub fn SegmentedControl<T: Clone + PartialEq + 'static>(
    options: Vec<(T, String)>,
    value: T,
    onchange: EventHandler<T>,
    /// 그룹 이름 (스크린리더)
    aria_label: Option<String>,
) -> Element {
    rsx! {
        // 패널을 바꾸는 탭이 아니라 값 하나를 고르는 컨트롤이라 라디오 그룹으로 읽힌다.
        div { class: "segmented", role: "radiogroup", aria_label,
            for (v, label) in options {
                button {
                    key: "{label}",
                    r#type: "button",
                    class: if v == value { "on" } else { "" },
                    role: "radio",
                    aria_checked: if v == value { "true" } else { "false" },
                    onclick: {
                        let v = v.clone();
                        move |_| onchange.call(v.clone())
                    },
                    "{label}"
                }
            }
        }
    }
}

#[component]
pub fn Toggle(
    on: bool,
    onchange: EventHandler<bool>,
    #[props(default)] disabled: bool,
    label: Option<String>,
) -> Element {
    rsx! {
        button {
            class: if on { "toggle on" } else { "toggle" },
            role: "switch",
            disabled,
            aria_checked: on,
            aria_label: label.unwrap_or_default(),
            onclick: move |_| onchange.call(!on),
        }
    }
}

/// 설명이 붙은 토글 한 줄 (설정 화면용).
#[component]
pub fn ToggleRow(
    title: String,
    desc: Option<String>,
    on: bool,
    onchange: EventHandler<bool>,
    #[props(default)] disabled: bool,
) -> Element {
    rsx! {
        div { class: "toggle-row",
            div {
                div { class: "t-title", "{title}" }
                if let Some(d) = desc {
                    div { class: "t-desc", "{d}" }
                }
            }
            Toggle { on, onchange, disabled, label: title.clone() }
        }
    }
}

#[component]
pub fn TextField(
    label: String,
    value: String,
    oninput: EventHandler<String>,
    placeholder: Option<String>,
    hint: Option<String>,
    error: Option<String>,
    #[props(default)] disabled: bool,
    /// input type (text, password, email, date, number, search)
    #[props(default = "text".to_string())]
    kind: String,
    autocomplete: Option<String>,
) -> Element {
    let invalid = if error.is_some() { "invalid" } else { "" };
    let has_error = error.is_some();
    rsx! {
        div { class: "field",
            label {
                "{label}"
                input {
                    class: "input {invalid}",
                    r#type: "{kind}",
                    value: "{value}",
                    disabled,
                    autocomplete: autocomplete.unwrap_or_else(|| "off".into()),
                    aria_invalid: has_error,
                    placeholder: placeholder.unwrap_or_default(),
                    oninput: move |e| oninput.call(e.value()),
                }
            }
            if let Some(e) = error {
                div { class: "error", "{e}" }
            } else if let Some(h) = hint {
                div { class: "hint", "{h}" }
            }
        }
    }
}

#[component]
pub fn EmptyState(
    icon: crate::components::icon::IconName,
    title: String,
    desc: String,
    action: Option<Element>,
) -> Element {
    rsx! {
        div { class: "empty",
            div { class: "empty-icon", aria_hidden: "true", crate::components::icon::Icon { name: icon, size: 26 } }
            div { class: "empty-title", "{title}" }
            div { class: "empty-desc", "{desc}" }
            if let Some(a) = action {
                div { {a} }
            }
        }
    }
}

#[component]
pub fn Skeleton(
    #[props(default = "100%".to_string())] width: String,
    #[props(default = "16px".to_string())] height: String,
) -> Element {
    rsx! { div { class: "skeleton", style: "width: {width}; height: {height};" } }
}

/// 코드 블록 + 복사 버튼.
#[component]
pub fn CodeBlock(code: String) -> Element {
    let mut copied = use_signal(|| false);
    let text = code.clone();
    rsx! {
        div { class: "code-wrap",
            pre { class: "code", "{code}" }
            div { class: "copy",
                Button {
                    variant: Variant::Ghost,
                    size: Size::Sm,
                    onclick: move |_| {
                        copy_to_clipboard(&text);
                        copied.set(true);
                    },
                    if copied() { "복사됨" } else { "복사" }
                }
            }
        }
    }
}

pub fn copy_to_clipboard(text: &str) {
    let js = format!(
        "navigator.clipboard && navigator.clipboard.writeText({});",
        js_string(text)
    );
    let _ = document::eval(&js);
}

/// JS 문자열 리터럴로 안전하게 감싼다.
pub fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '<' => out.push_str("\\u003c"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 오른쪽에서 열리는 상세 패널.
#[component]
pub fn Drawer(open: bool, title: String, onclose: EventHandler<()>, children: Element) -> Element {
    if !open {
        return rsx! {};
    }
    rsx! { DrawerPanel { title, onclose, children } }
}

static DRAWER_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// 열려 있는 동안만 존재하는 패널 본체 (열 때 포커스를 옮기고, 닫히며 원래 자리로 돌려준다).
#[component]
fn DrawerPanel(title: String, onclose: EventHandler<()>, children: Element) -> Element {
    let title_id = use_hook(|| {
        let n = DRAWER_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("drawer-title-{n}")
    });
    // 패널을 연 요소(표의 행 등)를 기억해 둔다. 아직 그려지기 전이라 포커스는 그 요소에 있다.
    use_hook(|| {
        let _ = document::eval(
            "(window.__drawerReturn = window.__drawerReturn || []).push(document.activeElement);",
        );
    });
    use_drop(|| {
        let _ = document::eval(
            "const el = (window.__drawerReturn || []).pop();
             if (el && el.isConnected && typeof el.focus === 'function') el.focus();",
        );
    });
    rsx! {
        div { class: "drawer-backdrop", onclick: move |_| onclose.call(()) }
        aside {
            class: "drawer",
            role: "dialog",
            aria_modal: "true",
            aria_labelledby: "{title_id}",
            tabindex: "-1",
            onmounted: move |e| async move {
                let _ = e.set_focus(true).await;
            },
            onkeydown: move |e| {
                if crate::logic::view::is_escape(&e.key()) {
                    e.stop_propagation();
                    onclose.call(());
                }
            },
            div { class: "drawer-head",
                div { id: "{title_id}", class: "drawer-title", "{title}" }
                button { class: "icon-btn", aria_label: "닫기", onclick: move |_| onclose.call(()), "✕" }
            }
            div { class: "drawer-body", {children} }
        }
    }
}

/// 프로젝트가 바뀌면 자식 화면을 통째로 새로 만든다.
///
/// 경로 매개변수 `pid`만 바뀌면 같은 컴포넌트가 그대로 남아 훅 상태(리소스·입력 중인 값·
/// 펼친 비밀 키)가 이전 프로젝트 것으로 남는다. 키를 프로젝트로 묶어 다시 만들게 한다.
#[component]
pub fn ProjectScope(pid: i32, children: Element) -> Element {
    // 키가 붙은 형제 목록에서만 키를 비교하므로, 원소 하나짜리 목록으로 감싼다.
    // 키가 바뀌면 이전 자식은 내려가고(spawn한 작업도 함께 취소) 새로 만들어진다.
    rsx! {
        for p in std::iter::once(pid) {
            Fragment { key: "{p}", {children.clone()} }
        }
    }
}

pub use crate::platform::sleep_ms;

#[component]
pub fn Toast(message: String) -> Element {
    rsx! { div { class: "toast", role: "status", "{message}" } }
}

// ---------------------------------------------------------------------------
// 데이터 상태 (로딩·오류·빈 상태)
// ---------------------------------------------------------------------------

/// API 리소스 렌더링: 로딩 중이면 `skeleton`, 오류면 재시도 상자, 성공이면 `f(값)`.
pub fn load<T: Clone + 'static>(
    res: Resource<crate::api::ApiResult<T>>,
    skeleton: Element,
    f: impl FnOnce(T) -> Element,
) -> Element {
    let state = res.read().clone();
    load_state(state, skeleton, f, move || {
        let mut r = res;
        r.restart();
    })
}

/// [`load`]와 같지만 여러 리소스를 합친 상태처럼 리소스가 아닌 값을 그린다.
pub fn load_state<T>(
    state: Option<crate::api::ApiResult<T>>,
    skeleton: Element,
    f: impl FnOnce(T) -> Element,
    mut retry: impl FnMut() + 'static,
) -> Element {
    match state {
        None => skeleton,
        Some(Ok(v)) => f(v),
        Some(Err(e)) => {
            rsx! { ErrorBox { error: e, onretry: move |_| retry() } }
        }
    }
}

#[component]
pub fn ErrorBox(
    error: crate::logic::errors::ApiError,
    onretry: Option<EventHandler<()>>,
) -> Element {
    let retry = error.retryable() || matches!(error, crate::logic::errors::ApiError::NotFound);
    rsx! {
        div { class: "error-box", role: "alert",
            div { class: "error-icon", "!" }
            div { class: "error-text",
                div { class: "error-title", "데이터를 불러오지 못했어요" }
                div { class: "error-desc", "{error.message()}" }
            }
            if retry {
                if let Some(h) = onretry {
                    Button { variant: Variant::Ghost, size: Size::Sm, onclick: move |_| h.call(()), "다시 시도" }
                }
            }
        }
    }
}

/// 지표 카드 자리 표시.
#[component]
pub fn SkeletonCards(#[props(default = 4)] n: usize) -> Element {
    rsx! {
        div { class: "grid grid-{n}", aria_busy: "true",
            for i in 0..n {
                div { key: "{i}", class: "card stat",
                    Skeleton { width: "40%", height: "14px" }
                    div { style: "height: 12px" }
                    Skeleton { width: "65%", height: "36px" }
                    div { style: "height: 10px" }
                    Skeleton { width: "30%", height: "12px" }
                }
            }
        }
    }
}

/// 표·차트 자리 표시.
#[component]
pub fn SkeletonBlock(#[props(default = 6)] rows: usize, #[props(default)] chart: bool) -> Element {
    rsx! {
        div { class: "skeleton-block", aria_busy: "true", aria_label: "불러오는 중",
            if chart {
                Skeleton { height: "200px" }
            } else {
                for i in 0..rows {
                    div { key: "{i}", class: "skeleton-row",
                        Skeleton { width: "{40 + (i * 13) % 35}%", height: "14px" }
                        Skeleton { width: "12%", height: "14px" }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 표
// ---------------------------------------------------------------------------

/// 정렬 가능한 표 머리글.
#[component]
pub fn SortTh<K: Copy + PartialEq + 'static>(
    label: String,
    k: K,
    sort: Signal<crate::logic::table::Sort<K>>,
    #[props(default = true)] right: bool,
) -> Element {
    let s = sort();
    let cls = if right { "r sortable" } else { "sortable" };
    rsx! {
        th { class: "{cls}", aria_sort: s.aria(k), scope: "col",
            button {
                class: "th-btn",
                onclick: move |_| {
                    let cur = sort();
                    sort.set(cur.toggle(k));
                },
                "{label}"
                span { class: "sort-arrow", "{s.arrow(k)}" }
            }
        }
    }
}

/// CSV 내보내기 버튼.
#[component]
pub fn CsvButton(file_name: String, headers: Vec<String>, rows: Vec<Vec<String>>) -> Element {
    rsx! {
        Button {
            variant: Variant::Ghost,
            size: Size::Sm,
            disabled: rows.is_empty(),
            onclick: move |_| {
                let h: Vec<&str> = headers.iter().map(String::as_str).collect();
                let csv = crate::logic::table::to_csv(&h, &rows);
                crate::platform::download_text(&file_name, "text/csv;charset=utf-8", &csv);
            },
            "CSV 내보내기"
        }
    }
}

// ---------------------------------------------------------------------------
// 입력
// ---------------------------------------------------------------------------

/// 여러 줄 입력.
#[component]
pub fn TextArea(
    label: String,
    value: String,
    oninput: EventHandler<String>,
    placeholder: Option<String>,
    hint: Option<String>,
    #[props(default)] disabled: bool,
) -> Element {
    rsx! {
        div { class: "field",
            label { "{label}" }
            textarea {
                class: "input textarea",
                value: "{value}",
                disabled,
                placeholder: placeholder.unwrap_or_default(),
                oninput: move |e| oninput.call(e.value()),
            }
            if let Some(h) = hint {
                div { class: "hint", "{h}" }
            }
        }
    }
}

/// 두 번 눌러야 실행되는 삭제 버튼.
#[component]
pub fn ConfirmButton(
    label: String,
    #[props(default = "정말 삭제할까요?".to_string())] confirm: String,
    #[props(default)] disabled: bool,
    onconfirm: EventHandler<()>,
) -> Element {
    let mut armed = use_signal(|| false);
    rsx! {
        if armed() {
            span { class: "row confirm",
                span { class: "confirm-text", "{confirm}" }
                Button { variant: Variant::Danger, size: Size::Sm, onclick: move |_| { armed.set(false); onconfirm.call(()); }, "확인" }
                Button { variant: Variant::Ghost, size: Size::Sm, onclick: move |_| armed.set(false), "취소" }
            }
        } else {
            Button { variant: Variant::Ghost, size: Size::Sm, disabled, onclick: move |_| armed.set(true), "{label}" }
        }
    }
}

/// 전역 알림: 3초 뒤 사라진다. 그 사이 새 알림이 오면 새 알림이 3초를 온전히 보인다.
#[component]
pub fn ToastHost() -> Element {
    let mut ctx = crate::state::use_app();
    let msg = ctx.toast.read().message.clone();
    use_effect(move || {
        let (has, generation) = {
            let t = ctx.toast.read();
            (t.message.is_some(), t.generation)
        };
        if has {
            spawn(async move {
                sleep_ms(3000).await;
                ctx.toast.with_mut(|t| t.expire(generation));
            });
        }
    });
    rsx! {
        if let Some(m) = msg {
            Toast { message: m }
        }
    }
}

/// 페이지 머리말.
#[component]
pub fn PageHead(title: String, desc: Option<String>, actions: Option<Element>) -> Element {
    rsx! {
        div { class: "page-head",
            div {
                h1 { class: "page-title", "{title}" }
                if let Some(d) = desc {
                    p { class: "page-desc", "{d}" }
                }
            }
            if let Some(a) = actions {
                div { class: "row", {a} }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::js_string;

    #[test]
    fn escapes_js() {
        assert_eq!(
            js_string("a\"b\\c\n</script>"),
            "\"a\\\"b\\\\c\\n\\u003c/script>\""
        );
    }
}
