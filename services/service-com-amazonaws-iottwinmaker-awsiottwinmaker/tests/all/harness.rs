// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every suite in this crate drives the service through.
//!
//! One shared engine with only this service mounted on it, called through
//! `MultiServiceRouter::dispatch` — the same entry the served emulator uses — so
//! a route, a label, a status code and a serialized body are all exercised, not
//! just a handler body.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    Entity, ResourceRef, SyncJob, DEFAULT_REGION, MAX_ENTITIES_PER_WORKSPACE,
    ROOT_PARENT_ENTITY_ID, SERVICE_PRINCIPAL, STATE_ACTIVE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here signs for; it must show up in every minted ARN.
pub(crate) const REGION: &str = "us-west-2";
/// The account segment `engine::emulator_arn` stamps into every ARN.
pub(crate) const ACCOUNT: &str = "000000000000";

/// One dispatched response: status, the restJson1 error code (which this
/// protocol carries in `x-amzn-errortype`), and the parsed JSON body.
#[derive(Debug)]
pub(crate) struct Resp {
    pub(crate) status: u16,
    pub(crate) error_type: Option<String>,
    pub(crate) body: Value,
}

impl Resp {
    pub(crate) fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }
}

/// One emulator: a shared engine with only this service mounted on it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The SAME engine the mounted service holds, kept so a suite can seed or
    /// inspect state an operation this slice has not implemented yet is the only
    /// writer of. Requests still go through [`Emulator::call`].
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests send unsigned-but-scoped requests: the `Authorization`
        // header is what routes them and names the region, not what authorises
        // them.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_iottwinmaker_awsiottwinmaker::EmulatedService::new(engine.clone()),
        );
        Self { router, engine }
    }

    /// One emulator carrying BOTH this service and IAM over the SAME engine.
    ///
    /// A caller-supplied role lives in the IAM store, so a suite about a rule
    /// this service decides against an IAM record — a `syncRole`'s trust policy
    /// — needs IAM mounted beside this service and seeds it through
    /// [`Emulator::create_iam_role`]. Every other suite uses [`Emulator::new`],
    /// which mounts this service alone and leaves the IAM store empty.
    pub(crate) fn with_iam() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_iottwinmaker_awsiottwinmaker::EmulatedService::new(engine.clone()),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                engine.clone(),
            ),
        );
        Self { router, engine }
    }

    /// One emulator carrying BOTH this service and the emulator's own Amazon S3
    /// over the SAME engine — exactly as the assembled router mounts them.
    ///
    /// A metadata transfer job's Amazon S3 end is an object of that store, so a
    /// suite about what a transfer READS out of a document and WRITES back into
    /// one needs the owner of objects mounted beside this service; the document
    /// it reads is then an object a `PutObject` really stored. Every other suite
    /// uses [`Emulator::new`], which mounts this service alone and leaves the
    /// object store absent.
    pub(crate) fn with_object_store() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_iottwinmaker_awsiottwinmaker::EmulatedService::new(engine.clone()),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            engine.clone(),
        ));
        Self { router, engine }
    }

    /// The router every request goes through, for a suite that drives another
    /// mounted service through its own SDK client over the in-process callback.
    pub(crate) fn router(&self) -> &MultiServiceRouter {
        &self.router
    }

    /// Create an IAM role carrying `document` as its trust policy, and answer
    /// the ARN IAM minted for it.
    ///
    /// Driven over IAM's OWN awsQuery wire through the same router, so the role
    /// this crate's rules are decided against is a record a real `CreateRole`
    /// call wrote; the ARN is read off the response rather than constructed, so
    /// it is the ARN IAM actually holds. Only meaningful on an emulator built by
    /// [`Emulator::with_iam`].
    pub(crate) async fn create_iam_role(&self, name: &str, document: &str) -> String {
        let body = format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={}&AssumeRolePolicyDocument={}",
            encode_query(name),
            encode_query(document)
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("https://iam.amazonaws.com/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(bytes::Bytes::from(body))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let xml = String::from_utf8(response.into_body().to_vec()).expect("a UTF-8 response body");
        assert_eq!(
            status, 200,
            "CreateRole {name:?} expected 200, got {status} {xml}"
        );
        let opened = xml
            .find("<Arn>")
            .unwrap_or_else(|| panic!("CreateRole {name:?} returned no <Arn>: {xml}"))
            + "<Arn>".len();
        let closed = xml[opened..]
            .find("</Arn>")
            .unwrap_or_else(|| panic!("CreateRole {name:?} left <Arn> unclosed: {xml}"));
        xml[opened..opened + closed].to_string()
    }

    /// This service's store in the shared engine — the very store the mounted
    /// handlers read and write, reached the way a handler reaches it.
    pub(crate) fn state(
        &self,
    ) -> engine::ServiceState<
        service_com_amazonaws_iottwinmaker_awsiottwinmaker::ComAmazonawsIottwinmakerAwsiottwinmakerState,
    > {
        service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::service_state(&*self.engine)
            .expect("the iottwinmaker store is reachable from the shared engine")
    }

    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Resp {
        self.call_signed_for(Some(REGION), method, path, body).await
    }

    /// One dispatched request signed for `region` — or UNSIGNED when `region` is
    /// `None`, the case the service answers out of its default Region.
    ///
    /// The `Authorization` header is what names the Region (the third field of
    /// its credential scope), so this is the seam a suite about Region scoping
    /// drives: [`Emulator::call`] is this with the suite-wide [`REGION`].
    pub(crate) async fn call_signed_for(
        &self,
        region: Option<&str>,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Resp {
        let payload = match &body {
            Some(value) => {
                bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
            }
            None => bytes::Bytes::new(),
        };
        let host = match region {
            // A signed request routes by its credential scope, so the suites' host
            // keeps the service's `api.` endpoint form.
            Some(region) => format!("api.iottwinmaker.{region}.amazonaws.com"),
            // An unsigned request carries no scope, so it routes by HOST and is
            // served out of the service's default Region: it addresses that
            // Region's own endpoint, which is the endpoint form that routes.
            None => format!("iottwinmaker.{DEFAULT_REGION}.amazonaws.com"),
        };
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json");
        if let Some(region) = region {
            builder = builder.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/iottwinmaker/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            );
        }
        let request = builder.body(payload).expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Resp {
            status,
            error_type,
            body,
        }
    }

    pub(crate) async fn create_workspace(&self, workspace_id: &str) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}"),
            Some(json!({})),
        )
        .await
    }

    pub(crate) async fn get_workspace(&self, id_or_arn: &str) -> Resp {
        self.call("GET", &format!("/workspaces/{id_or_arn}"), None).await
    }

    /// `UpdateWorkspace`: PUT on the workspace's own path, with the id as the
    /// only `@httpLabel` and the members the patch names in the body.
    pub(crate) async fn update_workspace(&self, workspace_id: &str, body: Value) -> Resp {
        self.call("PUT", &format!("/workspaces/{workspace_id}"), Some(body))
            .await
    }

    /// `CreateWorkspace` carrying the members the request names, for a suite
    /// that cares what a workspace was created WITH; [`Emulator::create_workspace`]
    /// is this with an empty body.
    pub(crate) async fn create_workspace_with(&self, workspace_id: &str, body: Value) -> Resp {
        self.call("POST", &format!("/workspaces/{workspace_id}"), Some(body))
            .await
    }

    pub(crate) async fn create_entity(&self, workspace_id: &str, body: Value) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/entities"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn get_entity(&self, workspace_id: &str, entity_id: &str) -> Resp {
        self.call(
            "GET",
            &format!("/workspaces/{workspace_id}/entities/{entity_id}"),
            None,
        )
        .await
    }

    /// `UpdateEntity`: PUT on the entity's own path, with both ids as
    /// `@httpLabel`s and the members the patch names in the body.
    pub(crate) async fn update_entity(
        &self,
        workspace_id: &str,
        entity_id: &str,
        body: Value,
    ) -> Resp {
        self.call(
            "PUT",
            &format!("/workspaces/{workspace_id}/entities/{entity_id}"),
            Some(body),
        )
        .await
    }

    /// `ListComponents`: POST
    /// /workspaces/{workspaceId}/entities/{entityId}/components-list, with both
    /// ids as `@httpLabel`s and `componentPath` and the page members
    /// (`maxResults`, `nextToken`) carried in the body.
    pub(crate) async fn list_components(
        &self,
        workspace_id: &str,
        entity_id: &str,
        body: Value,
    ) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/entities/{entity_id}/components-list"),
            Some(body),
        )
        .await
    }

    /// `ListProperties`: POST /workspaces/{workspaceId}/properties-list, with
    /// the workspace as the only `@httpLabel` and `entityId`, the component
    /// address (`componentName` or `componentPath`) and the page members
    /// (`maxResults`, `nextToken`) carried in the body.
    pub(crate) async fn list_properties(&self, workspace_id: &str, body: Value) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/properties-list"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn list_entities(&self, workspace_id: &str, body: Value) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/entities-list"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn delete_entity(
        &self,
        workspace_id: &str,
        entity_id: &str,
        recursive: bool,
    ) -> Resp {
        let query = if recursive { "?isRecursive=true" } else { "" };
        self.call(
            "DELETE",
            &format!("/workspaces/{workspace_id}/entities/{entity_id}{query}"),
            None,
        )
        .await
    }

    pub(crate) async fn delete_workspace(&self, workspace_id: &str) -> Resp {
        self.call("DELETE", &format!("/workspaces/{workspace_id}"), None)
            .await
    }

    /// `GetPricingPlan`: GET /pricingplan, no members and no body.
    pub(crate) async fn get_pricing_plan(&self) -> Resp {
        self.call("GET", "/pricingplan", None).await
    }

    /// `UpdatePricingPlan`: POST /pricingplan, with `pricingMode` and any
    /// `bundleNames` in the body (the request binds no label and no query).
    pub(crate) async fn update_pricing_plan(&self, body: Value) -> Resp {
        self.call("POST", "/pricingplan", Some(body)).await
    }

    /// `ListWorkspaces`: POST /workspaces-list, with any page members
    /// (`maxResults`, `nextToken`) carried in the body.
    pub(crate) async fn list_workspaces(&self, request: Value) -> Resp {
        self.call("POST", "/workspaces-list", Some(request)).await
    }

    pub(crate) async fn execute_query(&self, body: Value) -> Resp {
        self.call("POST", "/queries/execution", Some(body)).await
    }

    pub(crate) async fn create_component_type(
        &self,
        workspace_id: &str,
        component_type_id: &str,
        body: Value,
    ) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/component-types/{component_type_id}"),
            Some(body),
        )
        .await
    }

    /// `UpdateComponentType`: PUT on the component type's own path, with the
    /// members the patch names in the body.
    pub(crate) async fn update_component_type(
        &self,
        workspace_id: &str,
        component_type_id: &str,
        body: Value,
    ) -> Resp {
        self.call(
            "PUT",
            &format!("/workspaces/{workspace_id}/component-types/{component_type_id}"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn get_component_type(
        &self,
        workspace_id: &str,
        component_type_id: &str,
    ) -> Resp {
        self.call(
            "GET",
            &format!("/workspaces/{workspace_id}/component-types/{component_type_id}"),
            None,
        )
        .await
    }

    /// `ListComponentTypes`: POST /workspaces/{workspaceId}/component-types-list,
    /// with the page and filter members carried in the body.
    pub(crate) async fn list_component_types(&self, workspace_id: &str, body: Value) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/component-types-list"),
            Some(body),
        )
        .await
    }

    /// `DeleteComponentType`: DELETE on the component type's own path, with both
    /// ids carried as `@httpLabel`s and no body.
    pub(crate) async fn delete_component_type(
        &self,
        workspace_id: &str,
        component_type_id: &str,
    ) -> Resp {
        self.call(
            "DELETE",
            &format!("/workspaces/{workspace_id}/component-types/{component_type_id}"),
            None,
        )
        .await
    }

    /// `CreateScene`: POST /workspaces/{workspaceId}/scenes, with `sceneId` and
    /// every other member in the body (only `workspaceId` is an `@httpLabel`).
    pub(crate) async fn create_scene(&self, workspace_id: &str, body: Value) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/scenes"),
            Some(body),
        )
        .await
    }

    /// `GetScene`: GET on the scene's own path, with both ids as `@httpLabel`s
    /// and no body.
    pub(crate) async fn get_scene(&self, workspace_id: &str, scene_id: &str) -> Resp {
        self.call(
            "GET",
            &format!("/workspaces/{workspace_id}/scenes/{scene_id}"),
            None,
        )
        .await
    }

    /// `UpdateScene`: PUT on the scene's own path, with both ids as
    /// `@httpLabel`s and the members the patch names in the body.
    pub(crate) async fn update_scene(
        &self,
        workspace_id: &str,
        scene_id: &str,
        body: Value,
    ) -> Resp {
        self.call(
            "PUT",
            &format!("/workspaces/{workspace_id}/scenes/{scene_id}"),
            Some(body),
        )
        .await
    }

    /// `ListScenes`: POST /workspaces/{workspaceId}/scenes-list, with the
    /// workspace as the only `@httpLabel` and the page members (`maxResults`,
    /// `nextToken`) carried in the body.
    pub(crate) async fn list_scenes(&self, workspace_id: &str, body: Value) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/scenes-list"),
            Some(body),
        )
        .await
    }

    /// `DeleteScene`: DELETE on the scene's own path, with both ids as
    /// `@httpLabel`s and no body.
    pub(crate) async fn delete_scene(&self, workspace_id: &str, scene_id: &str) -> Resp {
        self.call(
            "DELETE",
            &format!("/workspaces/{workspace_id}/scenes/{scene_id}"),
            None,
        )
        .await
    }

    /// `CreateMetadataTransferJob`: POST /metadata-transfer-jobs, with every
    /// member (including the optional `metadataTransferJobId`) in the body.
    pub(crate) async fn create_metadata_transfer_job(&self, body: Value) -> Resp {
        self.call("POST", "/metadata-transfer-jobs", Some(body)).await
    }

    /// `GetMetadataTransferJob`: GET on the job's own path, with the id as the
    /// only `@httpLabel` and no body.
    pub(crate) async fn get_metadata_transfer_job(&self, metadata_transfer_job_id: &str) -> Resp {
        self.call(
            "GET",
            &format!("/metadata-transfer-jobs/{metadata_transfer_job_id}"),
            None,
        )
        .await
    }

    /// `ListMetadataTransferJobs`: POST /metadata-transfer-jobs-list, with the
    /// required end types, any `filters` and the page members all carried in the
    /// body (the request binds no label and no query).
    pub(crate) async fn list_metadata_transfer_jobs(&self, request: Value) -> Resp {
        self.call("POST", "/metadata-transfer-jobs-list", Some(request))
            .await
    }

    /// `CreateSyncJob`: POST
    /// /workspaces/{workspaceId}/sync-jobs/{syncSource}, with both the
    /// workspace and the sync source as `@httpLabel`s and `syncRole`/`tags` in
    /// the body.
    pub(crate) async fn create_sync_job(
        &self,
        workspace_id: &str,
        sync_source: &str,
        body: Value,
    ) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/sync-jobs/{sync_source}"),
            Some(body),
        )
        .await
    }

    /// `GetSyncJob`: GET /sync-jobs/{syncSource}, with the workspace carried in
    /// the `workspace` `@httpQuery` parameter the model binds it to rather than
    /// in the path — and omitted entirely when the caller names none.
    pub(crate) async fn get_sync_job(&self, workspace_id: Option<&str>, sync_source: &str) -> Resp {
        let query = match workspace_id {
            Some(workspace_id) => format!("?workspace={}", encode_query(workspace_id)),
            None => String::new(),
        };
        self.call("GET", &format!("/sync-jobs/{sync_source}{query}"), None)
            .await
    }

    /// `GetPropertyValue`: POST
    /// /workspaces/{workspaceId}/entity-properties/value, with the workspace as
    /// the only `@httpLabel` and every other member in the body.
    pub(crate) async fn get_property_value(&self, workspace_id: &str, body: Value) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/entity-properties/value"),
            Some(body),
        )
        .await
    }

    /// `BatchPutPropertyValues`: POST
    /// /workspaces/{workspaceId}/entity-properties, with the workspace as the
    /// only `@httpLabel` and the `entries` in the body.
    pub(crate) async fn batch_put_property_values(&self, workspace_id: &str, entries: Value) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/entity-properties"),
            Some(json!({"entries": entries})),
        )
        .await
    }

    /// `GetPropertyValueHistory`: POST
    /// /workspaces/{workspaceId}/entity-properties/history, with the workspace
    /// as the only `@httpLabel` and every other member in the body.
    pub(crate) async fn get_property_value_history(
        &self,
        workspace_id: &str,
        body: Value,
    ) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/entity-properties/history"),
            Some(body),
        )
        .await
    }

    /// `DeleteSyncJob`: DELETE on the sync job's own path, with both the
    /// workspace and the sync source as `@httpLabel`s and no body.
    pub(crate) async fn delete_sync_job(&self, workspace_id: &str, sync_source: &str) -> Resp {
        self.call(
            "DELETE",
            &format!("/workspaces/{workspace_id}/sync-jobs/{sync_source}"),
            None,
        )
        .await
    }

    /// `ListSyncJobs`: POST /workspaces/{workspaceId}/sync-jobs-list, with the
    /// workspace as the only `@httpLabel` and the page members (`maxResults`,
    /// `nextToken`) carried in the body.
    pub(crate) async fn list_sync_jobs(&self, workspace_id: &str, body: Value) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/sync-jobs-list"),
            Some(body),
        )
        .await
    }

    /// `ListSyncResources`: POST
    /// /workspaces/{workspaceId}/sync-jobs/{syncSource}/resources-list, with the
    /// workspace and the sync source as `@httpLabel`s and `filters` and the page
    /// members (`maxResults`, `nextToken`) carried in the body.
    pub(crate) async fn list_sync_resources(
        &self,
        workspace_id: &str,
        sync_source: &str,
        body: Value,
    ) -> Resp {
        self.list_sync_resources_signed_for(Some(REGION), workspace_id, sync_source, body)
            .await
    }

    /// `ListSyncResources` signed for `region`, the seam a case about Region
    /// scoping drives; [`Emulator::list_sync_resources`] is this with the
    /// suite-wide [`REGION`].
    pub(crate) async fn list_sync_resources_signed_for(
        &self,
        region: Option<&str>,
        workspace_id: &str,
        sync_source: &str,
        body: Value,
    ) -> Resp {
        self.call_signed_for(
            region,
            "POST",
            &format!("/workspaces/{workspace_id}/sync-jobs/{sync_source}/resources-list"),
            Some(body),
        )
        .await
    }

    /// `TagResource`: POST /tags with the ARN and the tag map in the body.
    pub(crate) async fn tag_resource(&self, resource_arn: &str, tags: Value) -> Resp {
        self.call(
            "POST",
            "/tags",
            Some(json!({"resourceARN": resource_arn, "tags": tags})),
        )
        .await
    }

    /// `ListTagsForResource`: POST /tags-list, with any extra page members
    /// (`maxResults`, `nextToken`) merged into the body.
    pub(crate) async fn list_tags_for_resource(&self, resource_arn: &str, request: Value) -> Resp {
        let mut body = request;
        body["resourceARN"] = json!(resource_arn);
        self.call("POST", "/tags-list", Some(body)).await
    }

    /// `UntagResource`: DELETE /tags, with the ARN and every key as repeated
    /// `@httpQuery` parameters — the binding the model declares for them.
    pub(crate) async fn untag_resource(&self, resource_arn: &str, tag_keys: &[&str]) -> Resp {
        let mut query = format!("?resourceARN={}", encode_query(resource_arn));
        for key in tag_keys {
            query.push_str("&tagKeys=");
            query.push_str(&encode_query(key));
        }
        self.call("DELETE", &format!("/tags{query}"), None).await
    }

    /// `CancelMetadataTransferJob`: PUT on the job's own `cancel` path, with the
    /// job id as the only `@httpLabel` and no body.
    pub(crate) async fn cancel_metadata_transfer_job(&self, metadata_transfer_job_id: &str) -> Resp {
        self.call(
            "PUT",
            &format!("/metadata-transfer-jobs/{metadata_transfer_job_id}/cancel"),
            None,
        )
        .await
    }
}

/// The `syncRole` a seeded sync job carries, and the one a suite that does not
/// care which role it names sends on a create: the modelled `RoleArn` pattern
/// `^arn:((aws)|(aws-cn)|(aws-us-gov)):iam::[0-9]{12}:role/` with this
/// emulator's own account segment.
pub(crate) const SYNC_ROLE: &str = "arn:aws:iam::000000000000:role/TwinMakerSyncRole";

/// The `PricingTier` a billable entity count places a bundled pricing plan in,
/// written out here rather than asked of the service.
///
/// The tier catalogue is the model's own `PricingTier` enum
/// (`smithy_query(query="shape",shape="PricingTier")`, mirrored in
/// `ops::types::PRICING_TIER_VALUES`); the counts are the boundaries of the
/// bands this emulator bills those tiers over, so each row sits exactly ON a
/// boundary or one entity PAST it, and the last row is past the top band.
pub(crate) const TIER_BY_ENTITY_COUNT: &[(usize, &str)] = &[
    (0, "TIER_1"),
    (1, "TIER_1"),
    (100, "TIER_1"),
    (101, "TIER_2"),
    (1_000, "TIER_2"),
    (1_001, "TIER_3"),
    (10_000, "TIER_3"),
    (10_001, "TIER_4"),
];

/// The smallest billable entity count [`TIER_BY_ENTITY_COUNT`] places in `tier`.
pub(crate) fn entity_count_in_tier(tier: &str) -> usize {
    TIER_BY_ENTITY_COUNT
        .iter()
        .find(|(_, banded)| *banded == tier)
        .map(|(count, _)| *count)
        .unwrap_or_else(|| panic!("no billable entity count is banded in {tier:?}"))
}

/// Give the account exactly `count` billable entities in [`REGION`], and answer
/// the ids of the workspaces holding them.
///
/// The workspaces are created through `CreateWorkspace` over HTTP, so they are
/// the records that operation writes; the entities themselves go straight into
/// the store the handlers read, exactly as `CreateEntity` writes one — the
/// record under its id and its ARN in the service's index — because the counts
/// a tier band is bounded by run into the thousands and one HTTP call each would
/// say nothing more about the rule under test. No workspace is given more
/// entities than the service's own ceiling admits.
///
/// Called again with a larger `count`, it GROWS the account to that count: the
/// workspaces and the entity records it already seeded are left where they are.
pub(crate) async fn seed_entities(emulator: &Emulator, count: usize) -> Vec<String> {
    let per_workspace = MAX_ENTITIES_PER_WORKSPACE;
    let workspace_ids: Vec<String> = (0..count.div_ceil(per_workspace))
        .map(|index| format!("seededEntities{index}"))
        .collect();
    for workspace_id in &workspace_ids {
        let already_there = {
            let state = emulator.state();
            let state = state.lock();
            state.region(REGION).workspaces.contains_key(workspace_id)
        };
        if already_there {
            continue;
        }
        let resp = emulator.create_workspace(workspace_id).await;
        assert_eq!(
            resp.status, 200,
            "seeding {count} entities: expected 200 from CreateWorkspace {workspace_id:?}, \
             got {resp:?}"
        );
    }

    let state = emulator.state();
    let mut state = state.lock();
    let state = state.region_mut(REGION);
    for index in 0..count {
        let workspace_id = workspace_ids[index / per_workspace].clone();
        let entity_id = format!("seeded{index}");
        let arn = entity_arn(&workspace_id, &entity_id);
        let now = SystemTime::now();
        let workspace = state
            .workspaces
            .get_mut(&workspace_id)
            .unwrap_or_else(|| panic!("the workspace {workspace_id:?} must exist before seeding"));
        workspace.entities.insert(
            entity_id.clone(),
            Entity {
                entity_id: entity_id.clone(),
                entity_name: entity_id.clone(),
                arn: arn.clone(),
                workspace_id: workspace_id.clone(),
                description: String::new(),
                parent_entity_id: ROOT_PARENT_ENTITY_ID.to_string(),
                components: BTreeMap::new(),
                tags: BTreeMap::new(),
                creation_date_time: now,
                update_date_time: now,
                state: STATE_ACTIVE.to_string(),
                settles_at: now,
            },
        );
        state.arn_index.insert(
            arn,
            ResourceRef::Entity {
                workspace_id,
                entity_id,
            },
        );
    }
    workspace_ids
}

/// Store one sync job for `sync_source` in `workspace_id`, written into the very
/// store the handlers read, exactly as `CreateSyncJob` writes one: the job
/// record under its source, and its ARN in the service's index.
///
/// Used for the sources the create path refuses (it supports one), so a suite
/// can build the multi-job workspace a listing or an isolation rule is about.
pub(crate) fn seed_sync_job(emulator: &Emulator, workspace_id: &str, sync_source: &str) {
    let arn = sync_job_arn(workspace_id, sync_source);
    let state = emulator.state();
    let mut state = state.lock();
    // Seeded into the store of the Region every request from this harness is
    // signed for, the one the handlers serve those requests out of.
    let state = state.region_mut(REGION);
    let workspace = state
        .workspaces
        .get_mut(workspace_id)
        .unwrap_or_else(|| panic!("the workspace {workspace_id:?} must exist before it is seeded"));
    workspace.sync_jobs.insert(
        sync_source.to_string(),
        SyncJob::accepted(
            sync_source.to_string(),
            arn.clone(),
            workspace_id.to_string(),
            SYNC_ROLE.to_string(),
            BTreeMap::new(),
            SystemTime::now(),
        ),
    );
    state.arn_index.insert(
        arn,
        ResourceRef::SyncJob {
            workspace_id: workspace_id.to_string(),
            sync_source: sync_source.to_string(),
        },
    );
}

pub(crate) fn workspace_arn(workspace_id: &str) -> String {
    format!("arn:aws:iottwinmaker:{REGION}:{ACCOUNT}:workspace/{workspace_id}")
}

/// The `functions` map a component type declares its WRITE CONNECTOR in — the
/// `dataWriter` entry `BatchPutPropertyValues` sets a property's values through,
/// so a component type a suite writes values to has to declare it.
///
/// The name comes from the service's own catalogue of data-connector functions
/// rather than being spelled here, and the connector is a Lambda one, which is
/// the form the `DataConnector` shape documents for a caller's own connector
/// (`smithy_query(query="shape",shape="DataConnector")`).
pub(crate) fn data_writer_functions() -> Value {
    let writer = service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::DATA_WRITER_FUNCTION;
    json!({
        writer: {
            "scope": "ENTITY",
            "implementedBy": {
                "lambda": {"arn": format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:writer")},
            },
        },
    })
}

pub(crate) fn entity_arn(workspace_id: &str, entity_id: &str) -> String {
    format!("arn:aws:iottwinmaker:{REGION}:{ACCOUNT}:workspace/{workspace_id}/entity/{entity_id}")
}

pub(crate) fn component_type_arn(workspace_id: &str, component_type_id: &str) -> String {
    format!(
        "arn:aws:iottwinmaker:{REGION}:{ACCOUNT}:workspace/{workspace_id}/componentType/\
         {component_type_id}"
    )
}

/// The `componentTypeId`s of the built-in component types every workspace is
/// PROVISIONED with when it is created, in id order.
///
/// Read off the service's own catalogue rather than restated here, so a suite
/// cannot drift from what `CreateWorkspace` seeds: a built-in added to or
/// dropped from the catalogue moves these expectations with it.
pub(crate) fn built_in_component_type_ids() -> Vec<String> {
    service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::built_in_component_types(
        REGION,
        "anyWorkspace",
        std::time::SystemTime::now(),
    )
    .into_keys()
    .collect()
}

/// `component_type_ids` together with every built-in id, in `componentTypeId`
/// order — what a listing of a workspace holding those component types reports,
/// since a workspace carries the service's built-ins beside whatever a caller
/// created in it.
pub(crate) fn with_built_in_component_types<I>(component_type_ids: I) -> Vec<String>
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let mut ids: Vec<String> = component_type_ids.into_iter().map(Into::into).collect();
    ids.extend(built_in_component_type_ids());
    ids.sort();
    ids
}

pub(crate) fn scene_arn(workspace_id: &str, scene_id: &str) -> String {
    format!("arn:aws:iottwinmaker:{REGION}:{ACCOUNT}:workspace/{workspace_id}/scene/{scene_id}")
}

/// A sync job's ARN: nested under its workspace's resource path, with the sync
/// SOURCE in place of an id — a sync job has none, the source being its
/// identity inside the workspace.
pub(crate) fn sync_job_arn(workspace_id: &str, sync_source: &str) -> String {
    format!(
        "arn:aws:iottwinmaker:{REGION}:{ACCOUNT}:workspace/{workspace_id}/sync-job/{sync_source}"
    )
}

/// A metadata transfer job's ARN: an account-level resource, so — unlike a
/// scene's — it nests under no workspace.
pub(crate) fn metadata_transfer_job_arn(metadata_transfer_job_id: &str) -> String {
    format!(
        "arn:aws:iottwinmaker:{REGION}:{ACCOUNT}:metadata-transfer-job/{metadata_transfer_job_id}"
    )
}

/// A trust policy with one statement, written as a client would send it.
pub(crate) fn trust_policy(effect: &str, action: &str, service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":{action}}}]}}"
    )
}

/// A trust policy granting THIS service `sts:AssumeRole` in the plainest form,
/// which is the document a role every rule below admits carries.
pub(crate) fn admitting_trust_policy() -> String {
    trust_policy(
        "Allow",
        "\"sts:AssumeRole\"",
        &format!("\"{SERVICE_PRINCIPAL}\""),
    )
}

/// The trust documents the one assume-role rule generalises over, and whether a
/// role carrying one may be used: `(case, document, admitted)`.
///
/// ONE table for every suite about that rule — `tests/all/sync_job_role_trust.rs`
/// for a sync job's role member, `tests/all/workspace_role_trust.rs` for a
/// workspace's — because one rule decides every member of this service that
/// names a role (`state::assume_role_refusal`), so two tables could drift apart
/// and stop testing the same forms.
///
/// The rows run over every form of the question: the trust forms IAM admits (the
/// action and the principal as strings or as lists, the wildcards, the casing),
/// the forms that withhold the grant (another principal, an explicit deny,
/// another action, a statement list granting nothing) and the case that is no
/// evidence either way — `None` for the document, the row whose ARN names no
/// role of this account at all, which is admitted.
///
/// There is no row for a trust document that is not a policy at all: IAM itself
/// refuses `CreateRole` for one ("AssumeRolePolicyDocument must be a JSON object
/// carrying a Statement element"), so no role of this account can carry such a
/// document.
pub(crate) fn trust_policy_rows() -> Vec<(&'static str, Option<String>, bool)> {
    let principal = format!("\"{SERVICE_PRINCIPAL}\"");
    vec![
        (
            "the action and the principal as strings",
            Some(admitting_trust_policy()),
            true,
        ),
        (
            "both as lists",
            Some(trust_policy(
                "Allow",
                "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                &format!("[\"ec2.amazonaws.com\",\"{SERVICE_PRINCIPAL}\"]"),
            )),
            true,
        ),
        (
            "a service wildcard in the action",
            Some(trust_policy("Allow", "\"sts:*\"", &principal)),
            true,
        ),
        (
            "every action",
            Some(trust_policy("Allow", "\"*\"", &principal)),
            true,
        ),
        (
            "the principal in another casing",
            Some(trust_policy(
                "Allow",
                "\"STS:ASSUMEROLE\"",
                "\"IoTTwinMaker.amazonaws.com\"",
            )),
            true,
        ),
        (
            "another service's principal only",
            Some(trust_policy(
                "Allow",
                "\"sts:AssumeRole\"",
                "\"ec2.amazonaws.com\"",
            )),
            false,
        ),
        (
            "an explicit deny",
            Some(trust_policy("Deny", "\"sts:AssumeRole\"", &principal)),
            false,
        ),
        (
            "an action that is not assuming the role",
            Some(trust_policy("Allow", "\"s3:GetObject\"", &principal)),
            false,
        ),
        (
            "a statement list granting nothing",
            Some("{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string()),
            false,
        ),
        ("a role this account does not hold", None, true),
    ]
}

/// The ARN of a role named `name` carrying `document`, or — when the row names
/// no document — a modelled `RoleArn` the IAM records hold no role for.
pub(crate) async fn trusted_role_arn(
    emulator: &Emulator,
    name: &str,
    document: Option<&String>,
) -> String {
    match document {
        Some(document) => emulator.create_iam_role(name, document).await,
        None => format!("arn:aws:iam::{ACCOUNT}:role/unknown-to-iam-{name}"),
    }
}

/// Percent-encode a value for use as a query-parameter value, the way an SDK
/// does: everything outside RFC 3986's unreserved set is escaped, so a tag key
/// carrying `:`, `/`, `=`, `+` or a space survives the round trip.
pub(crate) fn encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// Percent-encode a value for use as a non-greedy `@httpLabel`, the way an SDK
/// does: `:` and `/` are reserved in a path segment, so an ARN passed as the
/// `IdOrArn` label arrives encoded.
pub(crate) fn encode_label(value: &str) -> String {
    value.replace('%', "%25").replace(':', "%3A").replace('/', "%2F")
}
