// SPDX-License-Identifier: CC0-1.0

//! `DeleteConfigurationTemplate`, driven end to end over this service's awsQuery
//! surface.
//!
//! RULE under test: the operation removes the saved configuration the
//! `(ApplicationName, TemplateName)` pair names and nothing else — the
//! configuration set it held stops being described, its name is free to be used
//! again, and no other application's template of the same name and no
//! environment launched from it is touched ("the environment gets a copy of the
//! template. You can delete or modify the environment's copy of the template
//! without affecting the running environment"). The model declares no not-found
//! error for it, so a pair naming no stored template is a no-op success and the
//! call is idempotent; the one error it does model —
//! `OperationInProgressException` — is the answer while the application that
//! holds the template is itself being deleted, which is already taking its
//! configurations with it.
//!
//! The rows span what the rule generalises over: a template that exists, one the
//! application does not hold, one held by ANOTHER application, a template name
//! under an application that does not exist, and the same delete repeated.
//!
//! The HTTP harness (`Emulator`, `Answer`) is the one `all/lifecycle.rs` owns:
//! nothing here reaches into the store, so what is asserted is what a client
//! sees.

use crate::harness::{Answer, Emulator, SOLUTION_STACK};

/// The two applications the templates below belong to.
const APPLICATION: &str = "template-delete-app";
/// See [`APPLICATION`].
const OTHER_APPLICATION: &str = "template-keep-app";

/// Require a 200 from a call.
async fn ok(emulator: &Emulator, case: &str, action: &str, params: &[(&str, &str)]) -> Answer {
    let answer = emulator.call(action, params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// Save the configuration `template` under `application`, on the published
/// fixture stack.
async fn given_template(emulator: &Emulator, application: &str, template: &str) {
    ok(
        emulator,
        template,
        "CreateConfigurationTemplate",
        &[
            ("ApplicationName", application),
            ("TemplateName", template),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
        ],
    )
    .await;
}

/// The configuration-template names `DescribeApplications` reports for
/// `application`, sorted, with the answer they were read from.
async fn held_templates(emulator: &Emulator, case: &str, application: &str) -> (Vec<String>, Answer) {
    let described = ok(
        emulator,
        case,
        "DescribeApplications",
        &[("ApplicationNames.member.1", application)],
    )
    .await;
    let mut names: Vec<String> = described
        .values_at("Applications/member/ConfigurationTemplates/member")
        .into_iter()
        .map(str::to_string)
        .collect();
    names.sort();
    (names, described)
}

/// Delete the template `template` of `application`, requiring the success the
/// operation answers.
async fn delete(emulator: &Emulator, case: &str, application: &str, template: &str) {
    ok(
        emulator,
        case,
        "DeleteConfigurationTemplate",
        &[
            ("ApplicationName", application),
            ("TemplateName", template),
        ],
    )
    .await;
}

#[tokio::test]
async fn a_deleted_configuration_template_is_gone_and_only_that_one_is() {
    let emulator = Emulator::new();
    for application in [APPLICATION, OTHER_APPLICATION] {
        ok(
            &emulator,
            application,
            "CreateApplication",
            &[("ApplicationName", application)],
        )
        .await;
    }
    // The same template NAME under both applications, so the delete is asserted
    // to act on the pair rather than on the name.
    for application in [APPLICATION, OTHER_APPLICATION] {
        given_template(&emulator, application, "shared-name").await;
    }
    given_template(&emulator, APPLICATION, "kept").await;
    // An environment launched from the template that is about to be deleted.
    ok(
        &emulator,
        "setup",
        "CreateEnvironment",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", "from-template-env"),
            ("TemplateName", "shared-name"),
        ],
    )
    .await;

    let case = "a template the application holds";
    delete(&emulator, case, APPLICATION, "shared-name").await;

    // Gone: the application no longer holds it and its configuration set is no
    // longer described.
    let (held, answer) = held_templates(&emulator, case, APPLICATION).await;
    assert_eq!(
        held,
        vec!["kept".to_string()],
        "case {case:?}: only the deleted template may be gone, got body {}",
        answer.body
    );
    let described = emulator
        .call(
            "DescribeConfigurationSettings",
            &[
                ("ApplicationName", APPLICATION),
                ("TemplateName", "shared-name"),
            ],
        )
        .await;
    assert_eq!(
        described.status, 400,
        "case {case:?}: a deleted template's settings cannot be described, got {} and body {}",
        described.status, described.body
    );
    assert_eq!(
        described.error_code(),
        "InvalidParameterValue",
        "case {case:?}: unexpected error code, got body {}",
        described.body
    );

    // The other application's template of the same name is untouched.
    let case = "another application's template of the same name";
    let (held, answer) = held_templates(&emulator, case, OTHER_APPLICATION).await;
    assert_eq!(
        held,
        vec!["shared-name".to_string()],
        "case {case:?}: another application's template must survive, got body {}",
        answer.body
    );

    // So is the environment that was launched from the deleted template: it runs
    // the copy it was given.
    let case = "the environment launched from the deleted template";
    let environments = ok(
        &emulator,
        case,
        "DescribeEnvironments",
        &[("EnvironmentNames.member.1", "from-template-env")],
    )
    .await;
    assert_eq!(
        environments.value("Environments/member/TemplateName"),
        Some("shared-name"),
        "case {case:?}: the environment keeps the template it was launched from, got body {}",
        environments.body
    );
    assert_eq!(
        environments.value("Environments/member/SolutionStackName"),
        Some(SOLUTION_STACK.as_str()),
        "case {case:?}: the environment keeps running its platform version, got body {}",
        environments.body
    );

    // The name is free again: a re-create under it is accepted.
    let case = "the name after the delete";
    given_template(&emulator, APPLICATION, "shared-name").await;
    let (held, answer) = held_templates(&emulator, case, APPLICATION).await;
    assert_eq!(
        held,
        vec!["kept".to_string(), "shared-name".to_string()],
        "case {case:?}: the deleted name must be usable again, got body {}",
        answer.body
    );
}

#[tokio::test]
async fn deleting_a_configuration_template_that_is_not_there_is_a_no_op_success() {
    let emulator = Emulator::new();
    ok(
        &emulator,
        "setup",
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    given_template(&emulator, APPLICATION, "kept").await;

    // Rows differ along WHAT is not there: the template, the application, and a
    // template whose delete has already run.
    for (case, application, template) in [
        ("a template the application does not hold", APPLICATION, "never-created"),
        ("an application that does not exist", "no-such-app", "kept"),
        ("a template already deleted", APPLICATION, "kept"),
        ("that same delete repeated", APPLICATION, "kept"),
    ] {
        delete(&emulator, case, application, template).await;
    }

    // The deletes that named nothing left the one template that was there
    // alone until its own delete ran, and that one took only itself.
    let case = "after the no-op deletes";
    let (held, answer) = held_templates(&emulator, case, APPLICATION).await;
    assert!(
        held.is_empty(),
        "case {case:?}: only the named template may have been deleted, got {held:?} and body {}",
        answer.body
    );
    let applications = ok(
        &emulator,
        case,
        "DescribeApplications",
        &[("ApplicationNames.member.1", APPLICATION)],
    )
    .await;
    assert_eq!(
        applications.value("Applications/member/ApplicationName"),
        Some(APPLICATION),
        "case {case:?}: the application itself must survive, got body {}",
        applications.body
    );
}

#[tokio::test]
async fn a_template_of_an_application_being_deleted_reports_the_operation_in_progress() {
    let emulator = Emulator::new();
    ok(
        &emulator,
        "setup",
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    given_template(&emulator, APPLICATION, "doomed").await;
    // The application's own deletion is in flight until the clock carries it
    // out, and it is already taking its configurations with it.
    ok(
        &emulator,
        "setup",
        "DeleteApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;

    let case = "a template of an application being deleted";
    let refused = emulator
        .call(
            "DeleteConfigurationTemplate",
            &[
                ("ApplicationName", APPLICATION),
                ("TemplateName", "doomed"),
            ],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400, got {} and body {}",
        refused.status, refused.body
    );
    // The `awsQueryError` code the model gives that error shape, which is what a
    // client reads off the envelope.
    assert_eq!(
        refused.error_code(),
        "OperationInProgressFailure",
        "case {case:?}: unexpected error code, got body {}",
        refused.body
    );
    assert!(
        !refused.error_message().is_empty(),
        "case {case:?}: a refusal must say what to fix, got body {}",
        refused.body
    );
}
