//! launchd — the bridge as a launch agent, `~/Library/LaunchAgents/dev.luxe.remote.plist`.

use super::Launch;

const LABEL: &str = "dev.luxe.remote";

/// Started at login, and again if it stops.
const TEMPLATE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
{arguments}  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
</dict>
</plist>
"#;

/// The property list for `launch`.
fn plist(launch: &Launch) -> String {
    let program = launch.program.to_string_lossy();
    let arguments: String = std::iter::once(program.as_ref())
        .chain(launch.args.iter().map(String::as_str))
        .map(|word| format!("    <string>{}</string>\n", escape(word)))
        .collect();
    TEMPLATE.replace("{label}", LABEL).replace("{arguments}", &arguments)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(target_os = "macos")]
mod manage {
    use std::path::PathBuf;

    use super::super::{remove_file, run, write_file, Launch, ServiceError};
    use super::{plist, LABEL};

    fn plist_path() -> Result<PathBuf, ServiceError> {
        let home = luxe_core::user_dirs::home_dir().ok_or(ServiceError::Unsupported)?;
        Ok(home.join("Library").join("LaunchAgents").join(format!("{LABEL}.plist")))
    }

    /// `gui/<uid>`, the signed-in user's launchd domain. Asked of `id` because reading the uid
    /// directly needs `unsafe`.
    fn domain() -> Result<String, ServiceError> {
        let output = std::process::Command::new("id").arg("-u").output();
        let output = output.map_err(|error| ServiceError::Command {
            command: "id -u".into(),
            detail: error.to_string(),
        })?;
        Ok(format!("gui/{}", String::from_utf8_lossy(&output.stdout).trim()))
    }

    pub(crate) fn install(launch: &Launch) -> Result<String, ServiceError> {
        let path = plist_path()?;
        let domain = domain()?;
        // a loaded agent keeps its old command until it is booted out
        let _ = run("launchctl", &["bootout", &format!("{domain}/{LABEL}")]);
        write_file(&path, &plist(launch))?;
        run("launchctl", &["bootstrap", &domain, &path.to_string_lossy()])?;
        Ok(path.display().to_string())
    }

    pub(crate) fn remove() -> Result<bool, ServiceError> {
        let path = plist_path()?;
        if !path.exists() {
            return Ok(false);
        }
        let _ = run("launchctl", &["bootout", &format!("{}/{LABEL}", domain()?)]);
        remove_file(&path)
    }

    pub(crate) fn installed() -> bool {
        plist_path().is_ok_and(|path| path.exists())
    }
}

#[cfg(target_os = "macos")]
pub(crate) use manage::{install, installed, remove};

#[cfg(test)]
mod tests {
    use super::*;

    /// Each word is its own argument, escaped for XML.
    ///
    /// The break this catches: an unescaped `&` or `<` in a path or login, which makes the plist
    /// unreadable and launchd refuses it with no reason a user would find.
    #[test]
    fn each_word_is_an_escaped_argument() {
        let launch = Launch { program: "/Users/a&b/luxe".into(), args: vec!["remote".into()] };
        let plist = plist(&launch);
        assert!(plist
            .contains("    <string>/Users/a&amp;b/luxe</string>\n    <string>remote</string>\n"));
        assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"), "{plist}");
    }
}
