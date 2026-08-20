fn main() {
    let sha = std::env::var("BUILD_SHA").unwrap_or_else(|_| git_sha8());
    println!("cargo:rustc-env=BUILD_SHA={}", sha);
    println!("cargo:rerun-if-env-changed=BUILD_SHA");
    println!("cargo:rerun-if-changed=.git/HEAD");
}

fn git_sha8() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| format!("dev-{}", String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or_else(|| "dev".to_string())
}
