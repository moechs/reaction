use std::sync::Arc;

use async_trait::async_trait;
use pingora_core::{
    BError, ErrorSource, ErrorType, RetryType,
    server::{Server, configuration::ServerConf},
    upstreams::peer::HttpPeer,
};
use pingora_proxy::{ProxyHttp, Session, http_proxy_service};

use crate::{
    config::{ComputedBackend, ComputedCaseOptions, ComputedMainOptions},
    error::HttpBanError,
    request::RequestSummary,
};

// Those methods can't be tested directly.
// All stateless logic has been moved to external functions
// Integration tests are in ./tests.rs

pub type ProxyCtx = Option<Arc<ComputedBackend>>;

pub struct Proxy {
    config: ComputedMainOptions,
}

impl Proxy {
    pub fn new(config: ComputedMainOptions) -> Server {
        let mut server = Server::new_with_opt_and_conf(
            None,
            ServerConf {
                daemon: false,
                grace_period_seconds: Some(config.graceful_shutdown_seconds.saturating_sub(1)),
                graceful_shutdown_timeout_seconds: Some(1),
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
                cause: Some("request_filter has already answered".into()),
                context: None,
            })),
        }
    }

    // if plain text response, do here
    async fn request_filter(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<bool, BError> {
        let summary =
            match RequestSummary::from_request(self.config.proxy_count, &session.req_header()) {
                Ok(s) => s,
                Err(HttpBanError::BadSetup(e)) => return Err(e),
                Err(HttpBanError::MalformedRequest(e)) => {
                    session
                        .respond_error_with_body(400, format!("Bad Request\n{e}").into())
                        .await?;
                    return Ok(true);
                }
            };

        let domain = self.config.resolve_domain(summary.host, summary.path)?;

        let case = domain.resolve_case(summary.ip).await;

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
