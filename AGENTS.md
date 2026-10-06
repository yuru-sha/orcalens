# AGENTS.md

This repository uses Codex as the primary AI coding agent.

## Project context

Before making changes:

1. Read `README.md` and the relevant documentation under `docs/`.
2. Inspect nearby code and tests before proposing new abstractions.
3. Check existing Issues and Pull Requests when the task may already be tracked.
4. Replace the template-specific guidance in this file with concrete project commands and constraints after creating a repository from this template.

## Development principles

- Prefer the smallest change that fully solves the stated problem.
- Avoid speculative features, abstractions, dependencies, and agent infrastructure.
- Follow the existing architecture and naming conventions unless the task explicitly changes them.
- Use test-driven development when practical for behavior changes and bug fixes.
- Add regression coverage for corrected defects when it provides durable value.
- Never commit secrets, credentials, private tokens, or sensitive user data.
- Do not report checks as passing unless they were actually run.

## Code and comment guidance

Use each artifact to communicate a different kind of intent:

- Code should explain **How** the behavior is implemented.
- Test code should explain **What** behavior is expected.
- Commit messages should explain **Why** the change was made.
- Code comments should explain **Why not**: document non-obvious constraints, rejected alternatives, trade-offs, or reasons the seemingly simpler approach is incorrect.

Do not use comments to restate what the code already makes clear.

## Canonical commands

Replace this section with the exact commands supported by the project.

- Setup: TODO
- Test: TODO
- Lint / format: TODO
- Typecheck / static analysis: TODO
- Build: TODO

If a command is unavailable or already failing on the base branch, document that fact rather than broadening an unrelated change.

## Documentation layout

Keep these files at the repository root:

- `README.md`
- `README.ja.md`
- `CONTRIBUTING.md`
- `LICENSE`
- `AGENTS.md`

Put other long-form documentation under `docs/` unless a root-level location is required by a tool, ecosystem convention, or GitHub feature. Examples of reasonable exceptions include files such as `CHANGELOG.md` when the project uses them as a conventional root artifact.

Keep links valid after moving or renaming documentation.

## Branch And Pull Request Workflow

- Do not edit, commit, or push directly to `main`. Make changes on a feature branch and merge them through a pull request.
- Direct work on `main` is allowed only when the user explicitly authorizes it.

## Pull requests

A completed change should make it clear:

- what changed and why;
- which Issue or acceptance criteria it addresses;
- which validation commands were run;
- whether compatibility or migration is affected;
- what risks or follow-up work remain.
