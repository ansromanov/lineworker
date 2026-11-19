//! Which class a pull request is in, and since when. No model, no network: only the
//! snapshot a forge adapter made.

use crate::model::{CheckState, Mergeable, PullRequest, Queue, RemovalReason, Time};

/// Why a pull request is stuck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// It cannot be merged cleanly into the default branch.
    Conflict,
    /// The merge queue removed its current head (for a reason other than a conflict).
    RemovedFromQueue,
    /// A required check of its current head failed.
    Red,
    /// Every required check is green and nothing scheduled a merge.
    NotQueued,
}

/// Why lineworker leaves a pull request alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    Draft,
    /// The title starts with one of the prefixes that mean "not ready".
    WorkInProgress,
    /// A person asked for changes.
    ChangesRequested,
    /// It carries a label that tells lineworker to stay away (or that a person has it).
    Label(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Skip(SkipReason),
    /// Nothing is wrong yet: checks are running, or it waits in the queue.
    Moving,
    Stuck {
        class: Class,
        since: Time,
    },
}

/// What the classifier needs from the configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Title prefixes that mean "not ready", compared without case.
    pub not_ready_prefixes: Vec<String>,
    /// Labels that keep lineworker away.
    pub skip_labels: Vec<String>,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules {
            not_ready_prefixes: vec!["wip:".into(), "draft:".into()],
            skip_labels: vec!["lineworker: off".into(), "lineworker: owner".into()],
        }
    }
}

pub fn classify(pr: &PullRequest, rules: &Rules) -> State {
    if pr.draft {
        return State::Skip(SkipReason::Draft);
    }
    let title = pr.title.trim_start().to_lowercase();
    if rules
        .not_ready_prefixes
        .iter()
        .any(|p| title.starts_with(&p.to_lowercase()))
    {
        return State::Skip(SkipReason::WorkInProgress);
    }
    if pr.changes_requested {
        return State::Skip(SkipReason::ChangesRequested);
    }
    if let Some(label) = pr
        .labels
        .iter()
        .find(|l| rules.skip_labels.iter().any(|s| s.eq_ignore_ascii_case(l)))
    {
        return State::Skip(SkipReason::Label(label.clone()));
    }

    // What the queue said about the current head, if it removed it.
    let removed = match &pr.queue {
        Queue::Removed {
            at, head, reason, ..
        } if *head == pr.head => Some((*at, *reason)),
        _ => None,
    };

    if pr.mergeable == Mergeable::Conflict {
        // A conflict is at least as old as the push and the last move of the default branch.
        let since = pr.head_pushed_at.max(pr.base_moved_at);
        return stuck(Class::Conflict, since);
    }
    if let Some((at, reason)) = removed {
        return match reason {
            RemovalReason::Conflict => stuck(Class::Conflict, at),
            // New commits change the head, so a removal of this head for new commits is the
            // forge being late; anything else is a removal for a failure.
            RemovalReason::NewCommits => State::Moving,
            RemovalReason::FailedCheck | RemovalReason::Other => stuck(Class::RemovedFromQueue, at),
        };
    }

    let required: Vec<_> = pr.checks.iter().filter(|c| c.required).collect();
    let latest = |states: &[CheckState]| {
        required
            .iter()
            .filter(|c| states.contains(&c.state))
            .filter_map(|c| c.completed_at)
            .max()
            .unwrap_or(pr.head_pushed_at)
            .max(pr.head_pushed_at)
    };
    if required.iter().any(|c| c.state == CheckState::Failure) {
        return stuck(Class::Red, latest(&[CheckState::Failure]));
    }
    if required.iter().any(|c| c.state == CheckState::Pending) {
        return State::Moving;
    }
    if pr.mergeable == Mergeable::Unknown {
        // The forge will say soon whether it merges cleanly; queueing now could be wasted.
        return State::Moving;
    }
    match pr.queue {
        Queue::Queued { .. } => State::Moving,
        // Not queued, or removed with an older head.
        _ => stuck(
            Class::NotQueued,
            latest(&[CheckState::Success, CheckState::Skipped]),
        ),
    }
}

fn stuck(class: Class, since: Time) -> State {
    State::Stuck { class, since }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::{check, failure, pull};

    const T: Time = 1_800_000_000;

    fn class_of(pr: &PullRequest) -> State {
        classify(pr, &Rules::default())
    }

    #[test]
    fn a_green_queued_pull_request_is_moving() {
        assert_eq!(class_of(&pull(T)), State::Moving);
    }

    #[test]
    fn drafts_unready_titles_requested_changes_and_labels_are_skipped() {
        let mut pr = pull(T);
        pr.draft = true;
        assert_eq!(class_of(&pr), State::Skip(SkipReason::Draft));

        let mut pr = pull(T);
        pr.title = "  WIP: half done".into();
        assert_eq!(class_of(&pr), State::Skip(SkipReason::WorkInProgress));
        pr.title = "Draft: later".into();
        assert_eq!(class_of(&pr), State::Skip(SkipReason::WorkInProgress));
        // Only a prefix counts.
        pr.title = "fix: the draft: parser".into();
        assert_ne!(class_of(&pr), State::Skip(SkipReason::WorkInProgress));

        let mut pr = pull(T);
        pr.changes_requested = true;
        assert_eq!(class_of(&pr), State::Skip(SkipReason::ChangesRequested));

        let mut pr = pull(T);
        pr.labels = vec!["bug".into(), "Lineworker: Owner".into()];
        assert_eq!(
            class_of(&pr),
            State::Skip(SkipReason::Label("Lineworker: Owner".into()))
        );
    }

    #[test]
    fn a_conflict_dates_from_the_push_or_the_move_of_the_default_branch() {
        let mut pr = pull(T);
        pr.mergeable = Mergeable::Conflict;
        pr.base_moved_at = T + 500;
        assert_eq!(
            class_of(&pr),
            State::Stuck {
                class: Class::Conflict,
                since: T + 500
            }
        );
        pr.base_moved_at = T - 500;
        assert_eq!(
            class_of(&pr),
            State::Stuck {
                class: Class::Conflict,
                since: T
            }
        );
    }

    #[test]
    fn a_removal_of_the_current_head_is_a_class_and_of_an_old_head_is_history() {
        let mut pr = pull(T);
        pr.queue = Queue::Removed {
            at: T + 900,
            head: "h1".into(),
            reason: RemovalReason::FailedCheck,
            failure: Some(failure("test", "assertion failed")),
        };
        assert_eq!(
            class_of(&pr),
            State::Stuck {
                class: Class::RemovedFromQueue,
                since: T + 900
            }
        );
        // Removed for a conflict: a conflict.
        if let Queue::Removed { reason, .. } = &mut pr.queue {
            *reason = RemovalReason::Conflict;
        }
        assert_eq!(
            class_of(&pr),
            State::Stuck {
                class: Class::Conflict,
                since: T + 900
            }
        );
        // An older head was removed; this one is green and not queued.
        pr.head = "h2".into();
        assert_eq!(
            class_of(&pr),
            State::Stuck {
                class: Class::NotQueued,
                since: T + 60
            }
        );
    }

    #[test]
    fn a_failed_required_check_is_red_and_an_optional_one_is_not() {
        let mut pr = pull(T);
        pr.queue = Queue::NotQueued;
        pr.checks = vec![
            check("lint", CheckState::Success, Some(T + 30)),
            check("test", CheckState::Failure, Some(T + 400)),
        ];
        assert_eq!(
            class_of(&pr),
            State::Stuck {
                class: Class::Red,
                since: T + 400
            }
        );
        pr.checks[1].required = false;
        assert_eq!(
            class_of(&pr),
            State::Stuck {
                class: Class::NotQueued,
                since: T + 30
            }
        );
    }

    #[test]
    fn running_checks_and_an_unknown_merge_state_are_moving() {
        let mut pr = pull(T);
        pr.queue = Queue::NotQueued;
        pr.checks.push(check("slow", CheckState::Pending, None));
        assert_eq!(class_of(&pr), State::Moving);
        let mut pr = pull(T);
        pr.queue = Queue::NotQueued;
        pr.mergeable = Mergeable::Unknown;
        assert_eq!(class_of(&pr), State::Moving);
    }

    #[test]
    fn without_required_checks_a_pull_request_waits_since_its_push() {
        let mut pr = pull(T);
        pr.queue = Queue::NotQueued;
        pr.checks.clear();
        assert_eq!(
            class_of(&pr),
            State::Stuck {
                class: Class::NotQueued,
                since: T
            }
        );
    }
}
