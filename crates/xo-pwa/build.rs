use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=XO_VERSION");
    if let Some(head) = git_output(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
    }
    let version = std::env::var("XO_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(git_version);
    println!("cargo:rustc-env=XO_BUILD_VERSION={version}");
}

fn git_version() -> String {
    let exact_tag = git_output(&["describe", "--tags", "--exact-match", "--match", "[0-9]*"]);
    exact_tag.unwrap_or_else(|| {
        git_output(&["rev-parse", "--short=12", "HEAD"])
            .map_or_else(|| "dev".to_owned(), |value| format!("dev-{value}"))
    })
}

fn git_output(arguments: &[&str]) -> Option<String> {
    Command::new("git")
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}
