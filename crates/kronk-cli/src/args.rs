/// Replace or inject `-c <value>` in the argument list.
/// Removes any existing `-c` / `--ctx-size` and replaces with new value.
#[cfg(test)]
pub fn inject_context_size(args: &mut Vec<String>, ctx: u32) {
    // Find the first -c / --ctx-size flag
    let first_idx = args
        .iter()
        .position(|arg| arg == "-c" || arg == "--ctx-size");

    match first_idx {
        Some(idx) => {
            // Collect indices of all -c / --ctx-size flags and their values (pairs)
            // We need to track both flag and value indices
            let mut to_remove = Vec::new();
            let mut i = idx;
            while i < args.len() {
                if args[i] == "-c" || args[i] == "--ctx-size" {
                    // Remove the flag
                    to_remove.push(i);
                    i += 1;
                    // Remove the value if it exists and doesn't start with '-'
                    if i < args.len() && !args[i].starts_with('-') {
                        to_remove.push(i);
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }

            // Remove from end to start to maintain correct indices
            for i in to_remove.into_iter().rev() {
                args.remove(i);
            }

            // Insert new flag and value at the original position
            args.insert(idx, "-c".to_string());
            args.insert(idx + 1, ctx.to_string());
        }
        None => {
            // No existing flags, append at the end
            args.push("-c".to_string());
            args.push(ctx.to_string());
        }
    }
}
