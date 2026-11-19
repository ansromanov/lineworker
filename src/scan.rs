//! `lineworker scan`: what lineworker would do about each open pull request, without doing
//! it.

use std::fmt::Write;

use crate::classify::{Class, SkipReason, State, classify};
use crate::decide::{Action, Config, Decision, HandReason, WaitReason, decide};
use crate::github::Snapshot;
use crate::mainline::{MainState, RedReason, assess};
use crate::model::Time;
use crate::time::{clock, human};

/// The report of a scan, for people.
pub fn report(snapshot: &Snapshot, now: Time, config: &Config) -> String {
    let main = assess(&snapshot.main);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} · default branch {}: {}",
        snapshot.repo,
        snapshot.default_branch,
        main_line(&main)
    );
    if !main.bypass_merges.is_empty() {
        let shas: Vec<&str> = main.bypass_merges.iter().map(|s| short(s)).collect();
        let _ = writeln!(
            out,
            "  commits that did not come from a pull request: {}",
            shas.join(", ")
        );
    }
    if snapshot.pulls.is_empty() {
        let _ = writeln!(out, "no open pull requests");
    }
    for pr in &snapshot.pulls {
        let decision = decide(pr, &main, now, config);
        let since = match classify(pr, &config.rules) {
            State::Stuck { since, .. } => Some(since),
            _ => None,
        };
        let _ = writeln!(out, "#{} {}", pr.number, pr.title);
        let _ = writeln!(out, "  {}", describe(&decision, since, now));
    }
    out
}

fn main_line(main: &MainState) -> String {
    match &main.red {
        None => "green".into(),
        Some(red) => {
            let why = match red.reason {
                RedReason::SameFailureAcrossBatches => "different batches failed the same way",
                RedReason::ScheduledRunFailed => "the scheduled run failed",
            };
            match &red.failure {
                Some(f) => format!(
                    "RED since {} ({why}: {}, {})",
                    clock(red.since),
                    f.job,
                    f.first_error
                ),
                None => format!("RED since {} ({why})", clock(red.since)),
            }
        }
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

fn class_name(class: Class) -> &'static str {
    match class {
        Class::Conflict => "conflict",
        Class::RemovedFromQueue => "removed from the queue",
        Class::Red => "red",
        Class::NotQueued => "green, not queued",
    }
}

/// One line about a decision: what is wrong, for how long, and what lineworker would do.
pub fn describe(decision: &Decision, since: Option<Time>, now: Time) -> String {
    let for_how_long = |class: Class| match since {
        Some(s) => format!("{} for {}", class_name(class), human(now - s)),
        None => class_name(class).to_string(),
    };
    match decision {
        Decision::Skip(reason) => format!(
            "skipped: {}",
            match reason {
                SkipReason::Draft => "draft".to_string(),
                SkipReason::WorkInProgress => "not ready (title)".to_string(),
                SkipReason::ChangesRequested => "changes requested".to_string(),
                SkipReason::Label(l) => format!("label {l:?}"),
            }
        ),
        Decision::Nothing => "moving: nothing to do".into(),
        Decision::Wait { class, until, why } => {
            let why = match why {
                WaitReason::TooRecent => "not stuck long enough yet".to_string(),
                WaitReason::AuthorNotified => "its author's session was told".to_string(),
                WaitReason::MainIsRed { explained: true } => {
                    "the default branch is red and fails the same way".to_string()
                }
                WaitReason::MainIsRed { explained: false } => {
                    "the default branch is red".to_string()
                }
            };
            match until {
                Some(t) => format!(
                    "{}; waiting: {why} (until {})",
                    for_how_long(*class),
                    clock(*t)
                ),
                None => format!("{}; waiting: {why}", for_how_long(*class)),
            }
        }
        Decision::Act(action) => {
            let (class, what) = match action {
                Action::NotifyAuthor { class } => {
                    (Some(*class), "tell its author's session".to_string())
                }
                Action::Queue => (None, "put it into the merge queue".to_string()),
                Action::RerunInfrastructure { check } => (None, format!("rerun {check:?} once")),
                Action::RepairConflict => {
                    (Some(Class::Conflict), "repair the conflict".to_string())
                }
                Action::RepairFailure { class } => (Some(*class), "repair the failure".to_string()),
            };
            match class {
                Some(c) => format!("{} → would {what}", for_how_long(c)),
                None => format!("→ would {what}"),
            }
        }
        Decision::HandToOwner { class, why } => format!(
            "{} → would hand it to a person ({})",
            for_how_long(*class),
            match why {
                HandReason::RepairFailed => "a repair from this head did not fix it",
                HandReason::BudgetSpent => "the repairs of the day are spent",
            }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mainline::MainInput;
    use crate::model::Queue;
    use crate::model::tests::pull;

    const T: Time = 1_800_000_000;

    #[test]
    fn a_report_says_what_would_happen() {
        let mut queued = pull(T);
        queued.number = 1;
        let mut waiting = pull(T);
        waiting.number = 2;
        waiting.title = "feat: ready".into();
        waiting.queue = Queue::NotQueued;
        let mut draft = pull(T);
        draft.number = 3;
        draft.draft = true;
        let snapshot = Snapshot {
            repo: "acme/shop".into(),
            default_branch: "main".into(),
            pulls: vec![queued, waiting, draft],
            main: MainInput::default(),
        };
        let out = report(&snapshot, T + 7200, &Config::default());
        assert!(
            out.starts_with("acme/shop · default branch main: green\n"),
            "{out}"
        );
        assert!(
            out.contains("#1 fix: something\n  moving: nothing to do"),
            "{out}"
        );
        assert!(
            out.contains("#2 feat: ready\n  → would put it into the merge queue"),
            "{out}"
        );
        assert!(out.contains("#3 fix: something\n  skipped: draft"), "{out}");
    }

    #[test]
    fn waits_say_until_when() {
        let d = Decision::Wait {
            class: Class::Conflict,
            until: Some(T + 600),
            why: WaitReason::TooRecent,
        };
        assert_eq!(
            describe(&d, Some(T), T + 300),
            format!(
                "conflict for 5 min; waiting: not stuck long enough yet (until {})",
                clock(T + 600)
            )
        );
    }
}
