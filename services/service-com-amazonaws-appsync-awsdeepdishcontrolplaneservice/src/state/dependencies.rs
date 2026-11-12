// SPDX-License-Identifier: CC0-1.0

//! What inside one GraphQL API still USES one of its resources, and how a delete that
//! would break that use is refused.
//!
//! The resources of an API point at each other by name and by id: a function "resolves
//! against" the data source its `dataSourceName` names, and a pipeline resolver invokes
//! the functions its `pipelineConfig.functions` lists. A delete of the resource at the
//! far end of such a reference would leave the referring resource pointing at something
//! that no longer exists, so AppSync refuses it with its `BadRequestException` (HTTP
//! 400) naming what still uses the resource, and the resource is left in place — the
//! caller deletes the referring resource first.
//!
//! The lookups and the two sentences live together here because they are one rule read
//! from both ends: the delete that refuses and the create that established the
//! reference name the same members.

use super::{FunctionRecord, GraphqlApiRecord, ResolverRecord};

impl GraphqlApiRecord {
    /// Every function of this API that resolves against the data source `name`, in the
    /// order they were created.
    ///
    /// `dataSourceName` is "The `Function` `DataSource` name"
    /// (`FunctionConfiguration$dataSourceName`) — the data source the function resolves
    /// against — so a function names exactly one, and the creation order each record
    /// keeps is the one total order a report of them is stated in.
    pub fn functions_using_data_source(&self, name: &str) -> Vec<&FunctionRecord> {
        let mut functions: Vec<&FunctionRecord> = self
            .functions
            .values()
            .filter(|function| function.data_source_name == name)
            .collect();
        functions.sort_by_key(|function| function.seq);
        functions
    }

    /// The refusal text for deleting the data source `name` while a function of this API
    /// still resolves against it, or `None` when no function does.
    ///
    /// Stated over the REFERENCE rather than over one request's values: whichever
    /// functions name this data source are the ones the message lists, in their creation
    /// order, and a data source nothing names is deletable.
    pub fn data_source_in_use_refusal(&self, name: &str) -> Option<String> {
        let function_ids: Vec<&str> = self
            .functions_using_data_source(name)
            .iter()
            .map(|function| function.function_id.as_str())
            .collect();
        (!function_ids.is_empty()).then(|| data_source_in_use_message(&function_ids))
    }

    /// The refusal text for deleting the function `function_id` while a resolver of this
    /// API still invokes it, or `None` when no resolver does.
    ///
    /// The sentence names ONE resolver — the first of the API's resolvers that invokes
    /// the function, in creation order — because that is what AppSync's own sentence has
    /// room for: a resolver is addressed by the type and the field it is attached to
    /// ([`ResolverRecord::cursor`]), which is what the message carries.
    pub fn function_in_use_refusal(&self, function_id: &str) -> Option<String> {
        let resolvers = self.resolvers_using_function(function_id);
        let resolver: &ResolverRecord = resolvers.first()?;
        Some(function_in_use_message(
            &resolver.type_name,
            &resolver.field_name,
        ))
    }
}

/// The text a `BadRequestException` (HTTP 400) carries for a data source a function
/// still resolves against: the fixed sentence plus the bracketed list of the ids of the
/// functions that still name it, read from state.
///
/// The requirement the refusal answers is the status, the error shape and this sentence;
/// the ids are state's, never the request's — the request names only the data source.
pub fn data_source_in_use_message(function_ids: &[&str]) -> String {
    format!(
        "Data source is still in use by functions: [{}]",
        function_ids.join(", ")
    )
}

/// The text a `BadRequestException` (HTTP 400) carries for a function a resolver still
/// invokes: the fixed sentence plus the type and the field that address the referring
/// resolver, read from state.
pub fn function_in_use_message(type_name: &str, field_name: &str) -> String {
    format!(
        "Cannot delete a function which is currently used by a resolver. Resolver type: \
         {type_name}, field: {field_name}"
    )
}
