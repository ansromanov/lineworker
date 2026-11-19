//! GitHub: reads a repository through the REST API into the forge-neutral model.
//!
//! What it reads, per open pull request: the pull request (mergeability, auto-merge,
//! labels), the check runs and commit statuses of its head, the reviews, the comments (the
//! author's last sign of life and lineworker's own markers), and the time of the head
//! commit. Per repository: the required checks of the default branch (from its rules), the
//! latest scheduled run of the default branch and its recent commits.
//!
//! Limits of this first version: GitHub's merge queue is not read yet (a pull request with
//! auto-merge enabled counts as queued); which commits touch code is not known (every commit
//! that did not come from a pull request counts as a bypass merge); no check is recognized as
//! an infrastructure failure.

use std::time::Duration;

use serde_json::Value;

use crate::mainline::{MainCommit, MainInput};
use crate::markers;
use crate::model::{Check, CheckState, Failure, Mergeable, PullRequest, Queue, Time};
use crate::time::parse_rfc3339;

const API: &str = "https://api.github.com";

/// Everything the detector needs about one repository.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub repo: String,
    pub default_branch: String,
    pub pulls: Vec<PullRequest>,
    pub main: MainInput,
}

pub struct GitHub {
    api: String,
    token: Option<String>,
    agent: ureq::Agent,
}

impl GitHub {
    /// A client for `api` (normally `https://api.github.com`). Without a token only public
    /// repositories can be read, at 60 requests an hour.
    pub fn new(api: Option<&str>, token: Option<String>) -> GitHub {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .into();
        GitHub {
            api: api.unwrap_or(API).trim_end_matches('/').to_string(),
            token: token.filter(|t| !t.trim().is_empty()),
            agent,
        }
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        let mut req = self
            .agent
            .get(format!("{}{path}", self.api))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "lineworker");
        if let Some(t) = &self.token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        let mut res = req.call().map_err(|e| format!("GET {path}: {e}"))?;
        let status = res.status().as_u16();
        let body = res.body_mut().read_to_string().unwrap_or_default();
        if !(200..300).contains(&status) {
            let message = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v["message"].as_str().map(str::to_string))
                .unwrap_or_default();
            return Err(format!("GET {path}: HTTP {status} {message}")
                .trim()
                .to_string());
        }
        serde_json::from_str(&body).map_err(|e| format!("GET {path}: {e}"))
    }

    /// Reads `owner/name` into a snapshot.
    pub fn snapshot(&self, repo: &str) -> Result<Snapshot, String> {
        let r = format!("/repos/{repo}");
        let info = self.get(&r)?;
        let default_branch = info["default_branch"]
            .as_str()
            .ok_or("the repository has no default branch")?
            .to_string();
        // Rules may not be readable (or not exist): then every check counts as required.
        let required = self
            .get(&format!("{r}/rules/branches/{default_branch}"))
            .map(|v| required_checks(&v))
            .unwrap_or_default();
        let commits = self.get(&format!("{r}/commits?sha={default_branch}&per_page=30"))?;
        let base_moved_at = commits[0]["commit"]["committer"]["date"]
            .as_str()
            .and_then(parse_rfc3339)
            .unwrap_or(0);
        let scheduled = self
            .get(&format!(
                "{r}/actions/runs?branch={default_branch}&event=schedule&per_page=1"
            ))
            .ok()
            .and_then(|v| scheduled_run(&v));

        let mut pulls = Vec::new();
        for item in self
            .get(&format!("{r}/pulls?state=open&per_page=100"))?
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let Some(number) = item["number"].as_u64() else {
                continue;
            };
            let detail = self.get(&format!("{r}/pulls/{number}"))?;
            let head = detail["head"]["sha"].as_str().unwrap_or_default();
            let head_commit = self.get(&format!("{r}/commits/{head}"))?;
            let runs = self.get(&format!("{r}/commits/{head}/check-runs?per_page=100"))?;
            let statuses = self.get(&format!("{r}/commits/{head}/status"))?;
            let reviews = self.get(&format!("{r}/pulls/{number}/reviews?per_page=100"))?;
            let comments = self.get(&format!("{r}/issues/{number}/comments?per_page=100"))?;
            pulls.push(pull_request(PullData {
                detail: &detail,
                head_commit: &head_commit,
                checks: checks(&runs, &statuses, &required),
                reviews: &reviews,
                comments: &comments,
                base_moved_at,
            }));
        }

        Ok(Snapshot {
            repo: repo.to_string(),
            default_branch,
            pulls,
            main: MainInput {
                runs: vec![],
                scheduled,
                commits: main_commits(&commits),
                red_issue_closed_at: None,
            },
        })
    }
}

/// The required checks of a branch, from `GET /repos/{repo}/rules/branches/{branch}`.
pub fn required_checks(rules: &Value) -> Vec<String> {
    rules
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["type"] == "required_status_checks")
        .flat_map(|r| {
            r["parameters"]["required_status_checks"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|c| c["context"].as_str().map(str::to_string))
        .collect()
}

/// The checks of a head: its check runs and its commit statuses. With no required checks
/// known, every check counts as required.
pub fn checks(runs: &Value, statuses: &Value, required: &[String]) -> Vec<Check> {
    let is_required = |name: &str| required.is_empty() || required.iter().any(|r| r == name);
    let mut out: Vec<Check> = runs["check_runs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|run| {
            let name = run["name"].as_str().unwrap_or_default().to_string();
            let state = match (run["status"].as_str(), run["conclusion"].as_str()) {
                (Some("completed"), Some("success" | "neutral")) => CheckState::Success,
                (Some("completed"), Some("skipped")) => CheckState::Skipped,
                (Some("completed"), Some(_)) => CheckState::Failure,
                _ => CheckState::Pending,
            };
            Check {
                required: is_required(&name),
                name,
                state,
                completed_at: run["completed_at"].as_str().and_then(parse_rfc3339),
                infrastructure: false,
            }
        })
        .collect();
    for status in statuses["statuses"].as_array().into_iter().flatten() {
        let name = status["context"].as_str().unwrap_or_default().to_string();
        let state = match status["state"].as_str() {
            Some("success") => CheckState::Success,
            Some("failure" | "error") => CheckState::Failure,
            _ => CheckState::Pending,
        };
        let done = state != CheckState::Pending;
        out.push(Check {
            required: is_required(&name),
            name,
            state,
            completed_at: done
                .then(|| status["updated_at"].as_str().and_then(parse_rfc3339))
                .flatten(),
            infrastructure: false,
        });
    }
    out
}

/// Whether a reviewer's latest review asks for changes.
pub fn changes_requested(reviews: &Value) -> bool {
    let mut latest: Vec<(&str, &str)> = Vec::new();
    for review in reviews.as_array().into_iter().flatten() {
        let (Some(user), Some(state)) =
            (review["user"]["login"].as_str(), review["state"].as_str())
        else {
            continue;
        };
        // Comments do not change a reviewer's verdict.
        if !matches!(state, "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED") {
            continue;
        }
        match latest.iter_mut().find(|(u, _)| *u == user) {
            Some(entry) => entry.1 = state,
            None => latest.push((user, state)),
        }
    }
    latest.iter().any(|(_, s)| *s == "CHANGES_REQUESTED")
}

/// The latest scheduled run of the default branch: its time and its failure.
pub fn scheduled_run(runs: &Value) -> Option<(Time, Option<Failure>)> {
    let run = runs["workflow_runs"].as_array()?.first()?;
    if run["status"] != "completed" {
        return None;
    }
    let at = run["updated_at"].as_str().and_then(parse_rfc3339)?;
    let failed = !matches!(
        run["conclusion"].as_str(),
        Some("success" | "skipped" | "neutral")
    );
    let failure = failed.then(|| Failure {
        job: run["name"].as_str().unwrap_or("scheduled run").to_string(),
        first_error: format!(
            "the scheduled run {}",
            run["conclusion"].as_str().unwrap_or("failed")
        ),
    });
    Some((at, failure))
}

/// The recent commits of the default branch, oldest first, along the first parents only: the
/// commits of a branch that a merge commit brought in are not commits of the default branch.
/// A commit that came from a pull request (a squash `... (#12)` or a merge `Merge pull request
/// #12`) counts as via the queue.
pub fn main_commits(commits: &Value) -> Vec<MainCommit> {
    let list: Vec<&Value> = commits.as_array().into_iter().flatten().collect();
    let by_sha = |sha: &str| list.iter().copied().find(|c| c["sha"] == sha);
    let mut out = Vec::new();
    let mut next = list.first().copied();
    while let Some(c) = next {
        let message = c["commit"]["message"].as_str().unwrap_or_default();
        let subject = message.lines().next().unwrap_or_default();
        if let (Some(sha), Some(at)) = (
            c["sha"].as_str(),
            c["commit"]["committer"]["date"]
                .as_str()
                .and_then(parse_rfc3339),
        ) {
            out.push(MainCommit {
                sha: sha.to_string(),
                at,
                via_queue: subject.starts_with("Merge pull request #")
                    || ends_with_pr_number(subject),
                touches_code: true,
            });
        }
        next = c["parents"][0]["sha"].as_str().and_then(by_sha);
    }
    out.reverse();
    out
}

fn ends_with_pr_number(subject: &str) -> bool {
    subject
        .trim_end()
        .strip_suffix(')')
        .and_then(|s| s.rsplit_once("(#"))
        .is_some_and(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// What one pull request is made of.
pub struct PullData<'a> {
    pub detail: &'a Value,
    pub head_commit: &'a Value,
    pub checks: Vec<Check>,
    pub reviews: &'a Value,
    pub comments: &'a Value,
    pub base_moved_at: Time,
}

pub fn pull_request(d: PullData<'_>) -> PullRequest {
    let pr = d.detail;
    let author = pr["user"]["login"].as_str().unwrap_or_default().to_string();
    let head_pushed_at = d.head_commit["commit"]["committer"]["date"]
        .as_str()
        .and_then(parse_rfc3339)
        .unwrap_or(0);
    let mergeable = match (pr["mergeable"].as_bool(), pr["mergeable_state"].as_str()) {
        (Some(false), _) | (_, Some("dirty")) => Mergeable::Conflict,
        (Some(true), _) => Mergeable::Clean,
        _ => Mergeable::Unknown,
    };
    let queue = if pr["auto_merge"].is_object() {
        Queue::Queued {
            since: head_pushed_at,
        }
    } else {
        Queue::NotQueued
    };
    let comments: Vec<(&str, &str, Time)> = d
        .comments
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some((
                c["user"]["login"].as_str().unwrap_or_default(),
                c["body"].as_str().unwrap_or_default(),
                c["created_at"].as_str().and_then(parse_rfc3339)?,
            ))
        })
        .collect();
    let last_author_comment_at = comments
        .iter()
        .filter(|(user, body, _)| *user == author && !markers::is_marked(body))
        .map(|(_, _, at)| *at)
        .max();
    PullRequest {
        number: pr["number"].as_u64().unwrap_or_default(),
        title: pr["title"].as_str().unwrap_or_default().to_string(),
        draft: pr["draft"].as_bool().unwrap_or(false),
        labels: pr["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l["name"].as_str().map(str::to_string))
            .collect(),
        changes_requested: changes_requested(d.reviews),
        head: pr["head"]["sha"].as_str().unwrap_or_default().to_string(),
        head_pushed_at,
        base_moved_at: d.base_moved_at,
        mergeable,
        checks: d.checks,
        queue,
        last_author_comment_at,
        author_session_active: false,
        history: markers::history(comments.iter().map(|(_, body, at)| (*body, *at))),
        author,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn required_checks_come_from_the_rules_of_the_branch() {
        let rules = json!([
            {"type": "deletion"},
            {"type": "required_status_checks", "parameters": {"required_status_checks": [
                {"context": "CI passed", "integration_id": 15368}
            ]}}
        ]);
        assert_eq!(required_checks(&rules), vec!["CI passed".to_string()]);
        assert!(required_checks(&json!({"message": "Not Found"})).is_empty());
    }

    #[test]
    fn check_runs_and_statuses_become_checks() {
        let runs = json!({"check_runs": [
            {"name": "CI passed", "status": "completed", "conclusion": "success", "completed_at": "2026-11-19T06:00:00Z"},
            {"name": "lint", "status": "completed", "conclusion": "failure", "completed_at": "2026-11-19T06:01:00Z"},
            {"name": "docs", "status": "completed", "conclusion": "skipped", "completed_at": null},
            {"name": "slow", "status": "in_progress", "conclusion": null, "completed_at": null}
        ]});
        let statuses = json!({"statuses": [
            {"context": "external/ci", "state": "error", "updated_at": "2026-11-19T06:02:00Z"}
        ]});
        let all = checks(&runs, &statuses, &["CI passed".into()]);
        let state: Vec<_> = all
            .iter()
            .map(|c| (c.name.as_str(), c.state, c.required))
            .collect();
        assert_eq!(
            state,
            vec![
                ("CI passed", CheckState::Success, true),
                ("lint", CheckState::Failure, false),
                ("docs", CheckState::Skipped, false),
                ("slow", CheckState::Pending, false),
                ("external/ci", CheckState::Failure, false),
            ]
        );
        assert_eq!(all[0].completed_at, parse_rfc3339("2026-11-19T06:00:00Z"));
        // With no rules, every check counts.
        assert!(checks(&runs, &statuses, &[]).iter().all(|c| c.required));
    }

    #[test]
    fn the_latest_verdict_of_each_reviewer_counts() {
        let reviews = json!([
            {"user": {"login": "ann"}, "state": "CHANGES_REQUESTED"},
            {"user": {"login": "bob"}, "state": "APPROVED"},
            {"user": {"login": "ann"}, "state": "COMMENTED"}
        ]);
        assert!(changes_requested(&reviews));
        let resolved = json!([
            {"user": {"login": "ann"}, "state": "CHANGES_REQUESTED"},
            {"user": {"login": "ann"}, "state": "APPROVED"}
        ]);
        assert!(!changes_requested(&resolved));
    }

    #[test]
    fn a_pull_request_is_read_with_its_history() {
        let detail = json!({
            "number": 12, "title": "fix: x", "draft": false,
            "user": {"login": "agent"}, "labels": [{"name": "bug"}],
            "head": {"sha": "abc"}, "mergeable": false, "mergeable_state": "dirty",
            "auto_merge": null
        });
        let head_commit = json!({"commit": {"committer": {"date": "2026-11-19T05:00:00Z"}}});
        let comments = json!([
            {"user": {"login": "agent"}, "body": "rebased", "created_at": "2026-11-19T05:10:00Z"},
            {"user": {"login": "agent"}, "body": "<!-- lineworker: notified -->\nThe PR conflicts.", "created_at": "2026-11-19T05:20:00Z"},
            {"user": {"login": "other"}, "body": "<!-- lineworker: repair from=abc pushed=none -->", "created_at": "2026-11-19T05:30:00Z"}
        ]);
        let pr = pull_request(PullData {
            detail: &detail,
            head_commit: &head_commit,
            checks: vec![],
            reviews: &json!([]),
            comments: &comments,
            base_moved_at: 1,
        });
        assert_eq!(
            (pr.number, pr.author.as_str(), pr.head.as_str()),
            (12, "agent", "abc")
        );
        assert_eq!(pr.mergeable, Mergeable::Conflict);
        assert_eq!(pr.queue, Queue::NotQueued);
        assert_eq!(pr.labels, vec!["bug".to_string()]);
        // The marked comment is lineworker's, not a sign of life of the author.
        assert_eq!(
            pr.last_author_comment_at,
            parse_rfc3339("2026-11-19T05:10:00Z")
        );
        assert_eq!(
            pr.history.notified_author_at,
            parse_rfc3339("2026-11-19T05:20:00Z")
        );
        assert_eq!(pr.history.repairs.len(), 1);
        // Auto-merge enabled counts as queued; a null mergeable is unknown.
        let mut queued = detail.clone();
        queued["auto_merge"] = json!({"merge_method": "squash"});
        queued["mergeable"] = Value::Null;
        queued["mergeable_state"] = json!("unknown");
        let pr = pull_request(PullData {
            detail: &queued,
            head_commit: &head_commit,
            checks: vec![],
            reviews: &json!([]),
            comments: &json!([]),
            base_moved_at: 1,
        });
        assert!(matches!(pr.queue, Queue::Queued { .. }));
        assert_eq!(pr.mergeable, Mergeable::Unknown);
    }

    #[test]
    fn commits_from_pull_requests_are_not_bypass_merges() {
        // As GitHub lists them: newest first, branch commits of a merge included.
        let commit = |sha: &str, parents: &[&str], message: &str, hour: u32| {
            json!({"sha": sha, "parents": parents.iter().map(|p| json!({"sha": p})).collect::<Vec<_>>(),
                   "commit": {"message": message, "committer": {"date": format!("2026-11-19T0{hour}:00:00Z")}}})
        };
        let commits = json!([
            commit("c4", &["c3"], "hotfix straight to main", 4),
            commit(
                "c3",
                &["c1", "b1"],
                "Merge pull request #2 from x/y\n\nbody",
                3
            ),
            commit("b1", &["c1"], "a commit of the branch", 2),
            commit("c1", &["c0"], "feat: thing (#1)", 1),
            commit("c0", &[], "initial commit", 0)
        ]);
        let main = main_commits(&commits);
        assert_eq!(
            main.iter()
                .map(|c| (c.sha.as_str(), c.via_queue))
                .collect::<Vec<_>>(),
            vec![("c0", false), ("c1", true), ("c3", true), ("c4", false)]
        );
        assert!(!ends_with_pr_number("fix (#)") && !ends_with_pr_number("fix (#a1)"));
    }

    #[test]
    fn the_latest_scheduled_run_is_read() {
        let failed = json!({"workflow_runs": [
            {"name": "nightly", "status": "completed", "conclusion": "failure", "updated_at": "2026-11-19T02:00:00Z"}
        ]});
        let (at, failure) = scheduled_run(&failed).unwrap();
        assert_eq!(at, parse_rfc3339("2026-11-19T02:00:00Z").unwrap());
        assert_eq!(failure.unwrap().job, "nightly");
        let passed = json!({"workflow_runs": [
            {"name": "nightly", "status": "completed", "conclusion": "success", "updated_at": "2026-11-19T02:00:00Z"}
        ]});
        assert_eq!(scheduled_run(&passed).unwrap().1, None);
        assert_eq!(scheduled_run(&json!({"workflow_runs": []})), None);
    }
}
