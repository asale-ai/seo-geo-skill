//! URL / SSRF safety layer.
//!
//! Port of the canonical `url_safety.py` module: scheme and authority
//! validation, IPv4-obfuscation canonicalisation, a hard-blocked hostname
//! list covering every documented cloud metadata endpoint, and a resolver
//! that validates *every* address a hostname resolves to before a socket is
//! opened. The resolver is installed on the shared HTTP agent, so redirect
//! targets are validated with the same predicate as the original URL — DNS
//! rebinding between check and connect is not possible.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};

use url::Url;

/// Raised when a URL fails SSRF safety checks.
#[derive(Debug, Clone)]
pub struct UrlSafetyError(pub String);

impl fmt::Display for UrlSafetyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for UrlSafetyError {}

fn err<T>(msg: impl Into<String>) -> Result<T, UrlSafetyError> {
    Err(UrlSafetyError(msg.into()))
}

/// Hostnames refused before DNS resolution. Cloud metadata endpoints are the
/// most common SSRF target, so every documented address across AWS, Azure,
/// GCP, Oracle, and Alibaba is listed explicitly.
const BLOCKED_HOSTNAMES: &[&str] = &[
    "localhost",
    "ip6-localhost",
    "ip6-loopback",
    "metadata.google.internal",
    "metadata.goog",
    "metadata",
    "metadata.azure.com",
    "metadata.ec2.internal",
    "metadata.oraclecloud.com",
    "127.0.0.1",
    "0.0.0.0",
    "::1",
    "169.254.169.254",
    "fd00:ec2::254",
];

/// True iff the address is a public unicast address.
pub fn is_safe_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_documentation()
                // 100.64.0.0/10 carrier-grade NAT
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
                // 192.0.0.0/24 IETF protocol assignments
                || (v4.octets()[0] == 192 && v4.octets()[1] == 0 && v4.octets()[2] == 0)
                // 198.18.0.0/15 benchmarking
                || (v4.octets()[0] == 198 && (v4.octets()[1] & 0xfe) == 18)
                // 240.0.0.0/4 reserved
                || (v4.octets()[0] & 0xf0) == 240)
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_safe_ip(&IpAddr::V4(mapped));
            }
            !(v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                // fc00::/7 unique local
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // fe80::/10 link local
                || (v6.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

/// Does the string look like any glibc/`inet_aton`-friendly IPv4 form?
fn looks_like_obfuscated_ipv4(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }
    host.split('.').all(|part| {
        if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit())
        } else {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())
        }
    }) && host.split('.').count() <= 4
}

/// `inet_aton` semantics: dotted-quad, dotted-octal, dotted-hex, three-part,
/// two-part, and bare-integer forms all canonicalise to dotted quad.
fn inet_aton(host: &str) -> Option<Ipv4Addr> {
    let parse_part = |p: &str| -> Option<u64> {
        if let Some(hex) = p.strip_prefix("0x").or_else(|| p.strip_prefix("0X")) {
            u64::from_str_radix(hex, 16).ok()
        } else if p.len() > 1 && p.starts_with('0') {
            u64::from_str_radix(&p[1..], 8).ok()
        } else {
            p.parse::<u64>().ok()
        }
    };

    let parts: Vec<&str> = host.split('.').collect();
    let nums: Option<Vec<u64>> = parts.iter().map(|p| parse_part(p)).collect();
    let nums = nums?;

    let value: u64 = match nums.len() {
        1 => nums[0],
        2 => {
            if nums[0] > 0xff || nums[1] > 0x00ff_ffff {
                return None;
            }
            (nums[0] << 24) | nums[1]
        }
        3 => {
            if nums[0] > 0xff || nums[1] > 0xff || nums[2] > 0xffff {
                return None;
            }
            (nums[0] << 24) | (nums[1] << 16) | nums[2]
        }
        4 => {
            if nums.iter().any(|n| *n > 0xff) {
                return None;
            }
            (nums[0] << 24) | (nums[1] << 16) | (nums[2] << 8) | nums[3]
        }
        _ => return None,
    };
    if value > u32::MAX as u64 {
        return None;
    }
    Some(Ipv4Addr::from(value as u32))
}

/// Canonicalise a hostname so obfuscated forms cannot bypass the policy.
pub fn normalize_hostname(hostname: &str) -> Result<String, UrlSafetyError> {
    if hostname.is_empty() {
        return err("Empty hostname");
    }
    let mut h = hostname.trim().to_ascii_lowercase();
    if h.ends_with('.') && !h.ends_with("..") {
        h.pop();
    }
    if h.is_empty() {
        return err("Empty hostname");
    }
    // Bracketed IPv6 literals come through the url crate unbracketed already.
    if looks_like_obfuscated_ipv4(&h) {
        match inet_aton(&h) {
            Some(ip) => return Ok(ip.to_string()),
            None => {
                return err(format!("Malformed IPv4 obfuscation refused: {hostname:?}"));
            }
        }
    }
    Ok(h)
}

/// Reject authority forms where URL parsers and HTTP stacks can disagree.
fn reject_authority_confusion(raw: &str, parsed: &Url) -> Result<(), UrlSafetyError> {
    // Recover the raw authority substring so percent-encoding and backslashes
    // are visible; `Url` normalises them away.
    let authority = raw
        .split_once("://")
        .map(|(_, rest)| {
            rest.split(['/', '?', '#'])
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .unwrap_or_default();
    let authority_lower = authority.to_ascii_lowercase();

    if authority.contains('\\') || authority_lower.contains("%5c") {
        return err("URL authority contains a backslash");
    }
    if authority.contains('%') {
        return err("URL authority contains percent-encoding");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() || authority.contains('@') {
        return err("URL userinfo is not allowed");
    }
    if raw.contains("#@") || raw.to_ascii_lowercase().contains("%23@") {
        return err("URL fragment/userinfo confusion refused");
    }
    Ok(())
}

/// Parse-time validator. Does not resolve DNS.
pub fn validate_url(raw: &str) -> bool {
    let parsed = match Url::parse(raw) {
        Ok(u) => u,
        Err(_) => return false,
    };
    if reject_authority_confusion(raw, &parsed).is_err() {
        return false;
    }
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    let host = match parsed.host_str() {
        Some(h) if !h.is_empty() => h,
        _ => return false,
    };
    let host = match normalize_hostname(host) {
        Ok(h) => h,
        Err(_) => return false,
    };
    if BLOCKED_HOSTNAMES.contains(&host.as_str()) {
        return false;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => is_safe_ip(&ip),
        Err(_) => true,
    }
}

/// Resolve and validate a URL's hostname. Returns `(normalized_url, pinned_ip)`.
///
/// Every resolved address must be public: a hostname with one public and one
/// private record is refused so an attacker cannot race the resolver.
pub fn validate_url_strict(raw: &str) -> Result<(String, IpAddr), UrlSafetyError> {
    let parsed = Url::parse(raw).map_err(|e| UrlSafetyError(format!("Invalid URL: {e}")))?;
    reject_authority_confusion(raw, &parsed)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return err(format!("Invalid URL scheme: {:?}", parsed.scheme()));
    }
    let host = parsed
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| UrlSafetyError("URL has no hostname".into()))?;
    let host = normalize_hostname(host)?;

    if BLOCKED_HOSTNAMES.contains(&host.as_str()) {
        return err(format!("Blocked hostname: {host}"));
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        if !is_safe_ip(&ip) {
            return err(format!("Blocked IP literal: {host}"));
        }
        return Ok((raw.to_string(), ip));
    }

    let port = parsed
        .port_or_known_default()
        .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });

    let mut addrs: Vec<SocketAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| UrlSafetyError(format!("DNS resolution failed for {host}: {e}")))?
        .collect();
    addrs.sort_by_key(|a| a.is_ipv6());

    if addrs.is_empty() {
        return err(format!("No DNS records for {host}"));
    }
    for addr in &addrs {
        if !is_safe_ip(&addr.ip()) {
            return err(format!(
                "DNS rebinding refused: {host} resolves to non-public IP {}",
                addr.ip()
            ));
        }
    }
    Ok((raw.to_string(), addrs[0].ip()))
}

/// Resolver installed on the shared HTTP agent. Applies [`is_safe_ip`] to
/// every address for every host looked up during a request, including
/// redirect targets and any host the connection pool revisits.
pub struct SafeResolver;

impl ureq::Resolver for SafeResolver {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        let (host, port) = netloc.rsplit_once(':').ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing port in netloc")
        })?;
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let normalized = normalize_hostname(host).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string())
        })?;
        if BLOCKED_HOSTNAMES.contains(&normalized.as_str()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("url_safety: blocked hostname {normalized}"),
            ));
        }
        let port: u16 = port
            .parse()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid port"))?;
        let mut addrs: Vec<SocketAddr> = (normalized.as_str(), port).to_socket_addrs()?.collect();
        // Try IPv4 first. Plenty of networks advertise IPv6 without working
        // egress, and a host whose AAAA record is unreachable would otherwise
        // look like a dead site rather than a local connectivity problem.
        addrs.sort_by_key(|a| a.is_ipv6());
        if addrs.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("url_safety: no records for {normalized}"),
            ));
        }
        for addr in &addrs {
            if !is_safe_ip(&addr.ip()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "url_safety: refused to resolve {normalized} to non-public IP {}",
                        addr.ip()
                    ),
                ));
            }
        }
        Ok(addrs)
    }
}

/// Prepend `https://` when the caller passed a bare hostname.
pub fn coerce_scheme(input: &str) -> String {
    if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_obfuscated_loopback() {
        assert!(!validate_url("http://2130706433/"));
        assert!(!validate_url("http://0x7f000001/"));
        assert!(!validate_url("http://0177.0.0.1/"));
        assert!(!validate_url("http://127.0.0.1/"));
    }

    #[test]
    fn blocks_metadata_and_fqdn_form() {
        assert!(!validate_url("http://169.254.169.254/latest/meta-data/"));
        assert!(!validate_url("http://metadata.google.internal./"));
    }

    #[test]
    fn blocks_authority_confusion() {
        assert!(!validate_url("https://user:pass@example.com/"));
        assert!(!validate_url("https://example.com\\@evil.test/"));
        assert!(!validate_url("https://exam%70le.com/"));
    }

    #[test]
    fn allows_public_hosts() {
        assert!(validate_url("https://example.com/path?a=1"));
        assert!(validate_url("http://1.1.1.1/"));
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(!validate_url("file:///etc/passwd"));
        assert!(!validate_url("ftp://example.com/"));
    }
}
