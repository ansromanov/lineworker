//! What to do about one pull request, given its class and the state of the default branch.
//!
//! The order of action is: wait until it has been stuck long enough, leave everything while
//! the default branch is red, tell the author's session if it is working, do what needs no
//! model (queue, rerun an infrastructure failure), then repair, and hand to a person what
//! repair cannot fix or the budget does not allow.

use crate::classify::{Class, Rules, SkipReason, State, classify};
use crate::mainline::MainState;
use crate::model::{CheckState, PullRequest, Queue, RemovalReason, Time};

const DAY: Time = 24 * 3600;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub rules: Rules,
    /// How long a pull request stays in one class, with no push or comment of its author,
    /// before lineworker acts.
    pub act_after_secs: Time,
    /// How long to wait for an author's working session after telling it.
    pub author_wait_secs: Time,
    /// Repairs of one pull request within a day.
    pub max_repairs_per_day: usize,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            rules: Rules::default(),
            act_after_secs: 10 * 60,
            author_wait_secs: 10 * 60,
            max_repairs_per_day: 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Tell the author's working session what is wrong.
    NotifyAuthor { class: Class },
    /// Put a green pull request into the merge queue.
    Queue,
    /// Rerun a check that failed before the repository's own commands ran.
    RerunInfrastructure { check: String },
    /// Merge the default branch into the pull request and resolve the conflict.
    RepairConflict,
    /// Fix what the failing check names.
    RepairFailure { class: Class },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitReason {
    /// Not stuck long enough, or its author pushed or commented recently.
    TooRecent,
    /// The author's session was told and has time to act.
    AuthorNotified,
    /// The default branch is red; `explained` when this pull request fails the same way.
    MainIsRed { explained: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandReason {
    /// A repair started from this head and did not push a fix.
    RepairFailed,
    /// The repairs allowed for a day are spent.
    BudgetSpent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Skip(SkipReason),
    /// Nothing is wrong.
    Nothing,
    Wait {
        class: Class,
        until: Option<Time>,
        why: WaitReason,
    },
    Act(Action),
    /// Give it to a person: one label, one comment, one message.
    HandToOwner {
        class: Class,
        why: HandReason,
    },
}

pub fn decide(pr: &PullRequest, main: &MainState, now: Time, config: &Config) -> Decision {
    let (class, since) = match classify(pr, &config.rules) {
        State::Skip(reason) => return Decision::Skip(reason),
        State::Moving => return Decision::Nothing,
        State::Stuck { class, since } => (class, since),
    };

    // Stuck long enough, and the author has not shown life since.
    let quiet_since = since
        .max(pr.head_pushed_at)
        .max(pr.last_author_comment_at.unwrap_or(Time::MIN));
    if now - quiet_since < config.act_after_secs {
        return Decision::Wait {
            class,
            until: Some(quiet_since + config.act_after_secs),
            why: WaitReason::TooRecent,
        };
    }

    // A queue failure that a later "main is fixed" explains is not this pull request's fault:
    // it goes back into the queue, once.
    let removed_failure = match &pr.queue {
        Queue::Removed {
            at,
            head,
            reason: RemovalReason::FailedCheck | RemovalReason::Other,
            failure,
        } if *head == pr.head => Some((*at, failure.as_ref())),
        _ => None,
    };

    if main.red.is_some() {
        return Decision::Wait {
            class,
            until: None,
            why: WaitReason::MainIsRed {
                explained: main.explains(removed_failure.and_then(|(_, f)| f)),
            },
        };
    }

    if pr.author_session_active {
        match pr.history.notified_author_at {
            Some(at) if at >= quiet_since => {
                if now - at < config.author_wait_secs {
                    return Decision::Wait {
                        class,
                        until: Some(at + config.author_wait_secs),
                        why: WaitReason::AuthorNotified,
                    };
                }
            }
            _ => return Decision::Act(Action::NotifyAuthor { class }),
        }
    }

    match class {
        Class::NotQueued => Decision::Act(Action::Queue),
        Class::RemovedFromQueue => {
            if let (Some((removed_at, _)), Some(cleared)) = (removed_failure, main.red_cleared_at)
                && cleared > removed_at
            {
                return Decision::Act(Action::Queue);
            }
            repair_failure(pr, class, now, config)
        }
        Class::Red => repair_failure(pr, class, now, config),
        Class::Conflict => match budget(pr, now, config) {
            Some(why) => Decision::HandToOwner { class, why },
            None => Decision::Act(Action::RepairConflict),
        },
    }
}

fn repair_failure(pr: &PullRequest, class: Class, now: Time, config: &Config) -> Decision {
    // An infrastructure failure gets one rerun per head, without a model.
    if !pr.history.reruns.contains(&pr.head)
        && let Some(check) = pr
            .checks
            .iter()
            .find(|c| c.required && c.state == CheckState::Failure && c.infrastructure)
    {
        return Decision::Act(Action::RerunInfrastructure {
            check: check.name.clone(),
        });
    }
    match budget(pr, now, config) {
        Some(why) => Decision::HandToOwner { class, why },
        None => Decision::Act(Action::RepairFailure { class }),
    }
}

/// Why no repair may start, if it may not.
fn budget(pr: &PullRequest, now: Time, config: &Config) -> Option<HandReason> {
    let repairs = &pr.history.repairs;
    if repairs.iter().any(|r| r.from_head == pr.head) {
        return Some(HandReason::RepairFailed);
    }
    let today = repairs.iter().filter(|r| now - r.at < DAY).count();
    (today >= config.max_repairs_per_day).then_some(HandReason::BudgetSpent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mainline::{RedMain, RedReason};
    use crate::model::tests::{check, failure, pull};
    use crate::model::{Mergeable, Repair};

    const T: Time = 1_800_000_000;
    const LATER: Time = T + 3600;

    fn decide_now(pr: &PullRequest, main: &MainState, now: Time) -> Decision {
        decide(pr, main, now, &Config::default())
    }

    fn conflicting() -> PullRequest {
        let mut pr = pull(T);
        pr.mergeable = Mergeable::Conflict;
        pr
    }

    fn red_main(failed: Option<&str>) -> MainState {
        MainState {
            red: Some(RedMain {
                reason: RedReason::SameFailureAcrossBatches,
                failure: failed.map(|e| failure("test", e)),
                since: T,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn skipped_and_moving_pull_requests_are_left_alone() {
        let mut pr = pull(T);
        assert_eq!(
            decide_now(&pr, &MainState::default(), LATER),
            Decision::Nothing
        );
        pr.draft = true;
        assert_eq!(
            decide_now(&pr, &MainState::default(), LATER),
            Decision::Skip(SkipReason::Draft)
        );
    }

    #[test]
    fn nothing_happens_before_the_pull_request_was_stuck_long_enough() {
        let pr = conflicting();
        assert_eq!(
            decide_now(&pr, &MainState::default(), T + 300),
            Decision::Wait {
                class: Class::Conflict,
                until: Some(T + 600),
                why: WaitReason::TooRecent
            }
        );
        // A comment of the author restarts the wait.
        let mut pr = conflicting();
        pr.last_author_comment_at = Some(LATER - 60);
        assert!(matches!(
            decide_now(&pr, &MainState::default(), LATER),
            Decision::Wait {
                why: WaitReason::TooRecent,
                ..
            }
        ));
    }

    #[test]
    fn a_stuck_conflict_is_repaired() {
        assert_eq!(
            decide_now(&conflicting(), &MainState::default(), LATER),
            Decision::Act(Action::RepairConflict)
        );
    }

    #[test]
    fn a_red_default_branch_stops_every_repair_and_says_whose_failure_it_is() {
        assert_eq!(
            decide_now(&conflicting(), &red_main(Some("boom")), LATER),
            Decision::Wait {
                class: Class::Conflict,
                until: None,
                why: WaitReason::MainIsRed { explained: false }
            }
        );
        let mut pr = pull(T);
        pr.queue = Queue::Removed {
            at: T + 100,
            head: "h1".into(),
            reason: RemovalReason::FailedCheck,
            failure: Some(failure("test", "boom")),
        };
        assert_eq!(
            decide_now(&pr, &red_main(Some("boom")), LATER),
            Decision::Wait {
                class: Class::RemovedFromQueue,
                until: None,
                why: WaitReason::MainIsRed { explained: true }
            }
        );
    }

    #[test]
    fn a_working_author_is_told_first_and_given_time() {
        let mut pr = conflicting();
        pr.author_session_active = true;
        assert_eq!(
            decide_now(&pr, &MainState::default(), LATER),
            Decision::Act(Action::NotifyAuthor {
                class: Class::Conflict
            })
        );
        pr.history.notified_author_at = Some(LATER);
        assert_eq!(
            decide_now(&pr, &MainState::default(), LATER + 60),
            Decision::Wait {
                class: Class::Conflict,
                until: Some(LATER + 600),
                why: WaitReason::AuthorNotified
            }
        );
        // The author did nothing in time: lineworker acts.
        assert_eq!(
            decide_now(&pr, &MainState::default(), LATER + 601),
            Decision::Act(Action::RepairConflict)
        );
        // An idle author is not waited for.
        let mut pr = conflicting();
        pr.author_session_active = false;
        assert_eq!(
            decide_now(&pr, &MainState::default(), LATER),
            Decision::Act(Action::RepairConflict)
        );
    }

    #[test]
    fn a_green_pull_request_that_nobody_queued_is_queued() {
        let mut pr = pull(T);
        pr.queue = Queue::NotQueued;
        assert_eq!(
            decide_now(&pr, &MainState::default(), LATER),
            Decision::Act(Action::Queue)
        );
    }

    #[test]
    fn a_head_the_queue_rejected_is_not_queued_again_unless_main_was_fixed_since() {
        let mut pr = pull(T);
        pr.queue = Queue::Removed {
            at: T + 100,
            head: "h1".into(),
            reason: RemovalReason::FailedCheck,
            failure: Some(failure("test", "boom")),
        };
        // The loop of putting the same head back is broken: it is repaired instead.
        assert_eq!(
            decide_now(&pr, &MainState::default(), LATER),
            Decision::Act(Action::RepairFailure {
                class: Class::RemovedFromQueue
            })
        );
        let fixed = MainState {
            red_cleared_at: Some(T + 200),
            ..Default::default()
        };
        assert_eq!(decide_now(&pr, &fixed, LATER), Decision::Act(Action::Queue));
        let fixed_before = MainState {
            red_cleared_at: Some(T + 50),
            ..Default::default()
        };
        assert_eq!(
            decide_now(&pr, &fixed_before, LATER),
            Decision::Act(Action::RepairFailure {
                class: Class::RemovedFromQueue
            })
        );
    }

    #[test]
    fn an_infrastructure_failure_is_rerun_once_then_repaired() {
        let mut pr = pull(T);
        pr.queue = Queue::NotQueued;
        let mut flaky = check("test", CheckState::Failure, Some(T + 100));
        flaky.infrastructure = true;
        pr.checks = vec![flaky];
        assert_eq!(
            decide_now(&pr, &MainState::default(), LATER),
            Decision::Act(Action::RerunInfrastructure {
                check: "test".into()
            })
        );
        pr.history.reruns.push("h1".into());
        assert_eq!(
            decide_now(&pr, &MainState::default(), LATER),
            Decision::Act(Action::RepairFailure { class: Class::Red })
        );
    }

    #[test]
    fn a_failed_repair_or_a_spent_budget_goes_to_the_owner() {
        let mut pr = conflicting();
        pr.history.repairs.push(Repair {
            at: T + 700,
            from_head: "h1".into(),
            pushed_head: None,
        });
        assert_eq!(
            decide_now(&pr, &MainState::default(), LATER),
            Decision::HandToOwner {
                class: Class::Conflict,
                why: HandReason::RepairFailed
            }
        );

        let mut pr = conflicting();
        pr.head = "h4".into();
        for (i, head) in ["h1", "h2", "h3"].iter().enumerate() {
            pr.history.repairs.push(Repair {
                at: LATER - 3000 + i as Time,
                from_head: head.to_string(),
                pushed_head: Some(format!("{head}+fix")),
            });
        }
        assert_eq!(
            decide_now(&pr, &MainState::default(), LATER),
            Decision::HandToOwner {
                class: Class::Conflict,
                why: HandReason::BudgetSpent
            }
        );
        // A day later the budget is back.
        assert_eq!(
            decide_now(&pr, &MainState::default(), LATER + DAY),
            Decision::Act(Action::RepairConflict)
        );
    }
}
