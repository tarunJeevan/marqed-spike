//! Throwaway marqed projection spike - single binary for testing block-identity heuristic and other unresolved architectural decisions

mod app;
mod doc;
mod geometry;
mod identity;
mod motion;
mod parse;
mod projection;
mod reveal;
mod widget;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use app::App;

const USAGE: &str = "usage: marqed-spike <path> [--repeat N] [--bench K]";

#[derive(Debug, PartialEq, Eq)]
struct Args {
    path: PathBuf,
    repeat: usize,
    bench: Option<usize>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut path: Option<PathBuf> = None;
    let mut repeat: usize = 1;
    let mut bench: Option<usize> = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--repeat" => repeat = count_value("--repeat", args.next())?,
            "--bench" => bench = Some(count_value("--bench", args.next())?),
            flag if flag.starts_with("--") => return Err(format!("unknown option `{flag}`")),
            _ if path.is_some() => return Err(format!("unexpected extra argument `{arg}`")),
            _ => path = Some(PathBuf::from(arg)),
        }
    }
    let path = path.ok_or("missing <path>")?;
    if repeat == 0 {
        return Err("`--repeat` must be at least 1".to_string());
    }
    Ok(Args {
        path,
        repeat,
        bench,
    })
}

fn count_value(flag: &str, value: Option<String>) -> Result<usize, String> {
    let value = value.ok_or(format!("`{flag}` needs a number"))?;
    value
        .parse()
        .map_err(|_| format!("`{flag}` needs a non-negative whole number, got `{value}`"))
}

/// Reads `path` as UTF-8 and joins `repeat` copies with a blank line.
fn load(path: &Path, repeat: usize) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "cannot read `{}`: {e}; check that the file exists and is valid UTF-8",
            path.display()
        )
    })?;
    Ok(vec![text; repeat].join("\n\n"))
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let source = match load(&args.path, args.repeat) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    if args.bench.is_some() {
        // Headless bench mode lands in T10 (C-12).
        eprintln!("note: --bench is not implemented yet; opening the window instead");
    }

    let result = iced::application(move || App::new(source.clone()), App::update, App::view)
        .title(App::title)
        .run();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: cannot start the window: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Result<Args, String> {
        parse_args(items.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_path_and_flags() {
        let parsed = args(&["f.md", "--repeat", "3", "--bench", "50"]).expect("test: expected Ok");
        assert_eq!(
            parsed,
            Args {
                path: PathBuf::from("f.md"),
                repeat: 3,
                bench: Some(50)
            }
        );
        assert_eq!(
            args(&["--bench", "5", "f.md"])
                .expect("test: expected Ok")
                .repeat,
            1
        );
    }

    #[test]
    fn rejects_bad_args() {
        assert!(args(&[]).is_err());
        assert!(args(&["a.md", "b.md"]).is_err());
        assert!(args(&["a.md", "--repeat"]).is_err());
        assert!(args(&["a.md", "--repeat", "x"]).is_err());
        assert!(args(&["a.md", "--repeat", "0"]).is_err());
        assert!(args(&["a.md", "--nope"]).is_err());
    }

    #[test]
    fn repeat_joins_with_blank_line() {
        let path = std::env::temp_dir().join("marqed_spike_repeat_test.md");
        std::fs::write(&path, "a").expect("test: expected Ok");
        assert_eq!(load(&path, 1).expect("test: expected Ok"), "a");
        assert_eq!(load(&path, 3).expect("test: expected Ok"), "a\n\na\n\na");
        std::fs::remove_file(&path).expect("test: expected Ok");
    }

    #[test]
    fn all_fixtures_load() {
        for name in ["spike.md", "no_trailing_newline.md", "empty.md"] {
            let path = Path::new("fixtures").join(name);
            assert!(load(&path, 1).is_ok(), "{name}");
        }
        let no_nl =
            load(Path::new("fixtures/no_trailing_newline.md"), 1).expect("test: expected Ok");
        assert!(!no_nl.ends_with('\n'));
        assert_eq!(
            load(Path::new("fixtures/empty.md"), 1).expect("test: expected Ok"),
            ""
        );
    }

    #[test]
    fn spike_fixture_has_required_constructs() {
        let s = load(Path::new("fixtures/spike.md"), 1).expect("test: expected Ok");
        assert!(s.starts_with("# "), "missing H1");
        for needle in [
            "\n## ",
            "### Title ###\n",
            "\n##\n",
            "\n   ### ",
            "***both***",
            "**a** **b**",
            "**x marker",
            "e\u{301}",
            "\u{1F44D}\u{1F3FD}",
            "\\n and",
            "C:\\new\\path",
            "\\*not bold\\*",
            "\n\n\n",
            "\n   \n",
            "\n- ",
            "```",
            "\n--------------\n",
        ] {
            assert!(s.contains(needle), "missing {needle:?}");
        }
    }

    #[test]
    fn unreadable_and_non_utf8_files_error() {
        assert!(load(Path::new("fixtures/does_not_exist.md"), 1).is_err());
        let path = std::env::temp_dir().join("marqed_spike_bad_utf8.md");
        std::fs::write(&path, [0xff, 0xfe, 0xfd]).expect("test: expected Ok");
        let err = load(&path, 1).expect_err("test: expected an error");
        assert!(err.contains("UTF-8"), "{err}");
        std::fs::remove_file(&path).expect("test: expected Ok");
    }
}
