//! The default branch first: whether it is red, and which commits came past the queue.
//!
//! A broken default branch looks like many broken pull requests: every batch of the queue
//! fails in the same place and the queue removes pull request after pull request. Repairing
//! them one by one would spend the whole budget on the wrong thing, so this is checked
//! before any pull request.

use std::collections::BTreeSet;

use crate::model::{Failure, Time};

/// One finished run of the merge queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueRun {
    /// The pull requests the run held.
    pub pulls: BTreeSet<u64>,
    pub finished_at: Time,
    /// `None` for a green run.
    pub failure: Option<Failure>,
}

/// A commit on the default branch, oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MainCommit {
    pub sha: String,
    pub at: Time,
    /// It came through the merge queue.
    pub via_queue: bool,
    /// It changes more than documentation.
    pub touches_code: bool,
}

/// What the forge adapter knows about the default branch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MainInput {
    /// Recent queue runs, in any order.
    pub runs: Vec<QueueRun>,
    /// The latest scheduled run of the default branch (a nightly build), if there is one:
    /// its time and its failure, `None` when it passed.
    pub scheduled: Option<(Time, Option<Failure>)>,
    pub commits: Vec<MainCommit>,
    /// When the last "main is red" issue was closed.
    pub red_issue_closed_at: Option<Time>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedReason {
    /// Consecutive queue runs that held different pull requests failed the same way.
    SameFailureAcrossBatches,
    /// The scheduled run of the default branch failed.
    ScheduledRunFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedMain {
    pub reason: RedReason,
    pub failure: Option<Failure>,
    pub since: Time,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MainState {
    pub red: Option<RedMain>,
    /// Commits that changed code without the queue: the likely cause of a red default branch,
    /// and worth reporting even while it is green.
    pub bypass_merges: Vec<String>,
    pub red_cleared_at: Option<Time>,
}

impl MainState {
    /// Whether a failure is the failure of the red default branch.
    pub fn explains(&self, failure: Option<&crate::model::Failure>) -> bool {
        matches!((&self.red, failure), (Some(red), Some(f)) if red.failure.as_ref() == Some(f))
    }
}

pub fn assess(input: &MainInput) -> MainState {
    let mut runs: Vec<&QueueRun> = input.runs.iter().collect();
    runs.sort_by_key(|r| r.finished_at);
    // The failed runs since the last green one, newest first.
    let failed: Vec<&QueueRun> = runs
        .iter()
        .rev()
        .take_while(|r| r.failure.is_some())
        .copied()
        .collect();

    let mut red = None;
    if let Some(latest) = failed.first() {
        let same: Vec<&&QueueRun> = failed
            .iter()
            .filter(|r| r.failure == latest.failure)
            .collect();
        let distinct_batches: BTreeSet<&BTreeSet<u64>> = same.iter().map(|r| &r.pulls).collect();
        if same.len() >= 2 && distinct_batches.len() >= 2 {
            red = Some(RedMain {
                reason: RedReason::SameFailureAcrossBatches,
                failure: latest.failure.clone(),
                since: same
                    .iter()
                    .map(|r| r.finished_at)
                    .min()
                    .unwrap_or(latest.finished_at),
            });
        }
    }
    if red.is_none()
        && let Some((at, Some(failure))) = &input.scheduled
    {
        red = Some(RedMain {
            reason: RedReason::ScheduledRunFailed,
            failure: Some(failure.clone()),
            since: *at,
        });
    }
    // A red default branch that was declared fixed after the failure began is not red any
    // more: the next queue run will tell.
    if let (Some(r), Some(closed)) = (&red, input.red_issue_closed_at)
        && closed > r.since
        && failed.iter().all(|run| run.finished_at < closed)
    {
        red = None;
    }

    MainState {
        red,
        bypass_merges: input
            .commits
            .iter()
            .filter(|c| !c.via_queue && c.touches_code)
            .map(|c| c.sha.clone())
            .collect(),
        red_cleared_at: input.red_issue_closed_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::failure;

    const T: Time = 1_800_000_000;

    fn run(pulls: &[u64], at: Time, failed: Option<&str>) -> QueueRun {
        QueueRun {
            pulls: pulls.iter().copied().collect(),
            finished_at: at,
            failure: failed.map(|e| failure("qemu", e)),
        }
    }

    #[test]
    fn one_failed_batch_is_a_broken_pull_request_not_a_broken_main() {
        let input = MainInput {
            runs: vec![run(&[1, 2], T, None), run(&[3], T + 100, Some("boom"))],
            ..Default::default()
        };
        assert_eq!(assess(&input).red, None);
    }

    #[test]
    fn different_batches_failing_the_same_way_make_main_red() {
        let input = MainInput {
            runs: vec![
                run(&[1], T, None),
                run(&[2, 3], T + 100, Some("boom")),
                run(&[4], T + 200, Some("boom")),
            ],
            ..Default::default()
        };
        let state = assess(&input);
        let red = state.red.clone().unwrap();
        assert_eq!(red.reason, RedReason::SameFailureAcrossBatches);
        assert_eq!(red.since, T + 100);
        assert!(state.explains(Some(&failure("qemu", "boom"))));
        assert!(!state.explains(Some(&failure("qemu", "other"))));
        assert!(!state.explains(None));
    }

    #[test]
    fn the_same_batch_failing_twice_or_different_failures_do_not() {
        let same_batch = MainInput {
            runs: vec![run(&[2], T, Some("boom")), run(&[2], T + 100, Some("boom"))],
            ..Default::default()
        };
        assert_eq!(assess(&same_batch).red, None);
        let different = MainInput {
            runs: vec![run(&[2], T, Some("boom")), run(&[3], T + 100, Some("bang"))],
            ..Default::default()
        };
        assert_eq!(assess(&different).red, None);
    }

    #[test]
    fn a_green_run_ends_the_streak() {
        let input = MainInput {
            runs: vec![
                run(&[2], T, Some("boom")),
                run(&[3], T + 100, Some("boom")),
                run(&[4], T + 200, None),
            ],
            ..Default::default()
        };
        assert_eq!(assess(&input).red, None);
    }

    #[test]
    fn a_failed_scheduled_run_makes_main_red() {
        let input = MainInput {
            scheduled: Some((T, Some(failure("nightly", "timeout")))),
            ..Default::default()
        };
        assert_eq!(
            assess(&input).red.unwrap().reason,
            RedReason::ScheduledRunFailed
        );
        let passed = MainInput {
            scheduled: Some((T, None)),
            ..Default::default()
        };
        assert_eq!(assess(&passed).red, None);
    }

    #[test]
    fn closing_the_red_issue_clears_it_until_the_queue_fails_again() {
        let mut input = MainInput {
            runs: vec![run(&[2], T, Some("boom")), run(&[3], T + 100, Some("boom"))],
            red_issue_closed_at: Some(T + 500),
            ..Default::default()
        };
        assert_eq!(assess(&input).red, None);
        input.runs.push(run(&[4], T + 600, Some("boom")));
        assert!(assess(&input).red.is_some());
    }

    #[test]
    fn code_merged_past_the_queue_is_named() {
        let input = MainInput {
            commits: vec![
                MainCommit {
                    sha: "a".into(),
                    at: T,
                    via_queue: true,
                    touches_code: true,
                },
                MainCommit {
                    sha: "b".into(),
                    at: T + 1,
                    via_queue: false,
                    touches_code: true,
                },
                MainCommit {
                    sha: "c".into(),
                    at: T + 2,
                    via_queue: false,
                    touches_code: false,
                },
            ],
            ..Default::default()
        };
        assert_eq!(assess(&input).bypass_merges, vec!["b".to_string()]);
    }
}
