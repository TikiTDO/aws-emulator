// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's cluster-parameter-group slice:
//! `CreateDBClusterParameterGroup`, `DescribeDBClusterParameterGroups`,
//! `DescribeDBClusterParameters`, `ModifyDBClusterParameterGroup`,
//! `ResetDBClusterParameterGroup` and `DeleteDBClusterParameterGroup`.
//!
//! This is the control plane every AWS SDK client of the shared `rds` signing name reaches
//! — an Amazon RDS, Amazon DocumentDB or Amazon Neptune client all sign `rds`, which this
//! service claims (`src/service.rs`) exactly as AWS serves all three APIs on the RDS
//! endpoint — so the suite drives it the way those clients do: real awsQuery form POSTs
//! through a `MultiServiceRouter`, carrying the SigV4 credential scope the router keys on,
//! with every write read back through a describe.
//!
//! The rules are asserted over TABLES spanning the class each rule generalises over, not
//! over one recorded request:
//!
//! * the value rule is driven by the published catalog ITSELF
//!   (`service_common::rds_cluster_parameters::DOCDB_CLUSTER_PARAMETERS`), every entry of
//!   it, with each enumerated value, each published range bound and one step past it;
//! * the name and family rules over every clause the model states and both sides of the
//!   published bound;
//! * the pending-change rule over both apply types the catalog publishes and both apply
//!   methods the model's `ApplyMethod` enum names, plus the absent one;
//! * the `Source` filter in both of the API's vocabularies, plus a value naming no source.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    ClusterParameterSpec, DOCDB_CLUSTER_PARAMETERS, MAX_PARAMETER_GROUP_NAME_LEN,
    PARAMETER_SOURCE_ENGINE_DEFAULT, PARAMETER_SOURCE_SYSTEM, PARAMETER_SOURCE_USER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{emulator, expect_error, expect_ok, optional_text, text, texts, REGION};

/// How many `tag` elements a response carries; `texts` is the sibling suite's reader.
fn count(xml: &str, tag: &str) -> usize {
    texts(xml, tag).len()
}

/// One awsQuery call against `region`, form-ENCODING every parameter value, returning the
/// HTTP status and the XML body.
///
/// The sibling suite's sender deliberately refuses a value that would need escaping, which
/// is right for the identifiers it sends; a published parameter value does need it
/// (`tls1.2+` carries a `+`, which a form body reads as a space), so the escaping is done
/// here. The encoder is three lines and the decoder it feeds is the framework's own — a
/// crate for it would change `Cargo.lock`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2014-10-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://rds.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{region}/rds/aws4_request, \
                 SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    (status, body)
}

/// [`call_in`] against [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    call_in(router, REGION, action, params).await
}

/// Percent-encode one query parameter value for the form body.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// An Amazon DocumentDB cluster parameter group family: one the emulator publishes a
/// parameter catalog for.
const DOCDB_FAMILY: &str = "docdb5.0";

/// Create a group and return its (lowercased) name.
///
/// `pub(crate)` so a sibling suite of this one test target creates a cluster parameter group
/// the same way rather than growing a second copy of this call.
pub(crate) async fn create_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    family: &str,
) -> String {
    expect_ok(
        case,
        call(
            router,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", name),
                ("DBParameterGroupFamily", family),
                ("Description", "a group under test"),
            ],
        )
        .await,
    );
    name.to_ascii_lowercase()
}

/// Set one parameter on `group`, with an optional apply method.
///
/// `pub(crate)` for the same reason [`create_group`] is.
pub(crate) async fn modify(
    router: &MultiServiceRouter,
    group: &str,
    name: &str,
    value: &str,
    apply_method: Option<&str>,
) -> (u16, String) {
    let mut params = vec![
        ("DBClusterParameterGroupName", group),
        ("Parameters.Parameter.1.ParameterName", name),
        ("Parameters.Parameter.1.ParameterValue", value),
    ];
    if let Some(method) = apply_method {
        params.push(("Parameters.Parameter.1.ApplyMethod", method));
    }
    call(router, "ModifyDBClusterParameterGroup", &params).await
}

// ---------------------------------------------------------------------------
// The lifecycle, end to end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_cluster_parameter_group_lifecycle_is_computed_from_persisted_state() {
    let router = emulator();

    // 1. CreateDBClusterParameterGroup stores the group and answers with it.
    let created = expect_ok(
        "create",
        call(
            &router,
            "CreateDBClusterParameterGroup",
            &[
                // A mixed-case name: `DBClusterParameterGroupName` is "stored as a
                // lowercase string", so every later read reports it lowercased.
                ("DBClusterParameterGroupName", "Probe-CPG"),
                ("DBParameterGroupFamily", DOCDB_FAMILY),
                ("Description", "probe"),
            ],
        )
        .await,
    );
    assert!(
        created.contains("<CreateDBClusterParameterGroupResponse"),
        "case \"create\": expected the awsQuery CreateDBClusterParameterGroupResponse \
         envelope, got {created}"
    );
    let expected_arn = format!("arn:aws:rds:{REGION}:000000000000:cluster-pg:probe-cpg");
    assert_eq!(
        (
            text(&created, "DBClusterParameterGroupName"),
            text(&created, "DBParameterGroupFamily"),
            text(&created, "Description"),
            text(&created, "DBClusterParameterGroupArn"),
        ),
        (
            "probe-cpg".to_string(),
            DOCDB_FAMILY.to_string(),
            "probe".to_string(),
            expected_arn.clone(),
        ),
        "case \"create\": the group the service answers with must be the one the request \
         asked for, got {created}"
    );

    // 2. ...and it reads back through DescribeDBClusterParameterGroups, by the lowercased
    //    name, by the name as sent, and in the unfiltered listing.
    for (case, params) in [
        (
            "describe by name",
            vec![("DBClusterParameterGroupName", "probe-cpg")],
        ),
        (
            "describe by the name as sent, mixed case",
            vec![("DBClusterParameterGroupName", "Probe-CPG")],
        ),
        ("describe every group", Vec::new()),
    ] {
        let listed = expect_ok(
            case,
            call(&router, "DescribeDBClusterParameterGroups", &params).await,
        );
        assert_eq!(
            (
                count(&listed, "DBClusterParameterGroup"),
                text(&listed, "DBClusterParameterGroupArn"),
            ),
            (1, expected_arn.clone()),
            "case {case:?}: expected exactly the group just created, got {listed}"
        );
    }

    // 3. A freshly created group has set no parameter of its own: every parameter its
    //    family publishes is reported at the engine's own default, and none under `user`.
    let fresh = expect_ok(
        "describe parameters of a fresh group",
        call(
            &router,
            "DescribeDBClusterParameters",
            &[
                ("DBClusterParameterGroupName", "probe-cpg"),
                ("Source", PARAMETER_SOURCE_USER),
            ],
        )
        .await,
    );
    assert_eq!(
        count(&fresh, "Parameter"),
        0,
        "case \"describe parameters of a fresh group\": a group carries no parameter of its \
         own until one is set, got {fresh}"
    );
    let inherited = expect_ok(
        "describe parameters of a fresh group",
        call(
            &router,
            "DescribeDBClusterParameters",
            &[("DBClusterParameterGroupName", "probe-cpg")],
        )
        .await,
    );
    assert_eq!(
        count(&inherited, "Parameter"),
        DOCDB_CLUSTER_PARAMETERS.len(),
        "case \"describe parameters of a fresh group\": the group inherits its family's \
         whole parameter list, got {inherited}"
    );

    // 4. ModifyDBClusterParameterGroup sets one and echoes the group name...
    let modified = expect_ok(
        "modify",
        modify(&router, "probe-cpg", "tls", "disabled", Some("pending-reboot")).await,
    );
    assert_eq!(
        text(&modified, "DBClusterParameterGroupName"),
        "probe-cpg",
        "case \"modify\": the answer must echo the group it wrote, got {modified}"
    );

    // 5. ...and the describe reports the value, with the metadata the family publishes.
    let after = expect_ok(
        "describe parameters after a modify",
        call(
            &router,
            "DescribeDBClusterParameters",
            &[
                ("DBClusterParameterGroupName", "probe-cpg"),
                ("Source", PARAMETER_SOURCE_USER),
            ],
        )
        .await,
    );
    let tls = DOCDB_CLUSTER_PARAMETERS
        .iter()
        .find(|spec| spec.name == "tls")
        .expect("the catalog publishes tls");
    assert_eq!(
        (
            count(&after, "Parameter"),
            text(&after, "ParameterName"),
            text(&after, "ParameterValue"),
            text(&after, "Source"),
            text(&after, "ApplyMethod"),
            text(&after, "AllowedValues"),
        ),
        (
            1,
            "tls".to_string(),
            "disabled".to_string(),
            PARAMETER_SOURCE_USER.to_string(),
            "pending-reboot".to_string(),
            tls.allowed_values.to_string(),
        ),
        "case \"describe parameters after a modify\": expected the parameter just set, got \
         {after}"
    );

    // 6. A second create under the same name is the modelled already-exists refusal.
    expect_error(
        "create the same name twice",
        call(
            &router,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "probe-cpg"),
                ("DBParameterGroupFamily", DOCDB_FAMILY),
                ("Description", "probe"),
            ],
        )
        .await,
        400,
        "DBParameterGroupAlreadyExists",
    );

    // 7. The group is waiting on a reboot for a static parameter, so it cannot be reset —
    //    and the message names what it is pending on.
    let refused = expect_error(
        "reset while a change is pending",
        call(
            &router,
            "ResetDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "probe-cpg"),
                ("ResetAllParameters", "true"),
            ],
        )
        .await,
        400,
        "InvalidDBParameterGroupState",
    );
    assert!(
        text(&refused, "Message").contains("tls"),
        "case \"reset while a change is pending\": the refusal must name the pending \
         parameter, got {refused}"
    );

    // 8. Every operation answers the modelled 404 for a group that is not there.
    for (case, action, params) in [
        (
            "describe groups for a missing group",
            "DescribeDBClusterParameterGroups",
            vec![("DBClusterParameterGroupName", "no-such-group")],
        ),
        (
            "describe parameters for a missing group",
            "DescribeDBClusterParameters",
            vec![("DBClusterParameterGroupName", "no-such-group")],
        ),
        (
            "modify a missing group",
            "ModifyDBClusterParameterGroup",
            vec![
                ("DBClusterParameterGroupName", "no-such-group"),
                ("Parameters.Parameter.1.ParameterName", "tls"),
                ("Parameters.Parameter.1.ParameterValue", "enabled"),
            ],
        ),
        (
            "reset a missing group",
            "ResetDBClusterParameterGroup",
            vec![
                ("DBClusterParameterGroupName", "no-such-group"),
                ("ResetAllParameters", "true"),
            ],
        ),
        (
            "delete a missing group",
            "DeleteDBClusterParameterGroup",
            vec![("DBClusterParameterGroupName", "no-such-group")],
        ),
    ] {
        expect_error(
            case,
            call(&router, action, &params).await,
            404,
            "DBParameterGroupNotFound",
        );
    }

    // 9. DeleteDBClusterParameterGroup removes it, and the describe no longer finds it.
    let deleted = expect_ok(
        "delete",
        call(
            &router,
            "DeleteDBClusterParameterGroup",
            &[("DBClusterParameterGroupName", "probe-cpg")],
        )
        .await,
    );
    assert!(
        deleted.contains("<DeleteDBClusterParameterGroupResponse"),
        "case \"delete\": expected the awsQuery DeleteDBClusterParameterGroupResponse \
         envelope, got {deleted}"
    );
    expect_error(
        "describe the deleted group",
        call(
            &router,
            "DescribeDBClusterParameterGroups",
            &[("DBClusterParameterGroupName", "probe-cpg")],
        )
        .await,
        404,
        "DBParameterGroupNotFound",
    );
}

// ---------------------------------------------------------------------------
// The value rule, over the whole published catalog
// ---------------------------------------------------------------------------

/// A value a parameter admits, and one it does not, derived from the parameter's OWN
/// published `AllowedValues` — an enumeration is split, a range is taken at each published
/// bound and one step past it.
///
/// `pub(crate)` so the sibling suite that drives this same rule over the OTHER cluster
/// engines' catalogs derives its values the same way rather than growing a second copy of
/// the derivation.
pub(crate) fn admitted_and_refused(spec: &ClusterParameterSpec) -> (Vec<String>, Vec<String>) {
    if spec.allowed_values.contains(',') {
        let admitted: Vec<String> = spec
            .allowed_values
            .split(',')
            .map(|value| value.trim().to_string())
            .collect();
        return (admitted, vec!["not-a-published-value".to_string()]);
    }
    // Split through the spec's OWN range reader, which keeps a negative low bound
    // (`-1-2147483647`) intact: the derivation here and the parsing the service
    // validates with then read one published range the same way.
    let (low, high) = spec
        .range()
        .expect("a non-enumerated AllowedValues is a <low>-<high> range");
    match spec.data_type {
        "integer" => {
            let low: i64 = low.parse().expect("an integer range bound");
            let high: i64 = high.parse().expect("an integer range bound");
            (
                vec![low.to_string(), high.to_string()],
                vec![
                    (low - 1).to_string(),
                    (high + 1).to_string(),
                    "not-a-number".to_string(),
                ],
            )
        }
        "float" => {
            let low: f64 = low.parse().expect("a float range bound");
            let high: f64 = high.parse().expect("a float range bound");
            (
                vec![low.to_string(), high.to_string()],
                vec![
                    (low - 1.0).to_string(),
                    (high + 1.0).to_string(),
                    "not-a-number".to_string(),
                ],
            )
        }
        other => panic!("unhandled DataType {other:?} for parameter {}", spec.name),
    }
}

/// Every parameter the DocumentDB families publish admits exactly the values its
/// `AllowedValues` names, and refuses anything else with `InvalidParameterValue` — a
/// message naming the parameter and the values it admits. An admitted value is readable
/// back from state.
///
/// The table is the published catalog itself, so it covers the parameters the issue never
/// mentioned (`profiler`, `ttl_monitor`, `change_stream_log_retention_duration`,
/// `profiler_sampling_rate`, `profiler_threshold_ms`, `audit_logs`) as well as the one it
/// did (`tls`), and every range is exercised at both published bounds and one step past
/// each.
#[tokio::test]
async fn a_parameter_value_outside_the_published_set_is_refused() {
    let router = emulator();
    let group = create_group(&router, "value rule", "value-rule-cpg", DOCDB_FAMILY).await;

    for spec in DOCDB_CLUSTER_PARAMETERS {
        let (admitted, refused) = admitted_and_refused(spec);
        for value in &admitted {
            let case = format!("{} = {value:?} (admitted)", spec.name);
            let answered = expect_ok(&case, modify(&router, &group, spec.name, value, None).await);
            assert_eq!(
                text(&answered, "DBClusterParameterGroupName"),
                group,
                "case {case:?}: expected the group name echoed, got {answered}"
            );
            let reported = expect_ok(
                &case,
                call(
                    &router,
                    "DescribeDBClusterParameters",
                    &[("DBClusterParameterGroupName", group.as_str())],
                )
                .await,
            );
            let values = texts(&reported, "ParameterValue");
            assert!(
                values.contains(value),
                "case {case:?}: expected the stored value to be reported, got {values:?}"
            );
        }
        for value in &refused {
            let case = format!("{} = {value:?} (refused)", spec.name);
            let answered = expect_error(
                &case,
                modify(&router, &group, spec.name, value, None).await,
                400,
                "InvalidParameterValue",
            );
            let message = text(&answered, "Message");
            assert!(
                message.contains(spec.name) && message.contains(spec.allowed_values),
                "case {case:?}: the refusal must name the parameter and its allowed values, \
                 got {message:?}"
            );
        }
    }

    // A parameter name the family publishes nothing for is refused the same way, naming the
    // parameter the request asked for.
    let answered = expect_error(
        "a parameter the family does not publish",
        modify(&router, &group, "no_such_parameter", "enabled", None).await,
        400,
        "InvalidParameterValue",
    );
    assert!(
        text(&answered, "Message").contains("no_such_parameter"),
        "the refusal must name the parameter the request asked for, got {answered}"
    );

    // The model types `ApplyMethod` as an enum, so a method outside it is refused at the
    // boundary — before the handler — under the code this service answers a parameter
    // value its member does not accept with.
    expect_error(
        "an apply method the model does not name",
        modify(&router, &group, "tls", "enabled", Some("whenever")).await,
        400,
        "InvalidParameterValue",
    );
}

/// A parameter the family's published catalog does not carry at EITHER level is still
/// accepted and read back from state, because refusing a value there would invent a
/// constraint — it simply carries none of the metadata only a published parameter has.
#[tokio::test]
async fn a_family_with_no_published_catalog_carries_any_parameter() {
    let router = emulator();
    // (case, family, the family's catalog publishes this parameter, parameter, value)
    let rows: [(&str, &str, bool, &str, &str); 3] = [
        ("a docdb family", "docdb4.0", true, "tls", "enabled"),
        (
            "a parameter no catalog of an aurora family carries",
            "aurora-postgresql15",
            false,
            "not_a_published_parameter",
            "4096",
        ),
        (
            "a neptune family",
            "neptune1.3",
            true,
            "neptune_query_timeout",
            "120000",
        ),
    ];

    for (case, family, publishes, name, value) in rows {
        let group = create_group(
            &router,
            case,
            &format!("catalog-{}", family.replace('.', "-")),
            family,
        )
        .await;
        expect_ok(case, modify(&router, &group, name, value, None).await);
        // The group's OWN parameters: the whole report also carries the family's published
        // defaults, which the `Source` filter is what selects between.
        let reported = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusterParameters",
                &[
                    ("DBClusterParameterGroupName", group.as_str()),
                    ("Source", PARAMETER_SOURCE_USER),
                ],
            )
            .await,
        );
        assert_eq!(
            (
                texts(&reported, "ParameterName"),
                texts(&reported, "ParameterValue"),
                optional_text(&reported, "AllowedValues").is_some(),
            ),
            (vec![name.to_string()], vec![value.to_string()], publishes),
            "case {case:?}: expected the stored parameter reported, with published metadata \
             only for a parameter its family's catalog carries, got {reported}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a create admits as a name and a family
// ---------------------------------------------------------------------------

/// A `DBClusterParameterGroupName` must satisfy the rule the model states for it
/// (`DBClusterParameterGroupNameMessage$DBClusterParameterGroupName`: "from 1 to 255 letters
/// or numbers. The first character must be a letter. Cannot end with a hyphen or contain
/// two consecutive hyphens"), and a `docdb`-prefixed `DBParameterGroupFamily` must name one
/// of the DocumentDB families; a family of another engine on the shared `rds` front door is
/// accepted, because this emulator publishes no catalog to refuse it against.
#[tokio::test]
async fn a_create_refuses_a_name_or_a_family_the_service_does_not_admit() {
    let router = emulator();
    let longest = format!("a{}", "b".repeat(MAX_PARAMETER_GROUP_NAME_LEN - 1));
    let too_long = format!("a{}", "b".repeat(MAX_PARAMETER_GROUP_NAME_LEN));

    // (case, name, family, admitted)
    let rows: [(&str, &str, &str, bool); 14] = [
        ("letters, digits and hyphens", "probe-cpg-1", "docdb5.0", true),
        ("the longest admitted name", &longest, "docdb5.0", true),
        ("one character past the bound", &too_long, "docdb5.0", false),
        ("an empty name", "", "docdb5.0", false),
        ("a name starting with a digit", "1-probe", "docdb5.0", false),
        ("a name starting with a hyphen", "-probe", "docdb5.0", false),
        ("a name ending with a hyphen", "probe-", "docdb5.0", false),
        ("two consecutive hyphens", "pro--be", "docdb5.0", false),
        // A `default.<family>` group's name carries a `.`, which the rule does not admit.
        ("a default group's name", "default.docdb5.0", "docdb5.0", false),
        ("an underscore", "probe_cpg", "docdb5.0", false),
        // Every family DocumentDB publishes is admitted...
        ("the 3.6 family", "probe-cpg-36", "docdb3.6", true),
        ("the 4.0 family", "probe-cpg-40", "docdb4.0", true),
        // ...and a `docdb` family it does not publish is not.
        ("a docdb family that does not exist", "probe-cpg-99", "docdb9.9", false),
        // Another engine's family on the same front door is accepted.
        ("another engine's family", "probe-cpg-aurora", "aurora-mysql8.0", true),
    ];

    for (case, name, family, admitted) in rows {
        let params = [
            ("DBClusterParameterGroupName", name),
            ("DBParameterGroupFamily", family),
            ("Description", "a group under test"),
        ];
        if admitted {
            let created = expect_ok(
                case,
                call(&router, "CreateDBClusterParameterGroup", &params).await,
            );
            assert_eq!(
                text(&created, "DBParameterGroupFamily"),
                family,
                "case {case:?}: the family the request named must be the one stored, got \
                 {created}"
            );
        } else {
            expect_error(
                case,
                call(&router, "CreateDBClusterParameterGroup", &params).await,
                400,
                "InvalidParameterValue",
            );
            // A refused create stored nothing: the name is not there to describe.
            expect_error(
                case,
                call(
                    &router,
                    "DescribeDBClusterParameterGroups",
                    &[("DBClusterParameterGroupName", name)],
                )
                .await,
                404,
                "DBParameterGroupNotFound",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pending changes, and what they refuse
// ---------------------------------------------------------------------------

/// A change the engine cannot apply without a reboot leaves the group pending, and a
/// pending group cannot be reset; a change applied immediately leaves nothing pending, so
/// the reset goes through and the value is gone from state afterwards.
#[tokio::test]
async fn a_change_waiting_on_a_reboot_blocks_a_reset() {
    // (case, parameter, value, apply method, resettable)
    let rows: [(&str, &str, &str, Option<&str>, bool); 6] = [
        // `tls` is static: "changes to static parameters require a reboot", so the change
        // is pending whatever the caller asks for.
        ("a static parameter, pending-reboot", "tls", "disabled", Some("pending-reboot"), false),
        ("a static parameter, immediate", "tls", "disabled", Some("immediate"), false),
        // `profiler` is dynamic: "changes to dynamic parameters are applied immediately".
        ("a dynamic parameter, immediate", "profiler", "enabled", Some("immediate"), true),
        (
            "a dynamic parameter, pending-reboot",
            "profiler",
            "enabled",
            Some("pending-reboot"),
            false,
        ),
        // With no method asked for, the change gets the method its apply type implies: a
        // dynamic parameter is applied immediately, so nothing is left pending.
        ("a dynamic parameter, no method", "profiler", "enabled", None, true),
        // A static parameter with no method asked for still waits for the reboot.
        ("a static parameter, no method", "tls", "disabled", None, false),
    ];

    for (case, parameter, value, apply_method, resettable) in rows {
        // One router per row: a reset is a state change, so the rows must not see each
        // other's.
        let router = emulator();
        let group = create_group(&router, case, "pending-cpg", DOCDB_FAMILY).await;
        expect_ok(
            case,
            modify(&router, &group, parameter, value, apply_method).await,
        );

        if resettable {
            let reset = expect_ok(
                case,
                call(
                    &router,
                    "ResetDBClusterParameterGroup",
                    &[
                        ("DBClusterParameterGroupName", group.as_str()),
                        ("Parameters.Parameter.1.ParameterName", parameter),
                    ],
                )
                .await,
            );
            assert_eq!(
                text(&reset, "DBClusterParameterGroupName"),
                group,
                "case {case:?}: the reset must echo the group it cleared, got {reset}"
            );
            let reported = expect_ok(
                case,
                call(
                    &router,
                    "DescribeDBClusterParameters",
                    &[
                        ("DBClusterParameterGroupName", group.as_str()),
                        ("Source", PARAMETER_SOURCE_USER),
                    ],
                )
                .await,
            );
            assert_eq!(
                count(&reported, "Parameter"),
                0,
                "case {case:?}: the reset parameter must be gone from state, got {reported}"
            );
        } else {
            expect_error(
                case,
                call(
                    &router,
                    "ResetDBClusterParameterGroup",
                    &[
                        ("DBClusterParameterGroupName", group.as_str()),
                        ("ResetAllParameters", "true"),
                    ],
                )
                .await,
                400,
                "InvalidDBParameterGroupState",
            );
            // The refusal changed nothing: the value is still there.
            let reported = expect_ok(
                case,
                call(
                    &router,
                    "DescribeDBClusterParameters",
                    &[
                        ("DBClusterParameterGroupName", group.as_str()),
                        ("Source", PARAMETER_SOURCE_USER),
                    ],
                )
                .await,
            );
            assert_eq!(
                texts(&reported, "ParameterValue"),
                vec![value.to_string()],
                "case {case:?}: a refused reset must leave the group as it was, got {reported}"
            );
        }
    }
}

/// `ResetAllParameters` and a list of `Parameters` cannot be used together, and a reset of
/// a parameter the family does not publish is refused.
#[tokio::test]
async fn a_reset_refuses_a_combination_the_api_forbids() {
    let router = emulator();
    let group = create_group(&router, "reset guards", "reset-cpg", DOCDB_FAMILY).await;

    expect_error(
        "ResetAllParameters together with a Parameters list",
        call(
            &router,
            "ResetDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", group.as_str()),
                ("ResetAllParameters", "true"),
                ("Parameters.Parameter.1.ParameterName", "profiler"),
            ],
        )
        .await,
        400,
        "ValidationException",
    );

    expect_error(
        "reset a parameter the family does not publish",
        call(
            &router,
            "ResetDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", group.as_str()),
                ("Parameters.Parameter.1.ParameterName", "no_such_parameter"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
}

// ---------------------------------------------------------------------------
// What a describe reports, by source
// ---------------------------------------------------------------------------

/// `Source` selects what `DescribeDBClusterParameters` reports: the group's own parameters
/// (`customer`/`user`), the family's published defaults (`engine`/`engine-default`), or
/// nothing (`service`/`system`, which this emulator sets no parameter under, and any value
/// naming no source at all — a `Source` is a filter, so it narrows the answer instead of
/// refusing the request). A request naming NO source reports the group's whole detailed
/// parameter list: every parameter of its family, each under the source its current value
/// came from.
#[tokio::test]
async fn the_source_selects_which_parameters_a_describe_reports() {
    let router = emulator();
    let group = create_group(&router, "sources", "source-cpg", DOCDB_FAMILY).await;
    expect_ok(
        "sources",
        modify(&router, &group, "ttl_monitor", "disabled", None).await,
    );
    let published = DOCDB_CLUSTER_PARAMETERS.len();

    // (case, Source, parameters expected, the Source each carries — `None` where the rows
    // reported span more than one source). Both names of each source are rows: the request
    // vocabulary the model documents (`customer`, `engine`, `service`) and the one a
    // response reports (`user`, `engine-default`, `system`).
    let rows: [(&str, Option<&str>, usize, Option<&str>); 7] = [
        ("no source named", None, published, None),
        (
            "the customer's own parameters",
            Some("customer"),
            1,
            Some(PARAMETER_SOURCE_USER),
        ),
        (
            "the same source by its reported name",
            Some(PARAMETER_SOURCE_USER),
            1,
            Some(PARAMETER_SOURCE_USER),
        ),
        (
            "the engine's published defaults",
            Some("engine"),
            published,
            Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
        ),
        (
            "the same source by its reported name",
            Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
            published,
            Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
        ),
        (
            "the service's own parameters",
            Some("service"),
            0,
            Some(PARAMETER_SOURCE_SYSTEM),
        ),
        (
            "a value naming no source this API knows",
            Some("not-a-source"),
            0,
            Some(PARAMETER_SOURCE_SYSTEM),
        ),
    ];

    for (case, source, expected, source_reported) in rows {
        let mut params = vec![("DBClusterParameterGroupName", group.as_str())];
        if let Some(source) = source {
            params.push(("Source", source));
        }
        let reported = expect_ok(case, call(&router, "DescribeDBClusterParameters", &params).await);
        assert_eq!(
            count(&reported, "Parameter"),
            expected,
            "case {case:?} (Source={source:?}): expected {expected} parameter(s), got {reported}"
        );
        if let Some(source_reported) = source_reported.filter(|_| expected > 0) {
            let sources = texts(&reported, "Source");
            assert!(
                sources.iter().all(|value| value == source_reported),
                "case {case:?}: every parameter must carry Source {source_reported:?}, got \
                 {sources:?}"
            );
        }
    }

    // In that whole list the one parameter the group has SET is reported under `user` and
    // every other published parameter under the engine's own default.
    let reported = expect_ok(
        "no source named",
        call(
            &router,
            "DescribeDBClusterParameters",
            &[("DBClusterParameterGroupName", group.as_str())],
        )
        .await,
    );
    let sources = texts(&reported, "Source");
    assert_eq!(
        sources
            .iter()
            .filter(|value| *value == PARAMETER_SOURCE_USER)
            .count(),
        1,
        "expected exactly the one parameter the group set to carry Source \
         {PARAMETER_SOURCE_USER:?}, got {sources:?}"
    );
    assert_eq!(
        sources
            .iter()
            .filter(|value| *value == PARAMETER_SOURCE_ENGINE_DEFAULT)
            .count(),
        published - 1,
        "expected every other published parameter to carry Source \
         {PARAMETER_SOURCE_ENGINE_DEFAULT:?}, got {sources:?}"
    );

    // A `Source` cannot turn a missing group's 404 into anything else: the group is looked
    // up whatever the filter says.
    expect_error(
        "a source on a group that is not there",
        call(
            &router,
            "DescribeDBClusterParameters",
            &[
                ("DBClusterParameterGroupName", "no-such-group"),
                ("Source", PARAMETER_SOURCE_USER),
            ],
        )
        .await,
        404,
        "DBParameterGroupNotFound",
    );

    // `Filters` is "not currently supported": a request that carries one is REFUSED by both
    // describes rather than served as though the filter had been applied.
    for (case, action) in [
        ("filters on the group describe", "DescribeDBClusterParameterGroups"),
        ("filters on the parameter describe", "DescribeDBClusterParameters"),
    ] {
        expect_error(
            case,
            call(
                &router,
                action,
                &[
                    ("DBClusterParameterGroupName", group.as_str()),
                    ("Filters.Filter.1.Name", "a-filter-the-api-ignores"),
                    ("Filters.Filter.1.Values.Value.1", "anything"),
                ],
            )
            .await,
            400,
            "ValidationException",
        );
    }
}

/// A group's parameter map is a growth site a caller drives, so the group's TOTAL
/// parameter count is bounded as well as each request's: a family with no published
/// catalog admits any parameter name, and a modify that would leave the group past the cap
/// is refused with a 400 and writes nothing.
#[tokio::test]
async fn a_group_carries_at_most_the_capped_number_of_parameters() {
    use service_com_amazonaws_rds_amazonrdsv19::state::{
        MAX_PARAMETERS_PER_GROUP, MAX_PARAMETERS_PER_MODIFY,
    };

    let router = emulator();
    // A family this emulator publishes no catalog for: every parameter name is admitted,
    // which is the unbounded path the cap exists for.
    let group = create_group(&router, "the group cap", "capped-cpg", "aurora-mysql8.0").await;

    // Fill the group to the cap, 20 parameters at a time (the per-request bound).
    let mut stored = 0usize;
    while stored < MAX_PARAMETERS_PER_GROUP {
        let batch = MAX_PARAMETERS_PER_MODIFY.min(MAX_PARAMETERS_PER_GROUP - stored);
        let names: Vec<String> = (0..batch).map(|i| format!("p{}", stored + i)).collect();
        let mut params = vec![("DBClusterParameterGroupName", group.as_str())];
        let keys: Vec<(String, String)> = names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    format!("Parameters.Parameter.{}.ParameterName", index + 1),
                    name.clone(),
                )
            })
            .collect();
        let values: Vec<(String, String)> = names
            .iter()
            .enumerate()
            .map(|(index, _)| {
                (
                    format!("Parameters.Parameter.{}.ParameterValue", index + 1),
                    "1".to_string(),
                )
            })
            .collect();
        for ((name_key, name), (value_key, value)) in keys.iter().zip(values.iter()) {
            params.push((name_key.as_str(), name.as_str()));
            params.push((value_key.as_str(), value.as_str()));
        }
        expect_ok(
            "fill the group to the cap",
            call(&router, "ModifyDBClusterParameterGroup", &params).await,
        );
        stored += batch;
    }
    let reported = expect_ok(
        "the group at the cap",
        call(
            &router,
            "DescribeDBClusterParameters",
            &[
                ("DBClusterParameterGroupName", group.as_str()),
                ("Source", PARAMETER_SOURCE_USER),
            ],
        )
        .await,
    );
    assert_eq!(
        count(&reported, "Parameter"),
        // The describe clamps its own page size, so the count is the page, not the map.
        MAX_PARAMETERS_PER_GROUP.min(100),
        "a group filled to the cap must report its parameters, got {reported}"
    );

    // One more distinct parameter would grow it past the cap: refused, and nothing written.
    expect_error(
        "one parameter past the group cap",
        modify(&router, &group, "one_too_many", "1", None).await,
        400,
        "ValidationException",
    );
    let after = expect_ok(
        "the group after the refusal",
        call(
            &router,
            "DescribeDBClusterParameters",
            &[("DBClusterParameterGroupName", group.as_str())],
        )
        .await,
    );
    assert!(
        !texts(&after, "ParameterName").contains(&"one_too_many".to_string()),
        "the refused parameter must not be in state, got {after}"
    );

    // Overwriting a parameter the group already carries is not growth, so it is admitted
    // even at the cap.
    expect_ok(
        "overwriting a parameter at the cap",
        modify(&router, &group, "p0", "2", None).await,
    );
}

/// Nothing caller-sized reaches the store unchecked: a create is refused when the family
/// string is past the free-form bound, or the tag list past AWS's tagging limits, and the
/// refusal stores nothing.
#[tokio::test]
async fn a_create_refuses_caller_sized_values_the_store_will_not_hold() {
    use service_com_amazonaws_rds_amazonrdsv19::state::{
        MAX_FREE_FORM_LEN, MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN,
    };

    let router = emulator();
    let longest_family = "a".repeat(MAX_FREE_FORM_LEN);
    let too_long_family = "a".repeat(MAX_FREE_FORM_LEN + 1);
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let too_long_key = "k".repeat(MAX_TAG_KEY_LEN + 1);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let too_long_value = "v".repeat(MAX_TAG_VALUE_LEN + 1);

    // (case, name, family, tag key, tag value, tag count, admitted)
    let rows: [(&str, &str, &str, &str, &str, usize, bool); 7] = [
        ("the longest family string", "bounds-a", &longest_family, "k", "v", 1, true),
        ("one character past it", "bounds-b", &too_long_family, "k", "v", 1, false),
        ("the longest tag key", "bounds-c", DOCDB_FAMILY, &longest_key, "v", 1, true),
        ("one character past it", "bounds-d", DOCDB_FAMILY, &too_long_key, "v", 1, false),
        ("the longest tag value", "bounds-e", DOCDB_FAMILY, "k", &longest_value, 1, true),
        ("one character past it", "bounds-f", DOCDB_FAMILY, "k", &too_long_value, 1, false),
        (
            "one tag past the tagging limit",
            "bounds-g",
            DOCDB_FAMILY,
            "k",
            "v",
            MAX_TAGS_PER_RESOURCE + 1,
            false,
        ),
    ];

    for (case, name, family, key, value, tag_count, admitted) in rows {
        // The tag list, as the awsQuery form encodes a list of structures: each entry's
        // members carry the entry's 1-based index.
        let mut indexed: Vec<(String, String)> = Vec::new();
        for index in 1..=tag_count {
            indexed.push((format!("Tags.Tag.{index}.Key"), key.to_string()));
            indexed.push((format!("Tags.Tag.{index}.Value"), value.to_string()));
        }
        let mut call_params: Vec<(&str, &str)> = vec![
            ("DBClusterParameterGroupName", name),
            ("DBParameterGroupFamily", family),
            ("Description", "a group under test"),
        ];
        for (key, value) in &indexed {
            call_params.push((key.as_str(), value.as_str()));
        }

        if admitted {
            expect_ok(
                case,
                call(&router, "CreateDBClusterParameterGroup", &call_params).await,
            );
        } else {
            expect_error(
                case,
                call(&router, "CreateDBClusterParameterGroup", &call_params).await,
                400,
                "ValidationException",
            );
            // The refusal stored nothing.
            expect_error(
                case,
                call(
                    &router,
                    "DescribeDBClusterParameterGroups",
                    &[("DBClusterParameterGroupName", name)],
                )
                .await,
                404,
                "DBParameterGroupNotFound",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Regional scope
// ---------------------------------------------------------------------------

/// A cluster parameter group belongs to the region its create was signed for: another
/// region neither sees it nor collides with its name.
#[tokio::test]
async fn a_group_belongs_to_the_region_it_was_created_in() {
    let router = emulator();
    let group = create_group(&router, "regional", "regional-cpg", DOCDB_FAMILY).await;

    // (case, region, what that region sees)
    let other = "eu-central-1";
    let listed_here = expect_ok(
        "the region it was created in",
        call(&router, "DescribeDBClusterParameterGroups", &[]).await,
    );
    assert_eq!(
        count(&listed_here, "DBClusterParameterGroup"),
        1,
        "the creating region must see its own group, got {listed_here}"
    );
    let listed_there = expect_ok(
        "another region",
        call_in(&router, other, "DescribeDBClusterParameterGroups", &[]).await,
    );
    assert_eq!(
        count(&listed_there, "DBClusterParameterGroup"),
        0,
        "another region must see none of it, got {listed_there}"
    );
    expect_error(
        "describing it from another region",
        call_in(
            &router,
            other,
            "DescribeDBClusterParameterGroups",
            &[("DBClusterParameterGroupName", group.as_str())],
        )
        .await,
        404,
        "DBParameterGroupNotFound",
    );
    // ...and the same name is free there.
    expect_ok(
        "creating the same name in another region",
        call_in(
            &router,
            other,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", group.as_str()),
                ("DBParameterGroupFamily", DOCDB_FAMILY),
                ("Description", "a group under test"),
            ],
        )
        .await,
    );
}
