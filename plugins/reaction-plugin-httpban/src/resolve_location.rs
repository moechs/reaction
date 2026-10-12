use std::net::IpAddr;

use pingora_core::{BError, ErrorSource, ErrorType, RetryType};

use crate::{
    config::{ComputedCaseOptions, ComputedLocation, ComputedMainOptions},
    error::new_bad_setup,
};

impl ComputedMainOptions {
    pub fn resolve_location(&self, host: &str, path: &str) -> Result<&ComputedLocation, BError> {
        // let path = path.trim_end_matches('/');

        let path_map = self.locations.get(host).ok_or_else(|| {
            new_bad_setup(format!(
                "Can't serve request asking for domain {host}: this domain isn't in config"
            ))
        })?;

        let mut max_path_len = None;
        let mut chosen_location = None;

        for location in path_map {
            if path.starts_with(&location.path) {
                let new_max = match max_path_len {
                    Some(previous_max) => {
                        if previous_max < location.path.len() {
                            Some(location.path.len())
                        } else {
                            None
                        }
                    }
                    None => Some(location.path.len()),
                };
                if let Some(new_max) = new_max {
                    max_path_len = Some(new_max);
                    chosen_location = Some(location);
                }
            }
        }

        chosen_location.ok_or_else(|| {
            pingora_core::Error {
                etype: ErrorType::ConnectNoRoute,
                esource: ErrorSource::Downstream,
                retry: RetryType::Decided(false),
                cause: Some(
                    format!(
                        "Can't serve request asking for location {host}/{path}: this path isn't in config"
                    )
                    .into(),
                ),
                context: None,
            }
            .into()
        })
    }
}

impl ComputedLocation {
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

    #[tokio::test]
    async fn test_resolve_config_smol() {
        let (smol_main, _) = smol_config().await;

        let smol_location = smol_main
            .locations
            .iter()
            .next()
            .unwrap()
            .1
            .iter()
            .next()
            .unwrap();

        assert_eq!(
            smol_main
                .resolve_location("tokipona.ppom.me", "/")
                .ok()
                .map(|d| &d.path),
            Some(&smol_location.path)
        );
        assert_eq!(
            smol_main
                .resolve_location("tokipona.ppom.me", "/whatyouwant")
                .ok()
                .map(|d| &d.path),
            Some(&smol_location.path)
        );
        assert_eq!(
            smol_main
                .resolve_location("tokipona.ppom.me", "/any/subdir/")
                .ok()
                .map(|d| &d.path),
            Some(&smol_location.path)
        );
        assert_eq!(
            smol_main
                .resolve_location("tokipona.ppom.me", "/any/subdir/and_file")
                .ok()
                .map(|d| &d.path),
            Some(&smol_location.path)
        );
        assert!(smol_main.resolve_location("tokipona.ppom.me", "").is_err());

        assert!(smol_main.resolve_location("ppom.me", "").is_err());
    }

    #[tokio::test]
    async fn test_resolve_config_big() {
        let (big_main, _) = big_config().await;

        let tokipona_location = big_main
            .locations
            .get("tokipona.ppom.me")
            .unwrap()
            .iter()
            .find(|d| d.path == "/")
            .unwrap();

        assert_eq!(
            big_main
                .resolve_location("tokipona.ppom.me", "/")
                .ok()
                .map(|d| &d.path),
            Some(&tokipona_location.path)
        );
        assert_eq!(
            big_main
                .resolve_location("tokipona.ppom.me", "/whatyouwant")
                .ok()
                .map(|d| &d.path),
            Some(&tokipona_location.path)
        );
        assert_eq!(
            big_main
                .resolve_location("tokipona.ppom.me", "/any/subdir/")
                .ok()
                .map(|d| &d.path),
            Some(&tokipona_location.path)
        );
        assert_eq!(
            big_main
                .resolve_location("tokipona.ppom.me", "/any/subdir/and_file")
                .ok()
                .map(|d| &d.path),
            Some(&tokipona_location.path)
        );

        let ppom_location = big_main
            .locations
            .get("ppom.me")
            .unwrap()
            .iter()
            .find(|d| d.path == "/")
            .unwrap();
        let vault_location = big_main
            .locations
            .get("ppom.me")
            .unwrap()
            .iter()
            .find(|d| d.path == "/vault")
            .unwrap();

        assert_eq!(
            big_main
                .resolve_location("ppom.me", "/any/subdir/")
                .ok()
                .map(|d| &d.path),
            Some(&ppom_location.path)
        );
        assert_eq!(
            big_main
                .resolve_location("ppom.me", "/")
                .ok()
                .map(|d| &d.path),
            Some(&ppom_location.path)
        );
        assert_eq!(
            big_main
                .resolve_location("ppom.me", "/vault")
                .ok()
                .map(|d| &d.path),
            Some(&vault_location.path)
        );
        assert_eq!(
            big_main
                .resolve_location("ppom.me", "/vaultmètre")
                .ok()
                .map(|d| &d.path),
            Some(&vault_location.path)
        );
        assert_eq!(
            big_main
                .resolve_location("ppom.me", "/vault/")
                .ok()
                .map(|d| &d.path),
            Some(&vault_location.path)
        );
        assert_eq!(
            big_main
                .resolve_location("ppom.me", "/vault/zblorg")
                .ok()
                .map(|d| &d.path),
            Some(&vault_location.path)
        );
    }
}
