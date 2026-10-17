use super::*;
use PermissionAction::{Allow, Ask, Deny};

fn check(permission: &str, input: &str) -> PermissionRequest {
    if permission == "bash" {
        return build_request(
            "bash",
            &serde_json::json!({"command": input}),
            std::path::Path::new("."),
        );
    }
    PermissionRequest {
        permission: permission.to_string(),
        tool: permission.to_string(),
        checks: vec![Check { permission: permission.to_string(), input: input.to_string() }],
        always: vec![],
        summary: input.to_string(),
    }
}

#[test]
fn strictest_orders_deny_over_ask_over_allow() {
    assert_eq!(Allow.strictest(Ask), Ask);
    assert_eq!(Ask.strictest(Deny), Deny);
    assert_eq!(Deny.strictest(Allow), Deny);
    assert_eq!(Allow.strictest(Allow), Allow);
}

#[test]
fn default_is_permissive_but_guards_env_and_catastrophes() {
    let e = PermissionEngine::permissive();
    assert_eq!(e.decide(&check("bash", "ls -la")), Allow);
    assert_eq!(e.decide(&check("edit", "src/main.rs")), Allow);
    assert_eq!(e.decide(&check("read", "src/main.rs")), Allow);
    // Secrets: .env read denied, .env.example allowed.
    assert_eq!(e.decide(&check("read", ".env")), Deny);
    assert_eq!(e.decide(&check("read", "config/.env.local")), Deny);
    assert_eq!(e.decide(&check("read", ".env.example")), Allow);
    // Catastrophic bash denied even buried in a chain.
    assert_eq!(e.decide(&check("bash", "rm -rf /")), Deny);
    assert_eq!(e.decide(&check("bash", "echo hi && rm -rf /etc")), Deny);
    // Out-of-project work is a judgement call, so it asks.
    assert_eq!(e.decide(&check(EXTERNAL_DIRECTORY, "/etc/hosts")), Ask);
    // A repeated identical call is not — asking auto-allowed it in headless/loop runs, which is
    // precisely where a runaway costs the most.
    assert_eq!(e.decide(&check(DOOM_LOOP, "bash")), Deny);
}

#[test]
fn most_specific_rule_wins_regardless_of_order() {
    let global = PermissionConfig::Keyed(
        [(
            "bash".to_string(),
            PermValue::Map(
                [("*".to_string(), Deny), ("git *".to_string(), Allow)].into_iter().collect(),
            ),
        )]
        .into_iter()
        .collect(),
    )
    .to_ruleset();
    let e = PermissionEngine::new(global, vec![], false);
    assert_eq!(e.decide(&check("bash", "git status")), Allow); // specific "git *"
    assert_eq!(e.decide(&check("bash", "curl evil")), Deny); // catch-all "*"
}

#[test]
fn modes_layer_over_the_guardrails() {
    use PermissionMode::{ReadOnly, Strict, Yolo};
    // read-only: mutators denied, reads allowed.
    let ro = PermissionEngine::assemble(ReadOnly, vec![], vec![], false);
    assert_eq!(ro.decide(&check("edit", "src/x.rs")), Deny);
    assert_eq!(ro.decide(&check("bash", "ls")), Deny);
    assert_eq!(ro.decide(&check("read", "src/x.rs")), Allow);

    // strict: mutators ask, but a catastrophe is still a hard deny (guardrails outrank strict).
    let strict = PermissionEngine::assemble(Strict, vec![], vec![], false);
    assert_eq!(strict.decide(&check("bash", "ls")), Ask);
    assert_eq!(strict.decide(&check("bash", "rm -rf /")), Deny);
    assert_eq!(strict.decide(&check("read", "src/x.rs")), Allow);

    // yolo: guardrails off — even a catastrophe and a .env read are allowed.
    let yolo = PermissionEngine::assemble(Yolo, vec![], vec![], false);
    assert_eq!(yolo.decide(&check("bash", "rm -rf /")), Allow);
    assert_eq!(yolo.decide(&check("read", ".env")), Allow);
}

/// Your own `deny` outranks every mode, `yolo` included: `yolo` lifts the guardrails Luxe ships,
/// not the rules you wrote. Holding a server's calls back until an approved provider is in use
/// depends on exactly this.
#[test]
fn your_own_deny_holds_in_every_mode() {
    use PermissionMode::{Default as DefaultMode, Strict, Yolo};
    let global = || {
        PermissionConfig::Keyed(
            [(
                "mcp".to_string(),
                PermValue::Map([("asana/*".to_string(), Deny)].into_iter().collect()),
            )]
            .into_iter()
            .collect(),
        )
        .to_ruleset()
    };
    for mode in [DefaultMode, Strict, Yolo] {
        let e = PermissionEngine::assemble(mode, global(), vec![], false);
        assert_eq!(e.decide(&check("mcp", "asana/get_tasks")), Deny, "{mode:?}");
    }
    let yolo = PermissionEngine::assemble(Yolo, global(), vec![], false);
    assert_eq!(yolo.decide(&check("mcp", "linear/list_issues")), Allow, "only what it names");
}

/// The more specific MCP rule wins, which is why the guide says to *replace* a server-wide
/// `deny` with ask-before-writing rules rather than add them beside it: kept together, the
/// writes are asked about and the reads stay refused.
#[test]
fn a_narrower_mcp_rule_outranks_a_server_wide_one() {
    let global = PermissionConfig::Keyed(
        [(
            "mcp".to_string(),
            PermValue::Map(
                [("asana/*".to_string(), Deny), ("asana/create_*".to_string(), Ask)]
                    .into_iter()
                    .collect(),
            ),
        )]
        .into_iter()
        .collect(),
    )
    .to_ruleset();
    let e = PermissionEngine::assemble(PermissionMode::Default, global, vec![], false);
    assert_eq!(e.decide(&check("mcp", "asana/create_tasks")), Ask);
    assert_eq!(e.decide(&check("mcp", "asana/get_tasks")), Deny);
}

#[test]
fn assemble_keeps_project_tighten_only_under_a_mode() {
    use PermissionMode::Default as DefaultMode;
    let project = PermissionConfig::Keyed(
        [("bash".to_string(), PermValue::Scalar(Deny))].into_iter().collect(),
    )
    .to_ruleset();
    let e = PermissionEngine::assemble(DefaultMode, vec![], project, false);
    assert_eq!(e.decide(&check("bash", "ls")), Deny, "project tighten honored");
}

#[test]
fn project_config_can_tighten_but_not_loosen() {
    // Global allows bash broadly.
    let global = PermissionConfig::Keyed(
        [("bash".to_string(), PermValue::Scalar(Allow))].into_iter().collect(),
    )
    .to_ruleset();
    // Malicious project tries to loosen the .env read deny AND broaden bash — must not work.
    let project = PermissionConfig::Keyed(
        [
            ("read".to_string(), PermValue::Scalar(Allow)),
            ("bash".to_string(), PermValue::Scalar(Allow)),
        ]
        .into_iter()
        .collect(),
    )
    .to_ruleset();
    let e = PermissionEngine::new(global.clone(), project, false);
    assert_eq!(e.decide(&check("read", ".env")), Deny, "project cannot loosen .env deny");

    // A project that TIGHTENS (denies bash) is honored.
    let tighten = PermissionConfig::Keyed(
        [("bash".to_string(), PermValue::Scalar(Deny))].into_iter().collect(),
    )
    .to_ruleset();
    let e2 = PermissionEngine::new(global, tighten, false);
    assert_eq!(e2.decide(&check("bash", "ls")), Deny, "project can tighten to deny");
}

#[test]
fn trust_project_lets_project_loosen() {
    let project = PermissionConfig::Keyed(
        [(
            "external_directory".to_string(),
            PermValue::Map([("*".to_string(), Allow)].into_iter().collect()),
        )]
        .into_iter()
        .collect(),
    )
    .to_ruleset();
    let trusted = PermissionEngine::new(vec![], project.clone(), true);
    assert_eq!(trusted.decide(&check(EXTERNAL_DIRECTORY, "/etc/hosts")), Allow);
    let untrusted = PermissionEngine::new(vec![], project, false);
    assert_eq!(untrusted.decide(&check(EXTERNAL_DIRECTORY, "/etc/hosts")), Ask);
}

#[test]
fn session_approval_relaxes_ask_but_never_deny() {
    let mut e = PermissionEngine::permissive();
    // external_directory is ask by default.
    assert_eq!(e.decide(&check(EXTERNAL_DIRECTORY, "/tmp/x")), Ask);
    e.approve([rule(EXTERNAL_DIRECTORY, "/tmp/*", Allow)]);
    assert_eq!(e.decide(&check(EXTERNAL_DIRECTORY, "/tmp/x")), Allow);
    // Approving a denied catastrophe does nothing.
    e.approve([rule("bash", "rm -rf /", Allow)]);
    assert_eq!(e.decide(&check("bash", "rm -rf /")), Deny);
}

/// `~` has to become the *real* home, not `$HOME`: Windows does not set that variable, so a
/// user's `~/...` rule used to stay a literal tilde there and match nothing. Asserted against
/// whatever this machine's home actually is, since the lookup deliberately no longer reads an
/// environment variable a test could stub.
#[test]
fn home_expansion_in_patterns() {
    let home = crate::paths::display(&crate::user_dirs::home_dir().expect("a home directory"));
    assert_eq!(expand_home("~/projects/**"), format!("{home}/projects/**"));
    assert_eq!(expand_home("$HOME/x"), format!("{home}/x"));
    assert_eq!(expand_home("~"), home);
    assert!(!expand_home("~/projects/**").contains('\\'), "patterns are matched with `/`");
    assert_eq!(expand_home("/abs/path"), "/abs/path", "nothing else is touched");
}
