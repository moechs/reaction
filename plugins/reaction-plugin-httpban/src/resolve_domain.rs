use std::net::IpAddr;

use pingora_core::{BError, ErrorSource, ErrorType, RetryType};

use crate::{
    config::{ComputedCaseOptions, ComputedDomain, ComputedMainOptions},
    error::new_bad_setup,
};

impl ComputedMainOptions {
    pub fn resolve_domain(&self, host: &str, path: &str) -> Result<&ComputedDomain, BError> {
        // let path = path.trim_end_matches('/');

        let path_map = self.domains.get(host).ok_or_else(|| {
            new_bad_setup(format!(
                "Can't serve request asking for domain {host}: this domain isn't in config"
            ))
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
    pub async fn resolve_case(&self, ip: IpAddr) -> &ComputedCaseOptions {
        for (set, case) in &self.cases {
            if set.contains(ip).await {
                return case;
            }
        }
        &self.default
    }
}

#[cfg(test)]
mod tests {
    use crate::config::tests::{big_config, smol_config};

    #[test]
    fn test_host_path_ip_from_request() {
        // TODO test
    }

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
