// SPDX-License-Identifier: CC0-1.0

//! `DescribeLaunchTemplates` for a launch template that is GONE, and the members every
//! template it DOES report carries.
//!
//! THE RULE:
//!
//! * a launch template KEY this service ITSELF resolved before — an id it minted, or a name a
//!   template of the account carried — that names no template any more is reported ABSENT,
//!   with the key's own not-found code and EC2's one fixed sentence for an absent launch
//!   template, `At least one of the launch templates specified in the request does not
//!   exist.`, which names none of the request's values. An SDK's delete-then-confirm and
//!   waiter helpers read the not-found code as "the resource is gone" and a malformed-id
//!   code as a client bug they never retry, so which of the two a deleted template answers
//!   decides whether such a caller settles or loops.
//! * a template the read DOES report carries each modelled member the store holds for it:
//!   the `operator` that answers whether a service provider manages it, and the version
//!   counters its versions have made it carry.
//!
//! What this operation answers for a value it never minted is the other half of the first
//! rule, and it is pinned where it already lives — `an_id_or_a_name_that_names_no_template_is_refused`
//! in `tests/all/describe_launch_templates.rs`. This suite asks only about the keys this
//! service did resolve once, which no other suite drives through a delete.
//!
//! Bodies are read through the XML DOCUMENT rather than by substring, so the `operator`'s
//! own children are read inside it and never confused with a member of the template.
//!
//! The rows differ along what the rule generalises over: the name's character classes and
//! length, how many versions the template holds when it goes, which key the delete used, and
//! which key the probe uses and how many keys it names.

use service_com_amazonaws_ec2_amazonec2 as ec2;

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{self, element, error_code, error_message, indexed};
use super::wire::reported_operator;

/// The Region every call here is signed for. A launch template carries no Region member, so
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// EC2's one sentence for a launch template a request named that the account does not hold.
/// It is FIXED: it names neither the id nor the name the request sent, a request being able
/// to name several.
const ABSENT_SENTENCE: &str =
    "At least one of the launch templates specified in the request does not exist.";

/// A `(key, value)` request parameter.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One `LaunchTemplateData.<member>` parameter.
fn data(member: &str, value: &str) -> (String, String) {
    p(&format!("LaunchTemplateData.{member}"), value)
}

/// One call signed for [`REGION`].
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    ec2_query::call(router, REGION, action, params).await
}

/// One call that must succeed, as its body.
async fn ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Every launch template id the store holds, in the order the store holds them.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .launch_templates
        .keys()
        .cloned()
        .collect()
}

/// One probe of a gone template: the case it stands for, the form fields it sends, and the
/// error code it must be refused with.
type Probe = (String, Vec<(String, String)>, &'static str);

/// Which key a delete names its template by.
#[derive(Debug, Clone, Copy)]
enum DeleteBy {
    Id,
    Name,
}

/// One row of the template table: the case, the template's name, how many versions are
/// created after the first, which of those versions the template is then made to default to
/// (`None` leaving the default where the create put it), the `Operator.Principal` the create
/// names (`None` for a template the caller owns), and which key the delete that takes it
/// would name it by (read only for the rows this suite does delete).
struct TemplateRow {
    case: &'static str,
    name: &'static str,
    extra_versions: usize,
    set_default_version: Option<i64>,
    operator_principal: Option<&'static str>,
    delete_by: DeleteBy,
}

/// A template the caller owns: no row has to spell out the members only the managed row
/// varies.
impl TemplateRow {
    fn new(case: &'static str, name: &'static str) -> Self {
        Self {
            case,
            name,
            extra_versions: 0,
            set_default_version: None,
            operator_principal: None,
            delete_by: DeleteBy::Id,
        }
    }
}

/// The templates this suite takes away. The rows differ in the name's character classes and
/// length (the modelled floor of 3 characters up to a long one, with each of `().-/_`), in how
/// many versions the template holds when it goes (one, two, three) and whether its default was
/// moved off the first version, in whether a service provider manages it, and in which key the
/// delete names it by.
fn gone_rows() -> Vec<TemplateRow> {
    vec![
        TemplateRow::new(
            "the shortest name the model admits, one version, deleted by id",
            "web",
        ),
        TemplateRow {
            extra_versions: 1,
            ..TemplateRow::new(
                "a dotted and hyphenated name, two versions, deleted by id",
                "web.tier-1",
            )
        },
        TemplateRow {
            extra_versions: 2,
            set_default_version: Some(3),
            delete_by: DeleteBy::Name,
            ..TemplateRow::new(
                "a name with parentheses and an underscore, three versions with the last made \
                 default, deleted by name",
                "db_tier(3)",
            )
        },
        TemplateRow {
            delete_by: DeleteBy::Name,
            ..TemplateRow::new(
                "a long name with a slash, one version, deleted by name",
                "Analytics/Reports-0123456789012345678901234567890123456789",
            )
        },
        TemplateRow {
            extra_versions: 1,
            set_default_version: Some(2),
            // "The service provider that manages the resource" (`OperatorRequest$Principal`):
            // a template a provider created on the caller's behalf, which is what the
            // operator reports `managed` true for.
            operator_principal: Some("autoscaling.amazonaws.com"),
            ..TemplateRow::new(
                "a template a service provider manages, two versions with the last made \
                 default, deleted by id",
                "managed.by.a.provider",
            )
        },
    ]
}

/// The `(managed, principal)` the operator of the template `row` describes must report:
/// `false` and no principal for a template the caller owns, `true` and the principal for one
/// a service provider manages (`OperatorResponse$Managed`, `$Principal`).
fn expected_operator(row: &TemplateRow) -> (Option<String>, Option<String>) {
    match row.operator_principal {
        None => (Some("false".to_string()), None),
        Some(principal) => (Some("true".to_string()), Some(principal.to_string())),
    }
}

/// Create the template `row` describes, with the versions it names, and answer the id the
/// create minted.
async fn create_template(router: &MultiServiceRouter, row: &TemplateRow) -> String {
    let mut create = vec![
        p("LaunchTemplateName", row.name),
        data("ImageId", "ami-00000000000000001"),
    ];
    if let Some(principal) = row.operator_principal {
        create.push(p("Operator.Principal", principal));
    }
    let body = ok(router, row.case, "CreateLaunchTemplate", &create).await;
    let launch_template_id = element(&body, "launchTemplateId")
        .unwrap_or_else(|| panic!("case {:?}: the create must report its id: {body}", row.case));

    for round in 0..row.extra_versions {
        let body = ok(
            router,
            row.case,
            "CreateLaunchTemplateVersion",
            &[
                p("LaunchTemplateId", &launch_template_id),
                data("InstanceType", "t3.micro"),
            ],
        )
        .await;
        assert_eq!(
            element(&body, "versionNumber"),
            Some((round + 2).to_string()),
            "case {:?}: versions are numbered in the order they are created, got {body}",
            row.case
        );
    }
    if let Some(version) = row.set_default_version {
        let body = ok(
            router,
            row.case,
            "ModifyLaunchTemplate",
            &[
                p("LaunchTemplateId", &launch_template_id),
                p("SetDefaultVersion", &version.to_string()),
            ],
        )
        .await;
        assert_eq!(
            element(&body, "defaultVersionNumber"),
            Some(version.to_string()),
            "case {:?}: the modify reports the default version it was asked for, got {body}",
            row.case
        );
    }
    launch_template_id
}

/// A launch template key this service itself resolved once — an id it minted, or a name a
/// template of the account carried — that names no template any more is reported ABSENT, with
/// the key's own not-found code and EC2's fixed sentence. Before it goes, the template is
/// reported with its `operator` and the version counters its versions gave it.
#[tokio::test]
async fn a_key_whose_template_is_gone_is_reported_absent_rather_than_unreadable() {
    let (router, engine) = ec2_query::emulator();

    // One template that is never deleted, so a probe can name a gone key beside a key that
    // still resolves, and so the store is demonstrably left holding it.
    let kept = create_template(
        &router,
        &TemplateRow::new("the template that is kept", "kept.template"),
    )
    .await;

    let mut gone_ids: Vec<String> = Vec::new();
    for row in gone_rows() {
        let case = row.case;
        let launch_template_id = create_template(&router, &row).await;

        // While it is held, the read reports the template with the members the store holds:
        // the operator that answers whether a provider manages it, and the counters its
        // versions made it carry.
        let described = ok(
            &router,
            case,
            "DescribeLaunchTemplates",
            &indexed("LaunchTemplateId", &[launch_template_id.as_str()]),
        )
        .await;
        let expected_latest = (row.extra_versions + 1).to_string();
        let expected_default = row
            .set_default_version
            .map_or_else(|| "1".to_string(), |version| version.to_string());
        assert_eq!(
            (
                element(&described, "defaultVersionNumber"),
                element(&described, "latestVersionNumber"),
            ),
            (
                Some(expected_default.clone()),
                Some(expected_latest.clone())
            ),
            "case {case:?}: expected the counters ({expected_default}, {expected_latest}), \
             got {described}"
        );
        let operator = reported_operator(case, &described);
        let expected = expected_operator(&row);
        assert_eq!(
            operator, expected,
            "case {case:?}: expected the operator to answer (managed, principal) \
             {expected:?}, got {operator:?} from {described}"
        );

        // Take it away by the key this row names.
        let delete_params = match row.delete_by {
            DeleteBy::Id => vec![p("LaunchTemplateId", &launch_template_id)],
            DeleteBy::Name => vec![p("LaunchTemplateName", row.name)],
        };
        let deleted = ok(&router, case, "DeleteLaunchTemplate", &delete_params).await;
        assert_eq!(
            element(&deleted, "launchTemplateId").as_deref(),
            Some(launch_template_id.as_str()),
            "case {case:?}: the delete must report the template it removed, got {deleted}"
        );

        // Both keys are now absent, each refused with its own code and the ONE fixed
        // sentence. The id was minted by this service, so it is an id it could have resolved
        // — never a value it cannot read; the name answers under the NAME's own code, the
        // code `ec2::state::invalid_launch_template_name_not_found` records the recorded
        // answer for, with the same sentence the id path carries.
        let probes: Vec<Probe> = vec![
            (
                format!("{case}: the gone id"),
                indexed("LaunchTemplateId", &[launch_template_id.as_str()]),
                "InvalidLaunchTemplateId.NotFound",
            ),
            (
                format!("{case}: the gone id beside a template still held"),
                indexed(
                    "LaunchTemplateId",
                    &[kept.as_str(), launch_template_id.as_str()],
                ),
                "InvalidLaunchTemplateId.NotFound",
            ),
            (
                format!("{case}: the gone name"),
                indexed("LaunchTemplateName", &[row.name]),
                "InvalidLaunchTemplateName.NotFoundException",
            ),
            (
                format!("{case}: the gone name beside a template still held"),
                indexed("LaunchTemplateName", &["kept.template", row.name]),
                "InvalidLaunchTemplateName.NotFoundException",
            ),
        ];
        for (probe, params, expected_code) in probes {
            let (status, body) = api(&router, "DescribeLaunchTemplates", &params).await;
            assert_eq!(
                status, 400,
                "case {probe:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {probe:?}: expected {expected_code}, got {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, ABSENT_SENTENCE,
                "case {probe:?}: expected EC2's fixed sentence for an absent launch \
                 template, got {message:?} in {body}"
            );
            assert!(
                !message.contains(&launch_template_id) && !message.contains(row.name),
                "case {probe:?}: the sentence names none of the request's values, got \
                 {message:?}"
            );
        }
        gone_ids.push(launch_template_id);
    }

    // Several gone ids at once: still the absent reading, and still the sentence that names
    // none of them.
    let every_gone: Vec<&str> = gone_ids.iter().map(String::as_str).collect();
    let (status, body) = api(
        &router,
        "DescribeLaunchTemplates",
        &indexed("LaunchTemplateId", &every_gone),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"every gone id at once\": expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidLaunchTemplateId.NotFound",
        "case \"every gone id at once\": got {body}"
    );
    assert_eq!(
        error_message(&body),
        ABSENT_SENTENCE,
        "case \"every gone id at once\": got {body}"
    );

    // Every probe only REPORTED: the one template that was never deleted is still held, and
    // still reads back with its operator.
    assert_eq!(
        stored_ids(&engine),
        vec![kept.clone()],
        "the probes must leave the store holding exactly the template that was kept"
    );
    let described = ok(
        &router,
        "the template that is kept",
        "DescribeLaunchTemplates",
        &indexed("LaunchTemplateId", &[kept.as_str()]),
    )
    .await;
    assert_eq!(
        reported_operator("the template that is kept", &described),
        (Some("false".to_string()), None),
        "the kept template must still report its operator, got {described}"
    );
}
