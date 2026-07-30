//! Address policy for fetches whose targets come from untrusted HTML.
//!
//! Image URLs are lifted straight out of an article body, so they are attacker
//! influenced in a way the article URL itself is not — Chase typed that one.
//! The guard therefore sits on image fetches only.
//!
//! **The policy lives at address selection, not URL inspection.** Checking the
//! URL before the request is bypassable: ureq follows redirects, so
//! `https://cdn.example/x.png` → 302 → `http://169.254.169.254/` would sail
//! past a pre-flight host check. A [`Resolver`] is consulted for *every* hop,
//! and ureq connects to the address it returned, so there is no second lookup
//! to race.
//!
//! **Proxies must be off for that to hold.** ureq's default config is
//! `Proxy::try_from_env()`, and it deliberately skips local resolution when a
//! proxy will do it (`run.rs`: resolve only `if is_no_proxy || !is_proxy ||
//! is_proxy_local_resolve`). With `HTTP_PROXY` set, this resolver would vet the
//! proxy's address while the proxy received the unvalidated image host. The
//! image agent disables proxies for exactly this reason; see [`crate::images`].
//!
//! What this is not: a general network security boundary. An attacker who
//! controls a *public* address, or a dual-homed host, is still reachable — the
//! policy bounds where a URL can point, not what lives there.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ureq::Error;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::NextTimeout;

/// Wraps ureq's own resolver and drops every address that is not publicly
/// routable. If nothing survives, the host does not resolve.
#[derive(Debug, Default)]
pub struct PublicOnlyResolver(DefaultResolver);

impl Resolver for PublicOnlyResolver {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, Error> {
        let resolved = self.0.resolve(uri, config, timeout)?;
        let mut allowed = self.empty();
        for addr in &resolved {
            if is_public_ip(addr.ip()) {
                allowed.push(*addr);
            }
        }
        if allowed.is_empty() {
            // Deliberately the same error a nonexistent host produces: the
            // caller's job is to omit the image, not to explain the network.
            return Err(Error::HostNotFound);
        }
        Ok(allowed)
    }
}

/// Whether an address is publicly routable.
///
/// Modelled on `IpAddr::is_global`, which is still unstable in std (`ip`
/// feature), but deliberately **not** a faithful copy — it denies a superset.
///
/// Where it differs, and always toward denial:
///
/// - IPv4-mapped and IPv4-translated IPv6 are unwrapped and judged as IPv4
///   rather than denied outright, so `::ffff:8.8.8.8` works while
///   `::ffff:127.0.0.1` and `::ffff:0:a9fe:a9fe` do not.
/// - The well-known NAT64 prefix is judged by its embedded IPv4 address, which
///   std treats as globally reachable.
/// - Whole special-purpose blocks are denied without reproducing std's
///   exception lists — all of `2001::/23`, and deprecated site-local
///   `fec0::/10`. A few genuinely global addresses inside those blocks are
///   refused as collateral; none of them serve article images.
///
/// Conservative on purpose: wrongly omitting an image costs a reader one
/// figure, wrongly reaching a private host is the bug this module exists to
/// prevent. When in doubt, deny.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        // An IPv4-mapped address is an IPv4 destination wearing a v6 costume;
        // `::ffff:127.0.0.1` must not pass by skipping the v4 rules.
        IpAddr::V6(v6) => match embedded_ipv4(v6) {
            Some(v4) => is_public_v4(v4),
            None => is_public_v6(v6),
        },
    }
}

/// The IPv4 address an IPv6 address carries, if it is one of the two forms that
/// wrap v4 in v6.
///
/// `to_ipv4_mapped` alone is not enough. IPv4-*mapped* (`::ffff:0:0/96`) puts
/// `ffff` in group 5, but IPv4-*translated* (`::ffff:0:0:0/96`) puts it in group
/// 4 — so `::ffff:0:a9fe:a9fe` is `169.254.169.254` wearing a shape std's helper
/// does not recognize, and it would otherwise be judged an ordinary public
/// address.
fn embedded_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return Some(v4);
    }
    let segments = ip.segments();
    // ::ffff:0:0:0/96 — IPv4-translated.
    if segments[..4] == [0, 0, 0, 0] && segments[4] == 0xffff && segments[5] == 0 {
        return Some(ipv4_from_segments(segments[6], segments[7]));
    }
    None
}

fn ipv4_from_segments(high: u16, low: u16) -> Ipv4Addr {
    Ipv4Addr::new((high >> 8) as u8, high as u8, (low >> 8) as u8, low as u8)
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    // "This network" is the whole /8, not just 0.0.0.0.
    if a == 0 {
        return false;
    }
    if ip.is_loopback() || ip.is_private() || ip.is_link_local() {
        return false;
    }
    if ip.is_broadcast() || ip.is_multicast() || ip.is_documentation() {
        return false;
    }
    // Carrier-grade NAT, 100.64.0.0/10.
    if a == 100 && (64..128).contains(&b) {
        return false;
    }
    // IETF protocol assignments, 192.0.0.0/24. `.9` and `.10` (PCP anycast)
    // are the only globally routable members.
    if a == 192 && b == 0 && c == 0 && !matches!(ip.octets()[3], 9 | 10) {
        return false;
    }
    // Benchmarking, 198.18.0.0/15.
    if a == 198 && (b == 18 || b == 19) {
        return false;
    }
    // Reserved for future use, 240.0.0.0/4.
    if a >= 240 {
        return false;
    }
    true
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return false;
    }
    let segments = ip.segments();
    // Unique local, fc00::/7.
    if segments[0] & 0xfe00 == 0xfc00 {
        return false;
    }
    // Link-local unicast, fe80::/10.
    if segments[0] & 0xffc0 == 0xfe80 {
        return false;
    }
    // Site-local, fec0::/10. Deprecated by RFC 3879, but deprecated is not
    // unroutable — networks that still carry it would still carry us there.
    if segments[0] & 0xffc0 == 0xfec0 {
        return false;
    }
    // Documentation: 2001:db8::/32 and 3fff::/20.
    if (segments[0] == 0x2001 && segments[1] == 0x0db8) || segments[0] & 0xfff0 == 0x3ff0 {
        return false;
    }
    // Discard-only, 100::/64.
    if segments[0] == 0x0100 && segments[1..4] == [0, 0, 0] {
        return false;
    }
    // Local-use IPv4/IPv6 translation, 64:ff9b:1::/48 (RFC 8215). Explicitly
    // *not* globally reachable — it addresses a translator on the local network.
    if segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2] == 1 {
        return false;
    }
    // IETF protocol assignments, 2001::/23. This is the range that makes 6to4's
    // sibling Teredo reachable, and its handful of genuinely global members
    // (PCP and TURN anycast, AMT, AS112-v6) are of no use to an article image.
    // Deny the block rather than reproduce std's exception list.
    if segments[0] == 0x2001 && segments[1] < 0x0200 {
        return false;
    }
    // 6to4, 2002::/16. The bypass this closes is concrete: 2002:a9fe:a9fe::
    // embeds 169.254.169.254, so a 6to4 relay turns it into a cloud metadata
    // request that never looked like a private address.
    if segments[0] == 0x2002 {
        return false;
    }
    // Segment routing SIDs, 5f00::/16.
    if segments[0] == 0x5f00 {
        return false;
    }
    // NAT64 well-known prefix, 64:ff9b::/96 — the low 32 bits are a real IPv4
    // destination, so judge it as one rather than letting it through whole.
    // std treats this prefix as global; judging the embedded address is
    // deliberately stricter. It only covers the *well-known* prefix: RFC 6052
    // also allows operator-chosen prefixes, which are indistinguishable from
    // ordinary global addresses here.
    if segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2..6] == [0, 0, 0, 0] {
        return is_public_v4(ipv4_from_segments(segments[6], segments[7]));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn public(value: &str) -> bool {
        is_public_ip(IpAddr::from_str(value).unwrap())
    }

    #[test]
    fn private_and_special_v4_ranges_are_refused() {
        for addr in [
            "0.0.0.0",
            "0.1.2.3",
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "192.0.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "240.0.0.1",
            "255.255.255.255",
            "224.0.0.1",
        ] {
            assert!(!public(addr), "{addr} should be refused");
        }
    }

    #[test]
    fn ordinary_public_v4_is_allowed() {
        for addr in ["1.1.1.1", "8.8.8.8", "93.184.216.34", "192.0.0.9"] {
            assert!(public(addr), "{addr} should be allowed");
        }
    }

    #[test]
    fn ipv4_mapped_v6_is_judged_as_ipv4() {
        assert!(!public("::ffff:127.0.0.1"));
        assert!(!public("::ffff:169.254.169.254"));
        assert!(public("::ffff:8.8.8.8"));
    }

    #[test]
    fn nat64_prefix_is_judged_by_its_embedded_v4() {
        assert!(!public("64:ff9b::127.0.0.1"));
        assert!(public("64:ff9b::8.8.8.8"));
    }

    #[test]
    fn ipv4_translated_v6_is_judged_as_ipv4() {
        // `::ffff:0:0:0/96`, not the mapped `::ffff:0:0/96` — `ffff` sits one
        // group earlier, so `to_ipv4_mapped` does not see these at all.
        assert!(!public("::ffff:0:a9fe:a9fe"), "169.254.169.254 translated");
        assert!(!public("::ffff:0:7f00:1"), "127.0.0.1 translated");
        assert!(public("::ffff:0:808:808"), "8.8.8.8 translated");
    }

    #[test]
    fn deprecated_site_local_v6_is_refused() {
        for addr in ["fec0::1", "feff:ffff::1"] {
            assert!(!public(addr), "{addr} should be refused");
        }
    }

    #[test]
    fn private_and_special_v6_ranges_are_refused() {
        for addr in [
            "::",
            "::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "2001:db8::1",
            "3fff::1",
            "ff02::1",
            "100::1",
            "64:ff9b:1::1",
            "2001::1",
            "2001:2::1",
            "5f00::1",
        ] {
            assert!(!public(addr), "{addr} should be refused");
        }
    }

    #[test]
    fn sixtofour_cannot_smuggle_a_private_ipv4_target() {
        // 2002::/16 embeds an IPv4 address in the second and third groups, so
        // without an explicit deny a 6to4 relay turns this into a request to
        // 169.254.169.254 — cloud metadata, reached by an address that never
        // looked private.
        assert!(!public("2002:a9fe:a9fe::"));
        assert!(!public("2002:7f00:1::"));
        assert!(!public("2002:0808:0808::"));
    }

    #[test]
    fn ordinary_public_v6_is_allowed() {
        assert!(public("2606:4700:4700::1111"));
    }

    fn resolve(uri: Uri) -> Result<ResolvedSocketAddrs, Error> {
        let timeout = NextTimeout {
            after: ureq::unversioned::transport::time::Duration::Exact(
                std::time::Duration::from_secs(5),
            ),
            reason: ureq::Timeout::Resolve,
        };
        PublicOnlyResolver::default().resolve(&uri, &Config::builder().build(), timeout)
    }

    #[test]
    fn resolver_refuses_a_host_that_resolves_to_loopback() {
        // Deterministic and offline: localhost resolves through the system
        // resolver without leaving the machine.
        let result = resolve(Uri::from_static("http://localhost/photo.png"));

        assert!(
            matches!(result, Err(Error::HostNotFound)),
            "loopback host must not resolve for image fetches"
        );
    }

    #[test]
    fn resolver_refuses_literal_private_addresses() {
        // The loopback-by-name case above and the EPUB omission tests both pass
        // even with the filter removed — one goes through DNS, the other is
        // refused at connect. These literals can only fail because the policy
        // rejected them, which is what makes them the real guard.
        for uri in [
            Uri::from_static("http://127.0.0.1/photo.png"),
            Uri::from_static("http://169.254.169.254/latest/meta-data/"),
            Uri::from_static("http://10.0.0.1/photo.png"),
            Uri::from_static("http://192.168.1.1/photo.png"),
            Uri::from_static("http://[::1]/photo.png"),
            Uri::from_static("http://[fd00::1]/photo.png"),
        ] {
            let shown = uri.to_string();
            assert!(
                matches!(resolve(uri), Err(Error::HostNotFound)),
                "{shown} must not resolve for image fetches"
            );
        }
    }

    #[test]
    fn resolver_still_admits_an_ordinary_public_literal() {
        // The negative control: if the filter denied everything, the test above
        // would pass for the wrong reason.
        let result = resolve(Uri::from_static("http://1.1.1.1/photo.png"));

        assert!(result.is_ok(), "public literal must still resolve");
    }
}
