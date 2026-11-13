// SPDX-License-Identifier: CC0-1.0

//! Authoritative DNS responder over the Route 53 hosted-zone store for local and multi-machine
//! container sandboxes.
//!
//! By default this emulator opens no DNS socket: Route 53 is a control plane, and `TestDNSAnswer`
//! simulates queries in memory. When [`LISTEN_ADDRESS_VARIABLE`] (`EMULATOR_SANDBOX_DNS_ADDR`)
//! names a `host:port` (for example `172.25.0.1:53` on a single-host bridge or
//! `192.168.1.142:15353` across LAN container hosts), this module binds both UDP and TCP RFC 1035
//! listeners on that address and answers queries directly from the live
//! [`ComAmazonawsRoute53Awsdnsv20130401State`] hosted zones.
//!
//! ## Closed-world sandbox rule
//!
//! Every query is matched against the most specific hosted zone in the store (preferring a
//! private hosted zone over a public one when both carry the exact same apex, matching VPC
//! split-horizon resolution) plus any explicit `host=ipv4` entries in [`STATIC_HOSTS_VARIABLE`].
//! A query for any domain outside those zones receives `NXDOMAIN` (`RCODE = 3`) immediately and
//! is never forwarded to an upstream resolver, keeping sandboxed workloads hermetically inside
//! the emulated network.

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, OnceLock};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::task::JoinHandle;

use crate::ops::types::shared_types::ResourceRecordSet;
use crate::state::{
    canonical_dns_name, name_is_in_zone, normalize_zone_id, service_state,
    ComAmazonawsRoute53Awsdnsv20130401State, StoredZone,
};

/// Environment variable naming the `ip:port` this sandbox DNS responder binds on.
pub const LISTEN_ADDRESS_VARIABLE: &str = "EMULATOR_SANDBOX_DNS_ADDR";

/// Optional environment variable naming the fallback IPv4 address an external/AWS `AliasTarget`
/// (such as a CloudFront distribution domain or an ALB hostname without an embedded IPv4 literal)
/// resolves to on `A` queries. Defaults to the responder's own IPv4 bind address (or `127.0.0.1`
/// when bound to `0.0.0.0` or IPv6).
pub const GATEWAY_IP_VARIABLE: &str = "EMULATOR_SANDBOX_GATEWAY_IP";

/// Optional comma-separated `hostname=ipv4` overrides (`db.internal=192.168.1.142,...`) for
/// cross-machine container endpoints not backed by an explicit Route 53 hosted zone.
pub const STATIC_HOSTS_VARIABLE: &str = "EMULATOR_SANDBOX_DNS_HOSTS";

const DNS_HEADER_LEN: usize = 12;
const MAX_UDP_PACKET_LEN: usize = 512;
const MAX_TCP_PACKET_LEN: usize = 65_535;
const MAX_COMPRESSION_JUMPS: usize = 16;
const MAX_ALIAS_HOPS: usize = 8;
const DEFAULT_TTL: u32 = 60;

const QTYPE_A: u16 = 1;
const QTYPE_NS: u16 = 2;
const QTYPE_CNAME: u16 = 5;
const QTYPE_SOA: u16 = 6;
const QTYPE_TXT: u16 = 16;
const QTYPE_AAAA: u16 = 28;
const QTYPE_ANY: u16 = 255;
const CLASS_IN: u16 = 1;

const RCODE_NOERROR: u16 = 0;
const RCODE_FORMERR: u16 = 1;
const RCODE_NXDOMAIN: u16 = 3;

/// Start the process-wide sandbox DNS responder once when [`LISTEN_ADDRESS_VARIABLE`] is set and
/// a Tokio runtime is active.
pub fn start_if_configured<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) {
    static STARTED: OnceLock<()> = OnceLock::new();
    let Ok(raw_addr) = std::env::var(LISTEN_ADDRESS_VARIABLE) else {
        return;
    };
    let raw_addr = raw_addr.trim();
    if raw_addr.is_empty() {
        return;
    }
    let Ok(listen_addr) = raw_addr.parse::<SocketAddr>() else {
        tracing::warn!(
            address = raw_addr,
            "ignoring invalid EMULATOR_SANDBOX_DNS_ADDR socket address"
        );
        return;
    };
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let gateway_ip = configured_gateway_ip(listen_addr);
    let static_hosts = configured_static_hosts();
    STARTED.get_or_init(|| {
        handle.spawn(async move {
            match SandboxDnsServer::bind_with_hosts(listen_addr, state, gateway_ip, static_hosts)
                .await
            {
                Ok(server) => {
                    tracing::info!(
                        address = %server.local_addr(),
                        gateway_ip = %gateway_ip,
                        "Route 53 sandbox DNS responder listening on UDP and TCP"
                    );
                    // Keep the server handle alive for the lifetime of the emulator runtime.
                    std::mem::forget(server);
                }
                Err(error) => {
                    tracing::warn!(
                        address = %listen_addr,
                        %error,
                        "failed to bind Route 53 sandbox DNS responder"
                    );
                }
            }
        });
    });
}

fn configured_gateway_ip(listen_addr: SocketAddr) -> Ipv4Addr {
    if let Ok(raw) = std::env::var(GATEWAY_IP_VARIABLE) {
        if let Ok(ip) = raw.trim().parse::<Ipv4Addr>() {
            return ip;
        }
    }
    match listen_addr.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_unspecified() => ip,
        _ => Ipv4Addr::LOCALHOST,
    }
}

fn configured_static_hosts() -> HashMap<String, Ipv4Addr> {
    let mut map = HashMap::new();
    let Ok(raw) = std::env::var(STATIC_HOSTS_VARIABLE) else {
        return map;
    };
    for pair in raw.split(',') {
        let Some((host, ip_str)) = pair.split_once('=') else {
            continue;
        };
        let host = canonical_dns_name(host.trim());
        if let Ok(ip) = ip_str.trim().parse::<Ipv4Addr>() {
            if !host.is_empty() {
                map.insert(host, ip);
            }
        }
    }
    map
}

/// Running UDP + TCP authoritative DNS server backed by a [`ComAmazonawsRoute53Awsdnsv20130401State`] store.
pub struct SandboxDnsServer {
    local_addr: SocketAddr,
    udp_task: JoinHandle<()>,
    tcp_task: JoinHandle<()>,
}

impl SandboxDnsServer {
    /// Bind both UDP and TCP sockets on `listen_addr` (pass port `0` for an ephemeral test port,
    /// in which case TCP binds the exact port UDP was assigned).
    pub async fn bind(
        listen_addr: SocketAddr,
        state: ::engine::ServiceState<ComAmazonawsRoute53Awsdnsv20130401State>,
        gateway_ip: Ipv4Addr,
    ) -> io::Result<Self> {
        Self::bind_with_hosts(listen_addr, state, gateway_ip, HashMap::new()).await
    }

    /// Bind both UDP and TCP sockets with explicit static `host -> IPv4` overrides alongside
    /// Route 53 hosted zones.
    pub async fn bind_with_hosts(
        listen_addr: SocketAddr,
        state: ::engine::ServiceState<ComAmazonawsRoute53Awsdnsv20130401State>,
        gateway_ip: Ipv4Addr,
        static_hosts: HashMap<String, Ipv4Addr>,
    ) -> io::Result<Self> {
        let udp = UdpSocket::bind(listen_addr).await?;
        let bound_addr = udp.local_addr()?;
        let tcp = TcpListener::bind(bound_addr).await?;
        let static_hosts = Arc::new(static_hosts);

        let udp_state = state.clone();
        let udp_hosts = Arc::clone(&static_hosts);
        let udp_task = tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok((len, peer)) = udp.recv_from(&mut buf).await {
                let response = {
                    let store = udp_state.lock();
                    respond_wire(&buf[..len], &store, gateway_ip, &udp_hosts, MAX_UDP_PACKET_LEN)
                };
                if let Some(packet) = response {
                    let _ = udp.send_to(&packet, peer).await;
                }
            }
        });

        let tcp_state = state;
        let tcp_hosts = static_hosts;
        let tcp_task = tokio::spawn(async move {
            while let Ok((mut stream, _peer)) = tcp.accept().await {
                let conn_state = tcp_state.clone();
                let conn_hosts = Arc::clone(&tcp_hosts);
                tokio::spawn(async move {
                    let mut len_buf = [0u8; 2];
                    while stream.read_exact(&mut len_buf).await.is_ok() {
                        let msg_len = u16::from_be_bytes(len_buf) as usize;
                        if msg_len == 0 {
                            break;
                        }
                        let mut query = vec![0u8; msg_len];
                        if stream.read_exact(&mut query).await.is_err() {
                            break;
                        }
                        let response = {
                            let store = conn_state.lock();
                            respond_wire(
                                &query,
                                &store,
                                gateway_ip,
                                &conn_hosts,
                                MAX_TCP_PACKET_LEN,
                            )
                        };
                        let Some(packet) = response else {
                            break;
                        };
                        let Ok(resp_len) = u16::try_from(packet.len()) else {
                            break;
                        };
                        if stream.write_all(&resp_len.to_be_bytes()).await.is_err() {
                            break;
                        }
                        if stream.write_all(&packet).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });

        Ok(Self {
            local_addr: bound_addr,
            udp_task,
            tcp_task,
        })
    }

    /// Socket address both UDP and TCP listeners are bound to.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Abort both listener tasks cleanly.
    pub fn shutdown(self) {
        self.udp_task.abort();
        self.tcp_task.abort();
    }
}

#[derive(Debug, Clone)]
enum WireRdata {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    DomainName(String),
    Txt(String),
    Soa {
        mname: String,
        rname: String,
        serial: u32,
        refresh: u32,
        retry: u32,
        expire: u32,
        minimum: u32,
    },
}

#[derive(Debug, Clone)]
struct WireAnswer {
    name: String,
    rr_type: u16,
    ttl: u32,
    rdata: WireRdata,
}

/// Decode one RFC 1035 DNS request message and build its authoritative wire response.
pub fn respond_wire(
    query: &[u8],
    store: &ComAmazonawsRoute53Awsdnsv20130401State,
    gateway_ip: Ipv4Addr,
    static_hosts: &HashMap<String, Ipv4Addr>,
    max_len: usize,
) -> Option<Vec<u8>> {
    if query.len() < DNS_HEADER_LEN {
        return None;
    }
    let id = u16::from_be_bytes([query[0], query[1]]);
    let req_flags = u16::from_be_bytes([query[2], query[3]]);
    // Ignore packets that already have QR=1 (response).
    if (req_flags & 0x8000) != 0 {
        return None;
    }
    let rd = req_flags & 0x0100;
    let qdcount = u16::from_be_bytes([query[4], query[5]]);
    if qdcount == 0 {
        return Some(encode_response(id, rd, RCODE_FORMERR, &[], &[], max_len));
    }

    let Some((raw_qname, after_name)) = decode_name(query, DNS_HEADER_LEN) else {
        return Some(encode_response(id, rd, RCODE_FORMERR, &[], &[], max_len));
    };
    if after_name + 4 > query.len() {
        return Some(encode_response(id, rd, RCODE_FORMERR, &[], &[], max_len));
    }
    let qtype = u16::from_be_bytes([query[after_name], query[after_name + 1]]);
    let qclass = u16::from_be_bytes([query[after_name + 2], query[after_name + 3]]);
    let question_slice = &query[DNS_HEADER_LEN..after_name + 4];

    if qclass != CLASS_IN && qclass != QTYPE_ANY {
        return Some(encode_response(
            id,
            rd,
            RCODE_NXDOMAIN,
            question_slice,
            &[],
            max_len,
        ));
    }

    let qname = canonical_dns_name(&raw_qname);
    if let Some(ip) = static_hosts.get(&qname) {
        let answers = if qtype == QTYPE_A || qtype == QTYPE_ANY {
            vec![WireAnswer {
                name: qname,
                rr_type: QTYPE_A,
                ttl: DEFAULT_TTL,
                rdata: WireRdata::A(*ip),
            }]
        } else {
            Vec::new()
        };
        return Some(encode_response(
            id,
            rd,
            RCODE_NOERROR,
            question_slice,
            &answers,
            max_len,
        ));
    }

    let (rcode, answers) = resolve_in_store(store, &qname, qtype, gateway_ip);
    Some(encode_response(
        id,
        rd,
        rcode,
        question_slice,
        &answers,
        max_len,
    ))
}

fn best_matching_zone<'a>(
    store: &'a ComAmazonawsRoute53Awsdnsv20130401State,
    name: &str,
) -> Option<&'a StoredZone> {
    store
        .zones
        .values()
        .filter(|zone| name_is_in_zone(name, &zone.name))
        .max_by_key(|zone| (canonical_dns_name(&zone.name).len(), zone.private_zone))
}

fn wildcard_name_in_zone(name: &str, zone_name: &str) -> Option<String> {
    let canonical = canonical_dns_name(name);
    let trimmed = canonical.trim_end_matches('.');
    let (_leftmost, parent) = trimmed.split_once('.')?;
    let wildcard = format!("*.{parent}.");
    name_is_in_zone(&wildcard, zone_name).then_some(wildcard)
}

fn qtype_name(qtype: u16) -> Option<&'static str> {
    match qtype {
        QTYPE_A => Some("A"),
        QTYPE_NS => Some("NS"),
        QTYPE_CNAME => Some("CNAME"),
        QTYPE_SOA => Some("SOA"),
        QTYPE_TXT => Some("TXT"),
        QTYPE_AAAA => Some("AAAA"),
        _ => None,
    }
}

fn lookup_records<'a>(
    zone: &'a StoredZone,
    name: &str,
    rr_type: &str,
) -> Vec<&'a ResourceRecordSet> {
    let exact = zone.records_at(name, rr_type);
    if !exact.is_empty() || zone.holds_record_name(name) {
        return exact;
    }
    if let Some(wildcard) = wildcard_name_in_zone(name, &zone.name) {
        return zone.records_at(&wildcard, rr_type);
    }
    Vec::new()
}

fn name_exists_or_wildcard(zone: &StoredZone, name: &str) -> bool {
    if zone.holds_record_name(name) {
        return true;
    }
    wildcard_name_in_zone(name, &zone.name)
        .is_some_and(|wildcard| zone.holds_record_name(&wildcard))
}

fn resolve_in_store(
    store: &ComAmazonawsRoute53Awsdnsv20130401State,
    qname: &str,
    qtype: u16,
    gateway_ip: Ipv4Addr,
) -> (u16, Vec<WireAnswer>) {
    let Some(initial_zone) = best_matching_zone(store, qname) else {
        return (RCODE_NXDOMAIN, Vec::new());
    };

    if qtype == QTYPE_ANY {
        let mut answers = Vec::new();
        for (code, name) in [
            (QTYPE_A, "A"),
            (QTYPE_AAAA, "AAAA"),
            (QTYPE_CNAME, "CNAME"),
            (QTYPE_TXT, "TXT"),
            (QTYPE_NS, "NS"),
            (QTYPE_SOA, "SOA"),
        ] {
            for rrset in lookup_records(initial_zone, qname, name) {
                append_rrset_answers(
                    store,
                    qname,
                    code,
                    rrset,
                    gateway_ip,
                    &mut answers,
                    0,
                );
            }
        }
        if answers.is_empty() && !name_exists_or_wildcard(initial_zone, qname) {
            return (RCODE_NXDOMAIN, Vec::new());
        }
        return (RCODE_NOERROR, answers);
    }

    let Some(wanted_type) = qtype_name(qtype) else {
        let code = if name_exists_or_wildcard(initial_zone, qname) {
            RCODE_NOERROR
        } else {
            RCODE_NXDOMAIN
        };
        return (code, Vec::new());
    };

    let mut answers = Vec::new();
    let mut current_name = qname.to_string();
    for hop in 0..=MAX_ALIAS_HOPS {
        let Some(zone) = best_matching_zone(store, &current_name) else {
            break;
        };
        let direct = lookup_records(zone, &current_name, wanted_type);
        if !direct.is_empty() {
            for rrset in direct {
                append_rrset_answers(
                    store,
                    &current_name,
                    qtype,
                    rrset,
                    gateway_ip,
                    &mut answers,
                    hop,
                );
            }
            return (RCODE_NOERROR, answers);
        }

        if qtype != QTYPE_CNAME {
            let cnames = lookup_records(zone, &current_name, "CNAME");
            if let Some(cname_set) = cnames.first() {
                if let Some(target) = cname_set
                    .resource_records
                    .as_ref()
                    .and_then(|records| records.first())
                    .map(|rec| canonical_dns_name(&rec.value))
                {
                    let ttl = cname_set
                        .ttl
                        .and_then(|val| u32::try_from(val).ok())
                        .unwrap_or(DEFAULT_TTL);
                    answers.push(WireAnswer {
                        name: current_name.clone(),
                        rr_type: QTYPE_CNAME,
                        ttl,
                        rdata: WireRdata::DomainName(target.clone()),
                    });
                    current_name = target;
                    continue;
                }
            }
        }

        if answers.is_empty() && !name_exists_or_wildcard(zone, &current_name) {
            return (RCODE_NXDOMAIN, Vec::new());
        }
        return (RCODE_NOERROR, answers);
    }

    (RCODE_NOERROR, answers)
}

fn append_rrset_answers(
    store: &ComAmazonawsRoute53Awsdnsv20130401State,
    owner_name: &str,
    qtype: u16,
    rrset: &ResourceRecordSet,
    gateway_ip: Ipv4Addr,
    out: &mut Vec<WireAnswer>,
    depth: usize,
) {
    if depth > MAX_ALIAS_HOPS {
        return;
    }
    let ttl = rrset
        .ttl
        .and_then(|val| u32::try_from(val).ok())
        .unwrap_or(DEFAULT_TTL);

    if let Some(alias) = rrset.alias_target.as_ref() {
        let target_zone_id = normalize_zone_id(&alias.hosted_zone_id);
        let target_name = canonical_dns_name(&alias.dns_name);
        if let Some(target_zone) = store.zones.get(target_zone_id) {
            if let Some(type_str) = qtype_name(qtype) {
                for next in lookup_records(target_zone, &target_name, type_str) {
                    append_rrset_answers(
                        store,
                        owner_name,
                        qtype,
                        next,
                        gateway_ip,
                        out,
                        depth + 1,
                    );
                }
            }
            return;
        }
        // External/service AliasTarget (ALB `ip:port`, CloudFront domain, S3 website):
        if qtype == QTYPE_A {
            let ip = extract_ipv4_from_endpoint(&alias.dns_name).unwrap_or(gateway_ip);
            out.push(WireAnswer {
                name: owner_name.to_string(),
                rr_type: QTYPE_A,
                ttl,
                rdata: WireRdata::A(ip),
            });
        }
        return;
    }

    for rec in rrset.resource_records.iter().flatten() {
        let val = rec.value.trim();
        match qtype {
            QTYPE_A => {
                if let Ok(ip) = val.parse::<Ipv4Addr>() {
                    out.push(WireAnswer {
                        name: owner_name.to_string(),
                        rr_type: QTYPE_A,
                        ttl,
                        rdata: WireRdata::A(ip),
                    });
                }
            }
            QTYPE_AAAA => {
                if let Ok(ip) = val.parse::<Ipv6Addr>() {
                    out.push(WireAnswer {
                        name: owner_name.to_string(),
                        rr_type: QTYPE_AAAA,
                        ttl,
                        rdata: WireRdata::Aaaa(ip),
                    });
                }
            }
            QTYPE_CNAME | QTYPE_NS => {
                out.push(WireAnswer {
                    name: owner_name.to_string(),
                    rr_type: qtype,
                    ttl,
                    rdata: WireRdata::DomainName(canonical_dns_name(val)),
                });
            }
            QTYPE_TXT => {
                let unquoted = val
                    .strip_prefix('"')
                    .and_then(|inner| inner.strip_suffix('"'))
                    .unwrap_or(val);
                out.push(WireAnswer {
                    name: owner_name.to_string(),
                    rr_type: QTYPE_TXT,
                    ttl,
                    rdata: WireRdata::Txt(unquoted.to_string()),
                });
            }
            QTYPE_SOA => {
                if let Some(soa) = parse_soa_rdata(val) {
                    out.push(WireAnswer {
                        name: owner_name.to_string(),
                        rr_type: QTYPE_SOA,
                        ttl,
                        rdata: soa,
                    });
                }
            }
            _ => {}
        }
    }
}

fn extract_ipv4_from_endpoint(endpoint: &str) -> Option<Ipv4Addr> {
    let trimmed = endpoint.trim().trim_end_matches('.');
    if let Ok(ip) = trimmed.parse::<Ipv4Addr>() {
        return Some(ip);
    }
    let (host, _port) = trimmed.rsplit_once(':')?;
    host.parse::<Ipv4Addr>().ok()
}

fn parse_soa_rdata(value: &str) -> Option<WireRdata> {
    let mut parts = value.split_whitespace();
    let mname = canonical_dns_name(parts.next()?);
    let rname = canonical_dns_name(parts.next()?);
    let serial = parts.next()?.parse::<u32>().ok()?;
    let refresh = parts.next()?.parse::<u32>().ok()?;
    let retry = parts.next()?.parse::<u32>().ok()?;
    let expire = parts.next()?.parse::<u32>().ok()?;
    let minimum = parts.next()?.parse::<u32>().ok()?;
    Some(WireRdata::Soa {
        mname,
        rname,
        serial,
        refresh,
        retry,
        expire,
        minimum,
    })
}

fn decode_name(packet: &[u8], start: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut pos = start;
    let mut end_after_first = None;
    let mut jumps = 0usize;

    loop {
        let len_byte = *packet.get(pos)?;
        if len_byte == 0 {
            let final_pos = end_after_first.unwrap_or(pos + 1);
            let joined = if labels.is_empty() {
                ".".to_string()
            } else {
                format!("{}.", labels.join("."))
            };
            return Some((joined, final_pos));
        }
        if (len_byte & 0xC0) == 0xC0 {
            let second = *packet.get(pos + 1)?;
            if end_after_first.is_none() {
                end_after_first = Some(pos + 2);
            }
            jumps += 1;
            if jumps > MAX_COMPRESSION_JUMPS {
                return None;
            }
            pos = (((usize::from(len_byte) & 0x3F) << 8) | usize::from(second)) as usize;
            continue;
        }
        if (len_byte & 0xC0) != 0 {
            return None;
        }
        let label_len = usize::from(len_byte);
        let label_bytes = packet.get(pos + 1..pos + 1 + label_len)?;
        let label = std::str::from_utf8(label_bytes).ok()?;
        labels.push(label.to_string());
        pos += 1 + label_len;
    }
}

fn encode_domain_name(name: &str, out: &mut Vec<u8>) {
    let trimmed = name.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        out.push(0);
        return;
    }
    for label in trimmed.split('.') {
        let bytes = label.as_bytes();
        let len = bytes.len().min(63);
        out.push(len as u8);
        out.extend_from_slice(&bytes[..len]);
    }
    out.push(0);
}

fn encode_answer_rr(answer: &WireAnswer, out: &mut Vec<u8>) {
    encode_domain_name(&answer.name, out);
    out.extend_from_slice(&answer.rr_type.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    out.extend_from_slice(&answer.ttl.to_be_bytes());

    let mut rdata = Vec::new();
    match &answer.rdata {
        WireRdata::A(ip) => rdata.extend_from_slice(&ip.octets()),
        WireRdata::Aaaa(ip) => rdata.extend_from_slice(&ip.octets()),
        WireRdata::DomainName(domain) => encode_domain_name(domain, &mut rdata),
        WireRdata::Txt(text) => {
            let bytes = text.as_bytes();
            if bytes.is_empty() {
                rdata.push(0);
            } else {
                for chunk in bytes.chunks(255) {
                    rdata.push(chunk.len() as u8);
                    rdata.extend_from_slice(chunk);
                }
            }
        }
        WireRdata::Soa {
            mname,
            rname,
            serial,
            refresh,
            retry,
            expire,
            minimum,
        } => {
            encode_domain_name(mname, &mut rdata);
            encode_domain_name(rname, &mut rdata);
            rdata.extend_from_slice(&serial.to_be_bytes());
            rdata.extend_from_slice(&refresh.to_be_bytes());
            rdata.extend_from_slice(&retry.to_be_bytes());
            rdata.extend_from_slice(&expire.to_be_bytes());
            rdata.extend_from_slice(&minimum.to_be_bytes());
        }
    }

    let rdlength = u16::try_from(rdata.len()).unwrap_or(0);
    out.extend_from_slice(&rdlength.to_be_bytes());
    out.extend_from_slice(&rdata[..usize::from(rdlength)]);
}

fn encode_response(
    id: u16,
    rd_flag: u16,
    rcode: u16,
    question_slice: &[u8],
    answers: &[WireAnswer],
    max_len: usize,
) -> Vec<u8> {
    let qdcount: u16 = u16::from(!question_slice.is_empty());
    let mut pkt = Vec::with_capacity(128);
    // QR=1 (0x8000), AA=1 (0x0400), RA=0, preserve RD, attach RCODE.
    let mut flags: u16 = 0x8400 | rd_flag | (rcode & 0x000F);
    pkt.extend_from_slice(&id.to_be_bytes());
    pkt.extend_from_slice(&flags.to_be_bytes());
    pkt.extend_from_slice(&qdcount.to_be_bytes());
    pkt.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT placeholder at [6..8]
    pkt.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    pkt.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    pkt.extend_from_slice(question_slice);

    let mut ancount: u16 = 0;
    for ans in answers {
        let mut encoded = Vec::new();
        encode_answer_rr(ans, &mut encoded);
        if pkt.len() + encoded.len() > max_len {
            // Set TC=1 (truncated) so clients retry over TCP.
            flags |= 0x0200;
            pkt[2..4].copy_from_slice(&flags.to_be_bytes());
            break;
        }
        pkt.extend_from_slice(&encoded);
        ancount = ancount.saturating_add(1);
    }
    pkt[6..8].copy_from_slice(&ancount.to_be_bytes());
    pkt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::shared_types::{AliasTarget, ResourceRecord};
    use crate::state::RecordKey;
    use std::collections::BTreeMap;

    fn build_query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
        let mut pkt = Vec::new();
        pkt.extend_from_slice(&id.to_be_bytes());
        pkt.extend_from_slice(&0x0100u16.to_be_bytes()); // RD=1
        pkt.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT=1
        pkt.extend_from_slice(&0u16.to_be_bytes());
        pkt.extend_from_slice(&0u16.to_be_bytes());
        pkt.extend_from_slice(&0u16.to_be_bytes());
        encode_domain_name(name, &mut pkt);
        pkt.extend_from_slice(&qtype.to_be_bytes());
        pkt.extend_from_slice(&CLASS_IN.to_be_bytes());
        pkt
    }

    fn insert_rr(zone: &mut StoredZone, record: ResourceRecordSet) {
        zone.records.insert(RecordKey::of_record(&record), record);
    }

    fn sample_zone() -> StoredZone {
        let mut zone = StoredZone {
            id: "ZSANDBOX001".to_string(),
            name: "sandbox.local.".to_string(),
            caller_reference: "ref-1".to_string(),
            comment: None,
            private_zone: false,
            name_servers: vec!["ns-1.awsdns-00.org".to_string()],
            vpcs: Vec::new(),
            vpc_authorizations: Vec::new(),
            records: BTreeMap::new(),
            tags: BTreeMap::new(),
            dnssec_signing_enabled: false,
            key_signing_keys: BTreeMap::new(),
            deletion_protection_enabled: false,
            delegation_set_id: None,
            accelerated_recovery: None,
        };

        insert_rr(
            &mut zone,
            ResourceRecordSet {
                name: "db.sandbox.local.".to_string(),
                r#type: "A".to_string(),
                ttl: Some(60),
                resource_records: Some(vec![ResourceRecord {
                    value: "192.168.1.142".to_string(),
                }]),
                alias_target: None,
                cidr_routing_config: None,
                failover: None,
                geo_location: None,
                geo_proximity_location: None,
                health_check_id: None,
                multi_value_answer: None,
                region: None,
                set_identifier: None,
                traffic_policy_instance_id: None,
                weight: None,
            },
        );

        insert_rr(
            &mut zone,
            ResourceRecordSet {
                name: "writer.sandbox.local.".to_string(),
                r#type: "CNAME".to_string(),
                ttl: Some(30),
                resource_records: Some(vec![ResourceRecord {
                    value: "db.sandbox.local.".to_string(),
                }]),
                alias_target: None,
                cidr_routing_config: None,
                failover: None,
                geo_location: None,
                geo_proximity_location: None,
                health_check_id: None,
                multi_value_answer: None,
                region: None,
                set_identifier: None,
                traffic_policy_instance_id: None,
                weight: None,
            },
        );

        insert_rr(
            &mut zone,
            ResourceRecordSet {
                name: "api.sandbox.local.".to_string(),
                r#type: "A".to_string(),
                ttl: None,
                resource_records: None,
                alias_target: Some(AliasTarget {
                    dns_name: "172.25.0.1:49152".to_string(),
                    evaluate_target_health: false,
                    hosted_zone_id: "Z35SXDOTRQ7X7K".to_string(),
                }),
                cidr_routing_config: None,
                failover: None,
                geo_location: None,
                geo_proximity_location: None,
                health_check_id: None,
                multi_value_answer: None,
                region: None,
                set_identifier: None,
                traffic_policy_instance_id: None,
                weight: None,
            },
        );

        insert_rr(
            &mut zone,
            ResourceRecordSet {
                name: "*.apps.sandbox.local.".to_string(),
                r#type: "A".to_string(),
                ttl: Some(60),
                resource_records: Some(vec![ResourceRecord {
                    value: "172.25.0.10".to_string(),
                }]),
                alias_target: None,
                cidr_routing_config: None,
                failover: None,
                geo_location: None,
                geo_proximity_location: None,
                health_check_id: None,
                multi_value_answer: None,
                region: None,
                set_identifier: None,
                traffic_policy_instance_id: None,
                weight: None,
            },
        );

        insert_rr(
            &mut zone,
            ResourceRecordSet {
                name: "_acme.sandbox.local.".to_string(),
                r#type: "TXT".to_string(),
                ttl: Some(60),
                resource_records: Some(vec![ResourceRecord {
                    value: "\"token-123\"".to_string(),
                }]),
                alias_target: None,
                cidr_routing_config: None,
                failover: None,
                geo_location: None,
                geo_proximity_location: None,
                health_check_id: None,
                multi_value_answer: None,
                region: None,
                set_identifier: None,
                traffic_policy_instance_id: None,
                weight: None,
            },
        );

        zone
    }

    #[tokio::test]
    async fn serves_live_zones_over_udp_and_tcp_and_refuses_outside_domains_with_nxdomain() {
        let engine = ::engine::InMemoryEngine::new();
        let state = service_state(&engine).expect("route53 state");
        {
            let mut guard = state.lock();
            let zone = sample_zone();
            guard.zones.insert(zone.id.clone(), zone);
        }

        let server = SandboxDnsServer::bind(
            "127.0.0.1:0".parse().expect("loopback"),
            state,
            Ipv4Addr::new(172, 25, 0, 1),
        )
        .await
        .expect("bind sandbox dns");
        let addr = server.local_addr();

        // 1. UDP query for CNAME -> A chain (`writer.sandbox.local` -> `db.sandbox.local` -> 192.168.1.142)
        let client = UdpSocket::bind("127.0.0.1:0").await.expect("udp client");
        let query = build_query(0x1234, "writer.sandbox.local", QTYPE_A);
        client.send_to(&query, addr).await.expect("send udp");
        let mut resp = [0u8; 512];
        let (n, _) = client.recv_from(&mut resp).await.expect("recv udp");
        assert_eq!(&resp[0..2], &[0x12, 0x34]);
        assert_eq!(resp[3] & 0x0F, RCODE_NOERROR as u8);
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 2); // CNAME + A
        assert_eq!(&resp[n - 4..n], &[192, 168, 1, 142]);

        // 2. UDP query for ALB `AliasTarget` (`172.25.0.1:49152` -> `172.25.0.1`)
        let alias_q = build_query(0x2222, "api.sandbox.local", QTYPE_A);
        client.send_to(&alias_q, addr).await.expect("send alias");
        let (n, _) = client.recv_from(&mut resp).await.expect("recv alias");
        assert_eq!(resp[3] & 0x0F, RCODE_NOERROR as u8);
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 1);
        assert_eq!(&resp[n - 4..n], &[172, 25, 0, 1]);

        // 3. UDP query for wildcard `web.apps.sandbox.local` -> `172.25.0.10`
        let wild_q = build_query(0x3333, "web.apps.sandbox.local", QTYPE_A);
        client.send_to(&wild_q, addr).await.expect("send wildcard");
        let (n, _) = client.recv_from(&mut resp).await.expect("recv wildcard");
        assert_eq!(resp[3] & 0x0F, RCODE_NOERROR as u8);
        assert_eq!(&resp[n - 4..n], &[172, 25, 0, 10]);

        // 4. UDP query for external internet domain -> NXDOMAIN (closed-world sandbox)
        let ext_q = build_query(0x4444, "sqs.us-east-1.amazonaws.com", QTYPE_A);
        client.send_to(&ext_q, addr).await.expect("send external");
        let (_n, _) = client.recv_from(&mut resp).await.expect("recv external");
        assert_eq!(resp[3] & 0x0F, RCODE_NXDOMAIN as u8);
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 0);

        // 5. TCP length-prefixed query for TXT record
        let mut tcp = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect tcp");
        let txt_q = build_query(0x5555, "_acme.sandbox.local", QTYPE_TXT);
        let len_bytes = u16::try_from(txt_q.len()).expect("len").to_be_bytes();
        tcp.write_all(&len_bytes).await.expect("write len");
        tcp.write_all(&txt_q).await.expect("write query");
        let mut rlen_buf = [0u8; 2];
        tcp.read_exact(&mut rlen_buf).await.expect("read rlen");
        let rlen = u16::from_be_bytes(rlen_buf) as usize;
        let mut tcp_resp = vec![0u8; rlen];
        tcp.read_exact(&mut tcp_resp).await.expect("read tcp resp");
        assert_eq!(tcp_resp[3] & 0x0F, RCODE_NOERROR as u8);
        assert!(tcp_resp.windows(9).any(|win| win == b"token-123"));

        server.shutdown();
    }
}
