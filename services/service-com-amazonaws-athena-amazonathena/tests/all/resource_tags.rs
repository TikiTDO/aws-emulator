// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Athena's tagging slice: `TagResource` writes tags onto the resource
//! an ARN names, `ListTagsForResource` reports what that resource holds and
//! `UntagResource` removes named keys — all driven over the wire (awsJson1_1,
//! `x-amz-target: AmazonAthena.<Operation>`) through the same router the emulator boots, so
//! every assertion is about what an SDK client observes.
//!
//! The RULE these tests state: a tagging call addresses a resource by its ARN, and the
//! resource is whichever one that ARN's resource type and name name among the resources
//! `Tag` says Athena tags — "workgroups, data catalogs, and capacity reservations". A write
//! stores the submitted key and value on that resource's own tag list, replacing the value
//! of a key it already carries rather than holding the key twice; a listing reports exactly
//! that list, including the tags the resource's create call carried; a removal drops only
//! the named keys. An ARN naming a resource this account does not hold is the modelled
//! `ResourceNotFoundException`, an ARN naming no taggable Athena resource at all is a bad
//! request, and a key inside the namespace Amazon Web Services reserves is a bad request
//! wherever it is named.
//!
//! Each test asserts that rule over a TABLE rather than one request: the rows differ along
//! what the rule generalises over — which resource type is tagged, which operation is
//! called, how many tags a request carries, whether a key is already held, how a reserved
//! key is cased and how an ARN is malformed.

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    could_not_find_resource, reserved_tag_key_message, ARN_ACCOUNT_MISMATCH,
    CAPACITY_RESERVATION_RESOURCE_TYPE, DATA_CATALOG_RESOURCE_TYPE, DEFAULT_WORK_GROUP,
    INVALID_NEXT_TOKEN, MAX_TAGS_PER_RESOURCE, RESERVED_TAG_KEY_PREFIX, TAG_TEXT_PUNCTUATION,
    WORK_GROUP_RESOURCE_TYPE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, error_code, message, router};

/// The wire error code a request this service refuses itself carries (HTTP 400).
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The wire error code a call addressing a resource this account does not hold carries:
/// the error the model binds for it, at the `httpError` it declares (400).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The Region the harness signs its requests for, and so the Region the ARN of a resource
/// those requests address carries.
const REGION: &str = "us-east-1";

/// The three operations of this slice, each of which addresses ONE resource by `ResourceARN`.
const OPERATIONS: [&str; 3] = ["TagResource", "ListTagsForResource", "UntagResource"];

/// The ARN of the Athena resource of type `resource_type` named `name`.
///
/// Built through [`engine::emulator_arn`], the one canonical ARN builder, so the account
/// segment is the account every generated emulator presents.
fn arn(resource_type: &str, name: &str) -> String {
    ::engine::emulator_arn(
        "athena",
        REGION,
        &format!("{resource_type}/{name}"),
    )
}

/// A `Tags` member carrying `pairs`, in the order given.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// Write `pairs` onto the resource `resource_arn` names: `(status, body)`.
async fn tag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    pairs: &[(&str, &str)],
) -> (u16, Value) {
    call(
        router,
        "TagResource",
        &json!({ "ResourceARN": resource_arn, "Tags": tag_list(pairs) }),
    )
    .await
}

/// Remove `keys` from the resource `resource_arn` names: `(status, body)`.
async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[&str]) -> (u16, Value) {
    call(
        router,
        "UntagResource",
        &json!({ "ResourceARN": resource_arn, "TagKeys": keys }),
    )
    .await
}

/// The tags the resource `resource_arn` names holds, as `(key, value)` pairs sorted by key
/// so a comparison is about the SET of tags rather than the order one write listed them in.
async fn held_tags(router: &MultiServiceRouter, resource_arn: &str) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        "ListTagsForResource",
        &json!({ "ResourceARN": resource_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {resource_arn:?}: ListTagsForResource must answer 200, got {status} with {body:?}"
    );
    assert!(
        body["NextToken"].is_null(),
        "case {resource_arn:?}: a listing that reported every tag hands back no NextToken, \
         got {body:?}"
    );
    let mut pairs: Vec<(String, String)> = body["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {resource_arn:?}: expected a Tags list, got {body:?}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or("<no Key>").to_string(),
                tag["Value"].as_str().unwrap_or("<no Value>").to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

/// `pairs` as the sorted `(key, value)` list [`held_tags`] reports.
fn expected_tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut expected: Vec<(String, String)> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    expected.sort();
    expected
}

/// One taggable Athena resource type, with the call that registers one resource of it.
struct Taggable {
    /// What the row is, for an assertion message.
    case: &'static str,
    /// The resource type its ARN carries.
    resource_type: &'static str,
    /// The resource's name.
    name: &'static str,
    /// The create call that registers it, or `None` for the account's default workgroup,
    /// which every account holds without any create.
    create: Option<(&'static str, Value)>,
}

/// One resource of every type Athena holds tags on, plus the account's default workgroup —
/// the one taggable resource no create call registered.
///
/// The catalog's `Parameters` are the ones `CreateDataCatalog` requires of a `LAMBDA`
/// catalog, and the reservation's amount is inside the modelled `TargetDpusInteger` range;
/// neither says anything about tagging, they are only what makes the resource exist.
fn taggable_resources() -> Vec<Taggable> {
    vec![
        Taggable {
            case: "a created workgroup",
            resource_type: WORK_GROUP_RESOURCE_TYPE,
            name: "analytics_team",
            create: Some(("CreateWorkGroup", json!({ "Name": "analytics_team" }))),
        },
        Taggable {
            case: "the account's default workgroup",
            resource_type: WORK_GROUP_RESOURCE_TYPE,
            name: DEFAULT_WORK_GROUP,
            create: None,
        },
        Taggable {
            case: "a created data catalog",
            resource_type: DATA_CATALOG_RESOURCE_TYPE,
            name: "sales_catalog",
            create: Some((
                "CreateDataCatalog",
                json!({
                    "Name": "sales_catalog",
                    "Type": "LAMBDA",
                    "Parameters": {
                        "function": "arn:aws:lambda:us-east-1:111122223333:function:meta"
                    }
                }),
            )),
        },
        Taggable {
            case: "a created capacity reservation",
            resource_type: CAPACITY_RESERVATION_RESOURCE_TYPE,
            name: "reserved-24",
            create: Some((
                "CreateCapacityReservation",
                json!({ "Name": "reserved-24", "TargetDpus": 24 }),
            )),
        },
    ]
}

/// Register the resource `resource` describes, where one has to be registered at all.
async fn register(router: &MultiServiceRouter, resource: &Taggable) {
    if let Some((operation, request)) = resource.create.as_ref() {
        let (status, body) = call(router, operation, request).await;
        assert_eq!(
            status, 200, "case {:?}: {operation} must be accepted, got {status} with {body:?}",
            resource.case
        );
    }
}

/// One step of the write/list/remove walk every taggable resource is taken through.
struct Step {
    /// What this step asserts, for an assertion message.
    case: &'static str,
    /// The tags `TagResource` writes in this step.
    write: &'static [(&'static str, &'static str)],
    /// The keys `UntagResource` removes in this step.
    remove: &'static [&'static str],
    /// Every tag the resource must hold once this step has run.
    held: &'static [(&'static str, &'static str)],
}

/// RULE: a tag write stores the submitted key and value on the resource the ARN names, a
/// key the resource already carries takes the new value in place of the old one, a listing
/// reports exactly the tags the resource holds and a removal drops only the named keys —
/// for every resource type Athena holds tags on.
///
/// The rows differ along the resource type tagged, the number of tags one request carries,
/// whether a key is already held, whether its value is empty, which of the documented tag
/// characters (`+ - = . _ : / @`) it uses, and whether a removed key was held at all.
#[tokio::test]
async fn tags_written_on_a_resource_are_the_tags_a_listing_reports() {
    /// The walk, run against each resource type in turn: every step states the whole set
    /// the resource holds afterwards, so a step that lost or duplicated a tag fails here.
    const WALK: [Step; 5] = [
        Step {
            case: "one tag on a resource that holds none",
            write: &[("owner", "analytics")],
            remove: &[],
            held: &[("owner", "analytics")],
        },
        Step {
            case: "three more tags, one with an empty value and one using the documented \
                   tag characters",
            write: &[
                ("env", "prod"),
                ("blank", ""),
                ("cost+center=1._:/@", "1234+5=6._:/@"),
            ],
            remove: &[],
            held: &[
                ("blank", ""),
                ("cost+center=1._:/@", "1234+5=6._:/@"),
                ("env", "prod"),
                ("owner", "analytics"),
            ],
        },
        Step {
            case: "a key the resource already carries takes the new value",
            write: &[("owner", "platform")],
            remove: &[],
            held: &[
                ("blank", ""),
                ("cost+center=1._:/@", "1234+5=6._:/@"),
                ("env", "prod"),
                ("owner", "platform"),
            ],
        },
        Step {
            case: "removing one held key and one the resource never held",
            write: &[],
            remove: &["env", "never-held"],
            held: &[
                ("blank", ""),
                ("cost+center=1._:/@", "1234+5=6._:/@"),
                ("owner", "platform"),
            ],
        },
        Step {
            case: "removing every remaining key",
            write: &[],
            remove: &["owner", "blank", "cost+center=1._:/@"],
            held: &[],
        },
    ];

    for resource in taggable_resources() {
        let router = router();
        register(&router, &resource).await;
        let resource_arn = arn(resource.resource_type, resource.name);

        let initial = held_tags(&router, &resource_arn).await;
        assert_eq!(
            initial,
            Vec::<(String, String)>::new(),
            "case {:?}: a resource created with no Tags holds none, got {initial:?}",
            resource.case
        );

        for step in &WALK {
            if !step.write.is_empty() {
                let (status, body) = tag(&router, &resource_arn, step.write).await;
                assert_eq!(
                    status, 200,
                    "case {:?}/{:?}: TagResource must answer 200, got {status} with {body:?}",
                    resource.case, step.case
                );
            }
            if !step.remove.is_empty() {
                let (status, body) = untag(&router, &resource_arn, step.remove).await;
                assert_eq!(
                    status, 200,
                    "case {:?}/{:?}: UntagResource must answer 200, got {status} with {body:?}",
                    resource.case, step.case
                );
            }
            let held = held_tags(&router, &resource_arn).await;
            assert_eq!(
                held,
                expected_tags(step.held),
                "case {:?}/{:?}: expected the resource to hold {:?}, got {held:?}",
                resource.case,
                step.case,
                expected_tags(step.held)
            );
        }
    }
}

/// One create that carries `Tags`, and the tags the created resource must then hold.
struct CreatedWithTags {
    /// What the row is, for an assertion message.
    case: &'static str,
    /// The create that registers the resource.
    operation: &'static str,
    /// The resource type its ARN carries, and its name.
    resource_type: &'static str,
    name: &'static str,
    /// The create request, `Tags` included.
    request: Value,
    /// The tags that create carried.
    carried: &'static [(&'static str, &'static str)],
}

/// RULE: a listing reports the tags the resource HOLDS, which includes the ones its create
/// call carried — there is one copy of a resource's tags, so a tag written at create time
/// and a tag written afterwards are read back the same way.
///
/// The rows differ along which create carried the tags, how many it carried and whether a
/// value was empty; each row then has a later write add a key and replace a created one.
#[tokio::test]
async fn tags_a_create_carried_are_reported_by_a_listing() {
    let created = [
        CreatedWithTags {
            case: "a workgroup created with two tags",
            operation: "CreateWorkGroup",
            resource_type: WORK_GROUP_RESOURCE_TYPE,
            name: "wg_with_tags",
            request: json!({
                "Name": "wg_with_tags",
                "Tags": [{ "Key": "owner", "Value": "analytics" }, { "Key": "env", "Value": "dev" }]
            }),
            carried: &[("owner", "analytics"), ("env", "dev")],
        },
        CreatedWithTags {
            case: "a data catalog created with one tag",
            operation: "CreateDataCatalog",
            resource_type: DATA_CATALOG_RESOURCE_TYPE,
            name: "catalog_with_tags",
            request: json!({
                "Name": "catalog_with_tags",
                "Type": "LAMBDA",
                "Parameters": { "function": "arn:aws:lambda:us-east-1:111122223333:function:meta" },
                "Tags": [{ "Key": "team", "Value": "sales" }]
            }),
            carried: &[("team", "sales")],
        },
        CreatedWithTags {
            case: "a capacity reservation created with one tag carrying an empty value",
            operation: "CreateCapacityReservation",
            resource_type: CAPACITY_RESERVATION_RESOURCE_TYPE,
            name: "reserved-with-tags",
            request: json!({
                "Name": "reserved-with-tags",
                "TargetDpus": 24,
                "Tags": [{ "Key": "purpose", "Value": "" }]
            }),
            carried: &[("purpose", "")],
        },
    ];

    for row in created {
        let case = row.case;
        let operation = row.operation;
        let carried = row.carried;
        let router = router();
        let (status, body) = call(&router, operation, &row.request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must be accepted, got {status} with {body:?}"
        );
        let resource_arn = arn(row.resource_type, row.name);

        let held = held_tags(&router, &resource_arn).await;
        assert_eq!(
            held,
            expected_tags(carried),
            "case {case:?}: expected the created tags {:?} to be listed, got {held:?}",
            expected_tags(carried)
        );

        // A later write joins that same list: it replaces the value of a created key and
        // appends one the resource did not carry.
        let (status, body) = tag(
            &router,
            &resource_arn,
            &[("added", "later"), (carried[0].0, "rewritten")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource must answer 200, got {status} with {body:?}"
        );
        let mut after: Vec<(&str, &str)> = carried.to_vec();
        after[0] = (carried[0].0, "rewritten");
        after.push(("added", "later"));
        let held = held_tags(&router, &resource_arn).await;
        assert_eq!(
            held,
            expected_tags(&after),
            "case {case:?}: expected {:?} after the write, got {held:?}",
            expected_tags(&after)
        );
    }
}

/// RULE: a `ResourceARN` this service can resolve no held resource from is the modelled
/// `ResourceNotFoundException` (HTTP 400), stating the one sentence that whole class takes
/// — naming the ARN the request carried — and naming the addressed resource in
/// `ResourceName`.
///
/// The rows differ along HOW the ARN resolves to nothing: a resource of a taggable type
/// this account does not hold, whichever type that is; a value that is not an Athena
/// resource ARN; an Athena ARN of a type no tags are held on; and an ARN naming another
/// account's or another Region's resource. Each row is asserted on all three operations, so
/// no operation can report a miss as a success or word it its own way.
#[tokio::test]
async fn an_arn_no_held_resource_resolves_from_is_a_miss() {
    let account = ::engine::emulator_account_id();
    let unresolvable: Vec<(&str, String)> = vec![
        (
            "a workgroup no create registered",
            arn(WORK_GROUP_RESOURCE_TYPE, "absent_resource"),
        ),
        (
            "a data catalog no create registered",
            arn(DATA_CATALOG_RESOURCE_TYPE, "absent_resource"),
        ),
        (
            "a capacity reservation no create registered",
            arn(CAPACITY_RESERVATION_RESOURCE_TYPE, "absent_resource"),
        ),
        ("not an ARN at all", "analytics_team".to_string()),
        (
            "an ARN of another vendor",
            format!("arn:aws:s3:{REGION}:{account}:accesspoint/analytics"),
        ),
        (
            "an Athena ARN of a type no tags are held on",
            arn("notebook", "analytics_team"),
        ),
        (
            "an Athena ARN carrying a type but no name",
            format!("arn:aws:athena:{REGION}:{account}:workgroup/"),
        ),
        (
            "an Athena ARN whose resource field names only a type",
            format!("arn:aws:athena:{REGION}:{account}:workgroup"),
        ),
        (
            "an ARN with too few fields",
            format!("arn:aws:athena:{REGION}:workgroup/{HELD_WORK_GROUP}"),
        ),
        (
            "the held workgroup in another Region",
            format!("arn:aws:athena:eu-west-1:{account}:workgroup/{HELD_WORK_GROUP}"),
        ),
    ];
    let router = router();
    let (status, body) = call(
        &router,
        "CreateWorkGroup",
        &json!({ "Name": HELD_WORK_GROUP }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateWorkGroup({HELD_WORK_GROUP}) must be accepted, got {status} with {body:?}"
    );

    for (case, resource_arn) in unresolvable {
        let stated = could_not_find_resource(&resource_arn);
        for operation in OPERATIONS {
            let request = json!({
                "ResourceARN": resource_arn,
                "Tags": [{ "Key": "owner", "Value": "analytics" }],
                "TagKeys": ["owner"],
            });
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}/{operation}: expected {resource_arn:?} to be refused with \
                 400, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                RESOURCE_NOT_FOUND,
                "case {case:?}/{operation}: expected the code {RESOURCE_NOT_FOUND}, got {body:?}"
            );
            assert_eq!(
                message(&body),
                stated,
                "case {case:?}/{operation}: expected the message {stated:?}, got {body:?}"
            );
            assert_eq!(
                body["ResourceName"].as_str(),
                Some(resource_arn.as_str()),
                "case {case:?}/{operation}: expected ResourceName to name the addressed \
                 resource {resource_arn:?}, got {body:?}"
            );
        }
    }
}

/// The workgroup the miss table holds, so a row that changes only the ARN's account or
/// Region is a miss because of THAT and not because the workgroup is absent.
const HELD_WORK_GROUP: &str = "held_work_group";

/// RULE: a `ResourceARN` naming ANOTHER account's resource is refused ON THE REQUEST — this
/// service's own `InvalidRequestException` stating that the ARN's account is not the
/// caller's — rather than reported as a resource that happens to be missing, so the refusal
/// discloses nothing about whether the named resource exists.
///
/// The rows differ along the resource type the foreign ARN names and whether this account
/// holds a resource of that name at all, and each is asserted on all three operations.
#[tokio::test]
async fn an_arn_naming_another_accounts_resource_is_a_bad_request() {
    let foreign_account = "111122223333";
    let router = router();
    let (status, body) = call(
        &router,
        "CreateWorkGroup",
        &json!({ "Name": HELD_WORK_GROUP }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateWorkGroup({HELD_WORK_GROUP}) must be accepted, got {status} with {body:?}"
    );

    let foreign: [(&str, String); 3] = [
        (
            "a workgroup this account holds under that name",
            format!("arn:aws:athena:{REGION}:{foreign_account}:workgroup/{HELD_WORK_GROUP}"),
        ),
        (
            "a workgroup no account here holds",
            format!("arn:aws:athena:{REGION}:{foreign_account}:workgroup/absent_resource"),
        ),
        (
            "a data catalog in another account",
            format!("arn:aws:athena:{REGION}:{foreign_account}:datacatalog/sales_catalog"),
        ),
    ];
    for (case, resource_arn) in foreign {
        for operation in OPERATIONS {
            let request = json!({
                "ResourceARN": resource_arn,
                "Tags": [{ "Key": "owner", "Value": "analytics" }],
                "TagKeys": ["owner"],
            });
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}/{operation}: expected {resource_arn:?} to be refused with \
                 400, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_REQUEST,
                "case {case:?}/{operation}: expected the code {INVALID_REQUEST}, got {body:?}"
            );
            assert_eq!(
                message(&body),
                ARN_ACCOUNT_MISMATCH,
                "case {case:?}/{operation}: expected the message {ARN_ACCOUNT_MISMATCH:?}, \
                 got {body:?}"
            );
        }
    }

    // The workgroup the foreign ARNs named still holds nothing, so none of those calls
    // reached a resource of this account.
    let held = held_tags(&router, &arn(WORK_GROUP_RESOURCE_TYPE, HELD_WORK_GROUP)).await;
    assert_eq!(
        held,
        Vec::<(String, String)>::new(),
        "expected a refused cross-account write to have reached nothing, got {held:?}"
    );
}

/// RULE: a tag key inside the namespace Amazon Web Services reserves
/// (`RESERVED_TAG_KEY_PREFIX`, matched however it is cased) is neither a caller's to write
/// nor a caller's to remove, so every call naming one is refused with this service's own
/// `InvalidRequestException` and the resource's tags are left exactly as they were. A key
/// that merely CONTAINS the prefix, or begins with its letters without the colon, is
/// outside that namespace and is accepted.
///
/// The rows differ along the spelling and case of the key, and each is asserted on the tag
/// write, the tag removal and all three creates that take a `TagList`.
#[tokio::test]
async fn a_tag_key_in_the_reserved_namespace_is_refused_wherever_it_is_named() {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    let reserved: Vec<String> = vec![
        prefix.to_string(),
        format!("{prefix}cloudformation:stack-name"),
        format!("{}BILLING", prefix.to_uppercase()),
        format!("{}Region", capitalized(prefix)),
    ];
    let accepted: Vec<String> = vec![
        format!("mine-{prefix}thing"),
        prefix.trim_end_matches(':').to_string(),
        format!("{}-owner", prefix.trim_end_matches(':')),
    ];
    let router = router();
    let work_group = "reserved_keys_wg";
    let (status, body) = call(&router, "CreateWorkGroup", &json!({ "Name": work_group })).await;
    assert_eq!(
        status, 200,
        "CreateWorkGroup({work_group}) must be accepted, got {status} with {body:?}"
    );
    let resource_arn = arn(WORK_GROUP_RESOURCE_TYPE, work_group);

    for key in &reserved {
        let stated = reserved_tag_key_message();
        let (status, body) = tag(&router, &resource_arn, &[(key, "anything")]).await;
        assert_eq!(
            status, 400,
            "case {key:?}: expected TagResource to refuse a reserved key with 400, got \
             {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case {key:?}: expected the code {INVALID_REQUEST} from TagResource, got {body:?}"
        );
        assert_eq!(
            message(&body),
            stated,
            "case {key:?}: expected the message {stated:?} from TagResource, got {body:?}"
        );

        let (status, body) = untag(&router, &resource_arn, &[key.as_str()]).await;
        assert_eq!(
            status, 400,
            "case {key:?}: expected UntagResource to refuse a reserved key with 400, got \
             {status} with {body:?}"
        );
        assert_eq!(
            message(&body),
            stated,
            "case {key:?}: expected the message {stated:?} from UntagResource, got {body:?}"
        );

        // Every create that takes a `TagList` judges the key the same way.
        for (operation, request) in creates_carrying(key) {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {key:?}/{operation}: expected a reserved key to be refused with 400, \
                 got {status} with {body:?}"
            );
            assert_eq!(
                message(&body),
                stated,
                "case {key:?}/{operation}: expected the message {stated:?}, got {body:?}"
            );
        }

        let held = held_tags(&router, &resource_arn).await;
        assert_eq!(
            held,
            Vec::<(String, String)>::new(),
            "case {key:?}: a refused write stores nothing, got {held:?}"
        );
    }

    for key in &accepted {
        let (status, body) = tag(&router, &resource_arn, &[(key, "mine")]).await;
        assert_eq!(
            status, 200,
            "case {key:?}: a key outside the reserved namespace must be accepted, got \
             {status} with {body:?}"
        );
        let held = held_tags(&router, &resource_arn).await;
        assert!(
            held.contains(&(key.clone(), "mine".to_string())),
            "case {key:?}: expected the accepted key to be held, got {held:?}"
        );
        let (status, body) = untag(&router, &resource_arn, &[key.as_str()]).await;
        assert_eq!(
            status, 200,
            "case {key:?}: a key outside the reserved namespace must be removable, got \
             {status} with {body:?}"
        );
    }
}

/// `prefix` with its first character upper-cased, which is how a caller spells a reserved
/// prefix it believes to be case-sensitive.
fn capitalized(prefix: &str) -> String {
    let mut characters = prefix.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// Every create of this service that takes a `TagList`, each carrying the one tag `key`, so
/// one key can be judged on every path that writes tags.
fn creates_carrying(key: &str) -> Vec<(&'static str, Value)> {
    let tags = json!([{ "Key": key, "Value": "anything" }]);
    vec![
        (
            "CreateWorkGroup",
            json!({ "Name": "wg_reserved_create", "Tags": tags }),
        ),
        (
            "CreateDataCatalog",
            json!({
                "Name": "catalog_reserved_create",
                "Type": "LAMBDA",
                "Parameters": { "function": "arn:aws:lambda:us-east-1:111122223333:function:meta" },
                "Tags": tags
            }),
        ),
        (
            "CreateCapacityReservation",
            json!({ "Name": "reserved-create", "TargetDpus": 24, "Tags": tags }),
        ),
    ]
}

/// RULE: a resource holds at most `MAX_TAGS_PER_RESOURCE` tags and one key once, so a write
/// carrying more tags than that, or the same key twice, is refused — and a refused write
/// leaves the resource's tags exactly as they were.
///
/// The rows differ along the number of tags a request carries, at the limit, one past it
/// and far below it, and along where the repeated key sits in the list.
#[tokio::test]
async fn a_write_outside_the_rules_one_resources_tags_obey_is_refused() {
    let router = router();
    let work_group = "tag_bounds_wg";
    let (status, body) = call(&router, "CreateWorkGroup", &json!({ "Name": work_group })).await;
    assert_eq!(
        status, 200,
        "CreateWorkGroup({work_group}) must be accepted, got {status} with {body:?}"
    );
    let resource_arn = arn(WORK_GROUP_RESOURCE_TYPE, work_group);

    // At the documented limit the write is accepted, and every tag of it is held.
    let at_limit: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let borrowed: Vec<(&str, &str)> = at_limit
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (status, body) = tag(&router, &resource_arn, &borrowed).await;
    assert_eq!(
        status, 200,
        "case \"at the limit\": expected {MAX_TAGS_PER_RESOURCE} tags to be accepted, got \
         {status} with {body:?}"
    );
    let held = held_tags(&router, &resource_arn).await;
    assert_eq!(
        held.len(),
        MAX_TAGS_PER_RESOURCE,
        "case \"at the limit\": expected {MAX_TAGS_PER_RESOURCE} tags to be held, got \
         {held:?}"
    );

    // One past it, and a list repeating a key, are refused — and change nothing.
    let one_past: Vec<(String, String)> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("over-{index}"), format!("value-{index}")))
        .collect();
    let one_past: Vec<(&str, &str)> = one_past
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let refused: [(&str, Vec<(&str, &str)>); 3] = [
        ("one tag past the limit", one_past),
        (
            "the same key twice, side by side",
            vec![("owner", "one"), ("owner", "two")],
        ),
        (
            "the same key twice, with another between them",
            vec![("owner", "one"), ("env", "prod"), ("owner", "two")],
        ),
    ];
    for (case, pairs) in refused {
        let (status, body) = tag(&router, &resource_arn, &pairs).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the write to be refused with 400, got {status} with \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case {case:?}: expected the code {INVALID_REQUEST}, got {body:?}"
        );
        let after = held_tags(&router, &resource_arn).await;
        assert_eq!(
            after, held,
            "case {case:?}: a refused write leaves the resource's tags unchanged, got \
             {after:?}"
        );
    }
}

/// RULE: a listing answers with the tags the resource holds and hands back a `NextToken`
/// only where a page left tags behind it; a `NextToken` this service did not mint for THAT
/// resource names no page of the listing and is a bad request.
///
/// The rows differ along the `MaxResults` asked for — absent, at the modelled floor and far
/// above it — and along how a token fails to name a page.
#[tokio::test]
async fn a_listing_pages_the_tags_a_resource_holds() {
    let router = router();
    let work_group = "tag_paging_wg";
    let (status, body) = call(&router, "CreateWorkGroup", &json!({ "Name": work_group })).await;
    assert_eq!(
        status, 200,
        "CreateWorkGroup({work_group}) must be accepted, got {status} with {body:?}"
    );
    let resource_arn = arn(WORK_GROUP_RESOURCE_TYPE, work_group);
    let written: Vec<(String, String)> = (0..12)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let borrowed: Vec<(&str, &str)> = written
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (status, body) = tag(&router, &resource_arn, &borrowed).await;
    assert_eq!(
        status, 200,
        "TagResource must be accepted, got {status} with {body:?}"
    );

    // `MaxResults` has a modelled floor of 75, which is already above the tags one resource
    // can hold, so every page carries everything and none hands back a token.
    for max_results in [Value::Null, json!(75), json!(1000)] {
        let mut request = json!({ "ResourceARN": resource_arn });
        if !max_results.is_null() {
            request["MaxResults"] = max_results.clone();
        }
        let (status, body) = call(&router, "ListTagsForResource", &request).await;
        assert_eq!(
            status, 200,
            "case {max_results:?}: expected the listing to answer 200, got {status} with \
             {body:?}"
        );
        assert_eq!(
            body["Tags"].as_array().map(Vec::len),
            Some(written.len()),
            "case {max_results:?}: expected every one of the {} tags written, got {body:?}",
            written.len()
        );
        assert!(
            body["NextToken"].is_null(),
            "case {max_results:?}: a page that reported every tag hands back no NextToken, \
             got {body:?}"
        );
    }

    // A token nothing minted for this listing names no page of it.
    for (case, token) in [
        ("a token of this service's own form", "00"),
        ("a token that is not even hexadecimal", "not-a-token"),
        ("an empty-looking token", "0123456789abcdef"),
    ] {
        let (status, body) = call(
            &router,
            "ListTagsForResource",
            &json!({ "ResourceARN": resource_arn, "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {token:?} to be refused with 400, got {status} with \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case {case:?}: expected the code {INVALID_REQUEST}, got {body:?}"
        );
        assert_eq!(
            message(&body),
            INVALID_NEXT_TOKEN,
            "case {case:?}: expected the message {INVALID_NEXT_TOKEN:?}, got {body:?}"
        );
    }
}

/// RULE: the documented cap is on the tags the RESOURCE holds, not on the tags one request
/// carries, so a write is refused exactly when the set it would leave behind is larger than
/// `MAX_TAGS_PER_RESOURCE` — a write that only revalues keys the resource already carries
/// takes no further room, and room freed by a removal is room a later write can use.
///
/// The rows differ along what the write would add to a resource already holding the cap: a
/// new key, a key it already carries, and a new key after one was removed.
#[tokio::test]
async fn the_cap_is_on_the_tags_the_resource_holds() {
    let router = router();
    let work_group = "tag_total_wg";
    let (status, body) = call(&router, "CreateWorkGroup", &json!({ "Name": work_group })).await;
    assert_eq!(
        status, 200,
        "CreateWorkGroup({work_group}) must be accepted, got {status} with {body:?}"
    );
    let resource_arn = arn(WORK_GROUP_RESOURCE_TYPE, work_group);

    // Fill the resource to the cap one write at a time, so the resource holds the cap
    // without any single request carrying more than a few tags.
    let filled: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    for chunk in filled.chunks(10) {
        let pairs: Vec<(&str, &str)> = chunk
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let (status, body) = tag(&router, &resource_arn, &pairs).await;
        assert_eq!(
            status, 200,
            "case \"filling to the cap\": expected the write to be accepted, got {status} \
             with {body:?}"
        );
    }
    let held = held_tags(&router, &resource_arn).await;
    assert_eq!(
        held.len(),
        MAX_TAGS_PER_RESOURCE,
        "case \"filling to the cap\": expected {MAX_TAGS_PER_RESOURCE} tags to be held, got \
         {} of them",
        held.len()
    );

    // One more KEY would take the resource past the cap.
    let (status, body) = tag(&router, &resource_arn, &[("one-too-many", "v")]).await;
    assert_eq!(
        status, 400,
        "case \"a new key at the cap\": expected the write to be refused with 400, got \
         {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_REQUEST,
        "case \"a new key at the cap\": expected the code {INVALID_REQUEST}, got {body:?}"
    );

    // Revaluing a key the resource already carries adds nothing to the set.
    let (status, body) = tag(&router, &resource_arn, &[("key-0", "rewritten")]).await;
    assert_eq!(
        status, 200,
        "case \"revaluing a held key at the cap\": expected the write to be accepted, got \
         {status} with {body:?}"
    );
    let after = held_tags(&router, &resource_arn).await;
    assert!(
        after.contains(&("key-0".to_string(), "rewritten".to_string()))
            && after.len() == MAX_TAGS_PER_RESOURCE,
        "case \"revaluing a held key at the cap\": expected the revalued key among \
         {MAX_TAGS_PER_RESOURCE} tags, got {after:?}"
    );

    // Room a removal freed is room a later write can use.
    let (status, body) = untag(&router, &resource_arn, &["key-0"]).await;
    assert_eq!(
        status, 200,
        "case \"making room\": expected UntagResource to answer 200, got {status} with \
         {body:?}"
    );
    let (status, body) = tag(&router, &resource_arn, &[("one-too-many", "v")]).await;
    assert_eq!(
        status, 200,
        "case \"a new key in freed room\": expected the write to be accepted, got {status} \
         with {body:?}"
    );
}

/// RULE: a tag key and a tag value hold letters, numbers and the characters `Tag$Key` and
/// `Tag$Value` name (`TAG_TEXT_PUNCTUATION`), so a request carrying anything else is
/// refused and the resource's tags are left as they were.
///
/// The rows differ along which character is used, whether it sits in the key or the value,
/// and where in the text it sits; the accepted rows cover every character the model names
/// and a letter and a digit outside ASCII, since the documentation says "representable in
/// UTF-8" rather than naming an ASCII range.
#[tokio::test]
async fn a_tag_holds_only_the_characters_the_model_names() {
    let router = router();
    let work_group = "tag_text_wg";
    let (status, body) = call(&router, "CreateWorkGroup", &json!({ "Name": work_group })).await;
    assert_eq!(
        status, 200,
        "CreateWorkGroup({work_group}) must be accepted, got {status} with {body:?}"
    );
    let resource_arn = arn(WORK_GROUP_RESOURCE_TYPE, work_group);

    // Every character the model names, in one key and one value, beside a letter and a
    // digit that are not ASCII.
    let punctuation: String = TAG_TEXT_PUNCTUATION.iter().collect();
    let accepted: Vec<(&str, String, String)> = vec![
        (
            "every character the model names",
            format!("key{punctuation}"),
            format!("value{punctuation}"),
        ),
        (
            "a letter and a digit outside ASCII",
            "clé٣".to_string(),
            "valeur٣".to_string(),
        ),
        ("a bare digit", "0".to_string(), String::new()),
    ];
    for (case, key, value) in &accepted {
        let (status, body) = tag(&router, &resource_arn, &[(key, value)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the tag {key:?}={value:?} to be accepted, got {status} \
             with {body:?}"
        );
        let held = held_tags(&router, &resource_arn).await;
        assert!(
            held.contains(&(key.clone(), value.clone())),
            "case {case:?}: expected the accepted tag to be held, got {held:?}"
        );
    }
    let held = held_tags(&router, &resource_arn).await;

    // A character the model names on neither member.
    let refused: Vec<(&str, String, String)> = vec![
        (
            "a space inside the key",
            "cost center".to_string(),
            "1234".to_string(),
        ),
        (
            "a space inside the value",
            "owner".to_string(),
            "the analytics team".to_string(),
        ),
        (
            "a comma, which separates tags rather than belonging to one",
            "owner,env".to_string(),
            "analytics".to_string(),
        ),
        (
            "a character at the end of the value",
            "owner".to_string(),
            "analytics!".to_string(),
        ),
        (
            "a character at the start of the key",
            "#owner".to_string(),
            "analytics".to_string(),
        ),
    ];
    for (case, key, value) in &refused {
        let (status, body) = tag(&router, &resource_arn, &[(key, value)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the tag {key:?}={value:?} to be refused with 400, got \
             {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case {case:?}: expected the code {INVALID_REQUEST}, got {body:?}"
        );
        let after = held_tags(&router, &resource_arn).await;
        assert_eq!(
            after, held,
            "case {case:?}: a refused write leaves the resource's tags unchanged, got \
             {after:?}"
        );
    }
}
