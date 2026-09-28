use std::{net::IpAddr, str::FromStr};

use http::request::Parts;

use crate::error::{HttpBanError, new_bad_setup};

#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub struct RequestSummary<'a> {
    pub host: &'a str,
    pub path: &'a str,
    pub ip: IpAddr,
}

impl<'a> RequestSummary<'a> {
    pub fn from_request(
        proxy_count: usize,
        parts: &'a Parts,
    ) -> Result<RequestSummary<'a>, HttpBanError> {
        let path = match parts.uri.path() {
            "" => "/",
            path => path,
        };

        let host = parts
            .headers
            .get("Host")
            .ok_or("Missing 'Host' header")?
            .to_str()
            .map_err(|_| "Invalid 'Host' header encoding")?;

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
                let header_str = header.to_str().map_err(|err| {
                    new_bad_setup(format!("Invalid 'X-Forwarded-For' header encoding: {err}"))
                })?;

                // Split the first value if multiple are present, otherwise keep as-is
                ips.extend(header_str.split(','))
            }

            if ips.len() < proxy_count {
                return Err(new_bad_setup(format!(
                    "X-Forwarded-For is {} IPs long, but the configured proxy_count is at {proxy_count}. Some proxy should set the header but did'nt do it.",
                    ips.len()
                )).into());
            }

            let ip_str = ips[ips.len() - proxy_count].trim();

            IpAddr::from_str(ip_str).map_err(|err| {
                new_bad_setup(format!("Invalid 'X-Forwarded-For' header IP: {err}"))
            })?
        };

        Ok(Self { host, path, ip })
    }
}

#[cfg(test)]
#[test]
fn test_from_request() {
    use http::{Version, request::Builder};

    assert_eq!(
        RequestSummary::from_request(
            1,
            &Builder::new()
                .method("GET")
                .uri("exemple.fr")
                .version(Version::HTTP_2)
                .header("Host", "exemple.fr")
                .header("X-Forwarded-For", "203.0.113.12")
                .body(())
                .unwrap()
                .into_parts()
                .0
        )
        .unwrap(),
        RequestSummary {
            host: "exemple.fr",
            path: "/",
            ip: "203.0.113.12".parse().unwrap()
        }
    );

    assert_eq!(
        RequestSummary::from_request(
            3,
            &Builder::new()
                .method("GET")
                .uri("https://exemple.fr/is-it-true?reaction=cool")
                .version(Version::HTTP_2)
                .header("Host", "exemple.fr")
                // Client's local IP
                .header("X-Forwarded-For", "10.100.2.3")
                // Client's enterprise proxy
                .header("X-Forwarded-For", "203.0.113.12")
                .header("X-Forwarded-For", "10.0.1.2")
                .header("X-Forwarded-For", "10.0.1.3")
                .body(())
                .unwrap()
                .into_parts()
                .0
        )
        .unwrap(),
        RequestSummary {
            host: "exemple.fr",
            path: "/is-it-true",
            ip: "203.0.113.12".parse().unwrap()
        }
    );

    assert_eq!(
        RequestSummary::from_request(
            4,
            &Builder::new()
                .method("GET")
                .uri("https://exemple.fr/is-it-true?reaction=cool")
                .version(Version::HTTP_2)
                .header("Host", "exemple.fr")
                // Client's local IP
                .header("X-Forwarded-For", "10.100.2.3, 203.0.113.12")
                // Client's enterprise proxy
                .header("X-Forwarded-For", "10.0.1.2, 10.0.1.3")
                .header("X-Forwarded-For", "10.0.1.4")
                .body(())
                .unwrap()
                .into_parts()
                .0
        )
        .unwrap(),
        RequestSummary {
            host: "exemple.fr",
            path: "/is-it-true",
            ip: "203.0.113.12".parse().unwrap()
        }
    );

    assert!(
        // More proxies than X-Forwarded-For headers
        RequestSummary::from_request(
            4,
            &Builder::new()
                .method("GET")
                .uri("https://exemple.fr/is-it-true?reaction=cool")
                .version(Version::HTTP_2)
                .header("Host", "exemple.fr")
                .header("X-Forwarded-For", "10.0.1.2, 10.0.1.3")
                .header("X-Forwarded-For", "10.0.1.4")
                .body(())
                .unwrap()
                .into_parts()
                .0
        )
        .is_err()
    );

    assert!(
        // Missing Host
        RequestSummary::from_request(
            1,
            &Builder::new()
                .method("GET")
                .uri("exemple.fr")
                .version(Version::HTTP_2)
                .header("X-Forwarded-For", "203.0.113.12")
                .body(())
                .unwrap()
                .into_parts()
                .0
        )
        .is_err()
    );

    assert!(
        // Missing X-Forwarded-For
        RequestSummary::from_request(
            1,
            &Builder::new()
                .method("GET")
                .uri("exemple.fr")
                .header("Host", "exemple.fr")
                .version(Version::HTTP_2)
                .body(())
                .unwrap()
                .into_parts()
                .0
        )
        .is_err()
    );
}
