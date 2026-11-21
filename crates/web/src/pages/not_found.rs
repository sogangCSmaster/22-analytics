use crate::Route;
use crate::components::ui::EmptyState;
use dioxus::prelude::*;

#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    let path = format!("/{}", segments.join("/"));
    rsx! {
        div { style: "padding: 80px 16px;",
            EmptyState {
                icon: "?",
                title: "페이지를 찾을 수 없어요",
                desc: "{path} 주소가 없거나 이동했어요.",
                action: rsx! { Link { class: "btn btn-primary", to: Route::Overview {}, "개요로 가기" } },
            }
        }
    }
}
