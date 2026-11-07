# GitHub Actions Workflows

This directory contains the active GitHub Actions workflows for the Octocode monorepo.

## Overview

| Workflow | Trigger | Purpose |
|---|---|---|
| `ci.yml` | Pull requests | Repo health, lint, typecheck, build, test |

## CI (`ci.yml`)

The pull request workflow runs one `ci` job, with steps executed serially: set up Rust and
Node.js, enable Corepack, install dependencies, check workspace health, verify docs, build,
lint, verify build outputs, typecheck, test, install the pinned `cargo-audit` version, audit
Rust dependencies, then upload per-package coverage artifacts. Build precedes lint and
typecheck because workspace packages consume compiled sibling outputs. Coverage upload runs
even if an earlier step fails.

Useful local commands before opening a PR:

```bash
yarn health:check
yarn docs:verify
yarn build
yarn lint
yarn test
# From the repository root, install the pinned audit tool:
cargo install cargo-audit --version 0.22.2 --locked
# From packages/octocode-engine:
cargo audit -D unmaintained -D unsound -D yanked
```

If you want the full repo contract in one command, run:

```bash
yarn verify
```

## Manual Releases

npm publishing, Homebrew tap updates, and standalone binary uploads are manual.
Use the Release Guide for the current release order and verification checklist.

## Maintenance Notes

- Keep this file aligned with the actual workflow files in this directory.
- `yarn docs:verify` fails if this README references a workflow that does not exist.
