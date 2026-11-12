// SPDX-License-Identifier: CC0-1.0

//! The response elements EC2 renders that this checkout's Smithy model does not declare.
//!
//! # The gap this closes
//!
//! The model a service crate is generated from is a SNAPSHOT of the API, and the live EC2
//! wire has moved past it in places: a reported shape carries an element the shape's
//! modelled member list does not mention. A recorded `DescribeVpcs` answers every VPC with
//! `<dhcpOptionsSource>`, and every operation that reports a Verified Access instance
//! answers it with `<verifiedAccessInstanceTransitGatewayAttachmentSet>` — present and
//! empty for an instance with no transit-gateway attachment. The generated serializer walks
//! the model's members and only those, so both elements are missing from this emulator's
//! responses: a client that reads `dhcpOptionsSource` to decide whether a VPC's DHCP
//! options are its own or the account's reads nothing, and a strict deserializer that
//! iterates the attachment set is handed a null collection instead of an empty one.
//!
//! An element the model does not declare cannot be a field of a generated shape, so it
//! cannot be written from a handler. It is written HERE instead, at the one seam where this
//! service renders a success body ([`super::Ec2QueryEnvelope::serialize_output`]), the same
//! way the envelope's own `<return>` flag is: a member schema built by hand, written after
//! the shape's modelled members. The rule is the PROTOCOL's — "an element the live wire
//! renders on this shape is written whichever operation reports the shape" — so it reaches
//! every operation that answers with a VPC or a Verified Access instance (a create, a
//! modify, a describe, a delete) rather than being restated per handler.
//!
//! Nothing here invents a value: each element is derived from the record being reported —
//! the VPC's own DHCP option set association ([`WrittenMembers::dhcp_options_id`]), and the
//! transit-gateway attachments a Verified Access instance of this store holds, of which
//! there are none, no operation of this service attaching one.
//!
//! # What it does NOT reach
//!
//! An element whose value is NOT a function of the reported record — a subnet's inherited
//! DHCP option set id, which has to be resolved from the subnet's VPC — needs the store, and
//! a protocol has none. Those are left to the projection that owns them.

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};
use ::aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The namespace every shape of this service's model lives in, so an element is only ever
/// written onto a shape of EC2's own.
const EC2_NAMESPACE: &str = "com.amazonaws.ec2";

/// The scope a VPC's DHCP option set comes from, as `dhcpOptionsSource` names it: the VPC
/// itself, which is where every DHCP option set this store associates is associated
/// ([`crate::state::ComAmazonawsEc2Amazonec2State::insert_vpc`] gives the VPC the Region's
/// default set as its OWN association). The other scope the element reports — an
/// account-level default set, which no operation of this service creates — is why the
/// element exists at all, and why it is written from the association the VPC holds rather
/// than as a fact of every VPC.
const DHCP_OPTIONS_SOURCE_VPC: &str = "vpc";

/// The Smithy member name a VPC's DHCP option set id is written under, which is what says
/// the VPC has an association for `dhcpOptionsSource` to name the scope of.
const DHCP_OPTIONS_ID_MEMBER: &str = "DhcpOptionsId";

/// The Smithy member name a Verified Access instance's own id is written under, which is
/// what says the structure being walked IS such an instance.
const VERIFIED_ACCESS_INSTANCE_ID_MEMBER: &str = "VerifiedAccessInstanceId";

/// `<dhcpOptionsSource>` on a reported VPC.
///
/// A member schema rather than a modelled one: `com.amazonaws.ec2#Vpc` declares no such
/// member, so the element has no shape of its own to borrow. The XML codec names an element
/// from the member name, which is what makes this write `<dhcpOptionsSource>`.
static VPC_DHCP_OPTIONS_SOURCE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ec2#Vpc$dhcpOptionsSource",
        EC2_NAMESPACE,
        "dhcpOptionsSource",
    ),
    ShapeType::String,
    "dhcpOptionsSource",
    0,
);

/// `<verifiedAccessInstanceTransitGatewayAttachmentSet>` on a reported Verified Access
/// instance — a member schema for the same reason as [`VPC_DHCP_OPTIONS_SOURCE`].
static VERIFIED_ACCESS_INSTANCE_TRANSIT_GATEWAY_ATTACHMENT_SET: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ec2#VerifiedAccessInstance$verifiedAccessInstanceTransitGatewayAttachmentSet",
        EC2_NAMESPACE,
        "verifiedAccessInstanceTransitGatewayAttachmentSet",
    ),
    ShapeType::List,
    "verifiedAccessInstanceTransitGatewayAttachmentSet",
    0,
);

/// One BORROWED structure serialized with the recorded elements of every shape under it.
///
/// The entry point: a service that renders its own protocol envelope reaches its output as a
/// `&dyn SerializableStruct` and wraps it here. Every structure NESTED under that output is
/// re-entered, because the codec walks a nested structure with its own serializer, so the
/// rule reaches the reported shape wherever it sits — a list item, a member of its own.
pub struct RecordedMembersRef<'a>(pub &'a dyn SerializableStruct);

impl SerializableStruct for RecordedMembersRef<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut RecordedMemberSerializer {
            inner: s,
            written: None,
        })
    }
}

/// What the structure being walked wrote of the members a recorded element is derived from.
///
/// Collected while the shape's MODELLED members are written, so the element below is decided
/// by the record the response carries rather than by the shape it carries it in. It is also
/// what IDENTIFIES the shape: see [`recorded_writer_for`] for why a name alone does not.
#[derive(Debug, Default, Clone, Copy)]
struct WrittenMembers {
    /// Whether the structure wrote a DHCP option set id of its own — the set associated
    /// WITH IT, which is the scope `dhcpOptionsSource` reports.
    dhcp_options_id: bool,
    /// Whether the structure wrote a Verified Access instance id of its own, which is the
    /// id the attachment set below belongs to.
    verified_access_instance_id: bool,
}

/// How one shape's recorded elements are written, once its modelled members have been.
type RecordedWriter = fn(&mut dyn ShapeSerializer, WrittenMembers) -> Result<(), SerdeError>;

/// How `schema`'s own shape renders the elements the model does not declare for it, or
/// `None` for a shape the live wire renders exactly as the model declares it.
///
/// `schema` is what the codec was handed for the structure being written, which is the
/// structure's own schema when it is a list item and the MEMBER's schema when it is a member
/// of another structure. Both carry the same shape NAME in EC2's model — every member that
/// targets one of these shapes is named after it (`CreateVpcResult$Vpc`,
/// `ModifyVerifiedAccessInstanceResult$VerifiedAccessInstance`) — so one name answers for a
/// shape in either position.
///
/// A name is NOT on its own enough to identify the shape: a member may be named after one
/// shape and target another (`Explanation$Vpc` and `PathComponent$Vpc` are members named
/// `Vpc` that target `AnalysisComponent`). So the name only selects the writer, and each
/// writer then asks [`WrittenMembers`] for the member that only its own shape carries — the
/// identity of the structure and the derivation of its element being the same question.
fn recorded_writer_for(schema: &Schema) -> Option<RecordedWriter> {
    if schema.shape_id().namespace() != EC2_NAMESPACE {
        return None;
    }
    match schema.shape_id().shape_name() {
        "Vpc" => Some(write_vpc_recorded_members),
        "VerifiedAccessInstance" => Some(write_verified_access_instance_recorded_members),
        _ => None,
    }
}

/// A reported VPC's recorded elements: the scope its DHCP option set comes from.
///
/// Written only for a VPC that reported an option set association of its own: a VPC with no
/// association has no scope to name, so the element is left out rather than claiming one.
fn write_vpc_recorded_members(
    s: &mut dyn ShapeSerializer,
    written: WrittenMembers,
) -> Result<(), SerdeError> {
    if !written.dhcp_options_id {
        return Ok(());
    }
    s.write_string(&VPC_DHCP_OPTIONS_SOURCE, DHCP_OPTIONS_SOURCE_VPC)
}

/// A reported Verified Access instance's recorded elements: the transit-gateway attachments
/// it holds.
///
/// Always EMPTY, and present: no operation of this service attaches a transit gateway to a
/// Verified Access instance, so an instance of this store holds none — and an empty
/// collection is still a collection, which is the whole difference this closes for a client
/// that iterates it.
///
/// Written only for a structure that reported an instance id of its own, which is what makes
/// it one of these instances rather than a shape whose member merely shares the name.
fn write_verified_access_instance_recorded_members(
    s: &mut dyn ShapeSerializer,
    written: WrittenMembers,
) -> Result<(), SerdeError> {
    if !written.verified_access_instance_id {
        return Ok(());
    }
    s.write_list(
        &VERIFIED_ACCESS_INSTANCE_TRANSIT_GATEWAY_ATTACHMENT_SET,
        &|_items| Ok(()),
    )
}

/// One structure, serialized as its modelled members followed by the recorded elements its
/// shape renders on the live wire.
struct WithRecordedMembers<'a> {
    /// The structure as the generated serializer writes it.
    inner: &'a dyn SerializableStruct,
    /// How this shape's recorded elements are written.
    writer: RecordedWriter,
}

impl SerializableStruct for WithRecordedMembers<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        let mut written = WrittenMembers::default();
        {
            // The modelled members first, so the recorded elements sit after them and the
            // order the model gave this shape is untouched. The walk notes what it wrote,
            // which is what the recorded elements are derived from.
            let mut walk = RecordedMemberSerializer {
                inner: s,
                written: Some(&mut written),
            };
            self.inner.serialize_members(&mut walk)?;
        }
        (self.writer)(s, written)
    }
}

/// The serializer a wrapped output is walked with: every call is forwarded to the codec's own
/// serializer unchanged, except a STRUCTURE, which is re-entered so the rule reaches every
/// depth and so a shape with recorded elements gets them.
struct RecordedMemberSerializer<'a> {
    /// The codec's own serializer, or the next wrapper out.
    inner: &'a mut dyn ShapeSerializer,
    /// The members of the structure being walked, when this walk is collecting them for that
    /// structure's recorded elements. `None` while walking anything else, so a nested
    /// structure's members are never read as the enclosing one's.
    written: Option<&'a mut WrittenMembers>,
}

impl ShapeSerializer for RecordedMemberSerializer<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        match recorded_writer_for(schema) {
            Some(writer) => self
                .inner
                .write_struct(schema, &WithRecordedMembers { inner: value, writer }),
            None => self.inner.write_struct(schema, &RecordedMembersRef(value)),
        }
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_list(schema, &|inner| {
            write_elements(&mut RecordedMemberSerializer {
                inner,
                written: None,
            })
        })
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_map(schema, &|inner| {
            write_entries(&mut RecordedMemberSerializer {
                inner,
                written: None,
            })
        })
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.inner.write_boolean(schema, value)
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.inner.write_byte(schema, value)
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.inner.write_short(schema, value)
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.inner.write_integer(schema, value)
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.inner.write_long(schema, value)
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.inner.write_float(schema, value)
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.inner.write_double(schema, value)
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.inner.write_big_integer(schema, value)
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.inner.write_big_decimal(schema, value)
    }

    /// The leaves this notes as it forwards: the DHCP option set id a VPC reports, which is
    /// what [`write_vpc_recorded_members`] names the scope of, and the id a Verified Access
    /// instance reports, which is what identifies it as one.
    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        if let Some(written) = self.written.as_deref_mut() {
            if schema.shape_id().namespace() == EC2_NAMESPACE {
                match schema.shape_id().shape_name() {
                    DHCP_OPTIONS_ID_MEMBER => written.dhcp_options_id = true,
                    VERIFIED_ACCESS_INSTANCE_ID_MEMBER => {
                        written.verified_access_instance_id = true;
                    }
                    _ => {}
                }
            }
        }
        self.inner.write_string(schema, value)
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.inner.write_blob(schema, value)
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.inner.write_timestamp(schema, value)
    }

    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.inner.write_document(schema, value)
    }

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.inner.write_null(schema)
    }
}
