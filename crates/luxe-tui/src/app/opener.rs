//! Opener — explicit opt-in before App can launch a desktop program.

/// Where [`super::App::open_externally`] sends a file or an address.
///
/// Inactive by default so tests, fixtures and embedders cannot launch programs accidentally.
/// [`crate::main_loop::run`] installs [`ExternalOpener::Desktop`] alongside the real clipboard.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(any(test, feature = "fixture")), derive(Default))]
pub enum ExternalOpener {
    /// Launch nothing
    #[cfg_attr(not(any(test, feature = "fixture")), default)]
    Disabled,
    /// Keep what was asked for, and launch nothing
    #[cfg(any(test, feature = "fixture"))]
    Recorded(Vec<String>),
    /// Hand it to the desktop's own opener, detached
    Desktop,
}

#[cfg(any(test, feature = "fixture"))]
impl Default for ExternalOpener {
    fn default() -> Self {
        Self::Recorded(Vec::new())
    }
}

impl ExternalOpener {
    pub(super) fn should_launch(&mut self, _label: &str) -> bool {
        match self {
            Self::Disabled => false,
            Self::Desktop => true,
            #[cfg(any(test, feature = "fixture"))]
            Self::Recorded(opened) => {
                opened.push(_label.to_owned());
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only explicit desktop opt-in may reach the platform opener.
    #[test]
    fn disabled_opener_never_launches() {
        assert!(!ExternalOpener::Disabled.should_launch("a file"));
    }

    #[test]
    fn desktop_opener_allows_launching() {
        assert!(ExternalOpener::Desktop.should_launch("a file"));
    }

    #[test]
    fn fixture_default_records_without_launching() {
        let mut opener = ExternalOpener::default();
        assert!(!opener.should_launch("a file"));
        assert_eq!(opener, ExternalOpener::Recorded(vec!["a file".into()]));
    }
}
