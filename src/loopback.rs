//! Loopback checks shared by the HTTP client's endpoint policy and the
//! local HTTP listeners (`serve` and `shim-http`).
//!
//! Binding to loopback does not keep a browser out: a web page can POST to
//! 127.0.0.1 directly or through a rebound DNS name. A listener therefore
//! trusts a request only when it carries a loopback `Host`, a loopback
//! `Origin` when it carries one at all, and `Content-Type:
//! application/json`, which a cross-origin page cannot send without a CORS
//! preflight neither listener grants.

/// `localhost`, or an IPv4 or IPv6 loopback literal.
pub(crate) fn is_loopback_host(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    }
}

/// The `Host`, `Origin`, and `Content-Type` of one request, each allowed
/// at most once.
#[derive(Default)]
pub(crate) struct Provenance {
    host: Option<String>,
    origin: Option<String>,
    content_type: Option<String>,
    duplicate: bool,
}

impl Provenance {
    /// Note one header. A repeat is recorded rather than failing the read:
    /// the request is consumed in full so that closing the socket after the
    /// 400 cannot reset it, which on Linux would discard the response before
    /// the client reads it.
    pub(crate) fn record(&mut self, name: &str, value: &str) {
        let slot = if name.eq_ignore_ascii_case("host") {
            &mut self.host
        } else if name.eq_ignore_ascii_case("origin") {
            &mut self.origin
        } else if name.eq_ignore_ascii_case("content-type") {
            &mut self.content_type
        } else {
            return;
        };
        if slot.is_some() {
            self.duplicate = true;
            return;
        }
        *slot = Some(value.trim().to_owned());
    }

    /// The status and error for a request that did not come from this
    /// machine, or `None` when its `Host` and any `Origin` both name loopback.
    pub(crate) fn rejection(&self) -> Option<(u16, &'static str)> {
        if self.duplicate {
            return Some((400, "Host, Origin, and Content-Type may appear only once"));
        }
        if self
            .origin
            .as_deref()
            .is_some_and(|origin| !is_loopback_origin(origin))
        {
            return Some((403, "Origin is not a loopback origin"));
        }
        match self.host.as_deref() {
            None => Some((400, "Host header is required")),
            Some(host) if !is_loopback_authority(host) => {
                Some((403, "Host is not a loopback host"))
            }
            Some(_) => None,
        }
    }

    /// Whether `Content-Type` is `application/json`, parameters aside.
    pub(crate) fn is_json(&self) -> bool {
        self.content_type.as_deref().is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case("application/json")
        })
    }
}

/// An `Origin` value such as `http://localhost:6274`. The opaque `null`
/// origin of sandboxed frames and local files is not loopback.
fn is_loopback_origin(origin: &str) -> bool {
    url::Url::parse(origin).is_ok_and(|parsed| {
        matches!(parsed.scheme(), "http" | "https")
            && parsed.host().is_some_and(|host| is_loopback_host(&host))
    })
}

/// A `Host` value: host and optional port, nothing else.
fn is_loopback_authority(authority: &str) -> bool {
    !authority.contains(['/', '@', '?', '#', '\\'])
        && url::Url::parse(&format!("http://{authority}/"))
            .is_ok_and(|parsed| parsed.host().is_some_and(|host| is_loopback_host(&host)))
}
