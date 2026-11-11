// SPDX-License-Identifier: CC0-1.0

//! `GetParameters` end to end: `PutParameter` writes, the batch read reports every name
//! it resolves and reports the names it cannot resolve instead of refusing the request.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol"), through the crate's one test
//! harness (`all/parameter_store.rs`). Nothing reaches into the state struct, so a
//! response that is not computed from persisted state cannot pass: every read follows the
//! writes that created the parameters.
//!
//! The suites are tables. Their rows differ along what the rules under test generalise
//! over — how many names one call carries (both published bounds and one past them), the
//! name form (top-level, hierarchical, deep, full ARN), the parameter type, the selector
//! form, why a name resolves nothing, the `WithDecryption` flag and the signed Region —
//! rather than one recorded batch read.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::parameter_store::{
    call, call_in_region, error_code, kms_key_with_alias, router, router_with_kms, ACCOUNT, REGION,
};

/// The published bounds of `ParameterNameList`, the shape both `GetParameters$Names` and
/// `GetParametersResult$InvalidParameters` target
/// (`smithy_query(query="shape", shape="ParameterNameList")`: length 1..10).
const MIN_NAMES: usize = 1;

/// See [`MIN_NAMES`].
const MAX_NAMES: usize = 10;

/// A second Region, so a case can prove a parameter is reached only through the endpoint
/// it was created against. Deliberately not the Region the harness signs for.
const OTHER_REGION: &str = "eu-central-1";

/// One seeded parameter: its name, its `ParameterType` and the value its put supplied.
type Seed<'a> = (&'a str, &'a str, &'a str);

/// The parameters every table below reads, spanning the name forms Parameter Store
/// accepts (top-level, hierarchical, the published maximum depth of fifteen levels, and
/// names whose alphabetical order is not the order they are asked in) and every
/// `ParameterType` the model declares
/// (`smithy_query(query="shape", shape="ParameterType")`).
///
/// Exactly [`MAX_NAMES`] of them, so one call can ask for all of them.
fn seeds() -> Vec<Seed<'static>> {
    vec![
        ("Batch_top_level", "String", "top-level-value"),
        ("/batch/zeta", "String", "zeta-value"),
        ("/batch/alpha", "String", "alpha-value"),
        ("/batch/dev/db", "String", "db.example.internal"),
        ("/batch/dev/list", "StringList", "alpha,beta,gamma"),
        ("/batch/dev/secret", "SecureString", "s3cr3t"),
        ("/batch/prod/db", "String", "prod.example.internal"),
        ("/batch/prod/list", "StringList", "one,two"),
        ("/batch/0-digit.name_x", "String", "symbols-value"),
        (
            "/batch/a/b/c/d/e/f/g/h/i/j/k/l/m/deep",
            "String",
            "deep-value",
        ),
    ]
}

/// `PutParameter`, asserting it succeeded, and returning its response.
async fn put_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = call(router, "PutParameter", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutParameter {body} must succeed, got {status} {response}"
    );
    response
}

/// Write every parameter of [`seeds`] into `router`, signed for [`REGION`].
async fn seed(router: &MultiServiceRouter) {
    for (name, type_, value) in seeds() {
        put_ok(
            router,
            name,
            &json!({ "Name": name, "Type": type_, "Value": value }),
        )
        .await;
    }
}

/// `GetParameters` signed for [`REGION`].
async fn batch(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "GetParameters", body).await
}

/// `GetParameters`, asserting it succeeded, and returning its whole response.
async fn batch_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = batch(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetParameters {body} must succeed, got {status} {response}"
    );
    response
}

/// The `Name` of every reported parameter, in the order the answer lists them.
fn reported_names(response: &Value) -> Vec<String> {
    response["Parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("GetParameters reports a Parameters list, got {response}"))
        .iter()
        .map(|parameter| {
            parameter["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("a reported parameter carries a Name, got {parameter}"))
                .to_string()
        })
        .collect()
}

/// The names the answer could resolve no parameter for, in the order the answer lists
/// them.
fn invalid_names(response: &Value) -> Vec<String> {
    response["InvalidParameters"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("GetParameters reports an InvalidParameters list, got {response}")
        })
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("an invalid name is a string, got {name}"))
                .to_string()
        })
        .collect()
}

/// The one reported parameter named `name`.
fn reported<'a>(response: &'a Value, case: &str, name: &str) -> &'a Value {
    response["Parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: GetParameters reports a list, got {response}"))
        .iter()
        .find(|parameter| parameter["Name"] == json!(name))
        .unwrap_or_else(|| panic!("case {case:?}: {name} must be reported, got {response}"))
}

/// The ARN the service mints for `name` in `region`.
fn parameter_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:ssm:{region}:{ACCOUNT}:parameter/{}",
        name.strip_prefix('/').unwrap_or(name)
    )
}

/// Whatever the number of names a call carries, the batch read reports one parameter per
/// name it resolves, computed from what the put persisted, and reports it in alphabetical
/// order of the name.
#[tokio::test]
async fn a_batch_read_reports_one_parameter_per_name_it_resolves() {
    let router = router();
    seed(&router).await;
    let seeds = seeds();
    for value in ["String", "StringList", "SecureString"] {
        assert!(
            seeds.iter().any(|(_, type_, _)| *type_ == value),
            "the seeded table must cover ParameterType {value}, got {seeds:?}"
        );
    }

    // Every count from the published minimum to the published maximum of names per call.
    for count in MIN_NAMES..=MAX_NAMES {
        let asked: Vec<Seed<'_>> = seeds.iter().take(count).copied().collect();
        let case = format!("{count} name(s)");
        let names: Vec<&str> = asked.iter().map(|(name, _, _)| *name).collect();
        let response = batch_ok(
            &router,
            &case,
            &json!({ "Names": names, "WithDecryption": true }),
        )
        .await;

        let mut expected = names.clone();
        expected.sort_unstable();
        assert_eq!(
            reported_names(&response),
            expected,
            "case {case:?}: the resolved parameters are reported in alphabetical order, got \
             {response}"
        );
        assert_eq!(
            invalid_names(&response),
            Vec::<String>::new(),
            "case {case:?}: every name resolved, so no name is invalid, got {response}"
        );

        for (name, type_, value) in asked {
            let parameter = reported(&response, &case, name);
            assert_eq!(
                parameter["Value"],
                json!(value),
                "case {case:?}: {name} reports the stored value, got {parameter}"
            );
            assert_eq!(
                parameter["Type"],
                json!(type_),
                "case {case:?}: {name} reports the stored type, got {parameter}"
            );
            assert_eq!(
                parameter["Version"],
                json!(1),
                "case {case:?}: {name} reports the version its put created, got {parameter}"
            );
            assert_eq!(
                parameter["ARN"],
                json!(parameter_arn(REGION, name)),
                "case {case:?}: {name} reports its own ARN, got {parameter}"
            );
            assert_eq!(
                parameter["DataType"],
                json!("text"),
                "case {case:?}: {name} reports the default data type, got {parameter}"
            );
            assert_eq!(
                parameter["Selector"],
                Value::Null,
                "case {case:?}: {name} was asked for with no selector, so it reports none, got \
                 {parameter}"
            );
        }
    }
}

/// A full ARN names the same parameter a bare name does, and the parameter is reported
/// under its stored name either way.
#[tokio::test]
async fn an_arn_names_the_parameter_its_bare_name_names() {
    let router = router();
    seed(&router).await;

    for (name, _, value) in seeds() {
        let arn = parameter_arn(REGION, name);
        let response = batch_ok(&router, &arn, &json!({ "Names": [&arn] })).await;
        assert_eq!(
            reported_names(&response),
            vec![name.to_string()],
            "case {arn:?}: the ARN resolves the parameter it names, got {response}"
        );
        let parameter = reported(&response, &arn, name);
        if name != "/batch/dev/secret" {
            assert_eq!(
                parameter["Value"],
                json!(value),
                "case {arn:?}: the ARN read reports the stored value, got {parameter}"
            );
        }
        assert_eq!(
            parameter["ARN"],
            json!(arn),
            "case {arn:?}: the read reports the ARN it was asked by, got {parameter}"
        );
    }
}

/// Whatever the spelling of a name, the batch read resolves it to the parameter the
/// single read resolves it to, and reports the same `Parameter` for it.
#[tokio::test]
async fn a_batch_read_resolves_a_name_as_the_single_read_does() {
    let router = router();
    seed(&router).await;
    let held = "/batch/dev/db";

    // One row per form a read's `Name` takes: the stored spelling, the same name without
    // its leading separator, the full ARN, a version selector, and a name nothing holds.
    let spellings = [
        held.to_string(),
        held.trim_start_matches('/').to_string(),
        parameter_arn(REGION, held),
        format!("{held}:1"),
        "/batch/dev/missing".to_string(),
    ];
    for asked in spellings {
        let batched = batch_ok(&router, &asked, &json!({ "Names": [&asked] })).await;
        let (status, single) = call(&router, "GetParameter", &json!({ "Name": &asked })).await;
        let resolved = status == 200;
        assert_eq!(
            batched["Parameters"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            usize::from(resolved),
            "case {asked:?}: the batch read resolves what the single read resolves ({status} from \
             GetParameter), got {batched}"
        );
        if resolved {
            assert_eq!(
                batched["Parameters"][0], single["Parameter"],
                "case {asked:?}: both reads report the same parameter, got {batched} and {single}"
            );
            assert_eq!(
                invalid_names(&batched),
                Vec::<String>::new(),
                "case {asked:?}: the name resolved, so it is not invalid, got {batched}"
            );
        } else {
            assert_eq!(
                invalid_names(&batched),
                vec![asked.clone()],
                "case {asked:?}: the single read refused it ({status} {single}), so the batch \
                 read reports it invalid, got {batched}"
            );
        }
    }
}

/// Every reason a name resolves no parameter is REPORTED in `InvalidParameters` — the
/// batch read models no not-found refusal — while the names beside it are still answered.
#[tokio::test]
async fn a_name_this_endpoint_resolves_nothing_for_is_reported_not_refused() {
    let router = router();
    seed(&router).await;
    let held = "/batch/dev/db";

    let cases: Vec<(&str, String)> = vec![
        ("a name nobody created", "/batch/dev/missing".to_string()),
        (
            "a name whose hierarchy nobody created",
            "/nothing/here/at/all".to_string(),
        ),
        ("a version the parameter has not got", format!("{held}:9")),
        (
            "a label nothing assigned to any version",
            format!("{held}:production"),
        ),
        (
            "a name ending in a colon, naming neither a version nor a label",
            format!("{held}:"),
        ),
        (
            "an ARN naming another account",
            format!("arn:aws:ssm:{REGION}:999988887777:parameter{held}"),
        ),
        (
            "an ARN naming another Region",
            format!("arn:aws:ssm:{OTHER_REGION}:{ACCOUNT}:parameter{held}"),
        ),
        (
            "an ARN naming another service",
            "arn:aws:s3:::a-bucket-of-mine".to_string(),
        ),
        (
            "a name in the AWS-published hierarchy that AWS publishes nothing under",
            "/aws/service/ami-amazon-linux-latest/no-such-published-alias".to_string(),
        ),
        (
            "a name carrying an empty hierarchy level",
            "/batch//dev".to_string(),
        ),
    ];

    for (case, asked) in cases {
        let response = batch_ok(&router, case, &json!({ "Names": [held, &asked] })).await;
        assert_eq!(
            reported_names(&response),
            vec![held.to_string()],
            "case {case:?}: {asked} resolves nothing, so only {held} is reported, got {response}"
        );
        assert_eq!(
            invalid_names(&response),
            vec![asked.clone()],
            "case {case:?}: {asked} is reported invalid under the name it was asked by, got \
             {response}"
        );
    }
}

/// The spaces rule: a name is trimmed, and a trimmed name that still carries spaces
/// between characters fails the whole request with a `ValidationException` — the one
/// refusal a batch read makes.
#[tokio::test]
async fn a_name_is_trimmed_and_a_name_with_inner_spaces_is_refused() {
    let router = router();
    seed(&router).await;
    let held = "/batch/dev/db";

    for padded in [
        format!("  {held}"),
        format!("{held}  "),
        format!("\t{held}\n"),
    ] {
        let response = batch_ok(&router, &padded, &json!({ "Names": [&padded] })).await;
        assert_eq!(
            reported_names(&response),
            vec![held.to_string()],
            "case {padded:?}: the surrounding spaces are removed, so the name resolves, got \
             {response}"
        );
    }

    let refused = [
        "/batch/dev /db".to_string(),
        "my parameter".to_string(),
        format!("{held} :1"),
    ];
    for asked in refused {
        for names in [json!([&asked]), json!([held, &asked])] {
            let (status, response) = batch(&router, &json!({ "Names": names })).await;
            assert_eq!(
                status, 400,
                "case {asked:?}: a name with inner spaces fails the request, got {status} \
                 {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {asked:?}: the refusal is a ValidationException, got {response}"
            );
        }
    }
}

/// A `name:selector` suffix chooses which version of a parameter the batch read reports,
/// and the answer echoes the selector it resolved by.
#[tokio::test]
async fn a_selector_chooses_the_version_a_batch_read_reports() {
    let router = router();
    let name = "/batch/versions";
    let values = ["first", "second", "third"];
    for (index, value) in values.iter().enumerate() {
        let mut body = json!({ "Name": name, "Type": "String", "Value": value });
        if index > 0 {
            body["Overwrite"] = json!(true);
        }
        put_ok(&router, value, &body).await;
    }

    // Each version by number, the newest with no selector at all, and a version the
    // parameter has not got.
    let response = batch_ok(
        &router,
        "every selector form",
        &json!({
            "Names": [
                name,
                format!("{name}:1"),
                format!("{name}:2"),
                format!("{name}:3"),
                format!("{name}:4"),
            ],
        }),
    )
    .await;

    let reported = response["Parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("GetParameters reports a list, got {response}"));
    let versions: Vec<(Value, Value)> = reported
        .iter()
        .map(|parameter| (parameter["Version"].clone(), parameter["Selector"].clone()))
        .collect();
    assert_eq!(
        versions,
        vec![
            (json!(1), json!(":1")),
            (json!(2), json!(":2")),
            (json!(3), Value::Null),
            (json!(3), json!(":3")),
        ],
        "case every selector form: one entry per resolved selector, ordered by the version it \
         resolved to, got {response}"
    );
    for parameter in reported {
        let version = parameter["Version"].as_i64().unwrap_or_default();
        let expected = values
            .get(usize::try_from(version).unwrap_or_default().saturating_sub(1))
            .copied()
            .unwrap_or_default();
        assert_eq!(
            parameter["Value"],
            json!(expected),
            "case every selector form: version {version} reports the value its put supplied, got \
             {parameter}"
        );
    }
    assert_eq!(
        invalid_names(&response),
        vec![format!("{name}:4")],
        "case every selector form: a version the parameter has not got is invalid, got {response}"
    );
}

/// The names one call carries are a set, answered in alphabetical order whatever order
/// they were asked in: a name asked for twice names one parameter.
#[tokio::test]
async fn the_names_one_call_carries_are_a_set_in_alphabetical_order() {
    let router = router();
    seed(&router).await;

    let mut descending: Vec<&str> = seeds().iter().map(|(name, _, _)| *name).collect();
    descending.sort_unstable();
    descending.reverse();
    let mut ascending = descending.clone();
    ascending.reverse();

    let response = batch_ok(
        &router,
        "reverse alphabetical",
        &json!({ "Names": descending, "WithDecryption": true }),
    )
    .await;
    assert_eq!(
        reported_names(&response),
        ascending,
        "case reverse alphabetical: the answer is ordered by name, not by the order asked, got \
         {response}"
    );

    let repeated = "/batch/dev/db";
    let response = batch_ok(
        &router,
        "one name asked three times",
        &json!({ "Names": [repeated, repeated, repeated] }),
    )
    .await;
    assert_eq!(
        reported_names(&response),
        vec![repeated.to_string()],
        "case one name asked three times: it names one parameter, so it is reported once, got \
         {response}"
    );

    let missing = "/batch/dev/missing";
    let response = batch_ok(
        &router,
        "one unresolvable name asked twice",
        &json!({ "Names": [missing, missing] }),
    )
    .await;
    assert_eq!(
        invalid_names(&response),
        vec![missing.to_string()],
        "case one unresolvable name asked twice: it is reported invalid once, got {response}"
    );
}

/// `WithDecryption` selects which form a `SecureString`'s value takes, and selects nothing
/// for the types it is documented to be ignored for. The undecrypted form is the
/// ciphertext the store holds, which is what the single read reports for the same name.
#[tokio::test]
async fn with_decryption_selects_the_form_a_secure_string_reports() {
    let router = router_with_kms();
    let customer_key = kms_key_with_alias(&router, "alias/batch-parameters").await;

    // One row per key a `SecureString` can be sealed under — the Amazon Web Services
    // managed key a put naming none falls back to, and a customer managed key — plus the
    // two types the flag is ignored for.
    let cases: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "the managed key a put naming none falls back to",
            "/decrypt/default",
            "SecureString",
            None,
        ),
        (
            "a customer managed key",
            "/decrypt/customer",
            "SecureString",
            Some(customer_key.as_str()),
        ),
        ("a String", "/decrypt/plain", "String", None),
        ("a StringList", "/decrypt/list", "StringList", None),
    ];
    for (case, name, type_, key_id) in &cases {
        let mut body = json!({ "Name": name, "Type": type_, "Value": format!("value-of-{name}") });
        if let Some(key_id) = key_id {
            body["KeyId"] = json!(key_id);
        }
        put_ok(&router, case, &body).await;
    }

    let names: Vec<&str> = cases.iter().map(|(_, name, _, _)| *name).collect();
    let decrypted = batch_ok(
        &router,
        "WithDecryption true",
        &json!({ "Names": &names, "WithDecryption": true }),
    )
    .await;
    for flag in [json!(false), Value::Null] {
        let mut body = json!({ "Names": &names });
        if !flag.is_null() {
            body["WithDecryption"] = flag.clone();
        }
        let sealed = batch_ok(&router, &format!("WithDecryption {flag}"), &body).await;

        for (case, name, type_, _) in &cases {
            let plaintext = format!("value-of-{name}");
            assert_eq!(
                reported(&decrypted, case, name)["Value"],
                json!(plaintext),
                "case {case:?}: a decrypting read reports the value its put supplied, got \
                 {decrypted}"
            );
            let at_rest = reported(&sealed, case, name)["Value"].clone();
            if *type_ == "SecureString" {
                assert_ne!(
                    at_rest,
                    json!(plaintext),
                    "case {case:?}: a read that asks for no decryption reports the ciphertext, \
                     got {sealed}"
                );
            } else {
                assert_eq!(
                    at_rest,
                    json!(plaintext),
                    "case {case:?}: the flag is ignored for {type_}, got {sealed}"
                );
            }
            // The batch read and the single read of one name report the same form.
            let (status, single) = call(
                &router,
                "GetParameter",
                &json!({ "Name": name, "WithDecryption": false }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetParameter {name} must succeed, got {status} {single}"
            );
            assert_eq!(
                single["Parameter"]["Value"], at_rest,
                "case {case:?}: the batch read reports what the single read reports, got {single}"
            );
        }
    }
}

/// A batch read is a read: it changes nothing a later caller sees, however often it is
/// called and whether or not it decrypts.
#[tokio::test]
async fn a_batch_read_changes_nothing_a_later_caller_sees() {
    let router = router();
    seed(&router).await;
    let names: Vec<&str> = seeds().iter().map(|(name, _, _)| *name).collect();
    let body = json!({ "Names": &names, "WithDecryption": true });

    let first = batch_ok(&router, "first read", &body).await;
    for round in 1..=3 {
        let case = format!("read {round}");
        let again = batch_ok(&router, &case, &body).await;
        assert_eq!(
            again, first,
            "case {case:?}: a repeated batch read reports the same answer, got {again}"
        );
        let undecrypted = batch_ok(&router, &case, &json!({ "Names": &names })).await;
        assert_eq!(
            reported_names(&undecrypted),
            reported_names(&first),
            "case {case:?}: a read that decrypts nothing reports the same parameters, got \
             {undecrypted}"
        );
    }

    for (name, _, value) in seeds() {
        let case = format!("{name} after the reads");
        let (status, history) = call(
            &router,
            "GetParameterHistory",
            &json!({ "Name": name, "WithDecryption": true }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetParameterHistory must succeed, got {status} {history}"
        );
        let versions = history["Parameters"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            versions, 1,
            "case {case:?}: the reads created no version, got {history}"
        );
        assert_eq!(
            history["Parameters"][0]["Value"],
            json!(value),
            "case {case:?}: the stored value is untouched, got {history}"
        );
    }
}

/// Parameter Store is regional: a batch read resolves a name only against the parameters
/// of the endpoint it is signed for, so the same name reports that Region's own value and
/// a name only the other Region holds resolves nothing.
#[tokio::test]
async fn a_batch_read_resolves_only_the_signed_region_s_parameters() {
    let router = router();
    let shared = "/batch/region/shared";
    let cases = [(REGION, "west-value"), (OTHER_REGION, "central-value")];
    for (region, value) in cases {
        let body = json!({ "Name": shared, "Type": "String", "Value": value });
        let (status, response) = call_in_region(&router, region, "PutParameter", &body).await;
        assert_eq!(
            status, 200,
            "case {region}: PutParameter must succeed, got {status} {response}"
        );
    }
    // A parameter only one of the two Regions holds.
    let only_west = "/batch/region/west-only";
    put_ok(
        &router,
        only_west,
        &json!({ "Name": only_west, "Type": "String", "Value": "west-only-value" }),
    )
    .await;

    for (region, value) in cases {
        let (status, response) = call_in_region(
            &router,
            region,
            "GetParameters",
            &json!({ "Names": [shared, only_west] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region}: GetParameters must succeed, got {status} {response}"
        );
        let parameter = reported(&response, region, shared);
        assert_eq!(
            parameter["Value"],
            json!(value),
            "case {region}: the read reports the value this Region holds, got {parameter}"
        );
        assert_eq!(
            parameter["ARN"],
            json!(parameter_arn(region, shared)),
            "case {region}: the ARN names the Region the read was signed for, got {parameter}"
        );
        let expected_invalid: Vec<String> = if region == REGION {
            Vec::new()
        } else {
            vec![only_west.to_string()]
        };
        assert_eq!(
            invalid_names(&response),
            expected_invalid,
            "case {region}: a name only another Region holds resolves nothing, got {response}"
        );
    }
}

/// The published bound on how many names one call carries: the two bounds are answered and
/// a request one name past either is refused.
#[tokio::test]
async fn a_name_count_outside_the_published_bounds_is_refused() {
    let router = router();
    seed(&router).await;
    let names: Vec<&str> = seeds().iter().map(|(name, _, _)| *name).collect();

    for count in [MIN_NAMES, MAX_NAMES] {
        let case = format!("{count} names, at the bound");
        let response = batch_ok(
            &router,
            &case,
            &json!({ "Names": names.iter().take(count).collect::<Vec<_>>(), "WithDecryption": true }),
        )
        .await;
        assert_eq!(
            reported_names(&response).len(),
            count,
            "case {case:?}: every name at the bound is answered, got {response}"
        );
    }

    let one_too_many: Vec<String> = (0..=MAX_NAMES).map(|at| format!("/batch/n{at}")).collect();
    let past_bounds = [
        ("no name at all", json!([])),
        ("one name past the maximum", json!(one_too_many)),
    ];
    for (case, asked) in past_bounds {
        let (status, response) = batch(&router, &json!({ "Names": asked })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a name count past the bound is refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: the refusal is a ValidationException, got {response}"
        );
    }
}
