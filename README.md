# lineworker

**Keeps your pull requests moving: finds the stuck ones, repairs what it safely can, never merges.**

[![CI](https://github.com/ansromanov/lineworker/actions/workflows/ci.yml/badge.svg)](https://github.com/ansromanov/lineworker/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

When AI agents open pull requests, the agent that opened one is often gone by the time it breaks.
The pull request then sits red, in conflict, or out of the merge queue until someone looks.
lineworker is the one who looks, on a timer:

- a **detector without a model** sorts open pull requests into red, removed from the queue, in
  conflict, or green and not queued, and checks the default branch first, so that one broken
  `main` is not mistaken for many broken pull requests;
- **cheap fixes need no model**: a green pull request is queued, a flaky infrastructure step is rerun
  once, a conflict in a lock file or a generated index is resolved by regenerating it, and a head
  the queue rejected is not put back until something changed;
- **a repair** runs Claude Code headless in a clean workspace for what remains, pushes one commit
  only if the repository's own fast check passes, and reports what it spent;
- whatever it cannot fix goes to a person as **one label, one comment, one message**.

It never merges, approves or changes branch protection. Repairing conflicts comes first, because
that is what keeps pull requests waiting for hours after their authors are gone.

## Status

Early. The decisions are built and tested (`src/classify.rs`, `src/mainline.rs`, `src/decide.rs`);
lineworker can read a GitHub repository and say what it would do, but it does nothing yet:

```bash
GITHUB_TOKEN=... lineworker scan --github owner/repo
```

```text
acme/shop · default branch main: green
#41 feat: refunds
  conflict for 3 h → would repair the conflict
#42 fix: typo
  green, not queued for 2 h → would put it into the merge queue
#43 WIP: new checkout
  skipped: not ready (title)
```

Next: Gitea and its merge queue, conflict repair without a model (lock files, generated files),
then repair with Claude Code. The plan is in [`docs/design.md`](docs/design.md). lineworker works
on its own and can tell the author's session first through
[chappe](https://github.com/ansromanov/chappe), the coordination chat for AI agents.

## Build

```bash
cargo build
cargo test
```

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
