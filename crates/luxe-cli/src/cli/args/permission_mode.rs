//! Permission mode — CLI policy validation before startup.

use clap::builder::{EnumValueParser, PossibleValue, TypedValueParser};
use clap::ValueEnum;
use luxe_core::permission::PermissionMode;

/// Permission presets accepted on the command line.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PermissionModeArg {
    /// Keep the built-in guardrails without asking before every mutation.
    Default,
    /// Ask before mutating tools.
    Strict,
    /// Deny mutating tools.
    #[value(alias = "readonly", alias = "ro")]
    ReadOnly,
    /// Disable the built-in guardrails.
    Yolo,
}

impl From<PermissionModeArg> for PermissionMode {
    fn from(arg: PermissionModeArg) -> Self {
        match arg {
            PermissionModeArg::Default => Self::Default,
            PermissionModeArg::Strict => Self::Strict,
            PermissionModeArg::ReadOnly => Self::ReadOnly,
            PermissionModeArg::Yolo => Self::Yolo,
        }
    }
}

#[derive(Clone)]
pub(super) struct PermissionModeParser;

impl TypedValueParser for PermissionModeParser {
    type Value = PermissionModeArg;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, clap::Error> {
        // Keep the core parser's normalization without accepting an unknown policy
        let normalized = value.to_str().map(|raw| raw.trim().replace(['_', ' '], "-"));
        let value = normalized.as_deref().map(std::ffi::OsStr::new).unwrap_or(value);
        EnumValueParser::<PermissionModeArg>::new().parse_ref(cmd, arg, value)
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        Some(Box::new(
            PermissionModeArg::value_variants().iter().filter_map(ValueEnum::to_possible_value),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{permission_mode_from_args, Args};
    use clap::{error::ErrorKind, Parser};
    use luxe_core::permission::PermissionMode;

    /// A policy typo must stop startup instead of falling back to permissive defaults.
    #[test]
    fn an_unknown_permission_mode_is_refused() {
        for typo in ["strcit", "read-onyl", "", "unknown"] {
            let error = Args::try_parse_from(["luxe", "--permission-mode", typo])
                .expect_err("an invalid policy must stop startup");
            assert_eq!(error.kind(), ErrorKind::InvalidValue, "{typo:?}");
        }
    }

    /// CLI validation must preserve the mode parser's accepted spellings.
    #[test]
    fn accepted_permission_modes_keep_their_meaning() {
        for spelling in [
            "default",
            "strict",
            "read-only",
            "readonly",
            "ro",
            "yolo",
            "read_only",
            "read only",
            "STRICT",
            " Read_Only ",
        ] {
            let args = Args::try_parse_from(["luxe", "--permission-mode", spelling])
                .expect("a supported mode");
            assert_eq!(permission_mode_from_args(&args), PermissionMode::parse(spelling));
        }
    }

    /// Shortcuts retain precedence over a valid named mode; absence defers to config.
    #[test]
    fn permission_shortcuts_keep_their_precedence() {
        for (flags, expected) in [
            (vec![], None),
            (vec!["--permission-mode", "strict"], Some(PermissionMode::Strict)),
            (vec!["--permission-mode", "strict", "--read-only"], Some(PermissionMode::ReadOnly)),
            (vec!["--permission-mode", "strict", "--yolo"], Some(PermissionMode::Yolo)),
            (vec!["--read-only", "--yolo"], Some(PermissionMode::Yolo)),
        ] {
            let args =
                Args::try_parse_from(std::iter::once("luxe").chain(flags)).expect("valid flags");
            assert_eq!(permission_mode_from_args(&args), expected);
        }
    }
}
