//! What the detector knows about a repository, in terms that do not depend on the forge.
//!
//! A forge adapter fills these types from the API of GitHub, Gitea or another forge; the
//! rest of lineworker reads only them.

/// Unix time in seconds.
pub type Time = i64;

/// Whether the forge can merge a pull request into the default branch cleanly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mergeable {
    Clean,
    Conflict,
    /// The forge has not computed it yet (it does so lazily after a push).
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckState {
    Pending,
    Success,
    Failure,
    Skipped,
}

/// One check of the head commit of a pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    /// Required by the rules of the default branch.
    pub required: bool,
    pub state: CheckState,
    pub completed_at: Option<Time>,
    /// The check failed before the repository's own commands ran (the runner, a cache, a
    /// download): rerunning it once may be enough.
    pub infrastructure: bool,
}

/// What failed, precisely enough to tell one failure from another: the job and the first
/// error line of its log.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Failure {
    pub job: String,
    pub first_error: String,
}

/// Why the merge queue removed a pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalReason {
    FailedCheck,
    Conflict,
    NewCommits,
    Other,
}

/// Where a pull request is with respect to the merge queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Queue {
    NotQueued,
    Queued {
        since: Time,
    },
    /// The latest word of the queue about this pull request: it removed `head`.
    Removed {
        at: Time,
        head: String,
        reason: RemovalReason,
        failure: Option<Failure>,
    },
}

/// One repair lineworker ran, read back from its marked comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repair {
    pub at: Time,
    /// The head the repair started from.
    pub from_head: String,
    /// The commit it pushed, if its check passed.
    pub pushed_head: Option<String>,
}

/// What lineworker already did for a pull request. It keeps no database: an adapter reads
/// this back from the marked comments lineworker left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    pub notified_author_at: Option<Time>,
    /// Heads whose infrastructure failure lineworker already reran.
    pub reruns: Vec<String>,
    pub repairs: Vec<Repair>,
}

/// One open pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub draft: bool,
    pub author: String,
    pub labels: Vec<String>,
    /// A person asked for changes and has not withdrawn the request.
    pub changes_requested: bool,
    pub head: String,
    pub head_pushed_at: Time,
    /// When the default branch last moved: a conflict is at least as old as that or the push.
    pub base_moved_at: Time,
    pub mergeable: Mergeable,
    pub checks: Vec<Check>,
    pub queue: Queue,
    pub last_author_comment_at: Option<Time>,
    /// The notifier knows a working session of the author (for example through chappe).
    pub author_session_active: bool,
    pub history: History,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A pull request that is clean, green and queued, pushed at `at`; tests change what
    /// they need.
    pub(crate) fn pull(at: Time) -> PullRequest {
        PullRequest {
            number: 7,
            title: "fix: something".into(),
            draft: false,
            author: "agent".into(),
            labels: vec![],
            changes_requested: false,
            head: "h1".into(),
            head_pushed_at: at,
            base_moved_at: at - 100,
            mergeable: Mergeable::Clean,
            checks: vec![check("ci", CheckState::Success, Some(at + 60))],
            queue: Queue::Queued { since: at + 70 },
            last_author_comment_at: None,
            author_session_active: false,
            history: History::default(),
        }
    }

    pub(crate) fn check(name: &str, state: CheckState, completed_at: Option<Time>) -> Check {
        Check {
            name: name.into(),
            required: true,
            state,
            completed_at,
            infrastructure: false,
        }
    }

    pub(crate) fn failure(job: &str, first_error: &str) -> Failure {
        Failure {
            job: job.into(),
            first_error: first_error.into(),
        }
    }
}
