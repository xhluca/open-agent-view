#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use open_agent_view::adapters::{
    DiscoveryRequest, SessionMigrateNativeController, SessionMigrateNativeOwnership,
    SessionMigrateNativeSource, SessionSource,
};
use open_agent_view::control::{LaunchRequest, ProviderController};
use open_agent_view::domain::{Provider, SessionKind};

const UUID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

fn executable(directory: &Path, name: &str, body: &str) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn ownership(
    directory: &Path,
    provider: Provider,
) -> std::sync::Arc<SessionMigrateNativeOwnership> {
    let state = directory.join(format!("{}-state", provider.label()));
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    SessionMigrateNativeOwnership::load(provider, state.join("owned.json")).unwrap()
}

fn exercise(
    provider: Provider,
    executable: &Path,
    data_root: PathBuf,
    model: Option<&str>,
    expected_id: &str,
) {
    let owner = ownership(data_root.parent().unwrap(), provider.clone());
    let controller = SessionMigrateNativeController::host(
        provider.clone(),
        executable.to_string_lossy(),
        data_root.clone(),
        owner.clone(),
    )
    .unwrap();
    let source = SessionMigrateNativeSource::host(
        provider.clone(),
        executable.to_string_lossy(),
        data_root,
        owner,
    )
    .unwrap();
    let workspace = executable.parent().unwrap().join("work");
    fs::create_dir_all(&workspace).unwrap();

    let outcome = controller
        .launch_foreground(&LaunchRequest {
            provider: provider.clone(),
            model: model.map(str::to_owned),
            prompt: "repair parser".into(),
            cwd: workspace,
        })
        .unwrap();
    assert_eq!(outcome.provider_session_hint.as_deref(), Some(expected_id));

    let sessions = source
        .discover(&DiscoveryRequest {
            include_completed: true,
            ..DiscoveryRequest::default()
        })
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].provider, provider);
    assert_eq!(sessions[0].provider_session_id, expected_id);
    assert_eq!(sessions[0].kind, SessionKind::Managed);
    controller.open(&sessions[0]).unwrap();
    assert!(controller.interrupt(&sessions[0]).is_err());
}

#[test]
fn extended_harnesses_launch_discover_and_resume_in_isolated_native_processes() {
    let temp = tempfile::tempdir().unwrap();

    let omp_root = temp.path().join("omp-data");
    let omp_log = temp.path().join("omp-argv");
    let omp = executable(
        temp.path(),
        "omp",
        &format!(
            r#"
printf '%s\n' "$*" >> '{log}'
if [ "${{1:-}}" = models ]; then
  printf '%s\n' '{{"models":[{{"selector":"anthropic/opus"}}]}}'
  exit 0
fi
if [ "${{1:-}}" = --resume ] || [ "${{1:-}}" = --no-session ]; then exit 0; fi
mkdir -p '{root}/sessions/work'
printf '%s\n' '{{"type":"title","v":1,"title":"OMP task","updatedAt":"2026-08-26T12:00:00Z","pad":""}}' '{{"type":"session","version":3,"id":"abc-123","cwd":"{work}","timestamp":"2026-08-26T12:00:00Z"}}' '{{"type":"message","message":{{"role":"assistant","content":"OMP reply"}}}}' > '{root}/sessions/work/session.jsonl'
"#,
            log = omp_log.display(),
            root = omp_root.display(),
            work = temp.path().join("work").display(),
        ),
    );
    let omp_owner = ownership(temp.path(), Provider::OhMyPi);
    let omp_controller = SessionMigrateNativeController::host(
        Provider::OhMyPi,
        omp.to_string_lossy(),
        omp_root.clone(),
        omp_owner,
    )
    .unwrap();
    assert_eq!(
        omp_controller.available_models().unwrap(),
        ["anthropic/opus"]
    );
    exercise(
        Provider::OhMyPi,
        &omp,
        omp_root,
        Some("anthropic/opus"),
        "abc-123",
    );
    omp_controller.authenticate().unwrap();
    let omp_args = fs::read_to_string(omp_log).unwrap();
    assert!(omp_args.contains("models list --no-extensions --json"));
    assert!(omp_args.contains("--model anthropic/opus"));
    assert!(omp_args.contains("--resume"));
    assert!(omp_args.contains("--no-session"));

    let grok_root = temp.path().join("grok-data");
    let grok_log = temp.path().join("grok-argv");
    let grok = executable(
        temp.path(),
        "grok",
        &format!(
            r#"
printf '%s\n' "$*" >> '{log}'
if [ "${{1:-}}" = models ]; then
  printf '%s\n' 'Default model: grok-4.6' '' 'Available models:' '  * grok-4.6 (default)'
  exit 0
fi
if [ "${{1:-}}" = login ]; then exit 0; fi
case " $* " in *' --resume '*) exit 0;; esac
mkdir -p '{root}/sessions/work/{id}'
printf '%s\n' '{{"info":{{"id":"{id}","cwd":"{work}"}},"generated_title":"Grok task","session_summary":"Grok reply","current_model_id":"grok-4.6","created_at":"2026-08-26T12:00:00Z","updated_at":"2026-08-26T12:00:01Z"}}' > '{root}/sessions/work/{id}/summary.json'
printf '%s\n' '{{"params":{{"sessionId":"{id}","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"Grok reply"}}}}}}}}' > '{root}/sessions/work/{id}/updates.jsonl'
"#,
            log = grok_log.display(),
            root = grok_root.display(),
            id = UUID,
            work = temp.path().join("work").display(),
        ),
    );
    let grok_owner = ownership(temp.path(), Provider::Grok);
    let grok_controller = SessionMigrateNativeController::host(
        Provider::Grok,
        grok.to_string_lossy(),
        grok_root.clone(),
        grok_owner,
    )
    .unwrap();
    assert_eq!(grok_controller.available_models().unwrap(), ["grok-4.6"]);
    exercise(Provider::Grok, &grok, grok_root, Some("grok-4.6"), UUID);
    grok_controller.authenticate().unwrap();
    let grok_args = fs::read_to_string(grok_log).unwrap();
    assert!(grok_args.contains("--model grok-4.6"));
    assert!(grok_args.contains("--resume"));
    assert!(grok_args.contains("login"));

    let kilo_root = temp.path().join("kilo-data");
    fs::create_dir(&kilo_root).unwrap();
    let kilo_catalog = kilo_root.join("sessions.json");
    fs::write(&kilo_catalog, "[]").unwrap();
    let kilo_log = temp.path().join("kilo-argv");
    let kilo = executable(
        temp.path(),
        "kilo",
        &format!(
            r#"
printf '%s\n' "$*" >> '{log}'
if [ "${{1:-}}" = models ]; then printf '%s\n' 'anthropic/claude'; exit 0; fi
if [ "${{1:-}}" = auth ]; then exit 0; fi
if [ "${{1:-}}" = db ]; then cat '{catalog}'; exit 0; fi
if [ "${{1:-}}" = --session ]; then exit 0; fi
printf '%s\n' '[{{"id":"ses_fixture","title":"Kilo task","created":1000,"updated":2000,"projectId":"p","directory":"{work}"}}]' > '{catalog}'
"#,
            log = kilo_log.display(),
            catalog = kilo_catalog.display(),
            work = temp.path().join("work").display(),
        ),
    );
    let kilo_owner = ownership(temp.path(), Provider::KiloCode);
    let kilo_controller = SessionMigrateNativeController::host(
        Provider::KiloCode,
        kilo.to_string_lossy(),
        kilo_root.clone(),
        kilo_owner,
    )
    .unwrap();
    assert_eq!(
        kilo_controller.available_models().unwrap(),
        ["anthropic/claude"]
    );
    exercise(
        Provider::KiloCode,
        &kilo,
        kilo_root,
        Some("anthropic/claude"),
        "ses_fixture",
    );
    kilo_controller.authenticate().unwrap();
    let kilo_args = fs::read_to_string(kilo_log).unwrap();
    assert!(kilo_args.contains("db SELECT id, title, directory"));
    assert!(kilo_args.contains("run --interactive"));
    assert!(kilo_args.contains("--model anthropic/claude"));
    assert!(kilo_args.contains("--session ses_fixture"));
    assert!(kilo_args.contains("auth login"));

    let openhands_root = temp.path().join("openhands-data");
    fs::create_dir(&openhands_root).unwrap();
    let openhands_log = temp.path().join("openhands-argv");
    let openhands = executable(
        temp.path(),
        "openhands",
        &format!(
            r#"
printf '%s|%s\n' "$*" "${{LLM_MODEL:-}}" >> '{log}'
if [ "${{1:-}}" = login ] || [ "${{1:-}}" = --resume ]; then exit 0; fi
mkdir -p '{root}/{hex}/events'
printf '%s\n' '{{"id":"{id}","agent":{{"llm":{{"model":"openai/fixture"}}}},"workspace":{{"working_dir":"{work}"}}}}' > '{root}/{hex}/base_state.json'
printf '%s\n' '{{"id":"{id}","timestamp":"2026-08-26T12:00:00.000001","source":"user","kind":"MessageEvent","llm_message":{{"role":"user","content":[{{"type":"text","text":"OpenHands task"}}]}}}}' > '{root}/{hex}/events/event-00000-{id}.json'
printf '%s\n' '{{"id":"{id}","timestamp":"2026-08-26T12:00:01.000001","source":"agent","kind":"MessageEvent","llm_message":{{"role":"assistant","content":[{{"type":"text","text":"OpenHands reply"}}]}}}}' > '{root}/{hex}/events/event-00001-{id}.json'
"#,
            log = openhands_log.display(),
            root = openhands_root.display(),
            hex = UUID.replace('-', ""),
            id = UUID,
            work = temp.path().join("work").display(),
        ),
    );
    exercise(
        Provider::OpenHands,
        &openhands,
        openhands_root.clone(),
        Some("openai/fixture"),
        UUID,
    );
    let openhands_owner = ownership(temp.path(), Provider::OpenHands);
    let openhands_controller = SessionMigrateNativeController::host(
        Provider::OpenHands,
        openhands.to_string_lossy(),
        openhands_root,
        openhands_owner,
    )
    .unwrap();
    assert_eq!(
        openhands_controller.available_models().unwrap(),
        ["openai/fixture"]
    );
    openhands_controller.authenticate().unwrap();
    let openhands_args = fs::read_to_string(openhands_log).unwrap();
    assert!(openhands_args.contains("--override-with-envs --task repair parser|openai/fixture"));
    assert!(openhands_args.contains("--resume"));
    assert!(openhands_args.contains("login"));
}
