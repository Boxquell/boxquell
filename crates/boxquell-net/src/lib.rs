//! Host-side network gateway for boxquell sandboxes.
//!
//! The guest's only NIC ends in this crate. Frames are terminated by a userspace
//! TCP/IP stack, DNS is answered locally, and nothing leaves the host unless the
//! sandbox policy allows the destination. Every attempt is reported as a [`NetEvent`].

pub mod dns;
pub mod event;
pub mod gateway;
pub mod mitm;
pub mod policy;
pub mod proxy;
pub mod sniff;
pub mod stack;
pub mod tls;

pub use event::{EventKind, NetEvent, Phase};
pub use gateway::{spawn, GatewayHandle, SystemUpstream, Upstream};
pub use mitm::{Intercept, Secret};
pub use policy::{Action, Mode, Policy, Rule};
pub use stack::{GuestStream, DNS_IP, GATEWAY_IP, GUEST_IP};
pub use tls::CertAuthority;
