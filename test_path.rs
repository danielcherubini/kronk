use std::path::Path;

fn main() {
    let p = Path::new("foo/bar/");
    println!("ends_with /: {}", p.ends_with("/"));
    println!("ends_with \"\": {}", p.ends_with(""));
}
