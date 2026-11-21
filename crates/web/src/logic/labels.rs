//! 마케터용 한국어 이름 (채널, 이벤트, 기여 모델, 역할, 주문 상태).

use analytics_core::attribution::Model;
use analytics_core::dto::admin::Role;

/// 채널 id → 표시 이름. 모르는 채널(사용자 규칙)은 id를 그대로 쓴다.
pub fn channel(id: &str) -> String {
    todo!()
}

/// 이벤트 이름 → 설명 (표준 이벤트만). 커스텀 이벤트는 None.
pub fn event_hint(name: &str) -> Option<&'static str> {
    todo!()
}

pub fn model(m: Model) -> &'static str {
    match m {
        Model::LastClick => "마지막 클릭",
        Model::FirstClick => "첫 클릭",
        Model::Linear => "균등 배분",
        Model::TimeDecay => "시간 가중",
        Model::RawSum => "매체 합산",
    }
}

pub fn model_help(m: Model) -> &'static str {
    match m {
        Model::LastClick => "구매 직전 클릭에 매출을 모두 줍니다. 단기 효율을 빠르게 판단할 때 씁니다.",
        Model::FirstClick => "처음 데려온 매체에 매출을 모두 줍니다. 신규 고객 확보가 목표일 때 씁니다.",
        Model::Linear => "거쳐 간 매체에 똑같이 나눕니다. 여러 매체를 함께 운영할 때 씁니다.",
        Model::TimeDecay => "구매에 가까운 클릭일수록 더 줍니다. 오래 고민하는 고관여 상품에 씁니다.",
        Model::RawSum => "매체마다 주문 금액 전액을 자기 성과로 셉니다. 중복이 그대로 남아 있습니다.",
    }
}

/// 매출 보존 모델 (세그먼트 컨트롤 순서)
pub const MODELS: [Model; 4] = [Model::LastClick, Model::FirstClick, Model::Linear, Model::TimeDecay];

pub fn role(r: Role) -> &'static str {
    match r {
        Role::Viewer => "보기",
        Role::Analyst => "분석·설정",
        Role::Admin => "관리자",
    }
}

pub fn order_status(s: &str) -> (&'static str, bool) {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels() {
        assert_eq!(channel("naver_sa"), "네이버 검색광고");
        assert_eq!(channel("meta"), "메타");
        assert_eq!(channel("direct"), "직접 방문");
        assert_eq!(channel("organic_search"), "자연 검색");
        assert_eq!(channel("my_partner"), "my_partner");
        assert_eq!(channel(""), "(알 수 없음)");
        assert_eq!(channel("<b>x</b>"), "<b>x</b>");
    }

    #[test]
    fn events_and_status() {
        assert_eq!(event_hint("page_view"), Some("페이지 조회"));
        assert_eq!(event_hint("purchase"), Some("구매"));
        assert_eq!(event_hint("my_custom"), None);
        assert_eq!(order_status("confirmed"), ("확정", false));
        assert_eq!(order_status("cancelled"), ("취소", true));
        assert_eq!(order_status("weird"), ("기타", false));
        assert_eq!(order_status("PARTIALLY_REFUNDED"), ("부분 환불", false));
        assert_eq!(order_status("CANCELLED"), ("취소", true));
    }

    #[test]
    fn models() {
        assert_eq!(model(Model::LastClick), "마지막 클릭");
        assert!(MODELS.iter().all(|m| m.conserves_revenue()));
    }
}
