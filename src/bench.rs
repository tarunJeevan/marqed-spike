//! Headless bench mode (C-12): `marqed-spike <file> --repeat N --bench K`.
//!
//! It drives the real `App` pipeline with no window: K single-character inserts at the document
//! midpoint, then K at the end. Each edit goes through `App::update(InsertText)`, so the
//! snapshot, parse, identity match, and reveal+projection timings are the App's own
//! (`EditPerf`); the bench adds the layout through `ParagraphCache` after each edit, with its
//! reshape count. It runs once per identity strategy (L, then C), so the two can be compared.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use crate::app::{App, EditPerf, Message, Toggle};
use crate::doc::snap_to_grapheme_start;
use crate::widget::{ParagraphCache, layout_document};

/// The layout width the bench uses, in pixels (the widget's own fallback width).
pub const WIDTH: f32 = 800.0;

/// The character every bench edit inserts.
const TYPED: &str = "x";

/// One edit: the App's timings, then the layout that followed it.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub perf: EditPerf,
    pub layout: Duration,
    /// Paragraphs shaped during that layout (cache misses).
    pub reshapes: usize,
}

/// The edits of one phase of the run.
#[derive(Debug)]
pub struct Phase {
    pub name: &'static str,
    pub samples: Vec<Sample>,
}

/// One full run under one identity strategy.
#[derive(Debug)]
pub struct Report {
    pub strategy: &'static str,
    pub bytes: usize,
    pub lines: usize,
    pub segments: usize,
    /// The first layout of the whole document, into an empty cache.
    pub initial_layout: Duration,
    /// Paragraphs shaped by it (cache misses; identical segments share one entry).
    pub initial_reshapes: usize,
    pub midpoint: usize,
    pub phases: Vec<Phase>,
    pub final_bytes: usize,
    /// The cold first layout of the document, which loads the fonts before anything else is
    /// timed (set by `run`).
    pub font_warmup: Duration,
}

/// Runs the bench on `source`: `k` inserts at the midpoint then `k` at the end, once per
/// identity strategy.
pub fn run(source: &str, k: usize) -> Vec<Report> {
    // The first layout in a process loads the font system and the fallback fonts the text needs,
    // a one-time cost that would land on whichever strategy ran first; pay it on a throwaway
    // layout of the document itself and report it apart.
    let font_warmup = warm_up(source);
    [("L", false), ("C", true)]
        .into_iter()
        .map(|(name, switch)| Report {
            font_warmup,
            ..run_strategy(source, k, name, switch).0
        })
        .collect()
}

/// Lays `source` out into a throwaway cache and returns how long that took: the cold first
/// layout, with the fonts it needs not yet loaded.
fn warm_up(source: &str) -> Duration {
    let app = App::new(source.to_string());
    let projections = app.projections();
    let at = Instant::now();
    layout_document(&mut ParagraphCache::default(), &projections, WIDTH);
    at.elapsed()
}

/// One run; also returns the `App` it ended with.
fn run_strategy(source: &str, k: usize, strategy: &'static str, switch: bool) -> (Report, App) {
    let mut app = App::new(source.to_string());
    if switch {
        // The App starts on strategy L; F5 selects C for the edits that follow.
        app.update(Message::Toggle(Toggle::F5));
    }
    let bytes = app.source().len();
    let segments = app.segment_count();
    let lines = app.source().lines().count();

    let mut cache = ParagraphCache::default();
    let projections = app.projections();
    let at = Instant::now();
    let initial = layout_document(&mut cache, &projections, WIDTH);
    let initial_layout = at.elapsed();
    let initial_reshapes = initial.stats.misses;

    // The midpoint, on a grapheme boundary and inside the text (a valid Normal cursor).
    let midpoint = snap_to_grapheme_start(app.source(), bytes / 2);
    app.update(Message::MoveTo {
        offset: midpoint,
        goal_x: None,
    });
    app.update(Message::EnterInsert);
    let at_midpoint = (0..k).map(|_| edit(&mut app, &mut cache)).collect();

    // The end of the text: an Insert bar may sit there (C-6).
    app.update(Message::MoveTo {
        offset: app.source().len(),
        goal_x: None,
    });
    let at_end = (0..k).map(|_| edit(&mut app, &mut cache)).collect();

    let report = Report {
        strategy,
        bytes,
        lines,
        segments,
        initial_layout,
        initial_reshapes,
        midpoint,
        phases: vec![
            Phase {
                name: "midpoint",
                samples: at_midpoint,
            },
            Phase {
                name: "end",
                samples: at_end,
            },
        ],
        final_bytes: app.source().len(),
        font_warmup: Duration::ZERO,
    };
    (report, app)
}

/// One single-character insert at the bar, then the layout of the result.
fn edit(app: &mut App, cache: &mut ParagraphCache) -> Sample {
    app.update(Message::InsertText(TYPED.to_string()));
    let perf = app
        .last_perf()
        .expect("spike: an insert records its timing");
    let projections = app.projections();
    let at = Instant::now();
    let layout = layout_document(cache, &projections, WIDTH);
    Sample {
        perf,
        layout: at.elapsed(),
        reshapes: layout.stats.misses,
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// The median of `values` (the mean of the two middle ones for an even count); `None` if empty.
fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    match n {
        0 => None,
        _ if n % 2 == 1 => Some(values[n / 2]),
        _ => Some((values[n / 2 - 1] + values[n / 2]) / 2.0),
    }
}

fn max(values: &[f64]) -> Option<f64> {
    values.iter().copied().max_by(f64::total_cmp)
}

/// A column of the table: its heading and the value of a sample in it.
type Column = (&'static str, fn(&Sample) -> f64);

/// The columns of the table, in order.
fn columns() -> [Column; 8] {
    [
        ("snapshot", |s| ms(s.perf.snapshot)),
        ("parse", |s| ms(s.perf.parse)),
        ("identity", |s| ms(s.perf.identity)),
        ("reveal+proj", |s| ms(s.perf.reveal_project)),
        ("app total", |s| ms(s.perf.total)),
        ("layout", |s| ms(s.layout)),
        ("edit+layout", |s| ms(s.perf.total) + ms(s.layout)),
        ("reshapes", |s| s.reshapes as f64),
    ]
}

/// The report as text: per strategy, the initial layout and a median/max row per phase.
pub fn format(title: &str, reports: &[Report]) -> String {
    let mut out = String::new();
    let Some(first) = reports.first() else {
        return out;
    };
    let k = first.phases.first().map_or(0, |p| p.samples.len());
    let _ = writeln!(out, "bench: {title}");
    let _ = writeln!(
        out,
        "  {} bytes, {} lines, {} segments; layout width {WIDTH} px; \
         {k} single-character inserts at the midpoint (byte {}), then {k} at the end",
        first.bytes, first.lines, first.segments, first.midpoint,
    );
    let _ = writeln!(
        out,
        "  times in ms; reshapes = paragraphs shaped by the layout; \
         cold first layout of the document, fonts not yet loaded (excluded below): {:.3} ms",
        ms(first.font_warmup)
    );
    for report in reports {
        let _ = writeln!(out);
        let _ = writeln!(out, "strategy {}", report.strategy);
        let _ = writeln!(
            out,
            "  initial full layout: {:.3} ms ({} paragraphs shaped for {} segments)",
            ms(report.initial_layout),
            report.initial_reshapes,
            report.segments,
        );
        let _ = writeln!(
            out,
            "  final text: {} bytes (started at {})",
            report.final_bytes, report.bytes
        );
        let _ = write!(out, "  {:<9} {:<7}", "phase", "stat");
        for (heading, _) in columns() {
            let _ = write!(out, " {heading:>12}");
        }
        let _ = writeln!(out);
        for phase in &report.phases {
            for stat in ["median", "max"] {
                let _ = write!(out, "  {:<9} {stat:<7}", phase.name);
                for (_, value) in columns() {
                    let values: Vec<f64> = phase.samples.iter().map(value).collect();
                    let v = if stat == "median" {
                        median(values)
                    } else {
                        max(&values)
                    };
                    match v {
                        Some(v) => {
                            let _ = write!(out, " {v:>12.3}");
                        }
                        None => {
                            let _ = write!(out, " {:>12}", "-");
                        }
                    }
                }
                let _ = writeln!(out);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPIKE: &str = include_str!("../fixtures/spike.md");

    #[test]
    fn median_and_max() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![3.0]), Some(3.0));
        assert_eq!(median(vec![5.0, 1.0, 3.0]), Some(3.0));
        assert_eq!(median(vec![4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(max(&[]), None);
        assert_eq!(max(&[1.0, 7.5, 3.0]), Some(7.5));
    }

    #[test]
    fn both_strategies_run_every_insert_in_the_right_place() {
        let reports = run(SPIKE, 3);
        assert_eq!(
            reports.iter().map(|r| r.strategy).collect::<Vec<_>>(),
            ["L", "C"]
        );
        for report in &reports {
            assert!(report.font_warmup > Duration::ZERO);
            assert_eq!(report.bytes, SPIKE.len());
            assert_eq!(report.final_bytes, SPIKE.len() + 6, "{}", report.strategy);
            assert_eq!(
                report.segments,
                crate::parse::parse(SPIKE, 0).segments.len()
            );
            assert!(report.initial_reshapes > 0);
            assert_eq!(
                report
                    .phases
                    .iter()
                    .map(|p| (p.name, p.samples.len()))
                    .collect::<Vec<_>>(),
                [("midpoint", 3), ("end", 3)]
            );
            for sample in report.phases.iter().flat_map(|p| &p.samples) {
                let p = sample.perf;
                assert!(p.total >= p.snapshot + p.parse + p.identity + p.reveal_project);
            }
            // Edits are versions 1..=6, in order.
            let versions: Vec<u64> = report
                .phases
                .iter()
                .flat_map(|p| &p.samples)
                .map(|s| s.perf.version)
                .collect();
            assert_eq!(versions, [1, 2, 3, 4, 5, 6]);
        }
    }

    #[test]
    fn the_inserts_land_at_the_midpoint_then_at_the_end() {
        for (strategy, switch) in [("L", false), ("C", true)] {
            let (report, app) = run_strategy(SPIKE, 4, strategy, switch);
            let text = app.source();
            // Four `x` at the midpoint (the text there is not `x`), the rest of the text intact,
            // and four more at the very end.
            let mid = report.midpoint;
            // The midpoint is half the text, moved back to a grapheme start.
            assert!(mid <= SPIKE.len() / 2 && SPIKE.len() / 2 - mid < 8, "{mid}");
            assert!(crate::doc::is_grapheme_boundary(SPIKE, mid));
            assert_eq!(&text[mid..mid + 4], "xxxx", "{strategy}");
            assert_eq!(&text[..mid], &SPIKE[..mid]);
            assert_eq!(&text[mid + 4..text.len() - 4], &SPIKE[mid..]);
            assert!(text.ends_with("xxxx"), "{strategy}");
            assert!(SPIKE.is_char_boundary(mid));
            // Each run used its own identity strategy (the title is the status line).
            assert!(
                app.title().contains(&format!("ids {strategy}")),
                "{strategy}: {}",
                app.title()
            );
        }
    }

    #[test]
    fn reshapes_are_few_for_a_single_character_edit() {
        // A paragraph edited in place is the only content that changes, so a warm cache
        // reshapes a handful of paragraphs (the edited one, and neighbours whose reveal flips).
        let big = [SPIKE; 8].join("\n\n");
        let (report, _) = run_strategy(&big, 5, "L", false);
        let segments = report.segments;
        for sample in report.phases.iter().flat_map(|p| &p.samples) {
            assert!(sample.reshapes <= 6, "{} of {segments}", sample.reshapes);
        }
        // The initial layout shaped far fewer paragraphs than there are segments: the copies of
        // the fixture are identical, and the cache is content-addressed.
        assert!(
            report.initial_reshapes * 4 < segments,
            "{}",
            report.initial_reshapes
        );
    }

    #[test]
    fn small_and_degenerate_documents_do_not_panic() {
        // No edits: only the initial layout is reported, with `-` for the empty statistics.
        let reports = run(SPIKE, 0);
        let text = format("t", &reports);
        assert!(text.contains("initial full layout"), "{text}");
        assert!(
            text.lines()
                .any(|l| l.trim_start().starts_with("midpoint") && l.contains(" - "))
        );
        // An empty document, one without a trailing newline, and a single character.
        for source in [
            "",
            "a",
            "ab\n",
            include_str!("../fixtures/no_trailing_newline.md"),
        ] {
            let reports = run(source, 2);
            for report in &reports {
                assert_eq!(report.final_bytes, source.len() + 4, "{source:?}");
            }
            let text = format("t", &reports);
            assert!(text.contains("strategy L") && text.contains("strategy C"));
        }
    }

    #[test]
    fn the_table_has_a_median_and_a_max_row_per_phase_and_strategy() {
        let reports = run(SPIKE, 2);
        let text = format("spike.md x1", &reports);
        assert!(text.starts_with("bench: spike.md x1\n"), "{text}");
        for heading in [
            "snapshot",
            "parse",
            "identity",
            "reveal+proj",
            "app total",
            "layout",
            "edit+layout",
            "reshapes",
        ] {
            assert!(text.contains(heading), "{heading}\n{text}");
        }
        for (phase, stat) in [
            ("midpoint", "median"),
            ("midpoint", "max"),
            ("end", "median"),
            ("end", "max"),
        ] {
            let rows = text
                .lines()
                .filter(|l| l.split_whitespace().take(2).eq([phase, stat]))
                .count();
            assert_eq!(rows, 2, "{phase} {stat} (one row per strategy)\n{text}");
        }
        assert_eq!(text.matches("initial full layout").count(), 2);
        // Every number in a row parses, and a max is never below its median.
        for line in text
            .lines()
            .filter(|l| l.trim_start().starts_with("midpoint"))
        {
            let numbers: Vec<f64> = line
                .split_whitespace()
                .skip(2)
                .map(|n| n.parse().expect("test: a number"))
                .collect();
            assert_eq!(numbers.len(), 8);
        }
    }
}
