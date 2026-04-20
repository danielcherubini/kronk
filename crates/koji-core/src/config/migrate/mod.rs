use std::collections::{HashMap, HashSet};

pub mod model_to_db;

/// Check if an argument is a stale `--mmproj` flag.
///
/// Matches three forms:
///   1. "--mmproj <path>"             (single grouped token)
///   2. "--mmproj=<path>"             (inline equals)
///   3. "--mmproj"                    (standalone flag, value in next arg)
fn is_stale_mmproj_arg(arg: &str) -> bool {
    arg.starts_with("--mmproj ") || arg.starts_with("--mmproj=") || arg == "--mmproj"
}

/// Collect indices of stale `--mmproj` args and their associated value args.
///
/// Returns a sorted Vec of indices to remove. For the two-token form
/// ("--mmproj" followed by "<path>"), both indices are included.
fn collect_stale_mmproj_indices(args: &[String]) -> Vec<usize> {
    let mut stale = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if is_stale_mmproj_arg(&args[i]) {
            stale.push(i);
            // Two-token form: also mark the value arg.
            if args[i] == "--mmproj" && i + 1 < args.len() {
                stale.push(i + 1);
                i += 2;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    stale.sort();
    stale.dedup();
    stale
}

/// Recover the mmproj selection from a stale `--mmproj` arg before removal.
///
/// If `model_config.mmproj` is currently `None`, tries to find a quant entry
/// in `model_config.quants` whose `file` matches the basename of the stripped path.
fn recover_mmproj_selection(
    model_config: &mut crate::config::types::ModelConfig,
    stale_indices: &[usize],
) {
    if model_config.mmproj.is_some() {
        return; // Pre-existing selection preserved.
    }

    for &idx in stale_indices {
        let arg = &model_config.args[idx];
        let path: Option<String> = arg
            .strip_prefix("--mmproj ")
            .or_else(|| arg.strip_prefix("--mmproj="))
            .map(|rest| rest.to_string());

        if let Some(path) = path {
            let path_clean = path.trim_matches(|c: char| c == '"' || c == '\'');
            let filename = path_clean
                .replace('\\', "/")
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string();

            if !filename.is_empty() {
                tracing::debug!("Extracted filename '{}' from --mmproj path", filename);
                if let Some((key, q)) = model_config.quants.iter().find(|(_, q)| q.file == filename)
                {
                    model_config.mmproj = Some(key.clone());
                    tracing::info!(
                        "Recovered mmproj selection '{}' (file={:?}) from stale --mmproj arg",
                        key,
                        q.file
                    );
                } else {
                    tracing::warn!(
                        "Could not find mmproj entry with file '{}' in model quants map",
                        filename
                    );
                }
                break; // Only recover from the first stale arg found.
            }
        }
    }
}

/// Strip stale `--mmproj <path>` entries from `args` in every model config.
///
/// These were written by the broken v1.15.0 frontend code that munged the
/// `args` field directly. The new path is `ModelConfig.mmproj` + automatic
/// `--mmproj` injection in `build_full_args`.
///
/// As a best-effort recovery, if a stale `--mmproj` argument is found and
/// `model_config.mmproj` is currently `None`, the function tries to find a quant entry in `model_config.quants` whose `file` matches the basename of
/// the stripped path. If found, that entry's key is set as the active
/// `mmproj`. This preserves the user's intent across the migration.
///
/// Returns `true` if any model config was modified.
pub fn cleanup_stale_mmproj_args(
    model_configs: &mut HashMap<String, crate::config::types::ModelConfig>,
) -> bool {
    let mut changed = false;

    for (model_config_id, model_config) in model_configs.iter_mut() {
        // Collect stale indices first.
        let stale_indices = collect_stale_mmproj_indices(&model_config.args);

        if stale_indices.is_empty() {
            continue;
        }

        tracing::debug!(
            "Model '{}' has {} stale --mmproj arg(s) at indices {:?}",
            model_config_id,
            stale_indices.len(),
            stale_indices
        );

        // Recover mmproj selection before removing stale args.
        recover_mmproj_selection(model_config, &stale_indices);

        // Build a HashSet for O(1) index lookup during retain.
        let stale_set: HashSet<usize> = stale_indices.into_iter().collect();

        // Remove stale args using retain with index tracking.
        let mut idx = 0;
        model_config.args.retain(|_| {
            let keep = !stale_set.contains(&idx);
            idx += 1;
            keep
        });

        changed = true;
    }

    changed
}

/// Rename legacy directories (e.g., `models.d` → `models`).
pub fn rename_legacy_directories(config_dir: &std::path::Path) -> anyhow::Result<()> {
    let legacy_map = [
        ("models.d", "models"),
        ("configs.d", "configs"),
        ("profiles.d", "profiles"),
    ];

    for (old, new) in legacy_map {
        let old_path = config_dir.join(old);
        let new_path = config_dir.join(new);

        if old_path.exists() && !new_path.exists() {
            tracing::info!("Renaming legacy directory '{}' to '{}'", old, new);
            if let Err(e) = std::fs::rename(&old_path, &new_path) {
                tracing::warn!("Failed to rename {} to {}: {}", old, new, e);
            }
        }
    }

    Ok(())
}
