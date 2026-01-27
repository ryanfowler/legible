//! SSRF (Server-Side Request Forgery) protection module.
//!
//! This module provides a custom DNS resolver that validates all resolved IP addresses
//! to prevent SSRF and DNS rebinding attacks.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use ipnet::{Ipv4Net, Ipv6Net};
use once_cell::sync::Lazy;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// IPv4 networks that are blocked (private, loopback, link-local, etc.)
static BLOCKED_IPV4_NETS: Lazy<Vec<Ipv4Net>> = Lazy::new(|| {
    vec![
        // Loopback (127.0.0.0/8)
        "127.0.0.0/8".parse().unwrap(),
        // Private networks (RFC 1918)
        "10.0.0.0/8".parse().unwrap(),
        "172.16.0.0/12".parse().unwrap(),
        "192.168.0.0/16".parse().unwrap(),
        // Link-local (includes AWS/GCP/Azure metadata at 169.254.169.254)
        "169.254.0.0/16".parse().unwrap(),
        // Current network (RFC 1122)
        "0.0.0.0/8".parse().unwrap(),
        // Shared address space (RFC 6598) - carrier-grade NAT
        "100.64.0.0/10".parse().unwrap(),
        // IETF Protocol Assignments (RFC 6890)
        "192.0.0.0/24".parse().unwrap(),
        // Documentation (RFC 5737)
        "192.0.2.0/24".parse().unwrap(),
        "198.51.100.0/24".parse().unwrap(),
        "203.0.113.0/24".parse().unwrap(),
        // Benchmarking (RFC 2544)
        "198.18.0.0/15".parse().unwrap(),
        // Multicast (RFC 5771)
        "224.0.0.0/4".parse().unwrap(),
        // Reserved for future use (RFC 1112)
        "240.0.0.0/4".parse().unwrap(),
        // Broadcast
        "255.255.255.255/32".parse().unwrap(),
    ]
});

/// IPv6 networks that are blocked (private, loopback, link-local, etc.)
static BLOCKED_IPV6_NETS: Lazy<Vec<Ipv6Net>> = Lazy::new(|| {
    vec![
        // Loopback (::1)
        "::1/128".parse().unwrap(),
        // Unspecified address
        "::/128".parse().unwrap(),
        // IPv4-mapped addresses (handled specially, but block the range)
        "::ffff:0:0/96".parse().unwrap(),
        // IPv4-compatible (deprecated)
        "::/96".parse().unwrap(),
        // Unique local addresses (RFC 4193) - IPv6 private
        "fc00::/7".parse().unwrap(),
        // Link-local
        "fe80::/10".parse().unwrap(),
        // Multicast
        "ff00::/8".parse().unwrap(),
        // Documentation (RFC 3849)
        "2001:db8::/32".parse().unwrap(),
        // Teredo tunneling (potential bypass)
        "2001::/32".parse().unwrap(),
        // 6to4 (potential bypass)
        "2002::/16".parse().unwrap(),
    ]
});

/// Check if an IPv4 address is safe (not in blocked ranges).
fn is_ipv4_safe(ip: Ipv4Addr) -> bool {
    !BLOCKED_IPV4_NETS.iter().any(|net| net.contains(&ip))
}

/// Check if an IPv6 address is safe (not in blocked ranges).
fn is_ipv6_safe(ip: Ipv6Addr) -> bool {
    // Check if it's an IPv4-mapped IPv6 address (::ffff:x.x.x.x)
    if let Some(ipv4) = ip.to_ipv4_mapped() {
        return is_ipv4_safe(ipv4);
    }

    !BLOCKED_IPV6_NETS.iter().any(|net| net.contains(&ip))
}

/// Check if an IP address is safe to connect to.
///
/// Returns `true` if the IP is a publicly routable address,
/// `false` if it's a private, loopback, link-local, or other blocked address.
pub fn is_ip_safe(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => is_ipv4_safe(ipv4),
        IpAddr::V6(ipv6) => is_ipv6_safe(ipv6),
    }
}

/// A DNS resolver that validates resolved IP addresses against SSRF attacks.
///
/// This resolver performs standard DNS resolution, then filters out any
/// IP addresses that point to private networks, loopback, link-local,
/// or other potentially dangerous destinations.
#[derive(Debug, Clone, Copy)]
pub struct SafeResolver;

impl Resolve for SafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            // Use tokio's DNS resolver
            let addrs = tokio::net::lookup_host((name.as_str(), 0)).await?;

            // Filter to only safe addresses
            let safe_addrs: Vec<SocketAddr> = addrs.filter(|addr| is_ip_safe(addr.ip())).collect();

            if safe_addrs.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "URL resolves to a blocked IP address (private network, loopback, or reserved range)",
                )
                .into());
            }

            let addrs: Addrs = Box::new(safe_addrs.into_iter());
            Ok(addrs)
        })
    }
}

/// Maximum response size in bytes (10 MB).
pub const MAX_RESPONSE_SIZE: u64 = 10 * 1024 * 1024;

/// Build a reqwest client with SSRF protection enabled.
pub fn build_safe_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (compatible; Legible/1.0)")
        .dns_resolver(Arc::new(SafeResolver))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .expect("failed to build HTTP client")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loopback_blocked() {
        assert!(!is_ip_safe("127.0.0.1".parse().unwrap()));
        assert!(!is_ip_safe("127.0.0.2".parse().unwrap()));
        assert!(!is_ip_safe("127.255.255.255".parse().unwrap()));
        assert!(!is_ip_safe("::1".parse().unwrap()));
    }

    #[test]
    fn test_private_networks_blocked() {
        // 10.0.0.0/8
        assert!(!is_ip_safe("10.0.0.1".parse().unwrap()));
        assert!(!is_ip_safe("10.255.255.255".parse().unwrap()));

        // 172.16.0.0/12
        assert!(!is_ip_safe("172.16.0.1".parse().unwrap()));
        assert!(!is_ip_safe("172.31.255.255".parse().unwrap()));
        // 172.32.x.x is NOT private
        assert!(is_ip_safe("172.32.0.1".parse().unwrap()));

        // 192.168.0.0/16
        assert!(!is_ip_safe("192.168.0.1".parse().unwrap()));
        assert!(!is_ip_safe("192.168.255.255".parse().unwrap()));
    }

    #[test]
    fn test_link_local_blocked() {
        // IPv4 link-local (includes cloud metadata endpoints)
        assert!(!is_ip_safe("169.254.0.1".parse().unwrap()));
        assert!(!is_ip_safe("169.254.169.254".parse().unwrap())); // AWS/GCP metadata
        assert!(!is_ip_safe("169.254.255.255".parse().unwrap()));

        // IPv6 link-local
        assert!(!is_ip_safe("fe80::1".parse().unwrap()));
    }

    #[test]
    fn test_ipv6_private_blocked() {
        // Unique local addresses (fc00::/7)
        assert!(!is_ip_safe("fc00::1".parse().unwrap()));
        assert!(!is_ip_safe("fd00::1".parse().unwrap()));
    }

    #[test]
    fn test_ipv4_mapped_ipv6_blocked() {
        // ::ffff:127.0.0.1
        assert!(!is_ip_safe("::ffff:127.0.0.1".parse().unwrap()));
        // ::ffff:10.0.0.1
        assert!(!is_ip_safe("::ffff:10.0.0.1".parse().unwrap()));
        // ::ffff:169.254.169.254
        assert!(!is_ip_safe("::ffff:169.254.169.254".parse().unwrap()));
    }

    #[test]
    fn test_public_ips_allowed() {
        assert!(is_ip_safe("8.8.8.8".parse().unwrap())); // Google DNS
        assert!(is_ip_safe("1.1.1.1".parse().unwrap())); // Cloudflare DNS
        assert!(is_ip_safe("93.184.216.34".parse().unwrap())); // example.com
        assert!(is_ip_safe(
            "2606:2800:220:1:248:1893:25c8:1946".parse().unwrap()
        )); // example.com IPv6
    }

    #[test]
    fn test_multicast_blocked() {
        assert!(!is_ip_safe("224.0.0.1".parse().unwrap()));
        assert!(!is_ip_safe("239.255.255.255".parse().unwrap()));
        assert!(!is_ip_safe("ff02::1".parse().unwrap()));
    }

    #[test]
    fn test_documentation_ranges_blocked() {
        assert!(!is_ip_safe("192.0.2.1".parse().unwrap()));
        assert!(!is_ip_safe("198.51.100.1".parse().unwrap()));
        assert!(!is_ip_safe("203.0.113.1".parse().unwrap()));
        assert!(!is_ip_safe("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn test_special_ranges_blocked() {
        // Current network
        assert!(!is_ip_safe("0.0.0.0".parse().unwrap()));
        // Broadcast
        assert!(!is_ip_safe("255.255.255.255".parse().unwrap()));
        // Reserved
        assert!(!is_ip_safe("240.0.0.1".parse().unwrap()));
        // Carrier-grade NAT
        assert!(!is_ip_safe("100.64.0.1".parse().unwrap()));
    }
}
