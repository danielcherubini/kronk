# Rename kronk → tama

## Goal

Rename the project from `kronk` to `tama` because the name `kronk` is already

**Status:** ✅ COMPLETED - See git commits `6d3a220` ("docs: rename kronk -> tama across README, AGENTS, TODO, MIGRATION, plans"), `8281739` ("chore: rename workspace crates from kronk-* to tama-*"), `ab25016` ("refactor: rename HTTP API routes /kronk/v1 -> /tama/v1 and OpenAPI specs"), `bb8b734` ("refactor: rename Rust identifiers and imports kronk -> tama"), `d731eab` ("refactor(platform): rename service names kronk -> tama")
taken by another similar project. This is a hard rename with no backward
compatibility: all crates, binary names, HTTP API routes, environment
variables, data directories, service names, installer, CI workflows, and
documentation must reference `tama` exclusively. The only concession to
existing users is a one-time auto-migration of the user data directory
(`~/.config/kronk` → `~/.config/tama`) on first run of the new binary.

At the same time, remove the deprecated `proxy start` subcommand entirely and
replace all Emperor's New Groove joke quotes with plain, boring status
messages.

## Architecture

- **Cargo workspace** with four crates, renamed to `tama-core`, `tama-cli`,
  `tama-mock`, `tama-web`. The CLI crate's package name is `tama` and it
  produces a single binary called `tama`.
- **HTTP API** served by `tama-core` exposes management routes under
  `/tama/v1/*` (clean break from `/kronk/v1/*`). The `tama-web` Leptos SSR
  frontend proxies these routes from the browser.
- **User data** lives under the directory returned by
  `ProjectDirs::from("", "", "tama")`. On first run, if that directory does
  not exist but a legacy `ProjectDirs::from("", "", "kronk")` directory does,
  the new binary renames (or copies+removes) the legacy directory to the new
  location and logs a one-line notice. The SQLite database inside is renamed
  from `kronk.db` → `tama.db` at the same time.
- **Platform services** (systemd unit on Linux, Windows service, Windows
  firewall rule) are renamed from `kronk`/`kronk.service` → `tama`/
  `tama.service`. No migration is performed for existing installations —
  users reinstall.
- **Environment variables** prefixed `KRONK_*` become `TAMA_*`. No fallback
  reads of the old names.
- **Installer** `installer/kronk.iss` is renamed to `installer/tama.iss` and
  its AppId, AppName, default install dir, and output file name are updated.
- **CI workflows** (`.github/workflows/*.yml`) build `tama-*` crates, produce
  `tama` artifacts, and invoke the renamed installer.

## Tech Stack

- Rust stable, Cargo workspace
- `anyhow`, `tokio`, `axum`, `leptos`, `sqlx`/`rusqlite`
- `directories` crate for platform data paths
- Trunk for building the `tama-web` WASM frontend
- Inno Setup for the Windows installer
- GitHub Actions for CI/CD

---

## Task 1: Repo-root cleanup — COMPLETED

**Context:** Scratch files and unused binaries had accumulated at the repo
root and needed to be removed before the rename to keep the diff clean.

**Files:**
- `test_path`, `test_zip`, `llama.log` (scratch binaries/logs)
- `patch_delta_net.py`, `test_inference.py`, `rebuild_ik.bat` (one-off scripts)
- `migration_summary_report.md` (stale doc)
- `unsloth/`, top-level `kronk-core/` stub, `worktree/` (stale directories)
- `.gitignore` (add `.ruff_cache/` and `worktree/`)
- `docs/plans/2026-04-06-dashboard-time-series-graphs.md` (untracked, commit it)

**What to implement:** Delete the scratch files and directories, tidy
`.gitignore`, and commit the previously-untracked dashboard time-series plan
alongside the cleanup.

**Acceptance criteria:**
- Repo root contains only project-relevant files and directories.
- `git status` is clean.
- Committed on `main` as `chore: remove scratch files and unused binaries
  from repo root` (commit `8965c0a`).

---

## Task 2: Rename workspace crates — COMPLETED

**Context:** The workspace currently has four `kronk-*` crates. Renaming the
directories and `Cargo.toml` package names is the structural foundation of
the rename; everything else hangs off this.

**Files:**
- `Cargo.toml` (workspace root, `members` list)
- `Cargo.lock` (regenerate)
- `crates/kronk-core/` → `crates/tama-core/`
- `crates/kronk-cli/` → `crates/tama-cli/`
- `crates/kronk-mock/` → `crates/tama-mock/`
- `crates/kronk-web/` → `crates/tama-web/`
- Each renamed crate's `Cargo.toml` (package name, dependency names, binary
  name, repo URL, deb/rpm metadata, description)
- `crates/tama-core/src/proxy/kronk_handlers.rs` →
  `crates/tama-core/src/proxy/tama_handlers.rs` (file rename + `mod.rs`
  update)
- `crates/tama-core/src/proxy/server/router.rs` (point `/tama/v1/*` routes at
  the renamed handlers — this fixup is necessary for the workspace to build
  after Task 3)

**What to implement:**
1. `git mv` each crate directory from `kronk-*` to `tama-*`.
2. Update the workspace `Cargo.toml` `members` list.
3. Update each crate's `Cargo.toml`: package `name`, any
   `dependencies.tama-*` entries (paths and names), binary `name` in
   `[[bin]]`, `repository` URL, and (for the CLI) `[package.metadata.deb]` /
   `[package.metadata.generate-rpm]` fields.
4. Delete `Cargo.lock` and let `cargo` regenerate it on the next build.
5. Rename `kronk_handlers.rs` → `tama_handlers.rs`, update the `mod`
   declaration in `crates/tama-core/src/proxy/mod.rs`, and rewrite
   `router.rs` to use the renamed module and route prefix so the workspace
   compiles after the follow-up identifier rename.

**Acceptance criteria:**
- `cargo metadata --format-version=1` lists `tama-core`, `tama-cli`,
  `tama-mock`, `tama-web` and no `kronk-*` crates.
- `cargo build --workspace` succeeds after Task 3.
- Committed on `main` as `chore: rename workspace crates from kronk-* to
  tama-*` (commit `8281739`).

---

## Task 3: Rename Rust identifiers and imports — COMPLETED

**Context:** With the crates renamed, every Rust import path, function name,
and module reference that mentions `kronk` must be updated. This is a bulk
search-and-replace across all `.rs` files plus targeted function renames.

**Files:**
- Every `.rs` file in the workspace
- Specifically: `crates/tama-core/src/proxy/tama_handlers.rs`,
  `crates/tama-cli/src/handlers/serve.rs`, `crates/tama-cli/src/lib.rs`, and
  any test files referencing the old names

**What to implement:**
1. Bulk `sed` replace `use kronk_core::` → `use tama_core::`,
   `kronk_core::` → `tama_core::`, `kronk_web::` → `tama_web::`, `kronk::`
   → `tama::` across all `.rs` files.
2. Rename all eight `handle_kronk_*` functions in `tama_handlers.rs` to
   `handle_tama_*`.
3. Rename `extract_kronk_flags` → `extract_tama_flags` (definition,
   re-export, and call sites).
4. Run `cargo build --workspace` and fix any missed references.

**Acceptance criteria:**
- `rg 'kronk_' crates/ --glob '*.rs'` returns zero hits.
- `cargo build --workspace` succeeds.
- Committed on `main` as `refactor: rename Rust identifiers and imports
  kronk -> tama` (commit `bb8b734`).

---

## Task 4: Rename HTTP API routes

**Context:** The management HTTP API is exposed under `/kronk/v1/*`. It must
become `/tama/v1/*` everywhere: server router, frontend fetch calls, and
OpenAPI specs. The router fixup in Task 2 already updated the server side;
this task covers the rest.

**Files:**
- `crates/tama-web/src/server.rs` (proxy function name, route path, any
  hardcoded `/kronk/v1/` literals)
- `crates/tama-web/src/pages/dashboard.rs`
- `crates/tama-web/src/pages/models.rs`
- `crates/tama-web/src/pages/pull.rs`
- `crates/tama-web/tests/server_test.rs`
- `docs/openapi/kronk-api.yaml` → `docs/openapi/tama-api.yaml`
- `docs/openapi/kronk-web-api.yaml` → `docs/openapi/tama-web-api.yaml`
- `docs/openapi/openai-compat.yaml` (edit any cross-references)

**What to implement:**
1. In `tama-web/src/server.rs`, rename the `proxy_kronk` function to
   `proxy_tama`, update its route path from `/kronk/v1/*path` to
   `/tama/v1/*path`, and update any doc comments.
2. Replace every `/kronk/v1/` string literal in the Leptos pages with
   `/tama/v1/`.
3. `git mv` the two OpenAPI spec files to their `tama-*` names and update
   their `info.title`, `servers[].url`, and any `$ref` / path entries that
   mention `kronk`.
4. Update `docs/openapi/openai-compat.yaml` if it references the renamed
   spec files.
5. Run `trunk build` in `crates/tama-web/` and the `tama-web` SSR test
   suite to confirm the frontend compiles and the proxy route works.

**Acceptance criteria:**
- `rg '/kronk/v1' crates/ docs/` returns zero hits.
- `rg 'proxy_kronk' crates/` returns zero hits.
- `crates/tama-web/` builds cleanly with `trunk build`.
- `cargo test --package tama-web --features ssr` passes.

---

## Task 5: Rename environment variables `KRONK_*` → `TAMA_*`

**Context:** Four environment variables are read from Rust code:
`KRONK_PROXY_URL`, `KRONK_LOGS_DIR`, `KRONK_CONFIG_PATH`, `KRONK_VERSION`.
All must be renamed with no fallback reads of the old names.

**Files:**
- `crates/tama-cli/src/handlers/web.rs`
- `crates/tama-web/src/server.rs`
- Any other source files found by `rg 'KRONK_'`
- `.github/workflows/release.yml` (if `KRONK_VERSION` is referenced)

**What to implement:**
1. `rg 'KRONK_'` across the whole repo to find every occurrence.
2. Replace each `KRONK_*` with the matching `TAMA_*` in source code and any
   CI workflow files.
3. Update any doc comments or log messages that mention the old names.

**Acceptance criteria:**
- `rg 'KRONK_'` returns zero hits across the repo.
- `cargo build --workspace` passes.

---

## Task 6: Rename data directory + auto-migration

**Context:** User data lives at `ProjectDirs::from("", "", "kronk")` with a
SQLite database at `<config_dir>/kronk.db`. The new code should use `tama`
everywhere, but because we want existing users to keep their data, the
binary performs a one-time auto-migration on first run: if the new
directory does not exist but the legacy one does, rename the directory and
the database file.

**Files:**
- `crates/tama-core/src/config/loader.rs` (ProjectDirs call)
- `crates/tama-core/src/db/mod.rs` (`kronk.db` → `tama.db`)
- NEW: `crates/tama-core/src/config/rename_legacy.rs`
- `crates/tama-core/src/config/mod.rs` (declare and call the new module)

**What to implement:**
1. Update the `ProjectDirs::from` call to use `"tama"`.
2. Update the database filename constant from `kronk.db` to `tama.db`.
3. Write a new `rename_legacy` module exposing a single function
   `migrate_legacy_data_dir() -> Result<Option<Migration>>`. It computes
   both legacy and new paths via `ProjectDirs`, returns `Ok(None)` if the
   new path already exists or if the legacy path does not exist, and
   otherwise:
   - Creates the parent of the new path if needed.
   - Attempts `std::fs::rename(legacy, new)`. If that fails with `ErrorKind`
     suggesting a cross-device move, falls back to a recursive copy + remove.
   - If a `kronk.db` file exists inside the newly moved directory, renames
     it to `tama.db` in place.
   - Returns `Ok(Some(Migration { from, to }))` so the caller can log a
     one-line notice.
4. Wire the call into the earliest stage of the CLI entry point (before
   anything else reads the config or opens the DB).
5. Add unit tests using `tempfile::tempdir()` that exercise: (a) no legacy
   dir → no-op, (b) new dir already exists → no-op, (c) legacy dir present
   → directory renamed and `kronk.db` → `tama.db`.

**Acceptance criteria:**
- `rg '"kronk"' crates/tama-core/src/` returns zero hits outside the
  `rename_legacy` module (which intentionally contains the legacy string).
- `rg 'kronk\.db' crates/` returns zero hits outside the same module.
- `cargo test --package tama-core rename_legacy` passes.
- Manual smoke test with a dummy `~/.config/kronk` directory shows it is
  renamed to `~/.config/tama` on first run and the database is renamed.

---

## Task 7: Platform services (systemd, Windows service, firewall)

**Context:** On Linux, the CLI installs a systemd unit named
`kronk.service`. On Windows, it installs a service named `kronk` and
creates a firewall rule with the same name. All three must be renamed. No
migration of existing installed services is performed — users reinstall.

**Files:**
- `crates/tama-core/src/config/resolve.rs` (`kronk-{}` service name prefix)
- `crates/tama-core/src/platform/linux.rs` (systemd unit name, file
  contents, install path)
- `crates/tama-core/src/platform/windows/install.rs` (service name,
  display name, description)
- `crates/tama-core/src/platform/windows/firewall.rs` (rule name)
- Any integration tests that assert on the service name

**What to implement:**
1. Replace every `kronk` literal used as a service/unit/rule name with
   `tama`.
2. Update the systemd unit template's `Description=`, any
   `ExecStart=` path hints, and the install filename
   (`/etc/systemd/system/tama.service`).
3. Update the Windows service `DisplayName` and `Description` strings to
   reference `tama`.
4. Update the Windows firewall rule display name.
5. Run `cargo test --package tama-core` and fix any test assertions that
   compared against the old name.

**Acceptance criteria:**
- `rg -i 'kronk' crates/tama-core/src/platform/` returns zero hits.
- `rg -i 'kronk' crates/tama-core/src/config/resolve.rs` returns zero hits.
- `cargo test --package tama-core` passes.

---

## Task 8: Delete `proxy start` subcommand + remove ENG quotes

**Context:** The `proxy start` CLI subcommand is a deprecated shim for the
old architecture and should be deleted entirely. Separately, the codebase
contains 18 Emperor's New Groove joke quotes in log/error messages that the
user wants replaced with plain, boring status messages.

**Files:**
- `crates/tama-cli/src/cli.rs` (delete the `Proxy` enum variant and its
  nested subcommand enum; update the top-level `about` string and any help
  text)
- `crates/tama-cli/src/lib.rs` (delete the dispatch arm for the `Proxy`
  variant; update any remaining `"kronk".to_string()` fallback to
  `"tama".to_string()`)
- `crates/tama-cli/src/handlers/serve.rs` (delete the `handle_proxy_start`
  function; replace "Starting Kronk..." log with "Starting tama...")
- `crates/tama-cli/src/handlers/service_cmd.rs` (three ENG-quote lines;
  service name fallbacks)
- `crates/tama-cli/src/handlers/run.rs` (six ENG-quote lines)
- `crates/tama-cli/src/handlers/profile.rs` (one ENG-quote line)
- `crates/tama-cli/src/commands/model.rs` (six ENG-quote lines)
- `crates/tama-cli/tests/tests.rs` (delete any tests that exercised
  `proxy start`)

**What to implement:**
1. Remove the `Proxy` variant from the CLI enum, its nested subcommand
   enum, and every reference to it in `lib.rs`.
2. Delete the `handle_proxy_start` function.
3. Find every ENG quote (`Pull the lever!`, `Wrong lever!`, `Right lever!`,
   `Oh yeah, it's all coming together.`, `Why do we even have that lever?`,
   `That doesn't make sense!`, `WRONG LEVER!`, etc.) and replace each with
   a plain equivalent such as `Starting…`, `Stopped.`, `Failed: {err}`,
   `Done.`, `Skipped: {reason}`, etc. Prefer the neutral phrasing already
   used elsewhere in the codebase.
4. Delete any test that asserted on a quote string.
5. Update help text in `cli.rs` so it no longer implies the presence of
   `proxy start`.

**Acceptance criteria:**
- `rg -i 'Pull the lever|Wrong lever|Right lever|coming together|Why do we
  even|doesn.t make sense|WRONG LEVER'` returns zero hits.
- `rg 'proxy.*start|handle_proxy_start|Proxy\(' crates/tama-cli/` returns
  zero hits.
- `cargo test --package tama` passes.
- Running `tama --help` does not mention a `proxy` subcommand.

---

## Task 9: Rename Windows installer

**Files:**
- `installer/kronk.iss` → `installer/tama.iss`
- Any `.bat` helpers in `installer/` that reference the old name
- `.github/workflows/release.yml` (installer filename)

**What to implement:**
1. `git mv installer/kronk.iss installer/tama.iss`.
2. Edit the Inno Setup script: update `AppId` (generate a fresh GUID or
   keep deterministic — user's call, default to fresh), `AppName`,
   `AppVerName`, `DefaultDirName`, `DefaultGroupName`, `OutputBaseFilename`,
   `UninstallDisplayName`, and any `Source:`/`DestName:` references.
3. Update the release workflow to invoke the renamed `.iss` file and
   upload the renamed installer artifact.

**Acceptance criteria:**
- `rg -i 'kronk' installer/` returns zero hits.
- The release workflow YAML references only `tama.iss` / `tama-*.exe`.

---

## Task 10: Makefile, CI workflows, web static assets

**Files:**
- `Makefile`
- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `crates/tama-web/index.html`
- `crates/tama-web/style.css`
- `crates/tama-web/dist/` (delete stale `kronk-web-*` files; regenerate)
- `crates/tama-web/.gitignore` or root `.gitignore` (add `dist/`)
- `config/kronk.toml` → `config/tama.toml`
- `modelcards/Tesslate/OmniCoder-9B.toml` (one comment)

**What to implement:**
1. In the `Makefile`, rename every `kronk` reference — build targets,
   install paths, binary names, package paths.
2. In both CI workflow files, update crate names (`tama-core`, `tama-cli`,
   `tama-web`), binary names (`tama`), artifact names, `cargo deb`/`cargo
   generate-rpm` invocations, and any step that builds or uploads the
   installer.
3. Update `index.html`'s `<title>` tag and any header comments in
   `style.css`.
4. Delete `crates/tama-web/dist/` contents, add the directory to
   `.gitignore`, and regenerate with `trunk build` to confirm fresh
   artifacts use `tama-web-*` filenames.
5. `git mv config/kronk.toml config/tama.toml` and update any comments
   inside it.
6. Fix the one stray comment in the OmniCoder-9B modelcard.

**Acceptance criteria:**
- `rg -i 'kronk' Makefile .github/ config/ modelcards/` returns zero hits.
- `rg -i 'kronk' crates/tama-web/index.html crates/tama-web/style.css`
  returns zero hits.
- `crates/tama-web/dist/` is gitignored and absent from `git ls-files`.
- CI workflows parse cleanly (GitHub's YAML validator or `actionlint`).

---

## Task 11: Documentation

**Files:**
- `README.md`
- `AGENTS.md`
- `TODO.md`
- `docs/MIGRATION.md`
- Every file under `docs/plans/*.md`
- Any other markdown file under `docs/`

**What to implement:**
1. Rewrite `README.md` with the new tagline **"A local AI server with
   automatic backend management."**, update all binary/crate/command
   examples to `tama`, drop the `icon.png` reference, and update the repo
   URL to `danielcherubini/tama`.
2. Update `AGENTS.md` build/test commands, project-structure diagram, and
   any prose mentioning `kronk`.
3. Update `TODO.md` — replace `kronk` with `tama` in checkbox items and
   headings.
4. Add a new `## v2.0 — Renamed to tama` section to `docs/MIGRATION.md`
   documenting: (a) the CLI binary is now `tama`; (b) environment variables
   are now `TAMA_*`; (c) the data directory auto-migrates from
   `~/.config/kronk` to `~/.config/tama` on first run; (d) HTTP routes are
   now under `/tama/v1/*`; (e) systemd/Windows services must be
   reinstalled.
5. Bulk `sed` replace `kronk` → `tama` and `Kronk` → `Tama` across every
   `docs/plans/*.md` file. Spot-check that the replacements don't mangle
   anything semantically (plans about the rename itself — this file — are
   exempt from the sweep).

**Acceptance criteria:**
- `rg -i 'kronk' README.md AGENTS.md TODO.md docs/` returns zero hits,
  except inside this plan file and the `MIGRATION.md` section that
  intentionally references the old name.
- Markdown renders correctly (spot-check README in a viewer).

---

## Task 12: Final log/tracing messages + verification sweep

**Context:** After the targeted renames, do a final pass over every
remaining log message, tracing span, error message, and user-visible
string, then run the full verification suite.

**Files:**
- `crates/tama-cli/src/service.rs` (`kronk-service.log`, "Starting
  Kronk…" logs)
- `crates/tama-cli/src/handlers/status.rs` ("KRONK Status" → "TAMA Status")
- `crates/tama-core/src/backends/installer/source.rs`
  (`kronk_cmake_*.bat` filenames)
- `crates/tama-core/src/models/registry.rs`
  (`/tmp/kronk_nonexistent_test_dir` paths)
- `crates/tama-core/src/proxy/tama_handlers.rs` (`Kronk management API`
  doc comments)
- Any other file surfaced by the final `rg -i 'kronk'` sweep

**What to implement:**
1. Run the verification commands listed below. Fix every remaining hit.
2. Where a log message contained a joke quote, replace it with a plain
   equivalent as per Task 8.
3. Ensure user-visible strings use the lowercase form `tama` consistently
   except at the start of a sentence or in UI titles.
4. Update the `git remote` URL to `git@github.com:danielcherubini/tama.git`
   once the user has renamed the GitHub repo.
5. Once verification is green, the working directory on disk
   (`/home/daniel/Coding/Rust/kronk`) may optionally be renamed to
   `/home/daniel/Coding/Rust/tama` — out of scope for the code diff.

**Verification commands:**

```bash
rg -i 'kronk' --hidden \
  --glob '!target/**' \
  --glob '!.git/**' \
  --glob '!Cargo.lock' \
  --glob '!crates/tama-web/dist/**' \
  --glob '!docs/plans/2026-04-06-rename-kronk-to-tama.md' \
  --glob '!docs/MIGRATION.md'

rg 'KRONK_'

rg -i 'Pull the lever|Wrong lever|Right lever|coming together|Why do we even|doesn.t make sense|WRONG LEVER'

cargo fmt --all --check
cargo build --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace

( cd crates/tama-web && trunk build )
cargo test --package tama-web --features ssr
```

**Acceptance criteria:**
- All three `rg` greps return zero hits.
- `cargo fmt --all --check` passes.
- `cargo build --workspace` passes.
- `cargo clippy --workspace -- -D warnings` passes.
- `cargo test --workspace` passes.
- `trunk build` succeeds in `crates/tama-web/`.
- `cargo test --package tama-web --features ssr` passes.
- A manual run of `./target/release/tama --help` shows `tama` in the
  header and no references to `kronk` or `proxy start`.
