# Design

lineworker keeps pull requests moving. It finds the ones that are stuck, repairs what it safely
can, reviews them if asked, and hands the rest to a person with a diagnosis. It never merges,
approves or touches branch protection: the merge queue or a person stays the only path to the
default branch.

This document is the plan. Nothing beyond a skeleton is built yet.

## Why

When AI agents open pull requests, the agent that opened one is often gone by the time it breaks:
its session ended, its machine is asleep. A pull request then sits red, in conflict, or out of the
merge queue for hours or days, and the only one who notices is the owner, by looking.

Experience from one repository with a merge queue and agents opening most pull requests, over two
days and about a hundred pull requests:

- **Most ejections from the queue were a failed check, and most of those were one loop.** An
  automatic pass put the same unchanged head back into the queue every ten minutes; the queue ran
  the whole suite, it failed again. Two pull requests caused about 120 full runs in 30 hours and
  slowed the queue for everyone.
- **Conflicts are what holds pull requests after their sessions end.** Four pull requests waited
  17 to 19 hours, each with one conflicting file, all of them files that every pull request of the
  day appends to (a workflow file, a `justfile`, a test runner, a docs index).
- **A broken default branch looks like many broken pull requests.** One change merged past the
  queue made every batch fail in the same test; the queue removed pull request after pull request,
  ten piled up for eight hours, and per-pull-request repair would have spent its whole budget on
  the wrong thing.

So: detect cheaply, check the default branch first, repair conflicts first, stop loops, and say
something to a person only when the machine cannot help.

## What it does

### Detector (no model)

A small loop reads the forge on a timer. It keeps no database: what it did is written as marked
comments on the pull requests and read back. For every open pull request it decides one class:

| Class | How it is seen |
| --- | --- |
| Red | A required check of the current head failed. |
| Removed from queue | The merge queue removed the current head. |
| Conflict | The forge says the pull request cannot be merged cleanly. |
| Not queued | All required checks are green and nothing scheduled a merge after the last push. |

Drafts, titles starting with `WIP:` or `Draft:`, and pull requests with a person's current
"request changes" are skipped. A pull request is acted on after it has been in one class for a
configured time (10 minutes by default) with no new commit and no comment from its author.

### The default branch first

Before classifying pull requests, the detector checks the default branch:

| Signal | How it is seen |
| --- | --- |
| Main is red | Two or more consecutive queue runs that held different pull requests failed in the same job and test, or the scheduled run of the default branch failed. |
| Bypass merge | A commit on the default branch that did not come through the queue and changes code. |

While the default branch is red, per-pull-request repair stops; pull requests whose failure matches
are not touched. The detector names the likely cause (the commits between the last green and the
first red run, a bypass merge in that range), opens or updates one issue, and, if nobody fixes it,
starts one repair whose branch is the default branch and whose pull request says which change it
fixes.

### Stop the requeue loop (no model)

A head that the queue removed for a failed check is not queued again until something changed: a
new commit in the pull request, or a new default branch after the "main is red" issue was closed.

### Order of action

1. **Tell the author**, if its session is known to be active (through
   [chappe](https://github.com/ansromanov/chappe) or another notifier), and wait a short time.
   With no active session, act at once.
2. **Without a model**: queue a green pull request that is not queued; rerun a job once if it
   failed before the repository's own commands ran (cache, runner); resolve conflicts that are
   only in files the repository can regenerate (lock files, generated indexes) by taking the
   default branch's side and running the generator.
3. **Repair with a model**, in a fresh workspace, for what remains.
4. **Hand to a person** if the repair fails or the budget is spent: one label, one comment with
   the diagnosis, one message. This is the only thing that reaches the owner.

### Repair

Input: repository, pull request, head commit, class, a link to the failing log.

1. Clone the default branch into a clean directory and fetch the pull request's branch into a
   second worktree as data. The model runs from the clean checkout of the default branch, so the
   pull request cannot bring its own agent settings, hooks, MCP servers or instruction files with
   authority.
2. Run Claude Code headless (`claude -p`) with a fixed prompt for the class, a fixed tool allow list
   (read, edit, `git`, the repository's fast check and formatters), a turn limit and JSON output.
3. By class:
   - **Conflict**: merge the default branch into the pull request's branch (a merge commit, never a
     rebase), resolve, regenerate what the repository generates.
   - **Red on formatting, lint, a policy report**: fix exactly what the log names.
   - **Red on a test**: one attempt from the log; if the cause is not clear, write a diagnosis
     instead of changing code.
   - Paths a repository marks as sensitive (for example unsafe code, memory management) get a
     diagnosis only.
4. Run the repository's fast check. Push one follow-up commit to the same branch only if it
   passes. Never force-push, amend, or open a second pull request.
5. Post one comment: class, what changed, what the check showed, and the usage line.

### Review (optional)

Once per head commit: the same workspace, a read-only tool list, a prompt that asks for findings a
reader can act on (correctness, broken rules of the repository's `AGENTS.md`), each with a file, a
line and a confidence, at most ten, no style remarks a linter already makes. Posted as one review of
type "comment" by a separate account: it never approves or requests changes, so it cannot block the
queue. Repair does not act on review findings.

## The first slice

Conflict repair, end to end, is built first: it is the smallest thing that removes the hours people
see.

1. The detector for the classes **Conflict** and **Not queued**, with the check of the default branch
   (a conflict repair on a red default branch is wasted) and the stop of the requeue loop.
2. Conflicts in generated files resolved without a model.
3. The repair for other conflicts.

Then the other classes, the repair of a red default branch, and review.

## Adapters

Everything specific to one setup is an adapter chosen in configuration:

| Concern | First adapters |
| --- | --- |
| Forge | GitHub, Gitea (then GitLab) |
| Merge queue | GitHub merge queue, a queue that comments on pull requests (such as `gitea-mq`), none |
| Runner of a repair | a local process, a container, a Kubernetes Job |
| Notifier | chappe, none |
| Model access | a Claude subscription token or an API key; the model is configuration |

## Budget and limits

- One repair or review at a time; repair before review.
- At most three repairs per pull request per day and one per head commit; a daily cap on reviews.
- A time limit and a turn limit on every run.
- When the model reports a usage limit, nothing starts until the reset time it reports.
- With a subscription, no run starts above configured shares of the five-hour and weekly limits,
  so that the owner's own sessions keep room.

## Usage accounting

Every run reports what it spent, from the final JSON of `claude -p --output-format json`: the model
actually used (a run on another model is a failure), input, output and cache tokens, turns, wall
time, the list-price equivalent, and the error if any. A run reads at least the system prompt and
tool descriptions of Claude Code, about 30 thousand tokens mostly from the cache, so runs are few
and long rather than many and short. The comment of a run ends with one line, for example
`haiku · 14 turns · 410k tokens (395k from cache) · $0.21 at list price`, and the totals are exposed
as metrics.

## Trust

- The pull request's text, diff, CI logs and comments are data. The prompts say so, and the tools
  of a run cannot reach anything beyond the branch.
- The forge token of a repair can push to branches and comment; branch protection keeps it out of
  the default branch. The review account can only comment.
- A run gets no other secret and, where the runner allows it, network access only to the forge,
  the model API and the package registries the fast check needs.

## Model access

Claude Code runs headless with either an API key or a subscription token (`claude setup-token`).
Before relying on a subscription for unattended runs, read Anthropic's current terms for your plan:
a subscription is meant for individual use, and routing other people's requests through it is not
allowed. An API key is supported as a first-class option, and nothing else depends on which one is
used.

## Deliberately not built

- No merging, approving, or changing branch protection.
- No acting on review findings automatically.
- No full slow test suite inside a repair: the merge queue runs it.
- No fallback to a second model.

## Open questions

- How the fix of a red default branch gets ahead of the waiting pull requests: a priority the queue
  honours, pausing the queue, or a person merging it. lineworker does not merge in any case.
- The exact read-only tool list for review on the pinned Claude Code version.
