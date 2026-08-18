# Contributing to DSH Control Panel

Thanks for your interest! This document describes how this project is developed and
released on GitHub.

## Repository layout

- `main` is the default and only long-lived branch. It must always build and pass tests.
- All development happens on short-lived feature branches: `feature/<what>` (or
  `fix/<what>`, `docs/<what>`), merged back via Pull Request.

## Development workflow

1. Fork / clone the repository and install dependencies:

   ```bash
   git clone https://github.com/<you>/dsh-control-panel.git
   cd dsh-control-panel
   pnpm install
   ```

2. Create a feature branch:

   ```bash
   git checkout -b feature/my-change
   ```

3. Make your changes. Keep them focused; commit with a clear message following the
   [Conventional Commits](https://www.conventionalcommits.org/) style:

   ```
   feat: add repair-install escalation to fresh re-clone
   fix: allow plugin install while the web service is running
   docs: describe the update dialog flow
   ```

4. Verify before pushing:

   ```bash
   pnpm build              # TypeScript + Vite build
   cd src-tauri && cargo test && cargo check
   ```

5. Push and open a Pull Request against `main`:

   ```bash
   git push -u origin feature/my-change
   ```

   CI (`build.yml`) runs `pnpm build` and `cargo test` automatically. Keep the PR
   small and self-explanatory; reviewers may ask for changes.

## Code style

- Frontend: TypeScript + React (antd 5), strings go through the `t()` helper in
  `src/i18n.ts` (both `zh-CN` and `en` dictionaries must be updated together).
- Backend: Rust in `src-tauri/src`, `cargo fmt` style; user-facing error strings go
  through `crate::i18n` (`t` / `t_fmt`) or `AppError::friendly()`.
- Never commit: `docs/` (requirements / design / process documents), local configs,
  logs, build outputs (see `.gitignore`).

## Release process (zip artifacts)

Releases are tag-driven. To publish a new version:

1. Update the version in `package.json`, `src-tauri/tauri.conf.json`,
   `src-tauri/Cargo.toml` and `src/constants.ts` (they must stay in sync), commit on
   `main`, then push.
2. Create and push a tag (tags must match `v*`):

   ```bash
   git tag v1.0.0
   git push origin v1.0.0
   ```

3. The `release.yml` workflow builds the portable zip on a Windows runner and attaches
   it to a GitHub Release. After it finishes, edit the release notes on GitHub
   (summary of changes, screenshots from `assets/`, checksums if desired).

## Reporting issues

Include: DSH Control Panel version, OS version, what you did, what happened, and the
log file next to the exe (`logs/control-panel-<date>.log`) or a screenshot.
