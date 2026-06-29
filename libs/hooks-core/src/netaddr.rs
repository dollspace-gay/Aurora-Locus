//! Pure-bytes IP predicates for the config-time URL SSRF guard (design-commit
//! 8 + 31). No networking, no DNS — just range checks on the parsed address.
//! `pub` so the wired crate (and a future audited outbound-HTTP module) can
//! reuse them for runtime guards; the addendum's pub(crate) note assumed a
//! single crate, but the workspace split needs cross-crate visibility.

use std::net::{Ipv4Addr, Ipv6Addr};

// --- IPv4 manual predicates (ranges std doesn't expose as stable methods) ---

/// 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24 (RFC 5737 documentation).
pub fn is_documentation_v4(a: &Ipv4Addr) -> bool {
    let o = a.octets();
    matches!((o[0], o[1], o[2]), (192, 0, 2) | (198, 51, 100) | (203, 0, 113))
}
/// 100.64.0.0/10 (RFC 6598 shared address space / CGNAT).
pub fn is_shared_v4(a: &Ipv4Addr) -> bool {
    let o = a.octets();
    o[0] == 100 && (o[1] & 0b1100_0000) == 0b0100_0000
}
/// 240.0.0.0/4 reserved (RFC 1112), excluding the 255.255.255.255 broadcast.
pub fn is_reserved_v4(a: &Ipv4Addr) -> bool {
    a.octets()[0] >= 240 && !a.is_broadcast()
}
/// 0.0.0.0/8 "this network" (RFC 1122).
pub fn is_this_network_v4(a: &Ipv4Addr) -> bool {
    a.octets()[0] == 0
}
/// 192.0.0.0/24 IETF protocol assignments (RFC 6890).
pub fn is_ietf_protocol_assignment_v4(a: &Ipv4Addr) -> bool {
    let o = a.octets();
    o[0] == 192 && o[1] == 0 && o[2] == 0
}
/// 198.18.0.0/15 benchmarking (RFC 2544).
pub fn is_benchmarking_v4(a: &Ipv4Addr) -> bool {
    let o = a.octets();
    o[0] == 198 && (o[1] == 18 || o[1] == 19)
}

// --- IPv6 manual predicates ---

/// fc00::/7 unique-local (RFC 4193).
pub fn is_unique_local_v6(a: &Ipv6Addr) -> bool {
    (a.segments()[0] & 0xfe00) == 0xfc00
}
/// fe80::/10 unicast link-local (RFC 4291).
pub fn is_unicast_link_local_v6(a: &Ipv6Addr) -> bool {
    (a.segments()[0] & 0xffc0) == 0xfe80
}
/// fec0::/10 site-local, deprecated (RFC 3879) — still rejected defensively.
pub fn is_site_local_deprecated_v6(a: &Ipv6Addr) -> bool {
    (a.segments()[0] & 0xffc0) == 0xfec0
}
/// 2001:db8::/32 documentation (RFC 3849); 2001:2::/48 benchmarking (RFC 5180).
pub fn is_documentation_or_benchmarking_v6(a: &Ipv6Addr) -> bool {
    let s = a.segments();
    (s[0] == 0x2001 && s[1] == 0x0db8) || (s[0] == 0x2001 && s[1] == 0x0002 && s[2] == 0)
}

/// Whether an IPv4 host must be rejected at config time (loopback, private,
/// link-local, multicast, broadcast, unspecified, or any reserved/special
/// range above). The defensive union — config-time hooks may only target
/// public unicast addresses.
pub fn reject_ipv4(a: &Ipv4Addr) -> bool {
    a.is_loopback()
        || a.is_private()
        || a.is_link_local()
        || a.is_multicast()
        || a.is_broadcast()
        || a.is_unspecified()
        || is_documentation_v4(a)
        || is_shared_v4(a)
        || is_reserved_v4(a)
        || is_this_network_v4(a)
        || is_ietf_protocol_assignment_v4(a)
        || is_benchmarking_v4(a)
}

/// Whether an IPv6 host must be rejected at config time. Includes
/// IPv4-mapped/compatible addresses (checked against the embedded v4).
pub fn reject_ipv6(a: &Ipv6Addr) -> bool {
    if a.is_loopback() || a.is_unspecified() || a.is_multicast() {
        return true;
    }
    // IPv4-mapped (::ffff:0:0/96) and IPv4-compatible — guard the embedded v4.
    if let Some(v4) = a.to_ipv4() {
        if reject_ipv4(&v4) {
            return true;
        }
    }
    is_unique_local_v6(a)
        || is_unicast_link_local_v6(a)
        || is_site_local_deprecated_v6(a)
        || is_documentation_or_benchmarking_v6(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn v4(s: &str) -> Ipv4Addr {
        Ipv4Addr::from_str(s).unwrap()
    }
    fn v6(s: &str) -> Ipv6Addr {
        Ipv6Addr::from_str(s).unwrap()
    }

    #[test]
    fn rejects_internal_ipv4() {
        for s in [
            "127.0.0.1", "10.0.0.1", "192.168.1.1", "172.16.0.1", "169.254.1.1",
            "0.0.0.0", "255.255.255.255", "224.0.0.1", "100.64.0.1", "192.0.2.5",
            "198.18.0.1", "240.0.0.1", "192.0.0.1",
        ] {
            assert!(reject_ipv4(&v4(s)), "{} should be rejected", s);
        }
    }

    #[test]
    fn allows_public_ipv4() {
        for s in ["1.1.1.1", "8.8.8.8", "93.184.216.34", "203.0.114.1"] {
            assert!(!reject_ipv4(&v4(s)), "{} should be allowed", s);
        }
    }

    #[test]
    fn rejects_internal_ipv6() {
        for s in ["::1", "::", "fe80::1", "fc00::1", "fec0::1", "2001:db8::1", "ff02::1", "::ffff:127.0.0.1"] {
            assert!(reject_ipv6(&v6(s)), "{} should be rejected", s);
        }
    }

    #[test]
    fn allows_public_ipv6() {
        assert!(!reject_ipv6(&v6("2606:4700:4700::1111")));
    }
}
