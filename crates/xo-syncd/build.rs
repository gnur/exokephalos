use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=XO_PWA_DIR");
    let root = std::env::var_os("XO_PWA_DIR")
        .map(PathBuf::from)
        .filter(|path| path.join("index.html").is_file())
        .unwrap_or_else(|| PathBuf::from("pwa-fallback"));
    println!("cargo:rerun-if-changed={}", root.display());

    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    assert!(files.iter().any(|(path, _)| path == "index.html"));

    let mut generated = String::from("pub static EMBEDDED_PWA: &[EmbeddedAsset] = &[\n");
    for (relative, absolute) in files {
        let absolute = absolute.canonicalize().expect("canonicalize PWA asset");
        let escaped = absolute
            .to_string_lossy()
            .chars()
            .flat_map(char::escape_default)
            .collect::<String>();
        writeln!(
            generated,
            "    EmbeddedAsset {{ path: {relative:?}, bytes: include_bytes!(\"{escaped}\") }},",
        )
        .expect("write generated asset table");
    }
    generated.push_str("];\n");
    std::fs::write(
        PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("embedded_pwa.rs"),
        generated,
    )
    .expect("write embedded PWA table");
}

fn collect(root: &Path, directory: &Path, output: &mut Vec<(String, PathBuf)>) {
    for entry in std::fs::read_dir(directory).expect("read PWA asset directory") {
        let entry = entry.expect("read PWA asset entry");
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, output);
        } else if path.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("PWA asset is below root")
                .to_string_lossy()
                .replace('\\', "/");
            output.push((relative, path));
        }
    }
}
