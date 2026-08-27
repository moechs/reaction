use async_trait::async_trait;
use pingora_core::{
    BError, ErrorSource, ErrorType, RetryType,
    server::{Server, configuration::ServerConf},
    upstreams::peer::HttpPeer,
};
use pingora_proxy::{ProxyHttp, Session, http_proxy_service};

use crate::config::{ComputedDomain, ComputedMainOptions};

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
        let domain = self.config.resolve_domain(host, path);
        todo!();
    }
}

impl ComputedMainOptions {
    async fn resolve_domain(&self, host: &str, path: &str) -> Result<&ComputedDomain, BError> {
        todo!()
    }
}
