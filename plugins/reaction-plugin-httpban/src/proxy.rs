use std::net::IpAddr;

use async_trait::async_trait;
use pingora_core::{
    BError, ErrorSource, ErrorType, RetryType,
    server::{Server, configuration::ServerConf},
    upstreams::peer::HttpPeer,
};
use pingora_proxy::{ProxyHttp, Session, http_proxy_service};

use crate::config::{ComputedCaseOptions, ComputedDomain, ComputedMainOptions};

pub type ProxyCtx = ();

pub struct Proxy {
    config: ComputedMainOptions,
}

impl Proxy {
    pub fn new(config: ComputedMainOptions, grace1: Option<u64>, grace2: Option<u64>) -> Server {
        let proxy = Proxy { config };
        let mut server = Server::new_with_opt_and_conf(
            None,
            ServerConf {
                daemon: false,
                grace_period_seconds: grace1,
                graceful_shutdown_timeout_seconds: grace2,
                ..Default::default()
            },
        );
        server.bootstrap();
        let my_proxy = http_proxy_service(&server.configuration, proxy);
        server.add_service(my_proxy);
        server
    }
}

#[async_trait]
impl ProxyHttp for Proxy {
    type CTX = ProxyCtx;

    fn new_ctx(&self) -> Self::CTX {
        ()
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>, BError> {
        // if upstream, do here
        todo!()
    }

    async fn request_filter(
        &self,
        session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> Result<bool, BError> {
        // if plain text response, do here
        let still_here = session.read_request().await?;
        if !still_here {
            return Ok(false);
        }

        let uri = session.req_header().as_owned_parts().uri;
        let host = uri.host().ok_or_else(|| {
            Box::new(pingora_core::Error {
                etype: ErrorType::InvalidHTTPHeader,
                esource: ErrorSource::Downstream,
                retry: RetryType::Decided(false),
                cause: Some("Missing 'Host' header".into()),
                context: None,
            })
        })?;
        let path = uri.path();
        let domain = self.config.resolve_domain(host, path)?;
        // FIXME no unwrap
        let case = domain.resolve_case(session.client_addr().unwrap().as_inet().unwrap().ip());
        todo!();
    }
}

impl ComputedMainOptions {
    fn resolve_domain(&self, host: &str, path: &str) -> Result<&ComputedDomain, BError> {
        // let path = path.trim_end_matches('/');

        let path_map = self.domains.get(host).ok_or_else(|| pingora_core::Error {
            etype: ErrorType::ConnectNoRoute,
            esource: ErrorSource::Downstream,
            retry: RetryType::Decided(false),
            cause: Some(
                format!(
                    "Can't serve request asking for domain {host}: this domain isn't in config"
                )
                .into(),
            ),
            context: None,
        })?;

        let mut max_path_len = None;
        let mut chosen_domain = None;

        for (domain_path, domain) in path_map {
            if path.starts_with(domain_path) {
                let new_max = match max_path_len {
                    Some(previous_max) => {
                        if previous_max < domain_path.len() {
                            Some(domain_path.len())
                        } else {
                            None
                        }
                    }
                    None => Some(domain_path.len()),
                };
                if let Some(new_max) = new_max {
                    max_path_len = Some(new_max);
                    chosen_domain = Some(domain);
                }
            }
        }

        chosen_domain.ok_or_else(|| {
            pingora_core::Error {
                etype: ErrorType::ConnectNoRoute,
                esource: ErrorSource::Downstream,
                retry: RetryType::Decided(false),
                cause: Some(
                    format!(
                        "Can't serve request asking for domain {host}: this domain isn't in config"
                    )
                    .into(),
                ),
                context: None,
            }
            .into()
        })
    }
}

impl ComputedDomain {
    async fn resolve_case(&self, ip: IpAddr) -> &ComputedCaseOptions {
        for (set, case) in &self.cases {
            if set.read().await.contains(ip) {
                return case;
            }
        }
        &self.default
    }
}

#[cfg(test)]
mod tests {
    use crate::config::tests::{big_config, smol_config};

    #[tokio::test]
    async fn test_resolve_config_smol() {
        let (smol_main, _) = smol_config().await;

        let smol_domain = smol_main
            .domains
            .iter()
            .next()
            .unwrap()
            .1
            .iter()
            .next()
            .unwrap()
            .1;

        assert_eq!(
            smol_main
                .resolve_domain("tokipona.ppom.me", "/")
                .ok()
                .map(|d| &d.name),
            Some(&smol_domain.name)
        );
        assert_eq!(
            smol_main
                .resolve_domain("tokipona.ppom.me", "/whatyouwant")
                .ok()
                .map(|d| &d.name),
            Some(&smol_domain.name)
        );
        assert_eq!(
            smol_main
                .resolve_domain("tokipona.ppom.me", "/any/subdir/")
                .ok()
                .map(|d| &d.name),
            Some(&smol_domain.name)
        );
        assert_eq!(
            smol_main
                .resolve_domain("tokipona.ppom.me", "/any/subdir/and_file")
                .ok()
                .map(|d| &d.name),
            Some(&smol_domain.name)
        );
        assert!(smol_main.resolve_domain("tokipona.ppom.me", "").is_err());

        assert!(smol_main.resolve_domain("ppom.me", "").is_err());
    }

    #[tokio::test]
    async fn test_resolve_config_big() {
        let (big_main, _) = big_config().await;

        let tokipona_domain = big_main
            .domains
            .get("tokipona.ppom.me")
            .unwrap()
            .get("/")
            .unwrap();

        assert_eq!(
            big_main
                .resolve_domain("tokipona.ppom.me", "/")
                .ok()
                .map(|d| &d.name),
            Some(&tokipona_domain.name)
        );
        assert_eq!(
            big_main
                .resolve_domain("tokipona.ppom.me", "/whatyouwant")
                .ok()
                .map(|d| &d.name),
            Some(&tokipona_domain.name)
        );
        assert_eq!(
            big_main
                .resolve_domain("tokipona.ppom.me", "/any/subdir/")
                .ok()
                .map(|d| &d.name),
            Some(&tokipona_domain.name)
        );
        assert_eq!(
            big_main
                .resolve_domain("tokipona.ppom.me", "/any/subdir/and_file")
                .ok()
                .map(|d| &d.name),
            Some(&tokipona_domain.name)
        );

        let ppom_domain = big_main.domains.get("ppom.me").unwrap().get("/").unwrap();
        let vault_domain = big_main
            .domains
            .get("ppom.me")
            .unwrap()
            .get("/vault")
            .unwrap();

        assert_eq!(
            big_main
                .resolve_domain("ppom.me", "/any/subdir/")
                .ok()
                .map(|d| &d.name),
            Some(&ppom_domain.name)
        );
        assert_eq!(
            big_main
                .resolve_domain("ppom.me", "/")
                .ok()
                .map(|d| &d.name),
            Some(&ppom_domain.name)
        );
        assert_eq!(
            big_main
                .resolve_domain("ppom.me", "/vault")
                .ok()
                .map(|d| &d.name),
            Some(&vault_domain.name)
        );
        assert_eq!(
            big_main
                .resolve_domain("ppom.me", "/vaultmètre")
                .ok()
                .map(|d| &d.name),
            Some(&vault_domain.name)
        );
        assert_eq!(
            big_main
                .resolve_domain("ppom.me", "/vault/")
                .ok()
                .map(|d| &d.name),
            Some(&vault_domain.name)
        );
        assert_eq!(
            big_main
                .resolve_domain("ppom.me", "/vault/zblorg")
                .ok()
                .map(|d| &d.name),
            Some(&vault_domain.name)
        );
    }
}
