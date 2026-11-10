// SPDX-License-Identifier: CC0-1.0

//! The store of this service's resources: the state struct every operation reaches
//! through [`super::service_state`], what a fresh engine hands a handler, and the
//! per-resource lookups every operation resolves an identifier through.
//!
//! One file for the registries so that resolving a resource — by region, by id, and
//! for an order by the Outpost it was placed on — is stated once: two operations that
//! resolve the same resource differently is how they drift apart. What is DERIVED from
//! the stored resources lives beside it, one file per concern: the capacity an
//! Outpost's hardware is configured with and weighs ([`super::capacity`]), the pricing
//! and ordering checks a quote is answered with ([`super::quotes`]), and the
//! transitions the clock owes together with the conflicts a change is refused on
//! ([`super::lifecycle`]).

use super::*;

/// Per-service state for com-amazonaws-outposts-outpostsolafservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsOutpostsOutpostsolafserviceState {
    /// Every persisted site, keyed by its bare `SiteId` (`os-…`). A minted id is
    /// unique across the whole store, and a site belongs to the region it was
    /// created in, so `GetSite`, `CreateOutpost` and `DeleteSite` resolve one
    /// through [`Self::site`] rather than off this map directly.
    pub sites: BTreeMap<String, SiteRecord>,
    /// Every persisted Outpost, keyed by its bare `OutpostId` (`op-…`). Resolved
    /// through [`Self::outpost`]; see [`Self::sites`].
    pub outposts: BTreeMap<String, OutpostRecord>,
    /// Every persisted capacity task, keyed by its `CapacityTaskId` (`cap-…`).
    ///
    /// A task is reachable only under the Outpost it runs on, so the Outpost's own
    /// region already scopes it (see [`RegionalRecord`]) and `GetCapacityTask`
    /// resolves it by id once that parent has been resolved.
    pub capacity_tasks: BTreeMap<String, CapacityTaskRecord>,
    /// Every persisted quote, keyed by its bare `QuoteId` (`oq-…`). Resolved through
    /// [`Self::quote`]; see [`Self::sites`].
    pub quotes: BTreeMap<String, QuoteRecord>,
    /// Every persisted order, keyed by the `OrderId` (`oo-…`) `CreateOrder` minted
    /// for it. Resolved through [`Self::order`]; see [`Self::sites`].
    pub orders: BTreeMap<String, OrderRecord>,
    /// Every persisted Outpost-server installation connection, keyed by the
    /// `ConnectionId` `StartConnection` minted for it. Resolved through
    /// [`Self::connection`]; see [`Self::sites`].
    ///
    /// A connection installs ONE hardware asset of ONE Outpost, so every stored
    /// connection names hardware the store still holds: the Outpost going away takes
    /// the connections started for its assets with it ([`Self::remove_outpost`]).
    pub connections: BTreeMap<String, ConnectionRecord>,
    /// This store's own tunnel-server public key, or `None` until the first
    /// connection mints it. See [`Self::server_public_key`].
    pub connection_server_public_key: Option<String>,
    /// The orderable-instance-type catalog `ListOrderableInstanceTypes` reports, in
    /// its stable order.
    ///
    /// Reference data rather than a caller's resource: it is seeded in
    /// [`ComAmazonawsOutpostsOutpostsolafserviceState::default`] from
    /// [`orderable_instance_type_catalog`] and no operation mutates it, so every
    /// read of it answers the same catalog.
    pub orderable_instance_types: Vec<OrderableInstanceTypeRecord>,
    /// The instance types this service holds the hardware specifications of, in the
    /// same stable order.
    ///
    /// Reference data, exactly as [`Self::orderable_instance_types`] is, and the
    /// WIDER of the two: it holds every size of every family this service offers
    /// ([`instance_type_specifications`]), while the catalog above publishes the one
    /// size of each family capacity is ordered in. A caller configures and runs any
    /// size its hardware holds, so the vCPU count of a size
    /// ([`Self::published_vcpus`]) and what one Outpost SUPPORTS are read off this
    /// table rather than off the narrower catalog.
    pub instance_type_specifications: Vec<OrderableInstanceTypeRecord>,
    /// The catalog of items `ListCatalogItems` reports, in its stable order.
    ///
    /// Reference data rather than a caller's resource, exactly as
    /// [`Self::orderable_instance_types`] is: it is seeded in
    /// [`ComAmazonawsOutpostsOutpostsolafserviceState::default`] from
    /// [`catalog_item_catalog`] and no operation mutates it, so every read of it
    /// answers the same catalog.
    pub catalog_items: Vec<CatalogItemRecord>,
    /// How many resources this store has created. Doubles as the monotonically
    /// increasing creation order every record records and every page token names,
    /// and as part of each minted id's seed so no two ids collide.
    pub created: u64,
}

/// The store a fresh engine hands a handler: no resources at all, and the
/// orderable-instance-type catalog already seeded, because that catalog is what the
/// service publishes rather than something a caller creates.
impl Default for ComAmazonawsOutpostsOutpostsolafserviceState {
    fn default() -> Self {
        Self {
            sites: BTreeMap::new(),
            outposts: BTreeMap::new(),
            capacity_tasks: BTreeMap::new(),
            quotes: BTreeMap::new(),
            orders: BTreeMap::new(),
            connections: BTreeMap::new(),
            connection_server_public_key: None,
            orderable_instance_types: orderable_instance_type_catalog(),
            instance_type_specifications: instance_type_specifications(),
            catalog_items: catalog_item_catalog(),
            created: 0,
        }
    }
}

impl ComAmazonawsOutpostsOutpostsolafserviceState {
    /// The site `site_id` names in `region`, or `None` when this region holds no
    /// such site. See [`RegionalRecord`].
    pub fn site(&self, region: &str, site_id: &str) -> Option<&SiteRecord> {
        in_region(&self.sites, region, site_id)
    }

    /// The same site, to CHANGE. See [`Self::site`].
    pub fn site_mut(&mut self, region: &str, site_id: &str) -> Option<&mut SiteRecord> {
        in_region_mut(&mut self.sites, region, site_id)
    }

    /// Remove the site `site_id` names in `region` and answer the record removed, or
    /// `None` — leaving the store untouched — when this region holds no such site.
    pub fn remove_site(&mut self, region: &str, site_id: &str) -> Option<SiteRecord> {
        in_region(&self.sites, region, site_id)?;
        self.sites.remove(site_id)
    }

    /// Every site of `region`, in the store's creation order — what `ListSites`
    /// pages through. See [`RegionalRecord`].
    pub fn sites_in<'a>(&'a self, region: &str) -> impl Iterator<Item = &'a SiteRecord> + 'a {
        all_in_region(&self.sites, region)
    }

    /// The Outpost `outpost_id` names in `region`. See [`Self::site`].
    pub fn outpost(&self, region: &str, outpost_id: &str) -> Option<&OutpostRecord> {
        in_region(&self.outposts, region, outpost_id)
    }

    /// The same Outpost, to CHANGE. See [`Self::outpost`].
    pub fn outpost_mut(&mut self, region: &str, outpost_id: &str) -> Option<&mut OutpostRecord> {
        in_region_mut(&mut self.outposts, region, outpost_id)
    }

    /// Remove the Outpost `outpost_id` names in `region`, and with it every record
    /// this store keeps that exists only as part of that Outpost. See
    /// [`Self::remove_site`].
    ///
    /// A record whose whole subject is the Outpost or something inside it has no
    /// existence once the Outpost is gone, so it goes with the Outpost rather than
    /// being left where nothing can reach it: nothing can name it any more, a read of
    /// it would answer for an Outpost the store does not hold, and the slot it holds
    /// in its own map would never come back to the store's ceiling
    /// ([`MAX_RESOURCES`]). This store keeps two such records outside
    /// [`OutpostRecord`] itself:
    ///
    /// * [`Self::capacity_tasks`] — a capacity task is a task OF an Outpost
    ///   ([`CapacityTaskRecord::outpost_id`]), and every capacity-task route resolves
    ///   that Outpost first. Every task left here is finished work: an unfinished one
    ///   is the conflict a delete refuses on ([`Self::unfinished_work_on_outpost`]).
    /// * [`Self::connections`] — a connection is the installation tunnel for ONE
    ///   hardware asset ([`ConnectionRecord::asset_id`]), and an asset belongs to
    ///   exactly one Outpost ([`OutpostRecord::assets`]), so a connection started for
    ///   an asset of this Outpost installs hardware that is going away with it.
    ///
    /// Each is matched the way the operations that read it resolve it: a task by the
    /// Outpost it names, and a connection by the asset it installs within the region
    /// that Outpost's inventory belongs to (see [`RegionalRecord`]).
    pub fn remove_outpost(&mut self, region: &str, outpost_id: &str) -> Option<OutpostRecord> {
        in_region(&self.outposts, region, outpost_id)?;
        let removed = self.outposts.remove(outpost_id)?;
        self.capacity_tasks
            .retain(|_, task| task.outpost_id != outpost_id);
        let installed: BTreeSet<&str> = removed
            .assets
            .iter()
            .filter_map(|asset| asset.asset_id.as_deref())
            .collect();
        self.connections.retain(|_, connection| {
            connection.region != region || !installed.contains(connection.asset_id.as_str())
        });
        Some(removed)
    }

    /// Every Outpost of `region`, in the store's creation order — what
    /// `ListOutposts` pages through. See [`RegionalRecord`].
    pub fn outposts_in<'a>(
        &'a self,
        region: &str,
    ) -> impl Iterator<Item = &'a OutpostRecord> + 'a {
        all_in_region(&self.outposts, region)
    }

    /// Every capacity task of `region`, in no particular order — what
    /// `ListCapacityTasks` pages through.
    ///
    /// A capacity task carries no region of its own: it is a task OF an Outpost, so
    /// the Outpost's region scopes it (see [`RegionalRecord`]), and a task whose
    /// Outpost this region does not hold is none of this region's inventory. Every
    /// stored task has a live parent, because an Outpost going away takes its tasks
    /// with it.
    pub fn capacity_tasks_in<'a>(
        &'a self,
        region: &str,
    ) -> impl Iterator<Item = &'a CapacityTaskRecord> + 'a {
        // The region is copied into the filter for the reason [`all_in_region`]
        // copies it: the iterator then borrows only the store.
        let region = region.to_string();
        self.capacity_tasks
            .values()
            .filter(move |task| self.outpost(region.as_str(), &task.outpost_id).is_some())
    }

    /// The quote `quote_id` names in `region`. See [`Self::site`].
    pub fn quote(&self, region: &str, quote_id: &str) -> Option<&QuoteRecord> {
        in_region(&self.quotes, region, quote_id)
    }

    /// The same quote, to CHANGE. See [`Self::quote`].
    pub fn quote_mut(&mut self, region: &str, quote_id: &str) -> Option<&mut QuoteRecord> {
        in_region_mut(&mut self.quotes, region, quote_id)
    }

    /// Remove the quote `quote_id` names in `region`. See [`Self::remove_site`].
    pub fn remove_quote(&mut self, region: &str, quote_id: &str) -> Option<QuoteRecord> {
        in_region(&self.quotes, region, quote_id)?;
        self.quotes.remove(quote_id)
    }

    /// Every quote of `region`, in the store's creation order — what `ListQuotes`
    /// pages through. See [`RegionalRecord`].
    pub fn quotes_in<'a>(&'a self, region: &str) -> impl Iterator<Item = &'a QuoteRecord> + 'a {
        all_in_region(&self.quotes, region)
    }

    /// The connection `connection_id` names in `region`. See [`Self::site`].
    pub fn connection(&self, region: &str, connection_id: &str) -> Option<&ConnectionRecord> {
        in_region(&self.connections, region, connection_id)
    }

    /// The order `order_id` names in `region`. See [`Self::site`].
    pub fn order(&self, region: &str, order_id: &str) -> Option<&OrderRecord> {
        in_region(&self.orders, region, order_id)
    }

    /// The order `order_id` names in `region` that was PLACED on `outpost_id`, or
    /// `None` when this store holds no such order — whether it holds none under that
    /// id at all, or holds one placed against another Outpost.
    ///
    /// An order is placed ON exactly one Outpost (`Order$OutpostId`, fixed by
    /// `CreateOrder` and never moved), and the hardware it delivers is installed on
    /// that Outpost and on no other. So wherever an `OrderId` arrives on a request
    /// addressed to an Outpost — where the member is the order "of the host
    /// associated with" that Outpost — the order it names has to be one of THAT
    /// Outpost's: an order placed against another Outpost names no hardware here, so
    /// for such a request it is as absent as an id nothing ever minted, and is
    /// refused with the same [`order_not_found`] wording. Stated once here so the
    /// operations that resolve the member in an Outpost's scope cannot disagree about
    /// what it names, exactly as [`Self::order`] states the region scope every order
    /// lookup shares. Scoped to `region` for the reason [`RegionalRecord`] gives.
    pub fn order_of_outpost(
        &self,
        region: &str,
        outpost_id: &str,
        order_id: &str,
    ) -> Option<&OrderRecord> {
        self.order(region, order_id)
            .filter(|record| record.order.outpost_id.as_deref() == Some(outpost_id))
    }

    /// The same order, to CHANGE. See [`Self::order`].
    pub fn order_mut(&mut self, region: &str, order_id: &str) -> Option<&mut OrderRecord> {
        in_region_mut(&mut self.orders, region, order_id)
    }

    /// Every order of `region`, in the store's creation order — what `ListOrders`
    /// pages through. See [`RegionalRecord`], and [`Self::quotes_in`], which is the
    /// same question asked of the other resource a region's inventory holds.
    pub fn orders_in<'a>(&'a self, region: &str) -> impl Iterator<Item = &'a OrderRecord> + 'a {
        all_in_region(&self.orders, region)
    }

    /// The `Order` a read of the stored `record` answers: the order as it is persisted,
    /// with the hardware its fulfilment installed reported on the line items that
    /// hardware was installed for.
    ///
    /// A member whose fact this service only holds once a later step has happened is
    /// absent until that step and reported from the stored fact afterwards — the rule
    /// `OrderFulfilledDate` follows ([`Self::settle_orders`]). The hardware installed
    /// for a line item is the one record of it this store keeps, the assets standing on
    /// the Outpost the order was placed on ([`OutpostRecord::assets`]), so an installed
    /// item is reported from there ([`report_installed_assets`]) and `ListAssets` and a
    /// read of the order can never disagree about which assets are installed. An order
    /// whose Outpost this region holds none of — one since deleted — has no installed
    /// hardware to report.
    pub fn order_report(&self, record: &OrderRecord) -> Order {
        let assets = record
            .order
            .outpost_id
            .as_deref()
            .and_then(|outpost_id| self.outpost(&record.region, outpost_id))
            .map(|outpost| outpost.assets.as_slice())
            .unwrap_or_default();
        let mut order = record.order.clone();
        if let Some(items) = order.line_items.as_deref_mut() {
            report_installed_assets(items, assets);
        }
        order
    }

    /// The hardware configuration this service publishes under the `SkuCode`
    /// `catalog_item_id`, or `None` for a code of that form it publishes nothing
    /// under — which this service holds no published hardware facts for.
    pub fn catalog_item(&self, catalog_item_id: &str) -> Option<&CatalogItemRecord> {
        self.catalog_items
            .iter()
            .find(|record| record.item.catalog_item_id.as_deref() == Some(catalog_item_id))
    }

    /// How many vCPUs of EC2 capacity this service publishes for the catalog item
    /// `catalog_item_id` names, or `None` for a SKU it publishes no configuration
    /// under — a code of the modelled `SkuCode` form that names nothing in the
    /// catalog, which this service holds no published hardware facts for.
    pub fn catalog_item_capacity_vcpus(&self, catalog_item_id: &str) -> Option<i32> {
        self.catalog_item(catalog_item_id)
            .map(|record| record.capacity_vcpus)
    }

    /// How much Amazon S3 storage capacity, in bytes, this service publishes for the
    /// catalog item `catalog_item_id` names, or `None` for a SKU it publishes no
    /// configuration under. See [`Self::catalog_item_capacity_vcpus`].
    pub fn catalog_item_s3_capacity_bytes(&self, catalog_item_id: &str) -> Option<i64> {
        self.catalog_item(catalog_item_id)
            .map(|record| record.s3_capacity_bytes)
    }

    /// The number of DEFAULT vCPUs this service publishes for one instance type, or
    /// `None` for a type it holds no specification for — a figure the emulator does
    /// not have, and never one it invents.
    ///
    /// Read off the one specification table ([`Self::instance_type_specifications`]),
    /// so what any report of this service says about an instance type cannot disagree
    /// with what another says about it. The table is the wider of the two: a size a
    /// caller configures on hardware it already holds is not one capacity is ordered
    /// in, so it is sized from here rather than from the narrower published catalog.
    pub fn published_vcpus(&self, instance_type: &str) -> Option<i32> {
        self.instance_type_specifications
            .iter()
            .find(|record| record.instance_type.instance_type.as_deref() == Some(instance_type))
            .and_then(|record| record.instance_type.vcp_us)
    }

    /// The EC2 capacity the hardware the order `order` is for carries: one entry per
    /// instance family a configuration its line items subscribe to publishes capacity
    /// of, each naming the largest size of that family that hardware runs, in the
    /// order the order's line items name them.
    ///
    /// What an order is FOR is the hardware configurations its line items name, and
    /// what such a configuration carries capacity of is what this service publishes
    /// for it ([`ec2_capacities`]) — the same records the configuration an Outpost
    /// arrived carrying is read off ([`Self::ordered_instance_type_names`]). A line
    /// item naming a catalog item this service publishes no configuration for
    /// contributes nothing, exactly as it contributes nothing to
    /// [`Self::subscribed_capacity_vcpus`]: the capacity of hardware the catalog does
    /// not describe is not a figure the emulator has, and never one it invents — so an
    /// order whose every line item names one carries no published capacity at all, and
    /// the store holds no hardware fact about the order to narrow anything by.
    ///
    /// Asked of ONE named order rather than of an Outpost's inventory, which is why it
    /// does not skip a cancelled order as [`Self::ordered_instance_type_names`] and
    /// [`Self::subscribed_capacity_vcpus`] do: those two are about the hardware an
    /// Outpost HOLDS or pays for, and nothing was ever delivered for a cancelled order,
    /// while what a named order IS FOR is its own line items whatever status it reached.
    pub fn ordered_ec2_capacity(&self, order: &Order) -> Vec<FamilyCapacity<'_>> {
        let mut capacities: Vec<FamilyCapacity<'_>> = Vec::new();
        for capacity in order
            .line_items
            .iter()
            .flatten()
            .filter_map(|item| self.catalog_item(item.catalog_item_id.as_deref()?))
            .flat_map(|record| record.item.ec2_capacities.iter().flatten())
        {
            let Some(family) = capacity.family.as_deref() else {
                continue;
            };
            let entry = FamilyCapacity {
                family,
                // The instance type the capacity names is the join of its family and
                // its size ([`capacity_instance_type`]), which is what the catalog
                // publishes the vCPU figure under.
                max_vcpus: capacity_instance_type(capacity)
                    .and_then(|instance_type| self.published_vcpus(&instance_type)),
            };
            // Two line items can name configurations that carry capacity of one
            // family, which is one fact about that hardware rather than two.
            if !capacities.contains(&entry) {
                capacities.push(entry);
            }
        }
        capacities
    }

    /// The tag map of the resource an Outposts resource ARN names, or `None` when
    /// this region's inventory holds no such resource (see [`RegionalRecord`]).
    ///
    /// A resource's tags live ON that resource, so every tagging operation reads
    /// and writes them through the stored record the ARN names — and it does so
    /// through this one place, so which resource kinds are taggable cannot differ
    /// between the operations that report tags and the ones that change them. A
    /// kind this store does not hold is `None`, which each of them answers as the
    /// not-found error it models.
    pub fn tags(
        &self,
        region: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Option<HashMap<String, String>> {
        let tags = match resource_type {
            SITE_RESOURCE_TYPE => &self.site(region, resource_id)?.site.tags,
            OUTPOST_RESOURCE_TYPE => &self.outpost(region, resource_id)?.outpost.tags,
            _ => return None,
        };
        // `None` from this function means NO SUCH RESOURCE. A resource whose
        // record carries no map carries no tags, which is an EMPTY map (see
        // [`tags_member`]), so the two cannot be confused.
        Some(tags.clone().unwrap_or_default())
    }

    /// The same tag map, to CHANGE: what `TagResource` writes into. See
    /// [`Self::tags`].
    ///
    /// The map is materialised when the stored record carries none, so a tag can
    /// be added to a resource created without any. That is not a change anybody
    /// can observe: [`tags_member`] already reports a resource with no tags as an
    /// empty map either way.
    pub fn tags_mut(
        &mut self,
        region: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Option<&mut HashMap<String, String>> {
        let tags = match resource_type {
            SITE_RESOURCE_TYPE => &mut self.site_mut(region, resource_id)?.site.tags,
            OUTPOST_RESOURCE_TYPE => &mut self.outpost_mut(region, resource_id)?.outpost.tags,
            _ => return None,
        };
        Some(tags.get_or_insert_with(HashMap::new))
    }

    /// Mint a fresh resource id of the form `<prefix>-<17 lowercase hex chars>`,
    /// and return it with the creation sequence to record on the new resource.
    ///
    /// `OutpostId` (`^op-[a-f0-9]{17}$`), `SiteId` (`^os-[a-f0-9]{17}$`),
    /// `QuoteId` (`^oq-[a-f0-9]{17}$`), `OrderId` (`^oo-[a-f0-9]{17}$`),
    /// `LineItemId` (`^ooi-[a-f0-9]{17}$`) and `CapacityTaskId`
    /// (`^cap-[a-f0-9]{17}$`) are each a short literal prefix and exactly 17
    /// lowercase hex characters, so one helper serves them all. `created` advances on every call and is hashed
    /// into the seed with the wall clock and the process id, so repeated calls never
    /// return the same id.
    pub fn mint_id(&mut self, prefix: &str) -> (String, u64) {
        let (digest, sequence) = self.mint_digest(prefix);
        let mut hex = hex::encode(&digest[..9]);
        // 9 bytes encode to 18 characters; the patterns want exactly 17.
        hex.truncate(17);
        (format!("{prefix}-{hex}"), sequence)
    }

    /// Mint a fresh `ConnectionId`, and return it with the creation sequence to
    /// record on the new connection.
    ///
    /// A `ConnectionId` is not a `<prefix>-<17 hex>` identifier like the rest of
    /// this service's: its pattern is base64-shaped
    /// (`^[a-zA-Z0-9+/=]{1,1024}$`), so the id is the padded base64 of the minted
    /// digest — 44 characters of that alphabet. See [`Self::mint_id`].
    pub fn mint_connection_id(&mut self) -> (String, u64) {
        let (digest, sequence) = self.mint_digest("connection");
        (::aws_smithy_types::base64::encode(&digest[..]), sequence)
    }

    /// This store's own tunnel-server public key, minted on first use and then
    /// stable for the life of the store.
    ///
    /// Every connection of one emulator terminates on the SAME tunnel server, so
    /// the key — like the endpoint beside it ([`CONNECTION_SERVER_HOST`]) — is a
    /// property of the store and not of a connection: a caller that starts two
    /// connections is told one server key twice. It is the padded base64 of a
    /// 32-byte digest, which is exactly the form the modelled `WireGuardPublicKey`
    /// declares: 43 characters of the base64 alphabet and the `=` that pads 32
    /// bytes.
    pub fn server_public_key(&mut self) -> String {
        self.connection_server_public_key
            .get_or_insert_with(|| {
                let seed = format!("wireguard-server|{}|{}", std::process::id(), seed_nanos());
                ::aws_smithy_types::base64::encode(&Sha256::digest(seed.as_bytes())[..])
            })
            .clone()
    }

    /// The hardware asset `asset_id` an Outpost of `region` holds, if this store
    /// holds one.
    ///
    /// An `AssetId` names one physical unit of hardware, installed in one rack of
    /// one Outpost, so at most one Outpost holds it: an operation that acts on an
    /// asset of this REGION's inventory, rather than of one named Outpost
    /// ([`Self::asset_of_outpost`]), resolves it here rather than walking the store
    /// itself. The hardware of another region's Outpost is not this region's to act
    /// on (see [`RegionalRecord`]).
    ///
    /// It answers the asset ITSELF and not the Outpost around it, because what an
    /// operation decides about a unit of hardware is decided by what that unit is
    /// ([`is_outpost_server`]) and the record is where that is held.
    pub fn asset_in_region(&self, region: &str, asset_id: &str) -> Option<&AssetInfo> {
        self.outposts_in(region).find_map(|record| {
            record
                .assets
                .iter()
                .find(|asset| asset.asset_id.as_deref() == Some(asset_id))
        })
    }

    /// The asset `asset_id` of the Outpost `outpost_id`, if that Outpost holds one.
    ///
    /// An asset is a nested resource of exactly ONE Outpost — the modelled route is
    /// `/outposts/{OutpostIdentifier}/assets` and [`OutpostRecord::assets`] holds
    /// the hardware recorded for it — so an `AssetId` is resolved WITHIN the Outpost
    /// the request names: an operation scoped to an Outpost resolves the asset it
    /// was given here rather than walking the store itself, and `None` means that
    /// Outpost holds no such asset.
    pub fn asset_of_outpost(&self, outpost_id: &str, asset_id: &str) -> Option<&AssetInfo> {
        self.outposts
            .get(outpost_id)?
            .assets
            .iter()
            .find(|asset| asset.asset_id.as_deref() == Some(asset_id))
    }

    /// Mint a fresh 32-byte digest for a resource of kind `kind`, with the creation
    /// sequence to record on it.
    ///
    /// `created` advances on every call and is hashed into the seed with the wall
    /// clock and the process id, so repeated calls never return the same digest.
    /// The two id forms this service mints ([`Self::mint_id`] and
    /// [`Self::mint_connection_id`]) are two renderings of this one digest.
    fn mint_digest(&mut self, kind: &str) -> ([u8; 32], u64) {
        self.created = self.created.wrapping_add(1);
        let seed = format!(
            "{kind}|{}|{}|{}",
            self.created,
            std::process::id(),
            seed_nanos()
        );
        let digest = Sha256::digest(seed.as_bytes());
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest);
        (bytes, self.created)
    }

    /// Take the next `count` creation sequences of this store, answering the FIRST of
    /// them.
    ///
    /// [`Self::created`] is this store's one creation order, so the hardware this
    /// service installs takes its sequences from it exactly as a created resource
    /// does ([`Self::mint_digest`]): a block of `count` consecutive sequences this
    /// store has never handed out and will never hand out again. That is what makes a
    /// unit's sequence identify that unit — see
    /// [`OutpostRecord::asset_sequence_base`].
    pub(super) fn reserve_sequences(&mut self, count: usize) -> u64 {
        let count = u64::try_from(count).unwrap_or_default();
        let first = self.created.wrapping_add(1);
        self.created = self.created.wrapping_add(count);
        first
    }
}
