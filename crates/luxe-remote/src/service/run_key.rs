//! Run key — the bridge started at sign-in from `HKCU\...\CurrentVersion\Run`, which needs no admin.
//!
//! `luxe.exe` is a console program, so started bare it would open a console window at every
//! sign-in. PowerShell's hidden window style starts it with none showing, beyond a brief flash.

use super::Launch;

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const VALUE: &str = "Luxe remote";

/// The command line the Run key holds for `launch`.
fn command_line(launch: &Launch) -> String {
    let program = launch.program.to_string_lossy();
    let words: Vec<String> = std::iter::once(program.as_ref())
        .chain(launch.args.iter().map(String::as_str))
        .map(|word| format!("'{}'", word.replace('\'', "''")))
        .collect();
    format!("powershell.exe -NoProfile -WindowStyle Hidden -Command \"& {}\"", words.join(" "))
}

#[cfg(windows)]
mod manage {
    use super::super::{run, Launch, ServiceError};
    use super::{command_line, RUN_KEY, VALUE};

    pub(crate) fn install(launch: &Launch) -> Result<String, ServiceError> {
        let command = command_line(launch);
        run("reg", &["add", RUN_KEY, "/v", VALUE, "/t", "REG_SZ", "/d", &command, "/f"])?;
        start_now(launch)?;
        Ok(format!(r"{RUN_KEY}\{VALUE}"))
    }

    /// The Run key only acts at the next sign-in; this one starts now, with no window.
    fn start_now(launch: &Launch) -> Result<(), ServiceError> {
        let mut command = tokio::process::Command::new(&launch.program);
        command.args(&launch.args).stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        luxe_core::program::detach_from_terminal(&mut command);
        command.spawn().map(drop).map_err(|error| ServiceError::Command {
            command: launch.program.display().to_string(),
            detail: error.to_string(),
        })
    }

    pub(crate) fn remove() -> Result<bool, ServiceError> {
        if !installed() {
            return Ok(false);
        }
        run("reg", &["delete", RUN_KEY, "/v", VALUE, "/f"])?;
        Ok(true)
    }

    pub(crate) fn installed() -> bool {
        run("reg", &["query", RUN_KEY, "/v", VALUE]).is_ok()
    }
}

#[cfg(windows)]
pub(crate) use manage::{install, installed, remove};

#[cfg(test)]
mod tests {
    use super::*;

    /// Each word reaches PowerShell as one literal string.
    ///
    /// The break this catches: words left bare or double-quoted, so a path with a space splits, or
    /// PowerShell expands a `$` in it, or an apostrophe ends the string early.
    #[test]
    fn each_word_is_one_literal_string() {
        let launch = Launch {
            program: r"C:\Users\Alex O'Neil\luxe.exe".into(),
            args: vec!["remote".into(), "--allow".into(), "$alex@github".into()],
        };
        assert_eq!(
            command_line(&launch),
            r#"powershell.exe -NoProfile -WindowStyle Hidden -Command "& 'C:\Users\Alex O''Neil\luxe.exe' 'remote' '--allow' '$alex@github'""#
        );
    }
}
