use super::*;

/// The receipt the 0.1.4 PowerShell installer writes, its placeholders filled the way the script
/// fills them for an install into `~/.local/bin`.
const WINDOWS_RECEIPT: &str = r#"{"binaries":["luxe"],"binary_aliases":{},"cdylibs":[],"cstaticlibs":[],"install_layout":"flat","install_prefix":"C:\\Users\\dev\\.local\\bin","modify_path":true,"provider":{"source":"cargo-dist","version":"0.33.0"},"source":{"app_name":"luxe-cli","name":"luxe","owner":"evanwhittaker","release_type":"github"},"version":"0.1.4"}"#;

/// The real receipt reads, and the two fields an update needs come out of it.
#[test]
fn a_real_receipt_names_where_and_whether_path_was_edited() {
    let receipt = InstallReceipt::parse(WINDOWS_RECEIPT).expect("the installer's receipt parses");
    assert_eq!(receipt.install_prefix, PathBuf::from(r"C:\Users\dev\.local\bin"));
    assert!(receipt.modify_path);

    let declined = WINDOWS_RECEIPT.replace(r#""modify_path":true"#, r#""modify_path":false"#);
    assert!(!InstallReceipt::parse(&declined).expect("parses").modify_path);
}

/// Something that is not a receipt is no receipt, rather than an installer copy in the wrong place.
#[test]
fn a_broken_receipt_is_none() {
    assert_eq!(InstallReceipt::parse("{"), None);
    assert_eq!(InstallReceipt::parse(r#"{"version":"0.1.4"}"#), None, "no prefix, no install");
}

/// A receipt covers a binary in its prefix, or in its `bin` for Cargo's layout, and nothing else.
#[test]
fn a_receipt_covers_only_its_own_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let receipt =
        |prefix: &Path| InstallReceipt { install_prefix: prefix.into(), modify_path: true };

    assert!(receipt(&bin).covers(&bin.join("luxe")), "flat: the prefix itself");
    assert!(receipt(dir.path()).covers(&bin.join("luxe")), "cargo layout: the prefix's bin");
    assert!(!receipt(&bin).covers(&dir.path().join("luxe")), "the prefix's parent is not it");
    assert!(!receipt(Path::new("/nowhere")).covers(&bin.join("luxe")));
}

/// The update goes where the receipt says, with its `PATH` choice; without a receipt for this
/// binary, beside the binary with `PATH` untouched.
#[test]
fn the_target_follows_the_receipt_and_falls_back_to_the_binary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let luxe = dir.path().join("luxe");
    let mine = InstallReceipt { install_prefix: dir.path().into(), modify_path: true };
    let other = InstallReceipt { install_prefix: "/elsewhere".into(), modify_path: true };

    let target = InstallTarget::for_binary(&luxe, Some(&mine)).expect("a target");
    assert_eq!(target, InstallTarget { dir: dir.path().into(), modify_path: true });

    for receipt in [None, Some(&other)] {
        let target = InstallTarget::for_binary(&luxe, receipt).expect("a target");
        assert_eq!(target, InstallTarget { dir: dir.path().into(), modify_path: false });
    }
}

/// The variables are the ones the 0.1.4 scripts read (`$env:LUXE_CLI_INSTALL_DIR`,
/// `LUXE_CLI_NO_MODIFY_PATH`), and the switch that would stop the receipt is never among them.
#[test]
fn the_script_variables_are_the_ones_the_installer_reads() {
    let names = |target: InstallTarget| -> Vec<String> {
        target.script_variables().into_iter().map(|(name, _)| name).collect()
    };
    let edits = InstallTarget { dir: "/home/dev/.local/bin".into(), modify_path: true };
    assert_eq!(names(edits.clone()), ["LUXE_CLI_INSTALL_DIR"]);
    assert_eq!(edits.script_variables()[0].1, OsString::from("/home/dev/.local/bin"));

    let keeps = InstallTarget { modify_path: false, ..edits };
    assert_eq!(names(keeps), ["LUXE_CLI_INSTALL_DIR", "LUXE_CLI_NO_MODIFY_PATH"]);
}

/// The script's address is the one the install instructions give, for this platform's script.
#[test]
fn the_script_url_is_the_releases_own() {
    let script = if cfg!(windows) { "luxe-cli-installer.ps1" } else { "luxe-cli-installer.sh" };
    assert_eq!(
        installer_script_url("v0.1.5"),
        format!("https://github.com/evanwhittaker/luxe/releases/download/v0.1.5/{script}")
    );
}
