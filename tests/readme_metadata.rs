use std::fs;
use std::path::Path;

use open_agent_view::domain::Provider;
use serde_json::Value;

fn public_harness_names() -> Vec<String> {
    let catalog_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("website/app/harnesses.json");
    let catalog: Value = serde_json::from_str(
        &fs::read_to_string(catalog_path).expect("read public harness catalog"),
    )
    .expect("parse public harness catalog");

    catalog["codingHarnesses"]
        .as_array()
        .expect("codingHarnesses array")
        .iter()
        .map(|entry| {
            entry["name"]
                .as_str()
                .expect("coding harness name")
                .to_owned()
        })
        .collect()
}

fn read_readme() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    fs::read_to_string(path)
        .expect("read README")
        .replace("\r\n", "\n")
}

fn disclosure_containing<'a>(readme: &'a str, marker: &str) -> &'a str {
    let marker_offset = readme.find(marker).expect("disclosure marker");
    let details_start = readme[..marker_offset]
        .rfind("<details>")
        .expect("opening disclosure");
    let details_end = readme[marker_offset..]
        .find("</details>")
        .map(|offset| marker_offset + offset)
        .expect("closed disclosure");
    &readme[details_start..details_end]
}

#[test]
fn readme_status_badges_match_release_metadata() {
    let readme = read_readme();
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
    let readme = read_readme();
    let details = disclosure_containing(&readme, "Compare feature support by harness");

    assert!(details.contains("Compare feature support by harness"));
    assert!(details.contains("| Harness | Launch | Model / shell picker |"));
    for harness in public_harness_names()
        .into_iter()
        .chain(["Terminal".to_owned()])
    {
        assert!(
            details.contains(&format!("| {harness} |")),
            "feature disclosure is missing {harness}"
        );
    }
}

#[test]
fn readme_collapses_the_windows_install_command() {
    let readme = read_readme();
    let details = disclosure_containing(&readme, "Windows PowerShell");

    assert!(details.contains("irm https://open-agent-view.github.io/install.ps1 | iex"));
    assert!(!readme.contains("## Quick start"));
}

#[test]
fn product_readme_and_website_share_one_exact_harness_inventory() {
    let catalog_names = public_harness_names();
    let product_names = Provider::CODING_HARNESSES
        .iter()
        .map(|provider| provider.public_name().to_owned())
        .collect::<Vec<_>>();

    assert_eq!(
        catalog_names, product_names,
        "the public website catalog must match the product picker exactly"
    );
    assert_eq!(catalog_names.len(), Provider::CODING_HARNESS_COUNT);

    let readme = read_readme();
    assert!(
        readme.contains("15 coding\nharnesses plus Terminal"),
        "README must state the exact coding-harness and Terminal counts"
    );

    let feature_rows = catalog_names
        .iter()
        .map(|name| {
            readme
                .find(&format!("| {name} |"))
                .expect("README feature row")
        })
        .collect::<Vec<_>>();
    assert!(
        feature_rows.windows(2).all(|pair| pair[0] < pair[1]),
        "README feature rows must follow product picker order"
    );
}
