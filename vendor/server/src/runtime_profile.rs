// SPDX-License-Identifier: CC0-1.0

//! One resolved network selection for the standalone process.
//!
//! The command-line overrides must be installed before constructing a router or
//! opening resource listeners. The first read freezes both CLI and environment
//! inputs, so resource listeners and certificate names cannot independently drift
//! away from the production API's bind. Explicit-address embedding APIs retain
//! their own address argument; they do not rewrite this process-wide selection.

use std::net::IpAddr;
use std::sync::OnceLock;

use crate::ServeError;

/// The network part of the boot profile. No listener is opened by resolving it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkProfile {
    bind_address: IpAddr,
    data_port: u16,
}

impl NetworkProfile {
    /// Resolve explicit CLI overrides before environment defaults. Malformed bind
    /// input is refused unless an explicit CLI bind supersedes it. Port parsing
    /// retains the existing default-on-invalid compatibility behavior.
    pub fn resolve(
        bind_override: Option<IpAddr>,
        port_override: Option<u16>,
        environment_bind: Option<&str>,
        environment_port: Option<&str>,
    ) -> Result<Self, ServeError> {
        Ok(Self {
            bind_address: match bind_override {
                Some(address) => address,
                None => crate::parse_bind_address(environment_bind)?,
            },
            data_port: port_override
                .unwrap_or_else(|| crate::parse_port(environment_port, crate::DEFAULT_DATA_PORT)),
        })
    }

    pub fn bind_address(&self) -> IpAddr {
        self.bind_address
    }

    /// Requested port; zero still means an OS-selected port. Publication must
    /// use the listener's actual post-bind port, never this value verbatim.
    pub fn data_port(&self) -> u16 {
        self.data_port
    }

    pub fn advertised_ip(&self) -> IpAddr {
        crate::dialable_host(self.bind_address)
    }
}

static NETWORK: OnceLock<Result<NetworkProfile, String>> = OnceLock::new();

/// The standalone process's immutable network selection. Errors are retained as
/// well as successes, rather than retrying against a changed environment later.
pub fn network() -> Result<&'static NetworkProfile, ServeError> {
    match NETWORK.get_or_init(|| {
        let (bind_override, port_override) = crate::cli::listen_overrides();
        let environment_bind = if bind_override.is_none() {
            match std::env::var(crate::BIND_ADDRESS_ENV) {
                Ok(value) => Some(value),
                Err(std::env::VarError::NotPresent) => None,
                Err(std::env::VarError::NotUnicode(_)) => {
                    return Err(format!(
                        "{} is set but is not valid UTF-8",
                        crate::BIND_ADDRESS_ENV
                    ));
                }
            }
        } else {
            None
        };
        let environment_port = std::env::var(crate::DATA_PORT_ENV).ok();
        NetworkProfile::resolve(
            bind_override,
            port_override,
            environment_bind.as_deref(),
            environment_port.as_deref(),
        )
        .map_err(|error| error.to_string())
    }) {
        Ok(profile) => Ok(profile),
        Err(message) => Err(ServeError::BindAddress(message.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_bind_wins_over_conflicting_or_malformed_environment() {
        let selected = "127.0.0.2".parse().unwrap();
        for environment in ["0.0.0.0", "not-an-address"] {
            let profile =
                NetworkProfile::resolve(Some(selected), Some(0), Some(environment), Some("9297"))
                    .unwrap();
            assert_eq!(profile.bind_address(), selected);
            assert_eq!(profile.advertised_ip(), selected);
            assert_eq!(profile.data_port(), 0);
        }
    }

    #[test]
    fn invalid_environment_without_override_is_not_a_default_bind() {
        assert!(NetworkProfile::resolve(None, None, Some("not-an-address"), None).is_err());
    }

    #[test]
    fn defaults_and_wildcard_publication_keep_existing_behavior() {
        let default = NetworkProfile::resolve(None, None, None, None).unwrap();
        assert_eq!(default.bind_address(), crate::DEFAULT_BIND_ADDRESS);
        assert_eq!(default.data_port(), crate::DEFAULT_DATA_PORT);
        let wildcard = NetworkProfile::resolve(None, None, Some("::"), Some("0")).unwrap();
        assert_eq!(wildcard.advertised_ip(), "::1".parse::<IpAddr>().unwrap());
        assert_eq!(wildcard.data_port(), 0);
    }
}
