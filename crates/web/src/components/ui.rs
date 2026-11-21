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
    #[props(default)] style: StatStyle,
) -> Element {
    let (card, value_class) = match style {
        StatStyle::Plain => ("card", "stat-value"),
        StatStyle::Hero => ("card primary", "stat-value xl"),
        StatStyle::Struck => ("card filled", "stat-value strike"),
    };
    rsx! {
        div { class: "{card}",
            div { class: "stat-label", "{label}" }
            div { class: "{value_class}", "{value}" }
            if sub.is_some() || delta.is_some() {
                div { class: "stat-sub",
                    if let Some(d) = delta {
                        span { class: if d >= 0.0 { "delta up" } else { "delta down" },
                            {format!("{}{:.1}%", if d >= 0.0 { "▲ " } else { "▼ " }, d.abs() * 100.0)}
                        }
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
) -> Element {
    rsx! {
        div { class: "segmented", role: "tablist",
            for (v, label) in options {
                button {
                    key: "{label}",
                    class: if v == value { "on" } else { "" },
                    role: "tab",
                    aria_selected: v == value,
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
pub fn Toggle(on: bool, onchange: EventHandler<bool>) -> Element {
    rsx! {
        button {
            class: if on { "toggle on" } else { "toggle" },
            role: "switch",
            aria_checked: on,
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
) -> Element {
    rsx! {
        div { class: "toggle-row",
            div {
                div { class: "t-title", "{title}" }
                if let Some(d) = desc {
                    div { class: "t-desc", "{d}" }
                }
            }
            Toggle { on, onchange }
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
) -> Element {
    let invalid = if error.is_some() { "invalid" } else { "" };
    rsx! {
        div { class: "field",
            label { "{label}" }
            input {
                class: "input {invalid}",
                value: "{value}",
                placeholder: placeholder.unwrap_or_default(),
                oninput: move |e| oninput.call(e.value()),
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
pub fn EmptyState(icon: String, title: String, desc: String, action: Option<Element>) -> Element {
    rsx! {
        div { class: "empty",
            div { class: "empty-icon", "{icon}" }
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
fn js_string(s: &str) -> String {
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
    rsx! {
        div { class: "drawer-backdrop", onclick: move |_| onclose.call(()) }
        aside { class: "drawer", role: "dialog", aria_modal: true,
            div { class: "drawer-head",
                div { class: "drawer-title", "{title}" }
                button { class: "icon-btn", aria_label: "닫기", onclick: move |_| onclose.call(()), "✕" }
            }
            div { class: "drawer-body", {children} }
        }
    }
}

/// 브라우저 setTimeout 기반 대기.
pub async fn sleep_ms(ms: u32) {
    let _ = document::eval(&format!(
        "await new Promise(r => setTimeout(r, {ms})); return true;"
    ))
    .await;
}

#[component]
pub fn Toast(message: String) -> Element {
    rsx! { div { class: "toast", role: "status", "{message}" } }
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
