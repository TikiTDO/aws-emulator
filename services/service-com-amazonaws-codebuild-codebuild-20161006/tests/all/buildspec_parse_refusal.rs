// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a build answers about a buildspec the YAML parser REFUSED,
//! driven over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! RULE under test: a buildspec that is not parseable YAML is accepted by the control
//! plane and refused by the build itself while it is processing the source — the
//! source-download phase is the one phase that fails, carrying the YAML parser's own
//! diagnostic (the problem the parser named and the line it named it at) rather than a
//! classification of CodeBuild's own, the build's terminal status is `FAILED` and not the
//! build-system `FAULT`, no command phase ran so none is reported, and nothing is
//! exported. A document the parser READS is not this refusal, however little of it a
//! build can use.
//!
//! Each row below differs along what that rule generalises over: which YAML malformation
//! the document carries, where in the document it is, and whether the text came from the
//! project's source or from `StartBuild`'s `buildspecOverride`.
//!
//! The line a row expects is the one CodeBuild's diagnostic names, which is the parse
//! MARK's line and so counts the document's first line as line 0.

use serde_json::json;

use super::project_build_lifecycle::{
    create_project, emulator, phase_of, phase_pairs, settled_build,
};

/// RULE: an unparseable buildspec is the source-download phase's failure, reported with
/// the parser's own diagnostic, and the build FAILED rather than FAULTed.
#[tokio::test]
async fn an_unparseable_buildspec_fails_the_source_phase_with_the_parser_diagnostic() {
    let router = emulator();
    /// One row: the case, whether the text is sent as `StartBuild`'s `buildspecOverride`
    /// (otherwise it is the project's own source buildspec), the text, the problem the
    /// parser is expected to name, and the mark line it is expected to name it at.
    type Row = (&'static str, bool, String, &'static str, u32);

    let rows: Vec<Row> = vec![
        (
            // Tabs are illegal as YAML indentation, and a tab can start no token. This is
            // the recorded instance: the tab is on the document's third line, which the
            // diagnostic's mark numbers 2.
            "a tab indenting a nested mapping, sent as an override",
            true,
            "version: 0.2\nphases:\n\tbuild:\n\t\tcommands:\n\t\t\t- echo indented with tabs\n"
                .to_string(),
            "found character that cannot start any token",
            2,
        ),
        (
            "the same tab indentation in the project's own buildspec",
            false,
            "version: 0.2\nphases:\n\tbuild:\n\t\tcommands:\n\t\t\t- echo indented with tabs\n"
                .to_string(),
            "found character that cannot start any token",
            2,
        ),
        (
            // A later line carries the malformation, so the position is the parser's and
            // not a constant.
            "a tab indenting a command list five lines in",
            false,
            "version: 0.2\nenv:\n  variables:\n    PLACE: spec\nphases:\n  build:\n    \
             commands:\n\t- echo one\n"
                .to_string(),
            "found character that cannot start any token",
            7,
        ),
        (
            // `@` is a YAML reserved indicator: it can start no token either.
            "a reserved indicator opening a line",
            false,
            "version: 0.2\n@phases:\n".to_string(),
            "found character that cannot start any token",
            1,
        ),
        (
            "a second mapping value on one line",
            false,
            "version: 0.2\nphases: build: oops\n".to_string(),
            "mapping values are not allowed in this context",
            1,
        ),
        (
            "a flow sequence nothing closes",
            false,
            "version: 0.2\nphases: [build\n".to_string(),
            "expected ',' or ']'",
            2,
        ),
        (
            "a quoted scalar nothing closes",
            false,
            "version: 0.2\nphases: 'build\n".to_string(),
            "unexpected end of stream",
            1,
        ),
        (
            // The tab here interrupts the plain scalar on the line before, which is the
            // construct the parser marks — the document's first line, numbered 0.
            "a tab indenting a line that continues a scalar",
            false,
            "version: 0.2\n\tphases: [oops\n".to_string(),
            "found character that cannot start any token",
            0,
        ),
    ];

    for (index, (case, as_override, buildspec, problem, line)) in rows.iter().enumerate() {
        let project = format!("unparseable-proj-{index}");
        let (source, extra) = if *as_override {
            (
                json!({"source": {"type": "NO_SOURCE", "buildspec": "version: 0.2"}}),
                json!({"buildspecOverride": buildspec}),
            )
        } else {
            (
                json!({"source": {"type": "NO_SOURCE", "buildspec": buildspec}}),
                json!({}),
            )
        };
        create_project(&router, &project, source).await;
        let build = settled_build(&router, &project, extra).await;

        assert_eq!(
            build["buildComplete"], json!(true),
            "case {case:?}: the build must settle, got {build}"
        );
        assert_eq!(
            build["buildStatus"], json!("FAILED"),
            "case {case:?}: a buildspec the parser refused FAILS the build, got {build}"
        );
        let pairs = phase_pairs(&build);
        let failed: Vec<&(String, Option<String>)> = pairs
            .iter()
            .filter(|(_, status)| status.as_deref() == Some("FAILED"))
            .collect();
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: exactly one phase fails, got {pairs:?}"
        );
        assert_eq!(
            failed[0].0, "DOWNLOAD_SOURCE",
            "case {case:?}: the source phase carries the refusal, got {pairs:?}"
        );
        assert!(
            pairs
                .iter()
                .all(|(_, status)| status.as_deref() != Some("FAULT")),
            "case {case:?}: a buildspec the caller wrote is no build-system fault, got {pairs:?}"
        );
        assert!(
            pairs.iter().all(|(phase, _)| phase != "BUILD"),
            "case {case:?}: no command phase ran, so none is reported, got {pairs:?}"
        );
        assert!(
            build["exportedEnvironmentVariables"]
                .as_array()
                .is_none_or(|exported| exported.is_empty()),
            "case {case:?}: a build that ran nothing exported nothing, got {build}"
        );

        let download = phase_of(&build, "DOWNLOAD_SOURCE")
            .unwrap_or_else(|| panic!("case {case:?}: a DOWNLOAD_SOURCE phase, got {build}"));
        let message = download["contexts"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(problem),
            "case {case:?}: expected the parser to report {problem:?}, got {download}"
        );
        assert!(
            message.ends_with(&format!("at line {line}")),
            "case {case:?}: expected the parser's position \"at line {line}\", got {download}"
        );
        assert!(
            download["contexts"][0]["statusCode"].is_null(),
            "case {case:?}: the parser's diagnostic carries no classification, got {download}"
        );
    }
}

/// RULE: a document the parser READS is not a parse refusal, however little of it a build
/// can use — a `commands` member that is not a list of command lines declares no command,
/// which is a build that ran nothing and failed nothing.
#[tokio::test]
async fn a_readable_buildspec_is_never_a_parse_refusal() {
    let router = emulator();
    // (case, the buildspec the parser reads)
    let rows: Vec<(&str, String)> = vec![
        (
            "a scalar under commands",
            "version: 0.2\nphases:\n  build:\n    commands: echo scalar\n".to_string(),
        ),
        (
            "an empty command list",
            "version: 0.2\nphases:\n  build:\n    commands: []\n".to_string(),
        ),
        (
            "a command list of one empty entry",
            "version: 0.2\nphases:\n  build:\n    commands:\n      -\n".to_string(),
        ),
        (
            // A tab is legal everywhere but as indentation: inside a scalar it is text.
            "a tab inside a command line's scalar",
            "version: 0.2\nphases:\n  build:\n    commands:\n      - echo one\ttwo\n".to_string(),
        ),
        (
            // YAML 1.2 is a superset of JSON, so the JSON form of a buildspec reads
            // through the same parser.
            "the whole buildspec written as JSON",
            "{\"version\": 0.2, \"phases\": {\"build\": {\"commands\": [\"echo json\"]}}}\n"
                .to_string(),
        ),
    ];

    for (index, (case, buildspec)) in rows.iter().enumerate() {
        let project = format!("readable-proj-{index}");
        create_project(
            &router,
            &project,
            json!({"source": {"type": "NO_SOURCE", "buildspec": buildspec}}),
        )
        .await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildStatus"], json!("SUCCEEDED"),
            "case {case:?}: a buildspec the parser read fails nothing, got {build}"
        );
        let download = phase_of(&build, "DOWNLOAD_SOURCE")
            .unwrap_or_else(|| panic!("case {case:?}: a DOWNLOAD_SOURCE phase, got {build}"));
        assert_eq!(
            download["phaseStatus"], json!("SUCCEEDED"),
            "case {case:?}: the source phase read the buildspec, got {download}"
        );
        assert!(
            download["contexts"].is_null(),
            "case {case:?}: a buildspec that was read carries no context, got {download}"
        );
    }
}
