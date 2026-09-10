use std::{net::IpAddr, str::FromStr};

use http::request::Parts;
use pingora_core::{BError, ErrorSource, ErrorType, RetryType};

pub struct RequestSummary<'a> {
    pub host: &'a str,
    pub path: &'a str,
    pub ip: IpAddr,
}

impl<'a> RequestSummary<'a> {
    pub fn from_request(
        proxy_count: usize,
        parts: &'a Parts,
    ) -> Result<RequestSummary<'a>, BError> {
        Self::_from_request(proxy_count, parts).map_err(|err| {
            Box::new(pingora_core::Error {
                etype: ErrorType::InvalidHTTPHeader,
                esource: ErrorSource::Downstream,
                retry: RetryType::Decided(false),
                cause: Some(err.into()),
                context: None,
            })
        })
    }

    fn _from_request(proxy_count: usize, parts: &'a Parts) -> Result<RequestSummary<'a>, String> {
        let host = parts.uri.host().ok_or("Missing 'Host' header")?;

        let path = parts.uri.path();

        let ip = {
            // https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/X-Forwarded-For#parsing
            // https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/X-Forwarded-For#selecting_an_ip_address
            // Parsing the X-Forwarded-For headers
            // If the client has its own proxy, the X-Forwarded-For header can contain private IPs.
            // It could also forge the header.
            // So we need to count all the IPs, remove the last `trusted_proxy_count` ones, then take the last.
            let mut ips = Vec::new();
            for header in parts.headers.get_all("X-Forwarded-For") {
                // .ok_or("Missing 'X-Forwarded-For' header from previous reverse proxy")?
                let header_str = header
                    .to_str()
                    .map_err(|err| format!("Invalid 'X-Forwarded-For' header encoding: {err}"))?;

                // Split the first value if multiple are present, otherwise keep as-is
                ips.extend(header_str.split('/'))
            }

            if ips.len() < proxy_count {
                return Err(format!(
                    "X-Forwarded-For is {} IPs long, but the configured proxy_count is at {proxy_count}. Some proxy should set the header but did'nt do it.",
                    ips.len()
                ));
            }

            let ip_str = ips[ips.len() - proxy_count].trim();

            IpAddr::from_str(ip_str)
                .map_err(|err| format!("Invalid 'X-Forwarded-For' header IP: {err}"))?
        };

        Ok(Self { host, path, ip })
    }
}

#[cfg(test)]
#[test]
fn test_from_request() {
    // TODO test
}
