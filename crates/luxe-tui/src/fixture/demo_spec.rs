//! The demo review's change, written as a reviewer would see it: files, hunks and chapters.

use luxe_core::guided_review::{ChapterKind, NoiseKind, ReviewFileStatus};

/// One hunk of the demo change, written the way a diff prints.
///
/// `lines` carries the leading `-`, `+` or space a unified diff uses, and the line numbers are
/// counted from them rather than typed — the numbers on screen then cannot disagree with the lines
/// beside them, which is the sort of detail a viewer notices without being able to say why.
pub(super) struct DemoHunk {
    /// Distinguishes this hunk from its siblings when its id is derived.
    pub(super) key: &'static str,
    pub(super) heading: &'static str,
    pub(super) old_start: u32,
    pub(super) new_start: u32,
    pub(super) lines: &'static [&'static str],
    /// The guide's sentence about this hunk. What a reviewer reads before the diff.
    pub(super) note: &'static str,
}

/// One file of the demo change.
pub(super) struct DemoFile {
    pub(super) path: &'static str,
    pub(super) status: ReviewFileStatus,
    pub(super) noise: NoiseKind,
    pub(super) hunks: &'static [DemoHunk],
}

/// One chapter, and the files it gathers.
pub(super) struct DemoChapter {
    pub(super) title: &'static str,
    pub(super) rationale: &'static str,
    pub(super) kind: ChapterKind,
    pub(super) files: &'static [DemoFile],
}

/// The change the demos walk: a free-form string becoming a type, and everything that follows.
///
/// Separate from [`fixture_guided_review`] on purpose, and not because one of them is wrong. That
/// one is deliberately one file and three lines so that two screenshots of different surfaces stay
/// comparable, and a pile of pinned assertions depends on it staying that size. A demo needs the
/// opposite: a change small enough to read in one screen does not need chapters, so a demo built on
/// the small corpus argues against the feature it is demonstrating.
///
/// Six files across four chapters, one of them formatting the reviewer can fold away. Every path,
/// symbol and line is written to be a change someone could plausibly have made — an invalid
/// category stops being representable, and the type flows outward from there to the router, the
/// HTTP boundary, and the tests.
pub(super) const DEMO_CHANGE: &[DemoChapter] = &[
    DemoChapter {
        title: "Make an invalid category unrepresentable",
        rationale:
            "A category was a string, so a typo type-checked. This is the type that stops it.",
        kind: ChapterKind::Core,
        // The short swap leads. It is the line the whole change is about, it fits on one screen,
        // and it has a deletion beside its addition — opening on a twenty-line pure insertion shows
        // a reviewer a wall of green and nothing to compare it against.
        files: &[
            DemoFile {
                path: "src/notification.rs",
                status: ReviewFileStatus::Modified,
                noise: NoiseKind::Semantic,
                hunks: &[
                    DemoHunk {
                        key: "notification field",
                        heading: "pub struct Notification",
                        old_start: 12,
                        new_start: 12,
                        lines: &[
                            " pub struct Notification {",
                            "-    pub category: String,",
                            "+    pub category: Category,",
                            "     pub title: String,",
                            "     pub body: String,",
                        ],
                        note: "The field the whole change exists for. Everything below is what \
                               this line forced.",
                    },
                    DemoHunk {
                        key: "notification label",
                        heading: "impl Notification",
                        old_start: 34,
                        new_start: 34,
                        lines: &[
                            " impl Notification {",
                            "-    pub fn label(&self) -> &str {",
                            "-        self.category.as_str()",
                            "-    }",
                            "+    pub fn label(&self) -> &'static str {",
                            "+        match self.category {",
                            "+            Category::Build => \"Build\",",
                            "+            Category::Deploy => \"Deploy\",",
                            "+            Category::Incident => \"Incident\",",
                            "+        }",
                            "+    }",
                        ],
                        note: "One label per category, from one match. The old version returned \
                               whatever string it was handed, which is how an empty badge got \
                               rendered.",
                    },
                ],
            },
            DemoFile {
                path: "src/notification/category.rs",
                status: ReviewFileStatus::Added,
                noise: NoiseKind::Semantic,
                hunks: &[DemoHunk {
                    key: "category enum",
                    heading: "pub enum Category",
                    old_start: 0,
                    new_start: 1,
                    lines: &[
                        "+/// What a notification is about, as a closed set.",
                        "+#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
                        "+pub enum Category {",
                        "+    Build,",
                        "+    Deploy,",
                        "+    Incident,",
                        "+}",
                        "+",
                        "+impl FromStr for Category {",
                        "+    type Err = UnknownCategory;",
                        "+",
                        "+    fn from_str(raw: &str) -> Result<Self, Self::Err> {",
                        "+        match raw {",
                        "+            \"build\" => Ok(Self::Build),",
                        "+            \"deploy\" => Ok(Self::Deploy),",
                        "+            \"incident\" => Ok(Self::Incident),",
                        "+            other => Err(UnknownCategory(other.to_owned())),",
                        "+        }",
                        "+    }",
                        "+}",
                    ],
                    note: "The closed set, and the one place a string is allowed to become one. \
                           `FromStr` returns a typed error rather than a default, so an unknown \
                           category has to be handled somewhere.",
                }],
            },
        ],
    },
    DemoChapter {
        title: "Thread it through the callers",
        rationale: "Both places that compared the old string, now matching on the type instead.",
        kind: ChapterKind::Consequence,
        files: &[
            DemoFile {
                path: "src/dispatch/router.rs",
                status: ReviewFileStatus::Modified,
                noise: NoiseKind::Semantic,
                hunks: &[DemoHunk {
                    key: "router match",
                    heading: "fn route",
                    old_start: 58,
                    new_start: 58,
                    lines: &[
                        " pub fn route(notification: &Notification) -> Channel {",
                        "-    if notification.category == \"incident\" {",
                        "-        return Channel::Pager;",
                        "-    }",
                        "-    Channel::Digest",
                        "+    match notification.category {",
                        "+        Category::Incident => Channel::Pager,",
                        "+        Category::Build | Category::Deploy => Channel::Digest,",
                        "+    }",
                        " }",
                    ],
                    note: "The match is exhaustive, so adding a category later fails to compile \
                           here instead of quietly routing to the digest.",
                }],
            },
            DemoFile {
                path: "src/api/webhook.rs",
                status: ReviewFileStatus::Modified,
                noise: NoiseKind::Semantic,
                hunks: &[DemoHunk {
                    key: "webhook parse",
                    heading: "async fn receive",
                    old_start: 41,
                    new_start: 41,
                    lines: &[
                        "     let payload: Payload = serde_json::from_slice(&body)?;",
                        "-    let notification = Notification {",
                        "-        category: payload.category,",
                        "+    let category = payload.category.parse().map_err(|_| {",
                        "+        Rejection::unprocessable(\"unknown notification category\")",
                        "+    })?;",
                        "+    let notification = Notification {",
                        "+        category,",
                        "         title: payload.title,",
                    ],
                    note: "The boundary where a string is still a string. Parsing here is what \
                           lets everything inside assume the type.",
                }],
            },
        ],
    },
    DemoChapter {
        title: "Cover what the string could not express",
        rationale: "A parse failure was not a case the old shape had, so it had no test.",
        kind: ChapterKind::Test,
        files: &[DemoFile {
            path: "tests/notification_category.rs",
            status: ReviewFileStatus::Added,
            noise: NoiseKind::Semantic,
            hunks: &[DemoHunk {
                key: "category tests",
                heading: "fn an_unknown_category_is_rejected",
                old_start: 0,
                new_start: 1,
                lines: &[
                    "+#[test]",
                    "+fn an_unknown_category_is_rejected() {",
                    "+    assert!(\"stagin\".parse::<Category>().is_err());",
                    "+}",
                    "+",
                    "+#[test]",
                    "+fn every_category_has_a_label() {",
                    "+    for category in Category::ALL {",
                    "+        assert!(!Notification::sample(category).label().is_empty());",
                    "+    }",
                    "+}",
                ],
                note: "The typo case, spelled as a typo. The second test walks the closed set, so \
                       a category added without a label fails here.",
            }],
        }],
    },
    DemoChapter {
        title: "Formatting the tool rewrote",
        rationale: "Import order the formatter changed while the real work was happening.",
        kind: ChapterKind::Supporting,
        files: &[DemoFile {
            path: "src/render/badge.rs",
            status: ReviewFileStatus::Modified,
            noise: NoiseKind::FormattingOnly,
            hunks: &[DemoHunk {
                key: "import churn",
                heading: "use",
                old_start: 1,
                new_start: 1,
                lines: &[
                    "-use std::fmt::{self, Display};",
                    "-use crate::notification::Notification;",
                    "+use crate::notification::Notification;",
                    "+use std::fmt::{self, Display};",
                ],
                note: "Nothing here changed behavior.",
            }],
        }],
    },
];
