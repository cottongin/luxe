use super::*;
use luxe_core::phase_gate::GateOutcome;

fn project_with_gate_command() -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf) {
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", home.path());
    std::fs::create_dir_all(home.path().join("luxe")).unwrap();
    std::fs::write(home.path().join("luxe/config.json"), "{}").unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".luxe")).unwrap();
    let marker = project.path().join("gate-ran");
    std::env::set_var("LUXE_GATE_TEST_MARKER", &marker);
    let command = vec![
        luxe_core::paths::display(&std::env::current_exe().unwrap()),
        "--exact".into(),
        "engine_setup::phase_reviewer::tests::gate_marker_child".into(),
        "--ignored".into(),
    ];
    let config = serde_json::json!({"gate": {"commands": [command], "review": false}});
    std::fs::write(project.path().join(".luxe/config.json"), config.to_string()).unwrap();
    (home, project, marker)
}

#[test]
#[ignore = "subprocess fixture for the project gate trust test"]
fn gate_marker_child() {
    let marker =
        std::env::var_os("LUXE_GATE_TEST_MARKER").expect("parent supplies the marker path");
    std::fs::write(marker, "trusted").unwrap();
}

/// The actual phase gate must not execute a project's command until the CLI grants trust.
#[tokio::test]
async fn a_project_gate_command_runs_only_with_explicit_trust() {
    std::env::remove_var(luxe_core::hooks::review_gate::RECURSION_GUARD_ENV);
    let (_home, project, marker) = project_with_gate_command();
    let mut settings = AgentSettings::resolve(project.path().to_path_buf(), Default::default());
    let controller = Arc::new(luxe_core::loop_controller::LoopController::new(
        project.path().join("loop_state.json"),
        project.path().join("STOP"),
    ));
    let gate = build_phase_gate(&settings, controller.clone()).await;
    let outcome = gate.run(0, "test", &|_| {}).await;
    assert!(matches!(outcome, GateOutcome::Passed), "{outcome:?}");
    assert!(!marker.exists(), "an untrusted project command must never execute");
    assert!(phase_gate_config(&settings).gate.commands().is_empty());
    settings.trust_project = true;
    let gate = build_phase_gate(&settings, controller).await;
    let outcome = gate.run(0, "test", &|_| {}).await;
    assert!(matches!(outcome, GateOutcome::Passed), "{outcome:?}");
    assert!(marker.exists(), "explicit trust must enable the project command");
    assert_eq!(phase_gate_config(&settings).gate.commands().len(), 1);
    std::env::remove_var("XDG_CONFIG_HOME");
    std::env::remove_var("LUXE_GATE_TEST_MARKER");
}
