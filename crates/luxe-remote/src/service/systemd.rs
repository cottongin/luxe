//! systemd — the bridge as a user unit, `~/.config/systemd/user/luxe-remote.service`.
//!
//! A user unit runs while you're signed in; with lingering on (`loginctl enable-linger`) it also
//! runs from boot. Setup doesn't turn lingering on: that is a machine-wide choice.

use super::Launch;

const TEMPLATE: &str = "[Unit]
Description=luxe remote: your sessions on your own devices
After=network-online.target

[Service]
ExecStart={command}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
";

/// The unit file for `launch`.
fn unit(launch: &Launch) -> String {
    let program = launch.program.to_string_lossy();
    let command: Vec<String> = std::iter::once(program.as_ref())
        .chain(launch.args.iter().map(String::as_str))
        .map(quote)
        .collect();
    TEMPLATE.replace("{command}", &command.join(" "))
}

/// One word of `ExecStart`, quoted the way systemd reads it: `%` and `$` would otherwise expand.
fn quote(word: &str) -> String {
    let escaped =
        word.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%").replace('$', "$$");
    format!("\"{escaped}\"")
}

#[cfg(all(unix, not(target_os = "macos")))]
mod manage {
    use std::path::PathBuf;

    use super::super::{remove_file, run, write_file, Launch, ServiceError};
    use super::unit;

    const UNIT: &str = "luxe-remote.service";

    fn unit_path() -> Result<PathBuf, ServiceError> {
        let config = luxe_core::user_dirs::config_home().ok_or(ServiceError::Unsupported)?;
        Ok(config.join("systemd").join("user").join(UNIT))
    }

    fn systemctl(args: &[&str]) -> Result<(), ServiceError> {
        let mut all = vec!["--user"];
        all.extend_from_slice(args);
        run("systemctl", &all)
    }

    pub(crate) fn install(launch: &Launch) -> Result<String, ServiceError> {
        let path = unit_path()?;
        write_file(&path, &unit(launch))?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", UNIT])?;
        // restart, not start, so a reinstall runs the new command
        systemctl(&["restart", UNIT])?;
        Ok(path.display().to_string())
    }

    pub(crate) fn remove() -> Result<bool, ServiceError> {
        let path = unit_path()?;
        if !path.exists() {
            return Ok(false);
        }
        systemctl(&["disable", "--now", UNIT])?;
        let removed = remove_file(&path)?;
        systemctl(&["daemon-reload"])?;
        Ok(removed)
    }

    pub(crate) fn installed() -> bool {
        unit_path().is_ok_and(|path| path.exists())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) use manage::{install, installed, remove};

#[cfg(test)]
mod tests {
    use super::*;

    /// The unit runs exactly the command it was given, whatever its path holds.
    ///
    /// The break this catches: writing words bare, so a path with a space splits in two, or a `%`
    /// or `$` in it is expanded by systemd into something else.
    #[test]
    fn the_unit_runs_the_command_as_given() {
        let launch = Launch {
            program: "/home/a b/.local/bin/luxe".into(),
            args: vec!["remote".into(), "--allow".into(), "50%$off@github".into()],
        };
        let unit = unit(&launch);
        let exec = unit.lines().find(|line| line.starts_with("ExecStart=")).unwrap();
        assert_eq!(
            exec,
            r#"ExecStart="/home/a b/.local/bin/luxe" "remote" "--allow" "50%%$$off@github""#
        );
        assert!(unit.contains("WantedBy=default.target"), "it starts at login");
    }
}
