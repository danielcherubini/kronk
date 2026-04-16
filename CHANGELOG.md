# Changelog

All notable changes to this project are documented here. Format loosely
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project adheres to [Semantic Versioning](https://semver.org/).

## [1.35.11] - 2026-04-16

### Fixed

- **Migration v9 no longer wipes `model_files`.** The v9 SQL rebuilt
  `model_configs` with `DROP TABLE`, which under `foreign_keys=ON` fired
  `ON DELETE CASCADE` on `model_files.model_id` and emptied the child table.
  Users who ran any v1.35.0–v1.35.10 release saw `llama-server` launched
  without `-m <path>` and the warning `Quant '<name>' not found in
  ModelConfig for model '<repo>'`. Migration v9 now toggles
  `PRAGMA foreign_keys=OFF` around the rebuild.
- **Auto-repair on startup.** The proxy scans `<models_dir>/<repo>/` on
  boot and rehydrates `model_files` from GGUF files on disk for any
  `model_configs` row that has no child rows. Mmproj files are detected
  and set as `selected_mmproj` when one isn't already chosen.

### Changed

- CI now runs `cargo clippy --workspace --all-targets -- -D warnings`, so
  test code is linted too. Previously only libs/bins were checked.

## [1.35.1] - 2026-04-15

### Fixed

- Accept either `config_key` or the integer `id` in model API routes.
- Use `config.loaded_from` for the DB path in server handlers so CLI
  commands pick up the right database when run from a non-default
  location.

## [1.35.0] - 2026-04-14

### Added

- Auto-increment integer `id` column on model tables. External APIs and
  the web UI now prefer the integer id over the slash-replaced config
  key for addressing models.

## Earlier releases

Release notes for v1.34.0 and earlier are not included here. See the
[GitHub Releases](https://github.com/danielcherubini/koji/releases) page
and `git log` for history.
