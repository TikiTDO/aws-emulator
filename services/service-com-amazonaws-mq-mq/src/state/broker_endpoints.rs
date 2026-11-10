// SPDX-License-Identifier: CC0-1.0

//! Where a client REACHES a broker: the web console URL and the wire-protocol
//! endpoints a running broker publishes.
//!
//! RULE: a broker that is RUNNING reports the instance allocated to it, carrying its
//! console URL and one endpoint URI per wire protocol the broker's ENGINE serves, in
//! the order Amazon MQ publishes those protocols in; a broker still being provisioned
//! has no instance allocated yet and reports none.
//!
//! The protocols, their URI schemes and their ports are the ones Amazon MQ publishes
//! per engine (the Developer Guide's broker endpoint tables): an ActiveMQ broker serves
//! OpenWire, AMQP, STOMP, MQTT and WebSocket, each over TLS on its own port, with the
//! ActiveMQ Web Console on 8162; a RabbitMQ broker serves AMQP over TLS, with its
//! management console on the HTTPS port.
//!
//! The ADDRESS they name is the host this emulator serves on, recorded on the broker
//! when it was created ([`Broker::published_host`]), so what a broker publishes leads
//! back to the emulator the client is already talking to rather than to a hostname
//! nothing here answers. This crate opens no wire listener for a broker: what is
//! reported is the control plane's record of where the broker is published, which is
//! what every member of `BrokerInstance` is.

use super::*;

/// One wire protocol a broker serves: the URI scheme a client dials it with, and the
/// port Amazon MQ publishes that protocol on.
#[derive(Debug, Clone, Copy)]
struct WireProtocol {
    scheme: &'static str,
    port: u16,
}

impl WireProtocol {
    /// The endpoint URI this protocol is reached at on `host`, in the
    /// `<scheme>://<host>:<port>` form Amazon MQ publishes an endpoint as.
    fn endpoint(self, host: &str) -> String {
        format!("{}://{host}:{}", self.scheme, self.port)
    }
}

/// The protocols an ActiveMQ broker serves, each over TLS, in the order Amazon MQ
/// publishes them in: AMQP, MQTT, OpenWire, STOMP and WebSocket \u2014 by protocol name,
/// which is the order a recorded broker answer lists its endpoints in.
const ACTIVEMQ_WIRE_PROTOCOLS: &[WireProtocol] = &[
    WireProtocol {
        scheme: "amqp+ssl",
        port: 5_671,
    },
    WireProtocol {
        scheme: "mqtt+ssl",
        port: 8_883,
    },
    WireProtocol {
        scheme: "ssl",
        port: 61_617,
    },
    WireProtocol {
        scheme: "stomp+ssl",
        port: 61_614,
    },
    WireProtocol {
        scheme: "wss",
        port: 61_619,
    },
];

/// The one protocol a RabbitMQ broker serves: AMQP over TLS.
const RABBITMQ_WIRE_PROTOCOLS: &[WireProtocol] = &[WireProtocol {
    scheme: "amqps",
    port: 5_671,
}];

/// The port the ActiveMQ Web Console is published on.
const ACTIVEMQ_CONSOLE_PORT: u16 = 8_162;

/// The port the RabbitMQ management console is published on: the HTTPS one.
const RABBITMQ_CONSOLE_PORT: u16 = 443;

/// The scheme every broker console URL carries: both consoles are served over TLS.
const CONSOLE_SCHEME: &str = "https";

/// One instance allocated to a broker, as a client addresses it: the console to open,
/// the endpoints to dial, and the broker's own IP address when it has one.
///
/// Held as plain values rather than as the modelled `BrokerInstance` so the record and
/// the response shape stay separate — the handler that answers builds the modelled
/// structure from this.
#[derive(Debug, Clone)]
pub struct BrokerInstanceAddress {
    pub console_url: String,
    pub endpoints: Vec<String>,
    pub ip_address: Option<String>,
}

impl Broker {
    /// The wire protocols this broker's engine serves. A value outside the model's
    /// `EngineType` set reaches no stored broker (a create refuses one), and ActiveMQ
    /// is the engine whose protocol set the model's own members describe.
    fn wire_protocols(&self) -> &'static [WireProtocol] {
        if self.engine_type == ENGINE_TYPE_RABBITMQ {
            RABBITMQ_WIRE_PROTOCOLS
        } else {
            ACTIVEMQ_WIRE_PROTOCOLS
        }
    }

    /// The port this broker's web console is published on.
    fn console_port(&self) -> u16 {
        if self.engine_type == ENGINE_TYPE_RABBITMQ {
            RABBITMQ_CONSOLE_PORT
        } else {
            ACTIVEMQ_CONSOLE_PORT
        }
    }

    /// The broker's own IP address, as `BrokerInstance$ipAddress` reports it ("The IP
    /// address of the Elastic Network Interface (ENI) attached to the broker. Does not
    /// apply to RabbitMQ brokers").
    ///
    /// It is the address the broker is published at, so it is reported only when that
    /// address IS an address rather than a name — a name is not an IP address, and no
    /// address is invented for it.
    fn ip_address(&self) -> Option<String> {
        if self.engine_type == ENGINE_TYPE_RABBITMQ {
            return None;
        }
        let host = self
            .published_host
            .trim_start_matches('[')
            .trim_end_matches(']');
        host.parse::<::std::net::IpAddr>()
            .ok()
            .map(|address| address.to_string())
    }

    /// The instances allocated to this broker as of `now`: one once the clock has made
    /// the broker's move to `RUNNING` due, and none while it is still being
    /// provisioned — an instance that is not allocated yet publishes nothing.
    ///
    /// Also none for a broker whose record carries no published host, which is the one
    /// case there is no address to report rather than a made-up one.
    pub fn allocated_instances(&self, now: SystemTime) -> Vec<BrokerInstanceAddress> {
        if self.broker_state(now) != BROKER_STATE_RUNNING || self.published_host.is_empty() {
            return Vec::new();
        }
        let host = self.published_host.as_str();
        vec![BrokerInstanceAddress {
            console_url: format!("{CONSOLE_SCHEME}://{host}:{}", self.console_port()),
            endpoints: self
                .wire_protocols()
                .iter()
                .map(|protocol| protocol.endpoint(host))
                .collect(),
            ip_address: self.ip_address(),
        }]
    }
}
