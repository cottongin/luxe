use super::*;
use crate::github::ListItem;

// Every fixture below is copied from real `gh` 2.101.0 output, never written by hand: a
// round-trip through our own types would sail straight past a field `gh` renamed.

mod branches;
mod checks;
mod discovery;
mod process;
mod pull_requests;
mod reviews;
mod threads;
mod writes;

use branches::*;
use discovery::*;
use pull_requests::*;
#[cfg(unix)]
use reviews::*;
use threads::*;
