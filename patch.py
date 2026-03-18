with open("crates/kronk-cli/src/commands/backend.rs", "r") as f:
    code = f.read()

old_block = """                let canonical_parent = match std::fs::canonicalize(parent) {
                    Ok(path) => path,
                    Err(_) => {
                        // Parent directory may not exist (e.g., user manually deleted it)
                        // In this case, skip removal since there's nothing to remove
                        println!("Skipping file removal: parent directory does not exist.");
                        return Ok(());
                    }
                };
                let managed = match std::fs::canonicalize(backends_dir()?) {
                    Ok(path) => path,
                    Err(_) => {
                        // Backends directory may not exist (e.g., user manually deleted it)
                        println!("Skipping file removal: backends directory does not exist.");
                        return Ok(());
                    }
                };
                if canonical_parent.starts_with(&managed) {"""

new_block = """                let canonical_parent_opt = std::fs::canonicalize(parent).ok();
                let managed_opt = std::fs::canonicalize(backends_dir()?).ok();

                if let (Some(canonical_parent), Some(managed)) = (canonical_parent_opt, managed_opt) {
                    if canonical_parent.starts_with(&managed) {"""

old_end = """                } else {
                    println!("Skipping file removal: path is outside managed directory.");
                }"""

new_end = """                } else {
                    println!("Skipping file removal: path is outside managed directory.");
                }
                } else {
                    println!("Skipping file removal: directory does not exist.");
                }"""

if old_block in code:
    code = code.replace(old_block, new_block)
    code = code.replace(old_end, new_end)
    with open("crates/kronk-cli/src/commands/backend.rs", "w") as f:
        f.write(code)
    print("Patched successfully")
else:
    print("Block not found!")
