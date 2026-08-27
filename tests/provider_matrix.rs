#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use open_agent_view::adapters::{
    DiscoveryEngine, DiscoveryRequest, FixtureSource, OpenCodeSource, PiSource,
};
use open_agent_view::domain::Provider;
use tempfile::tempdir;

const PI_SESSION: &str = r#"{"type":"session","version":3,"id":"pi-isolated","timestamp":"2026-08-18T12:00:00Z","cwd":"/work/pi"}
{"type":"message","id":"a1","parentId":null,"timestamp":"2026-08-18T12:00:01Z","message":{"role":"user","content":"Pi task"}}
{"type":"message","id":"a2","parentId":"a1","timestamp":"2026-08-18T12:00:02Z","message":{"role":"assistant","content":[{"type":"text","text":"Pi done"}],"stopReason":"stop"}}
"#;

fn open_code_script() -> (tempfile::TempDir, PathBuf) {
    let directory = tempdir().unwrap();
    let executable = directory.path().join("opencode");
    fs::write(
        &executable,
        r#"#!/bin/sh
if [ "$1" = session ] && [ "$2" = list ] && [ "$3" = --format ] && [ "$4" = json ]; then
  printf '%s\n' '[{"id":"oc-isolated","title":"OpenCode task","updated":1787089210008,"created":1787089195916,"projectId":"global","directory":"/work/opencode"}]'
  exit 0
fi
exit 64
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).unwrap();
    (directory, executable)
}

fn pi_store() -> tempfile::TempDir {
    let directory = tempdir().unwrap();
    let nested = directory.path().join("--work-pi--");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("session.jsonl"), PI_SESSION).unwrap();
    directory
}

#[test]
fn pi_discovers_in_isolation() {
    let store = pi_store();
    let mut engine = DiscoveryEngine::new();
    engine.add_source(PiSource::host(store.path()));

    let snapshot = engine.discover(&DiscoveryRequest {
        include_completed: true,
        include_external: true,
        ..DiscoveryRequest::default()
    });

    assert!(snapshot.warnings.is_empty());
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions[0].provider, Provider::Pi);
}

#[test]
fn opencode_discovers_in_isolation() {
    let (_directory, executable) = open_code_script();
    let mut engine = DiscoveryEngine::new();
    engine.add_source(OpenCodeSource::host(executable.display().to_string()));

    let snapshot = engine.discover(&DiscoveryRequest {
        include_completed: true,
        include_external: true,
        ..DiscoveryRequest::default()
    });

    assert!(snapshot.warnings.is_empty());
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions[0].provider, Provider::OpenCode);
}

#[test]
fn pi_opencode_claude_and_codex_coexist_without_collisions() {
    let store = pi_store();
    let (_directory, executable) = open_code_script();
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/populated-sessions.json");
    let mut engine = DiscoveryEngine::new();
    engine.add_source(PiSource::host(store.path()));
    engine.add_source(OpenCodeSource::host(executable.display().to_string()));
    engine.add_source(FixtureSource::new(fixture));

    let snapshot = engine.discover(&DiscoveryRequest {
        include_completed: true,
        include_interactive: true,
        include_external: true,
        cwd: None,
        ..DiscoveryRequest::default()
    });

    assert!(snapshot.warnings.is_empty());
    for provider in [
        Provider::Claude,
        Provider::Codex,
        Provider::Pi,
        Provider::OpenCode,
    ] {
        assert!(snapshot
            .sessions
            .iter()
            .any(|session| session.provider == provider));
    }
    let mut ids = snapshot
        .sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), snapshot.sessions.len());
}

#[test]
fn canonical_visual_fixture_contains_every_supported_provider_without_id_collisions() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/all-providers-sessions.json");
    let mut engine = DiscoveryEngine::new();
    engine.add_source(FixtureSource::new(fixture));

    let snapshot = engine.discover(&DiscoveryRequest {
        include_completed: true,
        include_interactive: true,
        cwd: None,
        ..DiscoveryRequest::default()
    });

    assert!(snapshot.warnings.is_empty());
    for provider in [
        Provider::Claude,
        Provider::Codex,
        Provider::Pi,
        Provider::OpenCode,
        Provider::Cursor,
        Provider::GitHubCopilot,
        Provider::Antigravity,
        Provider::MistralVibe,
        Provider::MuseCode,
        Provider::QwenCode,
        Provider::KimiCode,
        Provider::OhMyPi,
        Provider::Grok,
        Provider::KiloCode,
        Provider::OpenHands,
        Provider::Terminal,
    ] {
        assert!(
            snapshot
                .sessions
                .iter()
                .any(|session| session.provider == provider),
            "missing {} fixture session",
            provider.label()
        );
    }
    let mut ids = snapshot
        .sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), snapshot.sessions.len());
}
