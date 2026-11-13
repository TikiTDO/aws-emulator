// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a BUILD answers, driven over the wire the router serves:
//! `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! RULE under test: a build RUNS the commands its buildspec declares, phase by phase,
//! and its terminal `buildStatus` is what those commands answered — `SUCCEEDED` when
//! every command of every phase exited zero, `FAILED` when one did not, with the failing
//! phase's `phaseStatus` `FAILED` and its context naming the command and the status it
//! exited with. The phases after a failure that aborts the build never ran, so the build
//! reports none of them. A buildspec that cannot be read is the source-download phase's
//! `YAML_FILE_ERROR`, and a buildspec the PARSER refused is the parser's own diagnostic
//! (the suite that owns that rule is `buildspec_parse_refusal`).
//!
//! Each row below differs along what that rule generalises over: which phase declares the
//! command, how the command answers (a utility the environment runs, a program it has
//! none for, an `exit`), how the command line composes answers (`&&`, `||`, `!`, a
//! pipeline), and whether the phase's `on-failure` aborts the build.

use serde_json::{json, Value};

use super::project_build_lifecycle::{
    create_project, emulator, phase_pairs, settled_build,
};

/// A `CreateProject` source carrying `buildspec` inline, with no input source code of
/// its own — the form that lets a suite fix exactly what a build runs.
fn source_with(buildspec: &str) -> Value {
    json!({"source": {"type": "NO_SOURCE", "buildspec": buildspec}})
}

/// A buildspec whose `phases` declare `commands` for `phase`.
fn buildspec_for(phase: &str, commands: &[&str]) -> String {
    let listed = commands
        .iter()
        .map(|command| format!("      - {command}\n"))
        .collect::<String>();
    format!("version: 0.2\nphases:\n  {phase}:\n    commands:\n{listed}")
}

/// RULE: the build's terminal status is what its own commands answered.
#[tokio::test]
async fn a_builds_status_is_what_its_buildspec_commands_answered() {
    let router = emulator();
    // (case, buildspec, expected buildStatus, the phase expected to carry the failure)
    let rows: Vec<(&str, String, &str, Option<&str>)> = vec![
        (
            "a buildspec declaring no phase at all runs nothing and fails nothing",
            "version: 0.2".to_string(),
            "SUCCEEDED",
            None,
        ),
        (
            "a command the environment runs, exiting zero",
            buildspec_for("build", &["echo building"]),
            "SUCCEEDED",
            None,
        ),
        (
            "the utility specified to answer failure",
            buildspec_for("build", &["false"]),
            "FAILED",
            Some("BUILD"),
        ),
        (
            "an explicit non-zero exit",
            buildspec_for("build", &["exit 7"]),
            "FAILED",
            Some("BUILD"),
        ),
        (
            "a program this build environment has none for",
            buildspec_for("build", &["docker push example:latest"]),
            "FAILED",
            Some("BUILD"),
        ),
        (
            "a failing command whose failure the line swallows",
            buildspec_for("build", &["docker push example:latest || echo skipped"]),
            "SUCCEEDED",
            None,
        ),
        (
            "an and-list whose second command fails",
            buildspec_for("build", &["true && false"]),
            "FAILED",
            Some("BUILD"),
        ),
        (
            "a negated failing command",
            // Quoted in the buildspec: `!` opens a YAML tag, so an unquoted `! false`
            // is the scalar `false` rather than a negated command line.
            "version: 0.2\nphases:\n  build:\n    commands:\n      - '! false'\n".to_string(),
            "SUCCEEDED",
            None,
        ),
        (
            "a pipeline whose last command the environment has no program for",
            buildspec_for("build", &["echo hello | tr a-z A-Z"]),
            "FAILED",
            Some("BUILD"),
        ),
        (
            "the install phase failing",
            buildspec_for("install", &["false"]),
            "FAILED",
            Some("INSTALL"),
        ),
        (
            "the pre_build phase failing",
            buildspec_for("pre_build", &["false"]),
            "FAILED",
            Some("PRE_BUILD"),
        ),
        (
            "the post_build phase failing",
            buildspec_for("post_build", &["false"]),
            "FAILED",
            Some("POST_BUILD"),
        ),
        (
            "every phase succeeding",
            "version: 0.2\nphases:\n  install:\n    commands:\n      - echo a\n  \
             pre_build:\n    commands:\n      - echo b\n  build:\n    commands:\n      \
             - echo c\n  post_build:\n    commands:\n      - echo d\n"
                .to_string(),
            "SUCCEEDED",
            None,
        ),
    ];

    for (index, (case, buildspec, expected_status, failing_phase)) in rows.iter().enumerate() {
        let project = format!("spec-proj-{index}");
        create_project(&router, &project, source_with(buildspec)).await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildStatus"], json!(expected_status),
            "case {case:?}: expected buildStatus {expected_status}, got {build}"
        );
        assert_eq!(
            build["buildComplete"], json!(true),
            "case {case:?}: a settled build is complete, got {build}"
        );
        let pairs = phase_pairs(&build);
        match failing_phase {
            Some(phase) => {
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
                    failed[0].0, *phase,
                    "case {case:?}: expected {phase} to carry the failure, got {pairs:?}"
                );
            }
            None => assert!(
                pairs
                    .iter()
                    .all(|(_, status)| status.as_deref() != Some("FAILED")),
                "case {case:?}: no phase may fail, got {pairs:?}"
            ),
        }
        assert!(
            pairs
                .iter()
                .all(|(_, status)| status.as_deref() != Some("FAULT")),
            "case {case:?}: a build whose own commands decided it never FAULTs, got {pairs:?}"
        );
    }
}

/// RULE: the phases after a failure that ABORTS the build never ran, so the build reports
/// none of them; `on-failure: CONTINUE` goes on to the next phase, however many phases
/// have already failed.
#[tokio::test]
async fn an_aborting_phase_failure_reports_no_later_phase() {
    let router = emulator();
    let aborting = "version: 0.2\nphases:\n  pre_build:\n    commands:\n      - false\n  \
                    build:\n    commands:\n      - echo never\n";
    let continuing = "version: 0.2\nphases:\n  pre_build:\n    on-failure: CONTINUE\n    \
                      commands:\n      - false\n  build:\n    commands:\n      - echo ran\n";
    // A failure the first phase CONTINUES past, then two phases that succeed: every later
    // phase still runs, and only the build's status carries the failure.
    let continuing_three = "version: 0.2\nphases:\n  install:\n    on-failure: CONTINUE\n    \
                            commands:\n      - false\n  pre_build:\n    commands:\n      \
                            - echo second\n  build:\n    commands:\n      - echo third\n";

    create_project(&router, "abort-proj", source_with(aborting)).await;
    let aborted = settled_build(&router, "abort-proj", json!({})).await;
    let pairs = phase_pairs(&aborted);
    assert_eq!(
        aborted["buildStatus"], json!("FAILED"),
        "case \"ABORT\": expected FAILED, got {aborted}"
    );
    assert!(
        pairs.iter().all(|(phase, _)| phase != "BUILD"),
        "case \"ABORT\": the BUILD phase never ran, so it must not be reported, got {pairs:?}"
    );

    create_project(&router, "continue-proj", source_with(continuing)).await;
    let continued = settled_build(&router, "continue-proj", json!({})).await;
    let pairs = phase_pairs(&continued);
    assert_eq!(
        continued["buildStatus"], json!("FAILED"),
        "case \"CONTINUE\": the build still failed, got {continued}"
    );
    assert!(
        pairs
            .iter()
            .any(|(phase, status)| phase == "BUILD" && status.as_deref() == Some("SUCCEEDED")),
        "case \"CONTINUE\": the next phase ran and succeeded, got {pairs:?}"
    );

    create_project(&router, "continue-three-proj", source_with(continuing_three)).await;
    let three = settled_build(&router, "continue-three-proj", json!({})).await;
    let pairs = phase_pairs(&three);
    assert_eq!(
        three["buildStatus"], json!("FAILED"),
        "case \"CONTINUE past two more phases\": the build still failed, got {three}"
    );
    for (phase, expected) in [
        ("INSTALL", "FAILED"),
        ("PRE_BUILD", "SUCCEEDED"),
        ("BUILD", "SUCCEEDED"),
        ("UPLOAD_ARTIFACTS", "SUCCEEDED"),
        ("FINALIZING", "SUCCEEDED"),
    ] {
        // Every phase after the CONTINUE is still reported, with its own outcome.
        let reported = pairs.iter().find(|(reported, _)| reported == phase);
        assert_eq!(
            reported.and_then(|(_, status)| status.clone()),
            Some(expected.to_string()),
            "case \"CONTINUE past two more phases\": expected {phase} {expected}, got {pairs:?}"
        );
    }
}

/// RULE: a build that ran to success reports the phases of its own finishing work —
/// putting its artifact files where its artifacts say, and ending the session — whether
/// its commands ran out or an `exit 0` ended the session early. A build whose artifacts
/// name a destination has its artifact files looked for in the tree its commands ran
/// against, and this build environment has none, so the upload ends in CodeBuild's client
/// error for finding nothing to upload.
#[tokio::test]
async fn the_finishing_phases_report_what_the_build_put_where() {
    let router = emulator();
    // (case, extra CreateProject members, the buildspec, the UPLOAD_ARTIFACTS status, the
    // context message that phase carries, the build's status)
    /// One row: the case, the extra `CreateProject` members, the buildspec, the status
    /// the `UPLOAD_ARTIFACTS` phase takes, the context message it carries, and the
    /// build's own status.
    type Row = (&'static str, Value, String, &'static str, Option<&'static str>, &'static str);

    let rows: Vec<Row> = vec![
        (
            "a build whose artifacts name no destination has nothing to put anywhere",
            json!({}),
            buildspec_for("build", &["echo building"]),
            "SUCCEEDED",
            None,
            "SUCCEEDED",
        ),
        (
            "an exit 0 ends the session early and the build still finishes",
            json!({}),
            "version: 0.2\nphases:\n  pre_build:\n    commands:\n      - exit 0\n  \
             build:\n    commands:\n      - echo never\n"
                .to_string(),
            "SUCCEEDED",
            None,
            "SUCCEEDED",
        ),
        (
            "a destination, and a buildspec naming no artifact file",
            json!({"artifacts": {"type": "S3", "location": "artifact-destination"}}),
            buildspec_for("build", &["echo building"]),
            "FAILED",
            Some("No artifact files specified"),
            "FAILED",
        ),
        (
            "a destination, and artifact paths that match nothing",
            json!({"artifacts": {"type": "S3", "location": "artifact-destination"}}),
            format!(
                "{}artifacts:\n  files:\n    - '**/*'\n",
                buildspec_for("build", &["echo building"])
            ),
            "FAILED",
            Some("No matching artifact paths found"),
            "FAILED",
        ),
    ];

    for (index, (case, extra, buildspec, upload, message, expected)) in rows.iter().enumerate() {
        let project = format!("finish-proj-{index}");
        create_project(
            &router,
            &project,
            super::project_build_lifecycle::merged(source_with(buildspec), extra.clone()),
        )
        .await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildStatus"], json!(expected),
            "case {case:?}: expected buildStatus {expected}, got {build}"
        );
        let phase = build["phases"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|phase| phase["phaseType"] == json!("UPLOAD_ARTIFACTS"))
            .unwrap_or_else(|| panic!("case {case:?}: an UPLOAD_ARTIFACTS phase, got {build}"));
        assert_eq!(
            phase["phaseStatus"], json!(upload),
            "case {case:?}: expected the upload phase to be {upload}, got {phase}"
        );
        match message {
            Some(message) => {
                assert_eq!(
                    phase["contexts"][0]["statusCode"], json!("CLIENT_ERROR"),
                    "case {case:?}: expected CodeBuild's client-error code, got {phase}"
                );
                assert_eq!(
                    phase["contexts"][0]["message"], json!(message),
                    "case {case:?}: unexpected upload failure, got {phase}"
                );
            }
            None => assert!(
                phase["contexts"].is_null(),
                "case {case:?}: a successful upload carries no context, got {phase}"
            ),
        }
        let pairs = phase_pairs(&build);
        assert!(
            pairs
                .iter()
                .any(|(phase, status)| phase == "FINALIZING" && status.as_deref() == Some("SUCCEEDED")),
            "case {case:?}: a build that got through its commands finalises, got {pairs:?}"
        );
    }
}

/// RULE: a buildspec is caller-supplied text, so what running it can COST is bounded —
/// in commands, in the size and nesting of one command line, and in the bytes its
/// variables hold. A build whose commands exhaust a bound still settles with a status
/// rather than hanging or taking the process with it.
#[tokio::test]
async fn a_buildspec_past_a_bound_still_settles() {
    let router = emulator();
    // (case, the buildspec whose commands reach past a bound)
    let rows: Vec<(&str, String)> = vec![
        (
            // The grammar nests through these words rather than through any character,
            // so a line of them is what a parser's recursion has to be bounded against.
            "a command line nesting far past the nesting bound",
            format!(
                "version: 0.2\nphases:\n  build:\n    commands:\n      - '{}true'\n",
                "if true; then ".repeat(600)
            ),
        ),
        (
            // Each line doubles what the next one copies, which is how a handful of
            // commands inside every other bound could still grow without one.
            "a buildspec whose commands double a variable's value each time",
            format!(
                "version: 0.2\nphases:\n  build:\n    commands:\n      - A={}\n{}",
                "x".repeat(1024),
                (0..40)
                    .map(|_| "      - A=\"$A$A\"\n".to_string())
                    .collect::<String>()
            ),
        ),
        (
            "a buildspec declaring more commands than the command budget",
            format!(
                "version: 0.2\nphases:\n  build:\n    commands:\n{}",
                (0..1_200)
                    .map(|_| "      - echo one\n".to_string())
                    .collect::<String>()
            ),
        ),
        (
            // Every word of the line is live at the same instant, so their SUM is what a
            // line's expansion has to be bounded by — one word's size is not enough.
            "one command line expanding a grown value many times over",
            format!(
                "version: 0.2\nphases:\n  build:\n    commands:\n      - A={}\n{}      - echo{}\n",
                "x".repeat(1024),
                (0..6)
                    .map(|_| "      - A=\"$A$A\"\n".to_string())
                    .collect::<String>(),
                " $A".repeat(2_000)
            ),
        ),
    ];

    for (index, (case, buildspec)) in rows.iter().enumerate() {
        let project = format!("bounded-proj-{index}");
        create_project(&router, &project, source_with(buildspec)).await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildComplete"], json!(true),
            "case {case:?}: the build must settle, got {build}"
        );
        assert_eq!(
            build["buildStatus"], json!("FAILED"),
            "case {case:?}: a command this environment did not run fails the build, got {build}"
        );
        let pairs = phase_pairs(&build);
        assert!(
            pairs
                .iter()
                .any(|(phase, status)| phase == "BUILD" && status.as_deref() == Some("FAILED")),
            "case {case:?}: the phase that reached the bound carries the failure, got {pairs:?}"
        );
    }
}

/// RULE: the failing phase's context names the command that failed and the status it
/// exited with, which is how CodeBuild reports a command-execution failure.
#[tokio::test]
async fn a_failing_phase_reports_the_command_and_its_exit_status() {
    let router = emulator();
    // (case, the command, the exit status it answers)
    let rows = [
        ("the failure utility", "false", 1),
        ("an explicit status", "exit 7", 7),
        ("a program this environment has none for", "mvn package", 127),
    ];
    for (index, (case, command, status)) in rows.iter().enumerate() {
        let project = format!("context-proj-{index}");
        create_project(&router, &project, source_with(&buildspec_for("build", &[command]))).await;
        let build = settled_build(&router, &project, json!({})).await;
        let failing = build["phases"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|phase| phase["phaseStatus"] == json!("FAILED"))
            .unwrap_or_else(|| panic!("case {case:?}: a failing phase, got {build}"));
        assert_eq!(
            failing["contexts"][0]["statusCode"], json!("COMMAND_EXECUTION_ERROR"),
            "case {case:?}: expected the command-execution status code, got {failing}"
        );
        assert_eq!(
            failing["contexts"][0]["message"],
            json!(format!(
                "Error while executing command: {command}. Reason: exit status {status}"
            )),
            "case {case:?}: unexpected failure context, got {failing}"
        );
    }
}

/// RULE: a build whose buildspec cannot be read runs no phase of it; the source-download
/// phase fails, with CodeBuild's `YAML_FILE_ERROR` for a buildspec there was none of to
/// read and with the YAML parser's own diagnostic for one the parser refused.
#[tokio::test]
async fn a_buildspec_that_cannot_be_read_fails_the_download_phase() {
    let router = emulator();
    // (case, the project's source, the status code the context carries, its message)
    let rows: [(&str, Value, Option<&str>, &str); 6] = [
        (
            "a source that keeps its buildspec in a tree nothing fetched",
            json!({"source": {"type": "CODEPIPELINE"}}),
            Some("YAML_FILE_ERROR"),
            "YAML file does not exist",
        ),
        (
            // The tab interrupts the plain scalar the first line opens, which is the
            // construct the parser marks — and the diagnostic numbers the document's
            // first line 0.
            "a buildspec that is not YAML at all",
            source_with("version: 0.2\n\tphases: [oops"),
            None,
            "found character that cannot start any token at line 0",
        ),
        (
            // A `]` inside a comment closes no collection, so a scan that decremented on
            // one would read this document as flatter than it is; its real nesting is the
            // parser's answer. Every collection is closed, so the document is well-formed
            // YAML and only its DEPTH puts it outside the bounds.
            "a buildspec whose collections nest far past the nesting bound",
            source_with(&format!(
                "version: 0.2\nphases: {}{} # ]]]\n",
                "[".repeat(200),
                "]".repeat(200)
            )),
            Some("YAML_FILE_ERROR"),
            "Unable to parse YAML file",
        ),
        (
            // The flow nesting a document can reach is bounded BEFORE a parse runs, by
            // the indicators it carries: a flow collection needs one `[` per level and no
            // indentation, so a line of them would otherwise descend once per byte.
            "a buildspec opening more flow collections than the parse is given",
            source_with(&format!("version: 0.2\nphases: {}", "[".repeat(3_000))),
            Some("YAML_FILE_ERROR"),
            "Unable to parse YAML file",
        ),
        (
            // The indentation bound is measured per LINE, and a line of the document is
            // whatever the YAML scanner breaks one at. This row and the next carry the
            // same two-level mapping indented past the bound, written with the two line
            // breaks YAML has — a line feed, and a carriage return alone — so the bound
            // holds for a document however it ends its lines.
            "a buildspec indented past the bound, written with line feeds",
            source_with(&format!("version: 0.2\ndeep:\n{}key: value\n", " ".repeat(70))),
            Some("YAML_FILE_ERROR"),
            "Unable to parse YAML file",
        ),
        (
            "the same document written with carriage returns alone",
            source_with(&format!("version: 0.2\rdeep:\r{}key: value\r", " ".repeat(70))),
            Some("YAML_FILE_ERROR"),
            "Unable to parse YAML file",
        ),
    ];
    for (index, (case, source, status_code, message)) in rows.iter().enumerate() {
        let project = format!("yaml-proj-{index}");
        create_project(&router, &project, source.clone()).await;
        let build = settled_build(&router, &project, json!({})).await;
        assert_eq!(
            build["buildStatus"], json!("FAILED"),
            "case {case:?}: expected FAILED, got {build}"
        );
        let pairs = phase_pairs(&build);
        assert_eq!(
            pairs.last().map(|(phase, _)| phase.as_str()),
            Some("COMPLETED"),
            "case {case:?}: a settled build ends in COMPLETED, got {pairs:?}"
        );
        let download = build["phases"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|phase| phase["phaseType"] == json!("DOWNLOAD_SOURCE"))
            .unwrap_or_else(|| panic!("case {case:?}: a DOWNLOAD_SOURCE phase, got {build}"));
        assert_eq!(
            download["phaseStatus"], json!("FAILED"),
            "case {case:?}: the download phase carries the failure, got {download}"
        );
        assert_eq!(
            download["contexts"][0]["statusCode"],
            status_code.map_or(Value::Null, |code| json!(code)),
            "case {case:?}: unexpected YAML status code, got {download}"
        );
        assert_eq!(
            download["contexts"][0]["message"], json!(message),
            "case {case:?}: unexpected YAML failure message, got {download}"
        );
        assert!(
            pairs.iter().all(|(phase, _)| phase != "BUILD"),
            "case {case:?}: no phase of an unread buildspec ran, got {pairs:?}"
        );
    }
}

/// RULE: a buildspec's `env/exported-variables` are reported with the values the build's
/// own session ended holding, so what the commands computed is what comes back — and a
/// word's `$NAME` expansion resolves against that same session, whose variables are the
/// ones CodeBuild sets for the build, the buildspec's `env/variables` and the build's
/// `environment.environmentVariables`, in that order of precedence.
#[tokio::test]
async fn exported_variables_report_what_the_builds_session_held() {
    let router = emulator();
    let buildspec = "version: 0.2\nenv:\n  variables:\n    GREETING: hello\n    \
                     PLACE: spec\n  exported-variables:\n    - GREETING\n    - PLACE\n    \
                     - MESSAGE\n    - WHICH_BUILD\nphases:\n  build:\n    commands:\n      \
                     - MESSAGE=\"$GREETING $PLACE\"\n      - export WHICH_BUILD=$CODEBUILD_BUILD_NUMBER\n";
    create_project(
        &router,
        "exported-proj",
        super::project_build_lifecycle::merged(
            source_with(buildspec),
            json!({"environment": {
                "type": "LINUX_CONTAINER",
                "image": "aws/codebuild/standard:7.0",
                "computeType": "BUILD_GENERAL1_SMALL",
                "environmentVariables": [{"name": "PLACE", "value": "project"}],
            }}),
        ),
    )
    .await;
    let build = settled_build(&router, "exported-proj", json!({})).await;
    assert_eq!(
        build["buildStatus"], json!("SUCCEEDED"),
        "the exporting build must succeed, got {build}"
    );
    let exported: Vec<(String, String)> = build["exportedEnvironmentVariables"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|variable| {
            (
                variable["name"].as_str().unwrap_or_default().to_string(),
                variable["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let expected = vec![
        // The buildspec's own variable, which nothing replaced.
        ("GREETING".to_string(), "hello".to_string()),
        // The project's environment variable wins over the buildspec's.
        ("PLACE".to_string(), "project".to_string()),
        // Computed by the command, from the two above.
        ("MESSAGE".to_string(), "hello project".to_string()),
        // The build's own number, which CodeBuild sets for the build.
        ("WHICH_BUILD".to_string(), "1".to_string()),
    ];
    assert_eq!(
        exported, expected,
        "expected the session's values for every exported name, got {build}"
    );
}
