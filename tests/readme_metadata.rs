use std::fs;
use std::path::Path;

#[test]
fn readme_status_badges_work_for_a_private_repository() {
    let readme_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let readme = fs::read_to_string(readme_path).expect("read README");
    let release_badge = format!(
        "https://img.shields.io/badge/release-v{}-55d3da.svg",
        env!("CARGO_PKG_VERSION")
    );

    assert!(
        readme.contains("https://img.shields.io/badge/tests-verified-2ea44f.svg"),
        "README test badge must report the documented manual verification gate"
    );
    assert!(
        readme.contains(&release_badge),
        "README release badge must match the crate version"
    );
    assert!(
        !readme.contains("actions/workflows/ci.yml/badge.svg"),
        "README must not present hosted-runner availability as the test result"
    );
    assert!(
        !readme.contains("img.shields.io/github/v/release"),
        "Shields cannot query releases from the private repository"
    );
}
