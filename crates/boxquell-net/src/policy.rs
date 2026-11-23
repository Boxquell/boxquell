//! Per-sandbox network policy. Rules are checked in order and the first match
//! decides. What happens to everything else depends on the mode: a whitelist
//! blocks it, a blacklist lets it through.

use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Allow,
    Deny,
}

/// Serialized as its text form, e.g. `"allow example.com:443"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Rule {
    /// A name: `example.com` (exact), `*.example.com` (subdomains only) or `*` (every name).
    /// An address: `192.0.2.7` or a network such as `10.0.0.0/8`.
    pub host: String,
    /// `None` matches every port.
    pub port: Option<u16>,
    pub action: Action,
}

impl Rule {
    pub fn new(action: Action, spec: &str) -> Result<Self, String> {
        let spec = spec.trim().to_ascii_lowercase();
        let (host, port) = match spec.rsplit_once(':') {
            Some((h, p)) => (h, Some(p.parse::<u16>().map_err(|_| format!("invalid port in {spec:?}"))?)),
            None => (spec.as_str(), None),
        };
        if let Some(net) = parse_net(host) {
            return Ok(Self { host: net_text(net), port, action });
        }
        if host.contains('/') || host.split('.').all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit())) {
            return Err(format!("invalid address or network {host:?}: expected e.g. 192.0.2.7 or 10.0.0.0/8"));
        }
        let bare = host.strip_prefix("*.").unwrap_or(host);
        let valid = host == "*"
            || (!bare.is_empty() && bare.split('.').all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')));
        if !valid {
            return Err(format!("invalid host pattern {host:?}"));
        }
        Ok(Self { host: host.to_string(), port, action })
    }

    /// The network of an address rule; `None` for name rules.
    pub fn net(&self) -> Option<(Ipv4Addr, u8)> {
        parse_net(&self.host)
    }

    fn matches_host(&self, host: &str) -> bool {
        self.net().is_none() && host_matches(&self.host, host)
    }

    fn applies_to_port(&self, port: Option<u16>) -> bool {
        match (port, self.port) {
            (_, None) => true,
            (Some(p), Some(rp)) => p == rp,
            (None, Some(_)) => self.action == Action::Allow,
        }
    }
}

/// `a.b.c.d` or `a.b.c.d/len`, with the bits outside the prefix cleared.
fn parse_net(text: &str) -> Option<(Ipv4Addr, u8)> {
    let (addr, len) = match text.split_once('/') {
        Some((addr, len)) => (addr, len.parse::<u8>().ok().filter(|len| *len <= 32)?),
        None => (text, 32),
    };
    let addr: Ipv4Addr = addr.parse().ok()?;
    Some((Ipv4Addr::from(u32::from(addr) & mask(len)), len))
}

fn mask(len: u8) -> u32 {
    if len == 0 {
        0
    } else {
        u32::MAX << (32 - len)
    }
}

fn net_text((addr, len): (Ipv4Addr, u8)) -> String {
    if len == 32 {
        addr.to_string()
    } else {
        format!("{addr}/{len}")
    }
}

/// Whether the lowercase `host` falls under `pattern` (`name`, `*.name` or `*`).
pub fn host_matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        _ if pattern == "*" => true,
        Some(suffix) => host.len() > suffix.len() + 1 && host.ends_with(suffix) && host.as_bytes()[host.len() - suffix.len() - 1] == b'.',
        None => pattern == host,
    }
}

impl std::fmt::Display for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let action = if self.action == Action::Allow { "allow" } else { "deny" };
        match self.port {
            Some(p) => write!(f, "{action} {}:{p}", self.host),
            None => write!(f, "{action} {}", self.host),
        }
    }
}

impl From<Rule> for String {
    fn from(rule: Rule) -> String {
        rule.to_string()
    }
}

impl TryFrom<String> for Rule {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        s.parse()
    }
}

impl FromStr for Rule {
    type Err = String;
    /// `allow example.com:443`, `deny *.tracker.io`
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().split_once(char::is_whitespace) {
            Some(("allow", spec)) => Rule::new(Action::Allow, spec),
            Some(("deny", spec)) => Rule::new(Action::Deny, spec),
            _ => Err(format!("expected 'allow <host>' or 'deny <host>', got {s:?}")),
        }
    }
}

/// What happens to a destination no rule mentions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Blocked: only what is allowed gets out.
    #[default]
    Whitelist,
    /// Let through: only what is denied is stopped.
    Blacklist,
}

impl FromStr for Mode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "whitelist" => Ok(Self::Whitelist),
            "blacklist" => Ok(Self::Blacklist),
            _ => Err(format!("expected 'whitelist' or 'blacklist', got {s:?}")),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    #[serde(default)]
    pub mode: Mode,
    pub rules: Vec<Rule>,
}

/// Outcome of a policy lookup, with the text that goes into the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub allowed: bool,
    pub reason: String,
}

impl Policy {
    /// Decide about a host name using the name rules. `port` is `None` for name
    /// resolution, where a name is usable as soon as any port of it is allowed
    /// and port-specific denies do not apply.
    pub fn check(&self, host: &str, port: Option<u16>) -> Verdict {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        match self.rules.iter().find(|rule| rule.matches_host(&host) && rule.applies_to_port(port)) {
            Some(rule) => Verdict { allowed: rule.action == Action::Allow, reason: rule.to_string() },
            None => self.unmatched(),
        }
    }

    /// The first address rule covering `ip`, if any. Name rules, including `*`, never
    /// cover an address: private and local networks have to be named explicitly.
    pub fn check_ip(&self, ip: Ipv4Addr, port: Option<u16>) -> Option<Verdict> {
        self.rules
            .iter()
            .find(|rule| rule.net().is_some_and(|(net, len)| u32::from(ip) & mask(len) == u32::from(net)) && rule.applies_to_port(port))
            .map(|rule| Verdict { allowed: rule.action == Action::Allow, reason: rule.to_string() })
    }

    /// The verdict for a destination no rule mentions.
    pub fn unmatched(&self) -> Verdict {
        match self.mode {
            Mode::Whitelist => Verdict { allowed: false, reason: "no matching rule".into() },
            Mode::Blacklist => Verdict { allowed: true, reason: "blacklist mode: not blocked".into() },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(rules: &[&str]) -> Policy {
        Policy { mode: Mode::Whitelist, rules: rules.iter().map(|r| r.parse().unwrap()).collect() }
    }

    #[test]
    fn default_is_deny() {
        assert!(!Policy::default().check("example.com", Some(443)).allowed);
    }

    #[test]
    fn exact_and_wildcard_hosts() {
        let p = policy(&["allow example.com", "allow *.github.com"]);
        assert!(p.check("Example.COM.", Some(443)).allowed);
        assert!(!p.check("sub.example.com", Some(443)).allowed);
        assert!(p.check("api.github.com", Some(443)).allowed);
        assert!(!p.check("github.com", Some(443)).allowed);
        assert!(!p.check("evilgithub.com", Some(443)).allowed);
    }

    #[test]
    fn ports_and_order() {
        let p = policy(&["deny api.example.com:80", "allow *.example.com", "deny *"]);
        assert!(!p.check("api.example.com", Some(80)).allowed);
        assert!(p.check("api.example.com", Some(443)).allowed);
        // A port-specific deny must not stop the name from resolving.
        assert!(p.check("api.example.com", None).allowed);
        assert_eq!(p.check("other.org", Some(443)).reason, "deny *");

        let p = policy(&["allow db.internal:5432"]);
        assert!(p.check("db.internal", None).allowed);
        assert!(p.check("db.internal", Some(5432)).allowed);
        assert!(!p.check("db.internal", Some(22)).allowed);
    }

    #[test]
    fn exact_ignores_wildcards() {
        let p = policy(&["allow *", "allow 192.168.1.5:5432"]);
        assert!(!p.check_ip("192.168.1.9".parse().unwrap(), Some(22)).is_some_and(|v| v.allowed));
        assert!(p.check_ip("192.168.1.5".parse().unwrap(), Some(5432)).is_some_and(|v| v.allowed));
        assert!(!p.check_ip("192.168.1.5".parse().unwrap(), Some(22)).is_some_and(|v| v.allowed));
    }

    #[test]
    fn rejects_bad_rules() {
        assert!("allow".parse::<Rule>().is_err());
        assert!("permit example.com".parse::<Rule>().is_err());
        assert!("allow exa mple.com".parse::<Rule>().is_err());
        assert!("allow example.com:99999".parse::<Rule>().is_err());
        assert!("allow *.".parse::<Rule>().is_err());
    }

    #[test]
    fn address_rules_cover_networks_and_normalize() {
        let p = policy(&["deny 10.1.0.0/16", "allow 10.9.9.9/8:22", "allow 192.0.2.7"]);
        assert_eq!(p.rules[1].to_string(), "allow 10.0.0.0/8:22");
        let ip = |text: &str| text.parse::<Ipv4Addr>().unwrap();
        assert!(!p.check_ip(ip("10.1.2.3"), Some(22)).unwrap().allowed);
        assert!(p.check_ip(ip("10.2.2.3"), Some(22)).unwrap().allowed);
        assert!(p.check_ip(ip("10.2.2.3"), Some(80)).is_none());
        assert!(p.check_ip(ip("192.0.2.7"), None).unwrap().allowed);
        assert!(p.check_ip(ip("192.0.2.8"), None).is_none());
        // Address rules say nothing about names, and `*` says nothing about addresses.
        assert!(!p.check("192.0.2.7", None).allowed);
        assert!(policy(&["allow *"]).check_ip(ip("192.168.0.1"), None).is_none());
        assert!("allow 10.0.0.0/33".parse::<Rule>().is_err());
        assert!("allow 300.1.1.1".parse::<Rule>().is_err());
    }

    #[test]
    fn blacklist_lets_unmentioned_names_through() {
        let mut p = policy(&["deny *.tracker.example", "deny example.org:25"]);
        p.mode = Mode::Blacklist;
        assert!(p.check("example.com", Some(443)).allowed);
        assert!(!p.check("a.tracker.example", None).allowed);
        assert!(p.check("example.org", Some(443)).allowed);
        assert!(!p.check("example.org", Some(25)).allowed);
    }
}
