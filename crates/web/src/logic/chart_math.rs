//! 차트 좌표 계산 (SVG viewBox 단위). 렌더링 컴포넌트는 `components::chart`.

/// 1, 2, 2.5, 5, 10 × 10ⁿ 중 v 이상인 가장 작은 값. 0 이하·NaN이면 1.
pub fn nice_ceil(v: f64) -> f64 {
    if !v.is_finite() || v <= 0.0 {
        return 1.0;
    }
    let mag = 10f64.powf(v.log10().floor());
    let n = v / mag;
    let nice = if n <= 1.0 + 1e-9 {
        1.0
    } else if n <= 2.0 {
        2.0
    } else if n <= 2.5 {
        2.5
    } else if n <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// 0부터 `nice_ceil(max)`까지 `count`등분 눈금 (count+1개).
/// 간격은 1·2·2.5·5×10ⁿ. 칸 수는 `count`, `count+1`, `count-1` 중 위쪽 여백이 가장 작은 것.
pub fn ticks(max: f64, count: usize) -> Vec<f64> {
    let max = if max.is_finite() && max > 0.0 {
        max
    } else {
        1.0
    };
    let count = count.max(1);
    let (step, n) = [count, count + 1, count.saturating_sub(1).max(1)]
        .into_iter()
        .map(|c| (nice_ceil(max / c as f64), c))
        .fold(None::<(f64, usize)>, |best, (s, c)| match best {
            Some((bs, bc)) if bs * bc as f64 <= s * c as f64 => Some((bs, bc)),
            _ => Some((s, c)),
        })
        .unwrap_or((1.0, 1));
    (0..=n).map(|k| step * k as f64).collect()
}

/// 그리기 영역.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub w: f64,
    pub h: f64,
    pub pad_l: f64,
    pub pad_r: f64,
    pub pad_t: f64,
    pub pad_b: f64,
}

impl Frame {
    pub fn plot_w(&self) -> f64 {
        (self.w - self.pad_l - self.pad_r).max(0.0)
    }

    pub fn plot_h(&self) -> f64 {
        (self.h - self.pad_t - self.pad_b).max(0.0)
    }

    pub fn bottom(&self) -> f64 {
        self.h - self.pad_b
    }

    /// n개 점 중 i번째의 x. 점이 하나면 가운데.
    pub fn x(&self, i: usize, n: usize) -> f64 {
        if n <= 1 {
            self.pad_l + self.plot_w() / 2.0
        } else {
            self.pad_l + self.plot_w() * i as f64 / (n - 1) as f64
        }
    }

    /// 값 v의 y (0..=max 밖은 잘라낸다).
    pub fn y(&self, v: f64, max: f64) -> f64 {
        let v = if v.is_finite() { v } else { 0.0 };
        if max <= 0.0 || !max.is_finite() {
            return self.bottom();
        }
        let r = (v / max).clamp(0.0, 1.0);
        self.pad_t + self.plot_h() * (1.0 - r)
    }
}

/// 꺾은선 경로 `M..L..`. 점이 없으면 빈 문자열, 하나면 짧은 수평선.
pub fn line_path(values: &[f64], f: &Frame, max: f64) -> String {
    let n = values.len();
    let clean = |v: f64| if v.is_finite() { v } else { 0.0 };
    match n {
        0 => String::new(),
        1 => {
            let (x, y) = (f.x(0, 1), f.y(clean(values[0]), max));
            format!("M{:.1},{y:.1}L{:.1},{y:.1}", x - 4.0, x + 4.0)
        }
        _ => values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let cmd = if i == 0 { "M" } else { "L" };
                format!("{cmd}{:.1},{:.1}", f.x(i, n), f.y(clean(*v), max))
            })
            .collect(),
    }
}

/// 선 아래 면적 경로 (닫힘).
pub fn area_path(values: &[f64], f: &Frame, max: f64) -> String {
    let n = values.len();
    if n == 0 {
        return String::new();
    }
    let line = line_path(values, f, max);
    let (x0, x1) = if n == 1 {
        (f.x(0, 1) - 4.0, f.x(0, 1) + 4.0)
    } else {
        (f.x(0, n), f.x(n - 1, n))
    };
    format!("{line}L{x1:.1},{b:.1}L{x0:.1},{b:.1}Z", b = f.bottom())
}

/// 라벨을 최대 `max_labels`개만 보이도록 건너뛸 간격.
pub fn label_step(n: usize, max_labels: usize) -> usize {
    if max_labels == 0 {
        return n.max(1);
    }
    n.div_ceil(max_labels).max(1)
}

/// 세로 막대 (x, y, w, h). gap은 막대 사이 비율 (0..1).
pub fn bars(values: &[f64], f: &Frame, max: f64, gap: f64) -> Vec<(f64, f64, f64, f64)> {
    let n = values.len();
    if n == 0 {
        return vec![];
    }
    let slot = f.plot_w() / n as f64;
    let w = slot * (1.0 - gap.clamp(0.0, 0.95));
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let y = f.y(*v, max);
            let x = f.pad_l + slot * i as f64 + (slot - w) / 2.0;
            (x, y, w, f.bottom() - y)
        })
        .collect()
}

/// 값들의 백분율 (합 100). 음수는 0으로, 합이 0이면 모두 0.
pub fn shares(values: &[i64]) -> Vec<f64> {
    let clean: Vec<f64> = values.iter().map(|v| (*v).max(0) as f64).collect();
    let total: f64 = clean.iter().sum();
    clean
        .iter()
        .map(|v| if total > 0.0 { v / total * 100.0 } else { 0.0 })
        .collect()
}

/// 툴팁 정렬: 가로 위치(%)가 왼쪽 끝이면 오른쪽으로, 오른쪽 끝이면 왼쪽으로 펼친다.
pub fn tooltip_anchor(x_pct: f64) -> &'static str {
    if x_pct < 15.0 {
        "left"
    } else if x_pct > 85.0 {
        "right"
    } else {
        "center"
    }
}

/// 각 점의 마우스 감지 구간 (left%, width%). 이웃 점과의 중간에서 나눈다.
pub fn hover_zones(f: &Frame, n: usize) -> Vec<(f64, f64)> {
    if n == 0 {
        return vec![];
    }
    let pct = |x: f64| x / f.w * 100.0;
    let xs: Vec<f64> = (0..n).map(|i| pct(f.x(i, n))).collect();
    let mut edges = vec![0.0];
    edges.extend(xs.windows(2).map(|w| (w[0] + w[1]) / 2.0));
    edges.push(100.0);
    edges.windows(2).map(|e| (e[0], e[1] - e[0])).collect()
}

/// 축 라벨: 1234 → `1,234`, 12000 → `1.2만`, 1.5억.
pub fn axis_label(v: f64) -> String {
    let a = v.abs();
    if a >= 100_000_000.0 {
        format!("{}억", crate::format::trim_float(v / 100_000_000.0, 1))
    } else if a >= 10_000.0 {
        format!("{}만", crate::format::trim_float(v / 10_000.0, 1))
    } else if v.fract() != 0.0 {
        crate::format::trim_float(v, 1)
    } else {
        crate::format::int(v as i64)
    }
}

/// 막대 너비(%): max 대비 v. max가 0이면 0.
pub fn ratio_pct(v: f64, max: f64) -> f64 {
    if max <= 0.0 || !v.is_finite() || !max.is_finite() {
        0.0
    } else {
        (v / max * 100.0).clamp(0.0, 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const F: Frame = Frame {
        w: 100.0,
        h: 50.0,
        pad_l: 10.0,
        pad_r: 10.0,
        pad_t: 0.0,
        pad_b: 10.0,
    };

    #[test]
    fn nice_ceiling() {
        assert_eq!(nice_ceil(12_830.0), 20_000.0);
        assert_eq!(nice_ceil(2_400.0), 2_500.0);
        assert_eq!(nice_ceil(1.0), 1.0);
        assert_eq!(nice_ceil(0.0), 1.0);
        assert_eq!(nice_ceil(-5.0), 1.0);
        assert_eq!(nice_ceil(f64::NAN), 1.0);
        assert_eq!(nice_ceil(0.3), 0.5);
        assert_eq!(nice_ceil(9.2e15), 1e16);
    }

    #[test]
    fn tick_values() {
        assert_eq!(ticks(80.0, 4), vec![0.0, 20.0, 40.0, 60.0, 80.0]);
        // 4칸이면 400까지 늘어나므로 5칸(50 간격)을 고른다
        assert_eq!(ticks(230.0, 4), vec![0.0, 50.0, 100.0, 150.0, 200.0, 250.0]);
        assert!(ticks(12_830.0, 4).iter().all(|t| t.fract() == 0.0));
        assert_eq!(ticks(0.0, 2), vec![0.0, 0.5, 1.0]);
        assert_eq!(ticks(10.0, 0), vec![0.0, 10.0]);
    }

    #[test]
    fn frame_coords() {
        assert_eq!(F.x(0, 3), 10.0);
        assert_eq!(F.x(2, 3), 90.0);
        assert_eq!(F.x(0, 1), 50.0);
        assert_eq!(F.y(0.0, 10.0), 40.0);
        assert_eq!(F.y(10.0, 10.0), 0.0);
        assert_eq!(F.y(20.0, 10.0), 0.0);
        assert_eq!(F.y(-3.0, 10.0), 40.0);
        assert_eq!(F.y(5.0, 0.0), 40.0);
    }

    #[test]
    fn paths() {
        assert_eq!(line_path(&[], &F, 10.0), "");
        assert_eq!(line_path(&[0.0, 10.0], &F, 10.0), "M10.0,40.0L90.0,0.0");
        // 점 하나: 가운데 짧은 수평선
        assert_eq!(line_path(&[5.0], &F, 10.0), "M46.0,20.0L54.0,20.0");
        assert_eq!(area_path(&[], &F, 10.0), "");
        assert_eq!(
            area_path(&[0.0, 10.0], &F, 10.0),
            "M10.0,40.0L90.0,0.0L90.0,40.0L10.0,40.0Z"
        );
        // NaN·무한대는 0으로
        assert_eq!(
            line_path(&[f64::NAN, f64::INFINITY], &F, 10.0),
            "M10.0,40.0L90.0,40.0"
        );
    }

    #[test]
    fn label_steps() {
        assert_eq!(label_step(0, 7), 1);
        assert_eq!(label_step(7, 7), 1);
        assert_eq!(label_step(8, 7), 2);
        assert_eq!(label_step(30, 7), 5);
        assert_eq!(label_step(90, 7), 13);
        assert_eq!(label_step(5, 0), 5);
    }

    #[test]
    fn bar_geometry() {
        let b = bars(&[0.0, 10.0], &F, 10.0, 0.5);
        assert_eq!(b.len(), 2);
        // 칸 너비 40, 막대 20, 칸 가운데 정렬
        assert_eq!(b[0], (20.0, 40.0, 20.0, 0.0));
        assert_eq!(b[1], (60.0, 0.0, 20.0, 40.0));
        assert!(bars(&[], &F, 10.0, 0.5).is_empty());
    }

    #[test]
    fn share_percentages() {
        assert_eq!(shares(&[]), Vec::<f64>::new());
        assert_eq!(shares(&[0, 0]), vec![0.0, 0.0]);
        assert_eq!(shares(&[1, 3]), vec![25.0, 75.0]);
        assert_eq!(shares(&[-5, 5]), vec![0.0, 100.0]);
        let big = shares(&[i64::MAX, i64::MAX]);
        assert!((big[0] - 50.0).abs() < 1e-9);
    }

    #[test]
    fn tooltip_flips_near_edges() {
        assert_eq!(tooltip_anchor(5.0), "left");
        assert_eq!(tooltip_anchor(95.0), "right");
        assert_eq!(tooltip_anchor(50.0), "center");
        assert_eq!(tooltip_anchor(f64::NAN), "center");
    }

    #[test]
    fn hover_zones_split_at_midpoints() {
        assert!(hover_zones(&F, 0).is_empty());
        assert_eq!(hover_zones(&F, 1), vec![(0.0, 100.0)]);
        assert_eq!(
            hover_zones(&F, 3),
            vec![(0.0, 30.0), (30.0, 40.0), (70.0, 30.0)]
        );
        let z = hover_zones(&F, 90);
        let total: f64 = z.iter().map(|(_, w)| w).sum();
        assert!((total - 100.0).abs() < 1e-9);
    }

    #[test]
    fn axis_labels() {
        assert_eq!(axis_label(0.0), "0");
        assert_eq!(axis_label(2_500.0), "2,500");
        assert_eq!(axis_label(12_000.0), "1.2만");
        assert_eq!(axis_label(100_000.0), "10만");
        assert_eq!(axis_label(150_000_000.0), "1.5억");
        assert_eq!(axis_label(0.5), "0.5");
    }

    #[test]
    fn ratio() {
        assert_eq!(ratio_pct(5.0, 10.0), 50.0);
        assert_eq!(ratio_pct(5.0, 0.0), 0.0);
        assert_eq!(ratio_pct(-1.0, 10.0), 0.0);
        assert_eq!(ratio_pct(20.0, 10.0), 100.0);
    }
}
