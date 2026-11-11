// SPDX-License-Identifier: CC0-1.0

//! The schema and the codec one structure needs when the WIRE carries a member
//! the on-disk model does not declare.
//!
//! WHY THIS MODULE EXISTS. `#[smithy_macros::smithy(shape = ...)]` derives a
//! structure's `Schema`, its serializer and its deserializer from the model's
//! member list, and it admits EXACTLY that list: a Rust field with no member
//! behind it is a compile error, so a member the service really sends but the
//! on-disk model omits cannot be declared through the derive at all. The
//! macros below emit, for one structure, the same items the derive emits — one
//! `M_<MEMBER>` member schema per member (index = that member's position in
//! the member list), a `SCHEMA` naming them in that order, the
//! `<STRUCT>_SCHEMA` static and the `SerializableStruct` /
//! `DeserializableStruct` pair — from a member list this file gives, so the
//! extra member can be appended to it. An ERROR shape takes the same treatment
//! through its own macro, which emits the items the derive emits for one of
//! those instead (the `SCHEMA_<ERROR>` static an `#[operation_error]` enum arm
//! names, `shape_name`, and the serializer).
//!
//! The model's own members keep the indices and the order the model gives
//! them, and an unmodelled member is appended LAST, so a later model refresh
//! that declares the member only has to move it into place and put the derive
//! back.
//!
//! WHAT IS DELIBERATELY ABSENT, because the derive's own output would be inert
//! here too:
//!
//! * Constraint traits (`@length`, `@pattern`, `@range`). The validating walk
//!   that enforces them runs on operation INPUTS only
//!   (`aws_smithy_schema_server::constraints::validate`, which the generated
//!   service calls on the deserialized input before the handler runs). Every
//!   structure built with these macros is output-only.
//! * The `@http` chain, which the derive bakes on only when the shape is an
//!   operation's INPUT.
//! * `@xmlName`, on the shapes and on every member of them: each one's wire
//!   element IS its Smithy member name.
//! * Map key/value sub-member schemas. The XML codec reads the key and value
//!   element names off them and falls back to the Smithy defaults `key` and
//!   `value` when they are absent — and no map written through these macros
//!   renames either, so the defaults ARE the modelled element names.
//!
//! The structures that need them live in this service's `types/` modules, where
//! each shape is declared exactly once, so `types/mod.rs` pulls this module in
//! with `#[macro_use]` ahead of them — which is what puts these macros in their
//! textual scope. They are NOT `#[macro_export]`ed: `src/ops/mod.rs` reaches
//! `types` through an `include!`, so a `macro_rules!` declared under it counts
//! as macro-expanded and cannot be named by an absolute `$crate::` path
//! (rust-lang #52234).

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident`.
macro_rules! s3c_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (boolean) => {
        ::aws_smithy_schema::ShapeType::Boolean
    };
    (integer) => {
        ::aws_smithy_schema::ShapeType::Integer
    };
    (date_time) => {
        ::aws_smithy_schema::ShapeType::Timestamp
    };
    (map_of_string) => {
        ::aws_smithy_schema::ShapeType::Map
    };
    (struct_of) => {
        ::aws_smithy_schema::ShapeType::Structure
    };
}

/// One member's schema, as the vendored codegen emits it: the member's shape
/// id, its shape type, its wire name and its index in the member list.
macro_rules! s3c_member_schema {
    ($ns:literal, $local:literal, $wire:literal, $idx:literal, $kind:tt $(($inner:ty))?) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!($ns, "#", $local, "$", $wire),
                $ns,
                $wire,
            ),
            s3c_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for
/// that kind: a map writes each key and then its value against the MAP
/// member's own schema, which is where the codec reads the entry layout from.
macro_rules! s3c_write_member {
    ($s:ident, $m:expr, $v:expr, string) => {
        $s.write_string($m, $v.as_str())?;
    };
    ($s:ident, $m:expr, $v:expr, boolean) => {
        $s.write_boolean($m, *$v)?;
    };
    ($s:ident, $m:expr, $v:expr, integer) => {
        $s.write_integer($m, *$v)?;
    };
    ($s:ident, $m:expr, $v:expr, date_time) => {
        $s.write_timestamp($m, $v)?;
    };
    ($s:ident, $m:expr, $v:expr, map_of_string) => {
        $s.write_map($m, &|__sm| {
            for (__k, __mv) in $v.iter() {
                __sm.write_string($m, __k.as_str())?;
                __sm.write_string($m, __mv.as_str())?;
            }
            Ok(())
        })?;
    };
    ($s:ident, $m:expr, $v:expr, struct_of($inner:ty)) => {
        $s.write_struct($m, $v)?;
    };
}

/// Read one member, as the vendored codegen's `codegen::emit_read` does for
/// that kind.
///
/// Entry COUNTS are bounded by the request body the server reads with a cap,
/// and nesting DEPTH by the codec itself, which refuses a document nesting
/// deeper than its own maximum.
macro_rules! s3c_read_member {
    ($d:ident, $m:expr, string) => {
        $d.read_string($m)?
    };
    ($d:ident, $m:expr, boolean) => {
        $d.read_boolean($m)?
    };
    ($d:ident, $m:expr, integer) => {
        $d.read_integer($m)?
    };
    ($d:ident, $m:expr, date_time) => {
        $d.read_timestamp($m)?
    };
    ($d:ident, $m:expr, map_of_string) => {{
        let mut __out: ::std::collections::HashMap<::std::string::String, ::std::string::String> =
            ::std::collections::HashMap::new();
        $d.read_map($m, &mut |__k, __dm| {
            __out.insert(__k, __dm.read_string($m)?);
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, struct_of($inner:ty)) => {
        <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize($d)?
    };
}

/// Write a member only when the value is there: an `Option` one when it is
/// `Some`, a non-`Option` one (`@required`) always — the vendored codegen's own
/// `is_option` split.
macro_rules! s3c_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, req, $($kind:tt)+) => {
        s3c_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            s3c_write_member!($s, &$m, __v, $($kind)+);
        }
    };
}

/// The value a read slot contributes to the struct: a `@required` member absent
/// from the wire is refused by name, an optional one stays `None`.
macro_rules! s3c_member_value {
    ($slot:ident, $wire:literal, req) => {
        $slot.ok_or_else(|| ::aws_smithy_schema_server::missing_member($wire))?
    };
    ($slot:ident, $wire:literal, opt) => {
        $slot
    };
}

/// The schemas, the wire name and the serializer for one ERROR structure, from
/// the member list given here: what `#[smithy_macros::smithy(shape = ...)]`
/// emits for an error shape, for a member list that may carry a member the
/// on-disk model does not declare.
///
/// The items are the ones an `#[smithy_macros::operation_error]` enum arm
/// names: the `SCHEMA_<ERROR>` static — a `Schema` BY VALUE, which is what that
/// expansion takes a reference to — and the `shape_name` the wire error code is
/// read from, plus the serializer that writes the members. No deserializer: a
/// refusal this service answers is only ever written.
///
/// Each member entry is as [`s3c_schema_and_codec`]'s, in the model's member
/// order with an unmodelled member last. `Display` and `std::error::Error` stay
/// with the structure: how a refusal READS is per-error prose rather than wire
/// layout.
macro_rules! s3c_error_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $shape:literal, $ns:literal, $local:literal,
        $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $presence:tt, $($kind:tt)+ ) ),+ $(,)?
    ) => {
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    s3c_member_schema!($ns, $local, $wire, $idx, $($kind)+);
            )+

            pub const SCHEMA: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_struct(
                    ::aws_smithy_schema::ShapeId::from_static($shape, $ns, $local),
                    ::aws_smithy_schema::ShapeType::Structure,
                    &[$(&Self::$cname,)+],
                );

            /// The wire error code this refusal is matched on: the error
            /// shape's own name.
            pub const fn shape_name() -> &'static str {
                $local
            }
        }

        pub static $schema_static: ::aws_smithy_schema::Schema = $ty::SCHEMA;

        impl ::aws_smithy_schema::serde::SerializableStruct for $ty {
            fn serialize_members(
                &self,
                s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
            ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
                $(
                    s3c_write_present_member!(
                        s, self, Self::$cname, $field, $presence, $($kind)+
                    );
                )+
                Ok(())
            }
        }
    };
}

/// The schema, the serializer and the deserializer for one structure, from the
/// member list given here: everything `#[smithy_macros::smithy(shape = ...)]`
/// emits for an output shape, for a member list that may carry a member the
/// on-disk model does not declare.
///
/// Each member entry is `(CONST, field, "WireName", index, req|opt, kind)`;
/// `req` is a `@required` member (a bare Rust field, refused when absent on the
/// wire) and `opt` an `Option` one. The entries must be given in the model's
/// member order — the order `smithy_query(query="shape", shape=...)` lists them
/// in and the order each struct declares its fields in — with an unmodelled
/// member last.
macro_rules! s3c_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $shape:literal, $ns:literal, $local:literal,
        $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $presence:tt, $($kind:tt)+ ) ),+ $(,)?
    ) => {
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    s3c_member_schema!($ns, $local, $wire, $idx, $($kind)+);
            )+

            pub const SCHEMA: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_struct(
                    ::aws_smithy_schema::ShapeId::from_static($shape, $ns, $local),
                    ::aws_smithy_schema::ShapeType::Structure,
                    &[$(&Self::$cname,)+],
                );
        }

        pub static $schema_static: &::aws_smithy_schema::Schema = &$ty::SCHEMA;

        impl ::aws_smithy_schema::serde::SerializableStruct for $ty {
            fn serialize_members(
                &self,
                s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
            ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
                $(
                    s3c_write_present_member!(
                        s, self, Self::$cname, $field, $presence, $($kind)+
                    );
                )+
                Ok(())
            }
        }

        impl ::aws_smithy_schema_server::DeserializableStruct for $ty {
            fn deserialize(
                d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
            ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
                $( let mut $field = ::std::option::Option::None; )+
                d.read_struct(&Self::SCHEMA, &mut |member, d| {
                    match member.member_index() {
                        $(
                            ::std::option::Option::Some($idx) => {
                                $field = ::std::option::Option::Some(
                                    s3c_read_member!(d, &Self::$cname, $($kind)+),
                                );
                            }
                        )+
                        _ => {}
                    }
                    Ok(())
                })?;
                Ok(Self {
                    $( $field: s3c_member_value!($field, $wire, $presence) ),+
                })
            }
        }
    };
}
