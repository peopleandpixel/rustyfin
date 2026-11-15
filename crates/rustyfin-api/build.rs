//! Collects the path of every `.route("...")` call under `src/routes` into
//! `$OUT_DIR/route_templates.rs`, so `route_case` matches paths without case against the same
//! templates the router uses.

use std::path::Path;

fn collect(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let source = std::fs::read_to_string(&path).unwrap_or_default();
            for piece in source.split(".route(").skip(1) {
                let Some(rest) = piece.trim_start().strip_prefix('"') else {
                    continue;
                };
                if let Some(end) = rest.find('"') {
                    out.push(rest[..end].to_string());
                }
            }
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=src/routes");
    let mut templates = Vec::new();
    collect(Path::new("src/routes"), &mut templates);
    templates.sort();
    templates.dedup();
    let mut code = String::from("pub(crate) const ROUTE_TEMPLATES: &[&str] = &[\n");
    for t in &templates {
        code.push_str(&format!("    {t:?},\n"));
    }
    code.push_str("];\n");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("route_templates.rs");
    std::fs::write(out, code).unwrap();
}
