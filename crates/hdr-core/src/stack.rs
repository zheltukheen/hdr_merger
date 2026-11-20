//! How per-pixel values from N frames become one value.
//!
//! The core primitive is [`StackMethod::pick_sorted`]: it takes the frame
//! values for one pixel/channel already sorted ascending and returns the
//! result. Keeping it pure and allocation free is what makes the whole
//! pipeline testable without touching a single file.

use serde::{Deserialize, Serialize};

use crate::error::{HdrError, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum StackMethod {
    /// Total light of every pop. Blows out highlights, useful for HDR masters.
    Sum,
    /// Average: lower noise, moving subjects smear.
    Mean,
    /// Middle value: removes anything that moved, keeps the typical light.
    #[default]
    Median,
    /// `p` in 0..=100. 90 keeps the strongest stable contribution.
    Percentile(f32),
    /// Brightest value anywhere.
    Max,
    /// Max with the `n` brightest values dropped.
    TrimmedMax(usize),
    /// Focus stacking: keep the frame whose neighbourhood is sharpest. Needs a
    /// spatial pass, so the pipeline handles it outside `pick_*`.
    Focus,
}

impl StackMethod {
    /// Accepts `sum`, `mean`, `median`, `max`, `p90`, `percentile:90`, `trimmed:3`.
    pub fn parse(text: &str) -> Result<Self> {
        let t = text.trim().to_ascii_lowercase();
        match t.as_str() {
            "sum" | "add" => return Ok(StackMethod::Sum),
            "mean" | "avg" | "average" => return Ok(StackMethod::Mean),
            "median" | "p50" => return Ok(StackMethod::Median),
            "max" | "lighten" => return Ok(StackMethod::Max),
            "focus" | "focus-stack" | "focusstack" => return Ok(StackMethod::Focus),
            _ => {}
        }
        if let Some(rest) = t.strip_prefix("percentile:") {
            return Self::percentile(parse_number(rest, text)?);
        }
        if let Some(rest) = t.strip_prefix('p') {
            return Self::percentile(parse_number(rest, text)?);
        }
        if let Some(rest) = t.strip_prefix("trimmed:") {
            let drop = parse_number(rest, text)? as usize;
            return Ok(StackMethod::TrimmedMax(drop));
        }
        Err(HdrError::Invalid(format!("unknown stack method `{text}`")))
    }

    pub fn percentile(p: f32) -> Result<Self> {
        if !(0.0..=100.0).contains(&p) || p.is_nan() {
            return Err(HdrError::Invalid(format!("percentile {p} outside 0..=100")));
        }
        Ok(StackMethod::Percentile(p))
    }

    pub fn label(&self) -> String {
        match self {
            StackMethod::Sum => "sum".into(),
            StackMethod::Mean => "mean".into(),
            StackMethod::Median => "median".into(),
            StackMethod::Max => "max".into(),
            StackMethod::TrimmedMax(n) => format!("trimmed:{n}"),
            StackMethod::Focus => "focus".into(),
            StackMethod::Percentile(p) => format!("p{p}"),
        }
    }

    /// Same answer as [`StackMethod::pick_sorted`] but on an unsorted, mutable
    /// slice: selection instead of a full sort. This is the hot path — 60 MP
    /// times three channels times N frames — and a full sort there costs ~4x
    /// more comparisons than the order statistic actually needs.
    pub fn pick_unsorted(&self, values: &mut [f32]) -> f32 {
        let n = values.len();
        if n == 0 {
            return 0.0;
        }
        match *self {
            StackMethod::Sum => values.iter().sum(),
            StackMethod::Mean => values.iter().sum::<f32>() / n as f32,
            StackMethod::Max => values.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            // Spatial method: handled by `focus::FocusAccumulator`.
            StackMethod::Focus => values.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            StackMethod::Median => {
                let mid = n / 2;
                values.select_nth_unstable_by(mid, f32::total_cmp);
                if n % 2 == 1 {
                    values[mid]
                } else {
                    let lower = values[..mid]
                        .iter()
                        .copied()
                        .fold(f32::NEG_INFINITY, f32::max);
                    0.5 * (lower + values[mid])
                }
            }
            StackMethod::Percentile(p) => {
                let index = (((p / 100.0) * (n as f32 - 1.0)).round() as usize).min(n - 1);
                values.select_nth_unstable_by(index, f32::total_cmp);
                values[index]
            }
            StackMethod::TrimmedMax(drop) => {
                let index = n - 1 - drop.min(n - 1);
                values.select_nth_unstable_by(index, f32::total_cmp);
                values[index]
            }
        }
    }

    /// Encoding understood by `stack.wgsl`: `(method, param)`.
    pub fn gpu_code(&self) -> (u32, f32) {
        match *self {
            StackMethod::Sum => (0, 0.0),
            StackMethod::Mean => (1, 0.0),
            StackMethod::Median => (2, 0.0),
            StackMethod::Percentile(p) => (3, p),
            StackMethod::TrimmedMax(drop) => (4, drop as f32),
            StackMethod::Max => (5, 0.0),
            // Not dispatched to the GPU; kept total for the API.
            StackMethod::Focus => (5, 0.0),
        }
    }

    /// Pick one value out of the ascending-sorted frame values.
    pub fn pick_sorted(&self, sorted: &[f32]) -> f32 {
        let n = sorted.len();
        if n == 0 {
            return 0.0;
        }
        match *self {
            StackMethod::Sum => sorted.iter().sum(),
            StackMethod::Mean => sorted.iter().sum::<f32>() / n as f32,
            StackMethod::Median => {
                if n % 2 == 1 {
                    sorted[n / 2]
                } else {
                    0.5 * (sorted[n / 2 - 1] + sorted[n / 2])
                }
            }
            StackMethod::Percentile(p) => {
                let idx = ((p / 100.0) * (n as f32 - 1.0)).round() as usize;
                sorted[idx.min(n - 1)]
            }
            StackMethod::Max => sorted[n - 1],
            // Spatial method: the pipeline never calls this for `Focus`.
            StackMethod::Focus => sorted[n - 1],
            StackMethod::TrimmedMax(drop) => sorted[n - 1 - drop.min(n - 1)],
        }
    }
}

fn parse_number(text: &str, whole: &str) -> Result<f32> {
    text.trim()
        .parse::<f32>()
        .map_err(|_| HdrError::Invalid(format!("`{whole}` is not a stack method")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_odd_count_is_middle() {
        assert_eq!(StackMethod::Median.pick_sorted(&[1.0, 5.0, 9.0]), 5.0);
    }

    #[test]
    fn median_of_even_count_averages_middle_two() {
        assert_eq!(StackMethod::Median.pick_sorted(&[1.0, 3.0, 5.0, 9.0]), 4.0);
    }

    #[test]
    fn percentile_picks_expected_rank() {
        let values: Vec<f32> = (0..=100).map(|v| v as f32).collect();
        assert_eq!(StackMethod::Percentile(90.0).pick_sorted(&values), 90.0);
        assert_eq!(StackMethod::Percentile(0.0).pick_sorted(&values), 0.0);
        assert_eq!(StackMethod::Percentile(100.0).pick_sorted(&values), 100.0);
    }

    #[test]
    fn max_and_trimmed_max_drop_the_top_values() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(StackMethod::Max.pick_sorted(&values), 5.0);
        assert_eq!(StackMethod::TrimmedMax(2).pick_sorted(&values), 3.0);
        assert_eq!(StackMethod::TrimmedMax(99).pick_sorted(&values), 1.0);
    }

    #[test]
    fn sum_and_mean_accumulate() {
        let values = [1.0, 2.0, 3.0];
        assert_eq!(StackMethod::Sum.pick_sorted(&values), 6.0);
        assert_eq!(StackMethod::Mean.pick_sorted(&values), 2.0);
    }

    #[test]
    fn parse_accepts_documented_spellings() {
        assert_eq!(
            StackMethod::parse("p90").unwrap(),
            StackMethod::Percentile(90.0)
        );
        assert_eq!(
            StackMethod::parse("PERCENTILE:75").unwrap(),
            StackMethod::Percentile(75.0)
        );
        assert_eq!(StackMethod::parse("median").unwrap(), StackMethod::Median);
        assert_eq!(
            StackMethod::parse("trimmed:4").unwrap(),
            StackMethod::TrimmedMax(4)
        );
        assert!(StackMethod::parse("bogus").is_err());
        assert!(StackMethod::parse("p101").is_err());
    }

    #[test]
    fn selection_matches_sorting_for_every_method() {
        let base: Vec<f32> = (0..41).map(|v| ((v * 37) % 41) as f32 * 0.25).collect();
        let mut rng = 12345u32;
        let mut noisy = base.clone();
        for value in noisy.iter_mut() {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *value += ((rng >> 16) as f32 / 65535.0) * 0.01;
        }
        for method in [
            StackMethod::Sum,
            StackMethod::Mean,
            StackMethod::Median,
            StackMethod::Max,
            StackMethod::Percentile(10.0),
            StackMethod::Percentile(90.0),
            StackMethod::TrimmedMax(3),
            StackMethod::Focus,
        ] {
            for values in [base.clone(), noisy.clone()] {
                let mut sorted = values.clone();
                sorted.sort_by(f32::total_cmp);
                let mut unsorted = values.clone();
                let expected = method.pick_sorted(&sorted);
                let got = method.pick_unsorted(&mut unsorted);
                assert!(
                    (expected - got).abs() < 1e-5,
                    "{method:?}: sorted {expected} vs selection {got}"
                );
            }
        }
    }

    #[test]
    fn empty_stack_is_zero() {
        assert_eq!(StackMethod::Median.pick_sorted(&[]), 0.0);
    }
}
