use std::{net::IpAddr, sync::Arc};

use async_trait::async_trait;
use pingora_core::{
    BError, ErrorSource, ErrorType, RetryType,
    server::{Server, configuration::ServerConf},
    upstreams::peer::HttpPeer,
};
use pingora_proxy::{ProxyHttp, Session, http_proxy_service};

use crate::config::{ComputedBackend, ComputedCaseOptions, ComputedDomain, ComputedMainOptions};

pub type ProxyCtx = Option<Arc<ComputedBackend>>;

pub struct Proxy {
    config: ComputedMainOptions,
}

impl Proxy {
    pub fn new(config: ComputedMainOptions, grace1: Option<u64>, grace2: Option<u64>) -> Server {
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

        let socket_path = config.socket_path.clone();
        let socket_mode = config.socket_mode.clone();

        let mut my_proxy = http_proxy_service(&server.configuration, Proxy { config });
        my_proxy.add_uds(&socket_path, Some(socket_mode));
        server.add_service(my_proxy);
        server
    }
}

#[async_trait]
impl ProxyHttp for Proxy {
    type CTX = ProxyCtx;

    fn new_ctx(&self) -> Self::CTX {
        None
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>, BError> {
        match ctx {
            Some(backend) => Ok(backend.addr.clone().into()),
            None => Err(Box::new(pingora_core::Error {
                etype: ErrorType::InternalError,
                esource: ErrorSource::Internal,
                retry: RetryType::Decided(false),
                cause: Some("request_filter has not been called".into()),
                context: None,
            })),
        }
    }

    async fn request_filter(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
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
        let case = domain
            .resolve_case(session.client_addr().unwrap().as_inet().unwrap().ip())
            .await;

        match case {
            ComputedCaseOptions::Static(static_response) => {
                session
                    .respond_error_with_body(static_response.code, static_response.data.clone())
                    .await?;
                Ok(true)
            }
            ComputedCaseOptions::Backend(computed_backend) => {
                ctx.replace(computed_backend.clone());
                Ok(false)
            }
        }
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

        for domain in path_map {
            if path.starts_with(&domain.path) {
                let new_max = match max_path_len {
                    Some(previous_max) => {
                        if previous_max < domain.path.len() {
                            Some(domain.path.len())
                        } else {
                            None
                        }
                    }
                    None => Some(domain.path.len()),
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
            .unwrap();

        assert_eq!(
            smol_main
                .resolve_domain("tokipona.ppom.me", "/")
                .ok()
                .map(|d| &d.path),
            Some(&smol_domain.path)
        );
        assert_eq!(
            smol_main
                .resolve_domain("tokipona.ppom.me", "/whatyouwant")
                .ok()
                .map(|d| &d.path),
            Some(&smol_domain.path)
        );
        assert_eq!(
            smol_main
                .resolve_domain("tokipona.ppom.me", "/any/subdir/")
                .ok()
                .map(|d| &d.path),
            Some(&smol_domain.path)
        );
        assert_eq!(
            smol_main
                .resolve_domain("tokipona.ppom.me", "/any/subdir/and_file")
                .ok()
                .map(|d| &d.path),
            Some(&smol_domain.path)
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
            .iter()
            .find(|d| d.path == "/")
            .unwrap();

        assert_eq!(
            big_main
                .resolve_domain("tokipona.ppom.me", "/")
                .ok()
                .map(|d| &d.path),
            Some(&tokipona_domain.path)
        );
        assert_eq!(
            big_main
                .resolve_domain("tokipona.ppom.me", "/whatyouwant")
                .ok()
                .map(|d| &d.path),
            Some(&tokipona_domain.path)
        );
        assert_eq!(
            big_main
                .resolve_domain("tokipona.ppom.me", "/any/subdir/")
                .ok()
                .map(|d| &d.path),
            Some(&tokipona_domain.path)
        );
        assert_eq!(
            big_main
                .resolve_domain("tokipona.ppom.me", "/any/subdir/and_file")
                .ok()
                .map(|d| &d.path),
            Some(&tokipona_domain.path)
        );

        let ppom_domain = big_main
            .domains
            .get("ppom.me")
            .unwrap()
            .iter()
            .find(|d| d.path == "/")
            .unwrap();
        let vault_domain = big_main
            .domains
            .get("ppom.me")
            .unwrap()
            .iter()
            .find(|d| d.path == "/vault")
            .unwrap();

        assert_eq!(
            big_main
                .resolve_domain("ppom.me", "/any/subdir/")
                .ok()
                .map(|d| &d.path),
            Some(&ppom_domain.path)
        );
        assert_eq!(
            big_main
                .resolve_domain("ppom.me", "/")
                .ok()
                .map(|d| &d.path),
            Some(&ppom_domain.path)
        );
        assert_eq!(
            big_main
                .resolve_domain("ppom.me", "/vault")
                .ok()
                .map(|d| &d.path),
            Some(&vault_domain.path)
        );
        assert_eq!(
            big_main
                .resolve_domain("ppom.me", "/vaultmètre")
                .ok()
                .map(|d| &d.path),
            Some(&vault_domain.path)
        );
        assert_eq!(
            big_main
                .resolve_domain("ppom.me", "/vault/")
                .ok()
                .map(|d| &d.path),
            Some(&vault_domain.path)
        );
        assert_eq!(
            big_main
                .resolve_domain("ppom.me", "/vault/zblorg")
                .ok()
                .map(|d| &d.path),
            Some(&vault_domain.path)
        );
    }
}
