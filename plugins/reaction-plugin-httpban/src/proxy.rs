use async_trait::async_trait;
use pingora_core::{BError, upstreams::peer::HttpPeer};
use pingora_proxy::{ProxyHttp, Session};

use crate::config::ComputedMainOptions;

pub type ProxyCtx = ();

pub struct Proxy {
    config: ComputedMainOptions,
}

impl Proxy {
    pub fn new(config: ComputedMainOptions) -> Self {
        Proxy { config }
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
        _session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> Result<bool, BError> {
        // if plain text response, do here
        todo!()
    }
}
