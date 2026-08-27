use std::fs;
use std::path::Path;

#[test]
fn readme_status_badges_match_release_metadata() {
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
        "README intentionally uses its version-matched static release badge"
    );
}

#[test]
fn readme_keeps_the_feature_matrix_inside_a_disclosure() {
    let readme_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let readme = fs::read_to_string(readme_path).expect("read README");
    let details_start = readme.find("<details>").expect("feature disclosure");
    let details_end = readme[details_start..]
        .find("</details>")
        .map(|offset| details_start + offset)
        .expect("closed feature disclosure");
    let details = &readme[details_start..details_end];

    assert!(details.contains("Compare feature support by harness"));
    assert!(details.contains("| Harness | Launch | Model / shell picker |"));
    for harness in [
        "Claude Code",
        "OpenAI Codex",
        "Cursor",
        "GitHub Copilot",
        "OpenCode",
        "Pi",
        "Antigravity",
        "Mistral Vibe",
        "Muse Code",
        "Qwen Code",
        "Kimi Code",
        "Terminal",
    ] {
        assert!(
            details.contains(&format!("| {harness} |")),
            "feature disclosure is missing {harness}"
        );
    }
}
