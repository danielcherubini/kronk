/// Replace or inject `-c <value>` in the argument list.
/// Removes any existing `-c` / `--ctx-size` and replaces with new value.
pub fn inject_context_size(args: &mut Vec<String>, ctx: u32) {
    // Find the first -c / --ctx-size flag
    let first_idx = args
        .iter()
        .position(|arg| arg == "-c" || arg == "--ctx-size");

    match first_idx {
        Some(idx) => {
            // Remove all subsequent -c / --ctx-size flags and their values
            let mut skip_next = false;
            for (i, arg) in args.iter().enumerate() {
                if i <= idx {
                    continue;
                }
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if arg == "-c" || arg == "--ctx-size" {
                    skip_next = true;
                    continue;
                }
                args.remove(i);
            }

            // Replace the flag at first_idx with -c and the new value
            args[idx] = "-c".to_string();
            args.insert(idx + 1, ctx.to_string());
        }
        None => {
            // No existing flags, append at the end
            args.push("-c".to_string());
            args.push(ctx.to_string());
        }
    }
}
