use super::*;
use crate::app::Msg;
use crate::fixture::{seeded_app, FixtureScenario};
use luxe_core::{
    event::{GuidedReviewFailure, GuidedReviewFailureKind},
    guided_review::{
        FileId, FindingId, FindingSeverity, FindingState, NonTextChange, NonTextChangeKind,
        ReviewCheck, ReviewCheckStatus, ReviewItemRef,
    },
};
use std::path::Path;

mod agent_activity;
mod attention_in_walkthrough;
mod composer;
mod description;
mod footer_contract;
mod guide_interaction;
mod rail_divider;
mod scale;
mod verification;

mod context;
mod diff;
mod feedback;
mod footer;
mod guide;
mod loading;
mod previews;

use diff::*;
use previews::*;
