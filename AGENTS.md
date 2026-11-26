# AGENTS.md — lineworker

> Canonical instructions for this repository, read by Claude Code, Codex and other
> agents. Keep it short: no file tree and no symbol index, they rot. Use `ls`, `grep`
> and the module docs instead.

lineworker is a timer-driven helper that finds stuck pull requests, repairs what it safely can and never merges. Licence: MIT OR Apache-2.0. This repository is public: nothing private
(credentials, host names, internal paths, other people's licensed material) goes in.

## Layout

- `src/classify.rs`, `src/mainline.rs`, `src/decide.rs` — the model-free detector and the decisions.
- `src/github.rs` — reads a GitHub repository; `src/scan.rs` ties them together.
- `docs/design.md` — the design; read it before changing behaviour.

## Rules worth knowing

- lineworker never merges, approves or changes branch protection. Keep it that way.
- Decisions are pure functions of what was read; keep I/O out of `classify`, `mainline` and `decide`.
- Check the default branch first, so one broken `main` is not mistaken for many broken pull requests.

## Gate

Run this before opening a pull request. CI runs the same checks.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
python3 scripts/check-file-size.py
```

## Testing

Unit tests sit next to the code in `#[cfg(test)]` modules; decisions are tested with plain values, not the network. Add or update tests with every behaviour change; say in the pull request why
something cannot be tested.

## Workflow

- Work in a git worktree on a branch, never on `main`. Fetch and base the branch on the latest `origin/main`.
- `main` is protected: every change is a pull request with green CI. Never push to `main`.
- Commit messages and pull request titles are [Conventional Commits](https://www.conventionalcommits.org)
  (`feat:`, `fix:`, `docs:`, `refactor:`, `ci:`, `chore:`); the changelog and versions come from them.
- Files stay at or below 800 production lines (`scripts/check-file-size.py`). Split the module; never raise
  the limit or add to `scripts/file-size-baseline.txt` (it can only shrink).
- Update the documentation in the same pull request as the behaviour: README.md and `docs/design.md`.
- Not released yet (`version = "0.0.0"`). Releases will follow the same Conventional Commits and `git-cliff` flow as the other repositories.
- Never commit secrets. CI scans for them; a finding is a blocker, not noise.
