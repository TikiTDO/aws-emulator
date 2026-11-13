// SPDX-License-Identifier: CC0-1.0

//! What a build's commands WRITE, driven over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! RULE under test: a build's commands run in a build environment that has a working tree
//! of its own, so a command whose output is redirected to a path keeps what it wrote under
//! that path for the rest of the build — `>` writing the file from the start and `>>`
//! adding to what it holds — and a later command of the same build, in that phase or a
//! later one, reads it back and succeeds; a path the build never wrote is one that command
//! could not read, so it answers the status a shell answers for that and the phase carries
//! it. The tree belongs to the one build: another build of another project does not see it.
//!
//! The rows differ along what the rule generalises over: which redirection operator the
//! command used, how many commands wrote the file, which phase wrote and which read, how
//! the path is spelled, whether the path was ever written, and whether the words come from
//! the session's variables.
//!
//! The command FORMS this environment does not perform — an input redirection, a
//! redirection of another descriptor — are `shell_probe.rs`'s rows, and what a build's
//! terminal status is made of is `buildspec_execution.rs`'s.

use serde_json::{json, Value};

use super::project_build_lifecycle::{
    call, create_project, emulator, start_build, PAST_THE_SETTLE_SPAN,
};

/// A `CreateProject` source carrying `buildspec` inline, with no input source code of its
/// own — the form that lets a suite fix exactly what a build runs.
fn source_with(buildspec: &str) -> Value {
    json!({"source": {"type": "NO_SOURCE", "buildspec": buildspec}})
}

/// A buildspec whose `build` phase declares `commands`.
fn buildspec_for(commands: &[&str]) -> String {
    let listed = commands
        .iter()
        .map(|command| format!("      - '{command}'\n"))
        .collect::<String>();
    format!("version: 0.2\nphases:\n  build:\n    commands:\n{listed}")
}

/// Start a build of `project`, let the transition the clock owes it fall due, and hand back
/// the settled record.
async fn settled_build(
    router: &smithy_server_runtime::MultiServiceRouter,
    project: &str,
) -> Value {
    let build = start_build(router, json!({"projectName": project})).await;
    let id = build["id"].as_str().expect("a build id").to_string();
    tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
    let (status, read) = call(router, "BatchGetBuilds", json!({"ids": [id.clone()]})).await;
    assert_eq!(
        status, 200,
        "BatchGetBuilds {id}: expected 200, got {status} with {read}"
    );
    read["builds"][0].clone()
}

/// The context message the phase that failed carries, or an empty string for a build no
/// phase of which failed.
fn failure_message(build: &Value) -> String {
    build["phases"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|phase| phase["phaseStatus"] == json!("FAILED"))
        .and_then(|phase| phase["contexts"][0]["message"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// RULE: a redirected command writes the file it names into the build's own tree, and a
/// later command of the same build reads it.
#[tokio::test]
async fn a_builds_commands_write_and_read_the_builds_own_files() {
    let router = emulator();
    /// One row: the case, the buildspec, the build's status, and the message the phase that
    /// failed carries (empty for a build that succeeded).
    type Row = (&'static str, String, &'static str, &'static str);

    let rows: Vec<Row> = vec![
        (
            "a command writes a file and a later command reads it",
            buildspec_for(&["echo built > output.txt", "cat output.txt"]),
            "SUCCEEDED",
            "",
        ),
        (
            "a second write from the start replaces the file, which still reads",
            buildspec_for(&[
                "echo first > output.txt",
                "echo second > output.txt",
                "cat output.txt",
            ]),
            "SUCCEEDED",
            "",
        ),
        (
            "an appending write adds to a file the build already wrote",
            buildspec_for(&[
                "echo first > output.txt",
                "echo second >> output.txt",
                "cat output.txt",
            ]),
            "SUCCEEDED",
            "",
        ),
        (
            "an appending write to a path nothing wrote yet makes the file",
            buildspec_for(&["echo only >> output.txt", "cat output.txt"]),
            "SUCCEEDED",
            "",
        ),
        (
            "the same file written as ./output.txt and read as output.txt",
            buildspec_for(&["echo built > ./output.txt", "cat output.txt"]),
            "SUCCEEDED",
            "",
        ),
        (
            "a file under a directory path",
            buildspec_for(&["echo built > target/output.txt", "cat target/output.txt"]),
            "SUCCEEDED",
            "",
        ),
        (
            "the words of the write and the path come from the session's variables",
            format!(
                "version: 0.2\nenv:\n  variables:\n    ARTIFACT: {artifact}\n    \
                 WHAT: written\nphases:\n  build:\n    commands:\n      \
                 - 'echo $WHAT > $ARTIFACT'\n      - 'cat {artifact}'\n",
                artifact = "output.txt"
            ),
            "SUCCEEDED",
            "",
        ),
        (
            "a file one phase wrote is read by a later phase of the same build",
            "version: 0.2\nphases:\n  pre_build:\n    commands:\n      \
             - 'echo built > output.txt'\n  build:\n    commands:\n      \
             - 'cat output.txt'\n"
                .to_string(),
            "SUCCEEDED",
            "",
        ),
        (
            "a read of a path the build never wrote fails the phase that read it",
            buildspec_for(&["cat output.txt"]),
            "FAILED",
            "Error while executing command: cat output.txt. Reason: exit status 1",
        ),
        (
            "a read of one path the build wrote and one it did not still fails",
            buildspec_for(&["echo built > output.txt", "cat output.txt missing.txt"]),
            "FAILED",
            "Error while executing command: cat output.txt missing.txt. Reason: exit status 1",
        ),
        (
            "a file the build wrote and then a command that fails on its own",
            buildspec_for(&["echo built > output.txt", "false"]),
            "FAILED",
            "Error while executing command: false. Reason: exit status 1",
        ),
    ];

    for (index, (case, buildspec, expected, message)) in rows.iter().enumerate() {
        let project = format!("tree-proj-{index}");
        create_project(&router, &project, source_with(buildspec)).await;
        let build = settled_build(&router, &project).await;
        assert_eq!(
            build["buildStatus"],
            json!(expected),
            "case {case:?}: expected buildStatus {expected}, got {build}"
        );
        let reported = failure_message(&build);
        assert_eq!(
            reported, *message,
            "case {case:?}: expected the failure context {message:?}, got {reported:?}"
        );
    }
}

/// RULE: the tree belongs to the ONE build that wrote it — a build of another project does
/// not read what that build wrote, and neither does a second build of the same project.
#[tokio::test]
async fn one_builds_tree_is_its_own() {
    let router = emulator();

    create_project(
        &router,
        "tree-writer",
        source_with(&buildspec_for(&["echo built > shared.txt", "cat shared.txt"])),
    )
    .await;
    let wrote = settled_build(&router, "tree-writer").await;
    assert_eq!(
        wrote["buildStatus"],
        json!("SUCCEEDED"),
        "case \"the build that writes\": expected SUCCEEDED, got {wrote}"
    );

    // Another project's build reads the same path: its own tree holds no such file.
    create_project(
        &router,
        "tree-reader",
        source_with(&buildspec_for(&["cat shared.txt"])),
    )
    .await;
    let read = settled_build(&router, "tree-reader").await;
    assert_eq!(
        read["buildStatus"],
        json!("FAILED"),
        "case \"another project's build\": a file another build wrote is not there, got {read}"
    );

    // And a SECOND build of the project that only reads is the same answer: nothing a build
    // wrote outlives it.
    let again = settled_build(&router, "tree-reader").await;
    assert_eq!(
        again["buildStatus"],
        json!("FAILED"),
        "case \"a second build of the reader\": each build starts with its own tree, got \
         {again}"
    );
}
