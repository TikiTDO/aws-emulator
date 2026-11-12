// SPDX-License-Identifier: CC0-1.0

//! Folders: the create and the read that reports it, over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: a folder is identified by `(AwsAccountId, FolderId)`;
//! `CreateFolder` persists what the call submitted — reporting the folder's ARN and id —
//! defaulting the folder type and the sharing model the model gives defaults for,
//! nesting the folder under the folder its `ParentFolderArn` names so that its ancestor
//! path is the parent's path followed by the parent, and refusing a second create of the
//! same id; `DescribeFolder` answers from that stored record. So the rows below vary what
//! the rule generalises over (the id's form, whether a name is given and how long it is,
//! the folder type, the sharing model, the tags and permissions submitted, the nesting
//! depth and the account) rather than re-running one recorded call.

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the test client signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";

fn folder_arn(account: &str, folder_id: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:folder/{folder_id}")
}

async fn create_folder(
    harness: &Harness,
    account: &str,
    folder_id: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/folders/{folder_id}"),
            Some(body),
        )
        .await
}

async fn describe_folder(harness: &Harness, account: &str, folder_id: &str) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!("/accounts/{account}/folders/{folder_id}"),
            None,
        )
        .await
}

/// One row of the create-then-read table.
struct Row {
    /// The case label every assertion names.
    case: &'static str,
    account: &'static str,
    /// The `FolderId` the create names; the model admits len 1..2048 over the pattern
    /// `^[\w\-]+$`.
    /// Source: smithy_query(query="operation", operation="CreateFolder").
    folder_id: String,
    /// The `Name` the create submits, if any — `FolderName` is len 1..200 and the
    /// member is optional.
    name: Option<String>,
    /// The `FolderType` the create submits, if any. The model's `FolderType` enum
    /// admits SHARED and RESTRICTED, and documents SHARED as the default.
    /// Source: smithy_query(query="shape", shape="FolderType").
    folder_type: Option<&'static str>,
    /// The `SharingModel` the create submits, if any. The model's `SharingModel` enum
    /// admits ACCOUNT and NAMESPACE, and documents ACCOUNT as the default.
    /// Source: smithy_query(query="shape", shape="SharingModel").
    sharing_model: Option<&'static str>,
    tags: Option<Value>,
    permissions: Option<Value>,
    /// What the folder's reported `FolderType` and `SharingModel` must be: the
    /// submitted value, or the model's documented default when none was submitted.
    expected_folder_type: &'static str,
    expected_sharing_model: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            // Nothing beyond the id: both enums fall back to their documented default
            // and the folder has no display name to report.
            case: "defaults-only",
            account: ACCOUNT,
            folder_id: "reports".to_string(),
            name: None,
            folder_type: None,
            sharing_model: None,
            tags: None,
            permissions: None,
            expected_folder_type: "SHARED",
            expected_sharing_model: "ACCOUNT",
        },
        Row {
            case: "restricted-with-tags-and-permissions",
            account: ACCOUNT,
            folder_id: "finance_eu-west-2".to_string(),
            name: Some("Finance (EU)".to_string()),
            folder_type: Some("RESTRICTED"),
            sharing_model: Some("ACCOUNT"),
            tags: Some(json!([
                { "Key": "team", "Value": "finance" },
                { "Key": "cost-centre", "Value": "4711" },
            ])),
            permissions: Some(json!([{
                "Principal": format!("arn:aws:quicksight:{REGION}:{ACCOUNT}:user/default/analyst"),
                "Actions": ["quicksight:DescribeFolder", "quicksight:CreateFolder"],
            }])),
            expected_folder_type: "RESTRICTED",
            expected_sharing_model: "ACCOUNT",
        },
        Row {
            // The other member of the `SharingModel` enum, which the issue's own call
            // did not exercise.
            case: "namespace-sharing-model",
            account: ACCOUNT,
            folder_id: "shared-with-namespace".to_string(),
            name: Some("Namespace scoped".to_string()),
            folder_type: Some("SHARED"),
            sharing_model: Some("NAMESPACE"),
            tags: None,
            permissions: None,
            expected_folder_type: "SHARED",
            expected_sharing_model: "NAMESPACE",
        },
        Row {
            // The shortest id the pattern admits, and the longest name the model does:
            // `FolderName` is len 1..200.
            case: "single-character-id-and-longest-name",
            account: ACCOUNT,
            folder_id: "f".to_string(),
            name: Some("n".repeat(200)),
            folder_type: None,
            sharing_model: Some("ACCOUNT"),
            tags: Some(json!([{ "Key": "edge", "Value": "max-length" }])),
            permissions: None,
            expected_folder_type: "SHARED",
            expected_sharing_model: "ACCOUNT",
        },
        Row {
            // A long id, well past the one the issue cited, still inside len 1..2048.
            case: "long-id",
            account: ACCOUNT,
            folder_id: format!("folder-{}", "a".repeat(300)),
            name: Some("Long id".to_string()),
            folder_type: None,
            sharing_model: None,
            tags: None,
            permissions: None,
            expected_folder_type: "SHARED",
            expected_sharing_model: "ACCOUNT",
        },
        Row {
            // The same id a row above used, in another account: the account is part of
            // the folder's identity.
            case: "other-account",
            account: OTHER_ACCOUNT,
            folder_id: "reports".to_string(),
            name: Some("Reports".to_string()),
            folder_type: Some("RESTRICTED"),
            sharing_model: None,
            tags: None,
            permissions: None,
            expected_folder_type: "RESTRICTED",
            expected_sharing_model: "ACCOUNT",
        },
    ]
}

impl Row {
    fn body(&self) -> Value {
        let mut body = json!({});
        if let Some(name) = &self.name {
            body["Name"] = json!(name);
        }
        if let Some(folder_type) = self.folder_type {
            body["FolderType"] = json!(folder_type);
        }
        if let Some(sharing_model) = self.sharing_model {
            body["SharingModel"] = json!(sharing_model);
        }
        if let Some(tags) = &self.tags {
            body["Tags"] = tags.clone();
        }
        if let Some(permissions) = &self.permissions {
            body["Permissions"] = permissions.clone();
        }
        body
    }
}

#[tokio::test]
async fn create_folder_persists_it_and_describe_reports_it() {
    let harness = Harness::new();

    for row in rows() {
        let case = row.case;
        let folder_id = row.folder_id.as_str();
        let arn = folder_arn(row.account, folder_id);

        let (status, body) = create_folder(&harness, row.account, folder_id, row.body()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(arn),
            "case {case:?}: create reported the wrong Arn: {body}"
        );
        assert_eq!(
            body["FolderId"],
            json!(folder_id),
            "case {case:?}: create reported the wrong FolderId: {body}"
        );
        assert!(
            body["RequestId"]
                .as_str()
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: create reported no RequestId: {body}"
        );

        let (status, body) = describe_folder(&harness, row.account, folder_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with body {body}"
        );
        let reported = &body["Folder"];
        assert_eq!(
            reported["Arn"],
            json!(arn),
            "case {case:?}: describe reported the wrong Arn: {body}"
        );
        assert_eq!(
            reported["FolderId"],
            json!(folder_id),
            "case {case:?}: describe reported the wrong FolderId: {body}"
        );
        assert_eq!(
            reported["Name"],
            row.name.as_ref().map_or(Value::Null, |name| json!(name)),
            "case {case:?}: describe must report the submitted Name, and none when the \
             create submitted none: {body}"
        );
        assert_eq!(
            reported["FolderType"],
            json!(row.expected_folder_type),
            "case {case:?}: describe reported the wrong FolderType: {body}"
        );
        assert_eq!(
            reported["SharingModel"],
            json!(row.expected_sharing_model),
            "case {case:?}: describe reported the wrong SharingModel: {body}"
        );
        assert_eq!(
            reported["FolderPath"],
            Value::Null,
            "case {case:?}: a folder created with no ParentFolderArn is root-level, so \
             it has no ancestor path: {body}"
        );
        let created_time = reported["CreatedTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: describe reported no CreatedTime: {body}");
        });
        assert!(
            created_time > 0.0,
            "case {case:?}: CreatedTime must be the instant the folder was created, got \
             {created_time}"
        );
        assert_eq!(
            reported["LastUpdatedTime"].as_f64(),
            Some(created_time),
            "case {case:?}: a folder nothing has updated was last updated when it was \
             created: {body}"
        );

        // What the create submitted beside the folder itself is stored too, so the
        // reads that report it have something to report.
        {
            let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
            let store = state.lock();
            let record = store
                .folder(row.account, folder_id)
                .unwrap_or_else(|| panic!("case {case:?}: the created folder is not stored"));
            let stored_tags: Vec<Value> = record
                .tags
                .iter()
                .map(|tag| json!({ "Key": tag.key, "Value": tag.value }))
                .collect();
            assert_eq!(
                json!(stored_tags),
                row.tags.clone().unwrap_or_else(|| json!([])),
                "case {case:?}: the submitted Tags are not stored as given"
            );
            let stored_permissions: Vec<Value> = record
                .permissions
                .iter()
                .map(|permission| {
                    json!({ "Principal": permission.principal, "Actions": permission.actions })
                })
                .collect();
            assert_eq!(
                json!(stored_permissions),
                row.permissions.clone().unwrap_or_else(|| json!([])),
                "case {case:?}: the submitted Permissions are not stored as given"
            );
        }

        // A second create of the same id in the same account is refused, whatever it
        // submits, and the folder it refused to overwrite is untouched.
        let (status, body) = create_folder(&harness, row.account, folder_id, json!({})).await;
        assert_eq!(
            status, 409,
            "case {case:?}: re-creating the same folder expected 409, got {status} with \
             {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceExistsException"),
            "case {case:?}: an id already in use is an existing resource: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "Folder {} already exists",
                folder_arn(row.account, folder_id)
            )),
            "case {case:?}: the refusal must name the folder by its ARN: {body}"
        );
        let (status, body) = describe_folder(&harness, row.account, folder_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refused create must leave the folder readable, got \
             {status} with {body}"
        );
        assert_eq!(
            body["Folder"]["FolderType"],
            json!(row.expected_folder_type),
            "case {case:?}: the refused create must not have changed the folder: {body}"
        );
    }
}

#[tokio::test]
async fn folder_identity_is_the_account_and_the_id() {
    let harness = Harness::new();
    let shared = "shared-folder-id";

    for (case, account) in [("first-account", ACCOUNT), ("second-account", OTHER_ACCOUNT)] {
        let (status, body) =
            create_folder(&harness, account, shared, json!({ "Name": "Shared" })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: one folder id per account, so this create expected 200, got \
             {status} with {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(folder_arn(account, shared)),
            "case {case:?}: the ARN must carry the owning account: {body}"
        );
    }

    for (case, account) in [("first-account", ACCOUNT), ("second-account", OTHER_ACCOUNT)] {
        let (status, body) = describe_folder(&harness, account, shared).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Folder"]["Arn"],
            json!(folder_arn(account, shared)),
            "case {case:?}: describe must report that account's own folder: {body}"
        );
    }

    // An id no folder is held under is not found, in an account that created folders
    // and in one that created none.
    for (case, account, folder_id) in [
        ("account-with-folders", ACCOUNT, "never-created"),
        ("account-with-nothing", "123456789012", shared),
    ] {
        let (status, body) = describe_folder(&harness, account, folder_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: describe of a folder that does not exist expected 404, got \
             {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: an id no folder is held under is not found: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "Folder {} is not found",
                folder_arn(account, folder_id)
            )),
            "case {case:?}: the refusal must name the folder that was not found by its \
             ARN: {body}"
        );
    }
}

#[tokio::test]
async fn a_parent_folder_arn_nests_the_new_folder_under_it() {
    let harness = Harness::new();

    // A chain of folders, each created under the one before it: the ancestor path of
    // the folder at depth n is the n ARNs above it, outermost first.
    let depth = qs_state::MAX_FOLDER_ANCESTORS;
    let mut ancestors: Vec<String> = Vec::new();
    for level in 0..=depth {
        let folder_id = format!("level-{level}");
        let case = folder_id.clone();
        let mut body = json!({ "Name": format!("Level {level}") });
        if let Some(parent) = ancestors.last() {
            body["ParentFolderArn"] = json!(parent);
        }
        let (status, response) = create_folder(&harness, ACCOUNT, &folder_id, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a folder at depth {level} expected 200, got {status} with \
             {response}"
        );

        let (status, response) = describe_folder(&harness, ACCOUNT, &folder_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {response}"
        );
        let expected_path = if ancestors.is_empty() {
            Value::Null
        } else {
            json!(ancestors)
        };
        assert_eq!(
            response["Folder"]["FolderPath"], expected_path,
            "case {case:?}: the ancestor path must be every folder above this one, \
             outermost first: {response}"
        );

        ancestors.push(folder_arn(ACCOUNT, &folder_id));
    }

    // One level further would need more ancestors than the modelled `FolderPath` can
    // carry, so it is refused rather than reported short.
    let (status, body) = create_folder(
        &harness,
        ACCOUNT,
        "one-level-too-deep",
        json!({ "ParentFolderArn": ancestors.last().expect("the deepest folder") }),
    )
    .await;
    assert_eq!(
        status, 409,
        "a folder deeper than the modelled path can carry expected 409, got {status} \
         with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("LimitExceededException"),
        "nesting past the modelled depth exceeds a limit: {body}"
    );
    assert_eq!(
        body["Message"],
        json!(format!(
            "A folder can have at most {depth} ancestor folders."
        )),
        "the refusal must name the limit: {body}"
    );

    // And the refused folder was not persisted.
    let (status, body) = describe_folder(&harness, ACCOUNT, "one-level-too-deep").await;
    assert_eq!(
        status, 404,
        "a refused create must persist nothing, got {status} with {body}"
    );
}

#[tokio::test]
async fn a_parent_folder_arn_naming_no_folder_is_not_found() {
    let harness = Harness::new();

    let (status, body) = create_folder(&harness, ACCOUNT, "parent", json!({})).await;
    assert_eq!(status, 200, "the parent create expected 200, got {status} with {body}");
    // A folder of the same id in another account, to show the ARN is resolved in the
    // account the call addresses.
    let (status, body) = create_folder(&harness, OTHER_ACCOUNT, "parent", json!({})).await;
    assert_eq!(
        status, 200,
        "the other account's parent create expected 200, got {status} with {body}"
    );

    // Each row is an ARN that names no folder of this account: an unknown id, another
    // account's folder, another Region's folder, another service's resource, and a
    // string that is no ARN at all.
    let cases = [
        ("unknown-id", folder_arn(ACCOUNT, "no-such-folder")),
        ("other-account", folder_arn(OTHER_ACCOUNT, "parent")),
        (
            "other-region",
            format!("arn:aws:quicksight:eu-west-1:{ACCOUNT}:folder/parent"),
        ),
        (
            "other-resource-type",
            format!("arn:aws:quicksight:{REGION}:{ACCOUNT}:dataset/parent"),
        ),
        ("not-an-arn", "parent".to_string()),
    ];
    for (case, parent_folder_arn) in cases {
        let folder_id = format!("child-of-{case}");
        let (status, body) = create_folder(
            &harness,
            ACCOUNT,
            &folder_id,
            json!({ "ParentFolderArn": parent_folder_arn }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a ParentFolderArn naming no folder expected 404, got \
             {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: an ARN no folder is held under is not found: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "The parent folder {parent_folder_arn} was not found."
            )),
            "case {case:?}: the refusal must name the parent that was not found: {body}"
        );

        let (status, body) = describe_folder(&harness, ACCOUNT, &folder_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the refused create must persist nothing, got {status} with \
             {body}"
        );
    }

    // An empty ParentFolderArn names no parent at all: the model documents it as
    // creating a root-level folder, not as a parent that cannot be found.
    // Source: smithy_query(query="doc", shape="CreateFolderRequest",
    // member="ParentFolderArn").
    let (status, body) = create_folder(
        &harness,
        ACCOUNT,
        "empty-parent-arn",
        json!({ "ParentFolderArn": "" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an empty ParentFolderArn creates a root-level folder, got {status} with {body}"
    );
    let (status, body) = describe_folder(&harness, ACCOUNT, "empty-parent-arn").await;
    assert_eq!(status, 200, "describe expected 200, got {status} with {body}");
    assert_eq!(
        body["Folder"]["FolderPath"],
        Value::Null,
        "a root-level folder has no ancestor path: {body}"
    );
}
