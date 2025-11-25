use std::io;

use data_encoding::DecodeError;
use iroh::{PublicKey, SecretKey};
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
};

fn secret_key_path(dir: &str, cluster_name: &str) -> String {
    format!("{dir}/secret_key_{cluster_name}.txt")
}

pub async fn secret_key(dir: &str, cluster_name: &str) -> Result<SecretKey, String> {
    let path = secret_key_path(dir, cluster_name);
    if let Some(key) = get_secret_key(&path).await? {
        Ok(key)
    } else {
        let key = SecretKey::generate(&mut rand::rng());
        set_secret_key(&path, &key).await?;
        Ok(key)
    }
}

async fn get_secret_key(path: &str) -> Result<Option<SecretKey>, String> {
    let key = match fs::read_to_string(path).await {
        Ok(key) => Ok(key),
        Err(err) => match err.kind() {
            io::ErrorKind::NotFound => return Ok(None),
            _ => Err(format!("can't read secret key file: {err}")),
        },
    }?;
    let bytes = match key_b64_to_bytes(&key) {
        Ok(key) => Ok(key),
        Err(err) => Err(format!(
            "invalid secret key read from file: {err}. Please remove the `{path}` file from plugin directory.",
        )),
    }?;
    Ok(Some(SecretKey::from_bytes(&bytes)))
}

async fn set_secret_key(path: &str, key: &SecretKey) -> Result<(), String> {
    let secret_key = key.show();
    File::options()
        .mode(0o600)
        .write(true)
        .create(true)
        .open(path)
        .await
        .map_err(|err| format!("can't open `{path}` in plugin directory: {err}"))?
        .write_all(secret_key.as_bytes())
        .await
        .map_err(|err| format!("can't write to `{path}` in plugin directory: {err}"))
}

pub fn key_b64_to_bytes(key: &str) -> Result<[u8; 32], DecodeError> {
    let vec = data_encoding::BASE64URL.decode(key.as_bytes())?;
    if vec.len() != 32 {
        return Err(DecodeError {
            position: vec.len(),
            kind: data_encoding::DecodeKind::Length,
        });
    }
    let mut bytes = [0u8; 32];
    for i in 0..32 {
        bytes[i] = vec[i];
    }
    Ok(bytes)
}

pub fn key_bytes_to_b64(key: &[u8; 32]) -> String {
    data_encoding::BASE64URL.encode(key)
}

/// Implemented by PublicKey & SecretKey to display keys as base64 instead of hexadecimal.
/// Similar to Display/ToString
pub trait Show {
    fn show(&self) -> String;
}

impl Show for PublicKey {
    fn show(&self) -> String {
        key_bytes_to_b64(self.as_bytes())
    }
}

impl Show for SecretKey {
    fn show(&self) -> String {
        key_bytes_to_b64(&self.to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use assert_fs::{
        TempDir,
        prelude::{FileWriteStr, PathChild},
    };
    use iroh::{PublicKey, SecretKey};
    use tokio::fs::read_to_string;

    use crate::key::{
        get_secret_key, key_b64_to_bytes, key_bytes_to_b64, secret_key_path, set_secret_key,
    };

    #[test]
    fn secret_key_encode_decode() {
        for (secret_key, public_key) in [
            (
                "g7U1LPq2cgGSyk6CH_v1QpoXowSFKVQ8IcFljd_ZKGw=",
                "HhVh7ghqpXM9375HZ82OOeB504HBSS25wgug-1vUggY=",
            ),
            (
                "5EgRjwIpqd60IXWCGg5dFTtxkI-0fS1PlhoIhUjh1eY=",
                "LPSQ9pS7m_5vvNC-fhoBNeL2-eS2Fd6aO4ImSnXp3lc=",
            ),
        ] {
            assert_eq!(
                secret_key,
                &key_bytes_to_b64(&key_b64_to_bytes(secret_key).unwrap())
            );
            assert_eq!(
                public_key,
                &key_bytes_to_b64(&key_b64_to_bytes(public_key).unwrap())
            );

            let secret_key_parsed = SecretKey::from_bytes(&key_b64_to_bytes(secret_key).unwrap());
            let public_key_parsed =
                PublicKey::from_bytes(&key_b64_to_bytes(public_key).unwrap()).unwrap();

            assert_eq!(secret_key_parsed.public(), public_key_parsed);
        }
    }

    #[tokio::test]
    async fn secret_key_get() {
        let tmp_dir = TempDir::new().unwrap();
        let tmp_dir_str = tmp_dir.to_str().unwrap();
        for (secret_key, cluster_name) in [
            ("g7U1LPq2cgGSyk6CH_v1QpoXowSFKVQ8IcFljd_ZKGw=", "my_cluster"),
            ("5EgRjwIpqd60IXWCGg5dFTtxkI-0fS1PlhoIhUjh1eY=", "name"),
        ] {
            tmp_dir
                .child(&format!("secret_key_{cluster_name}.txt"))
                .write_str(secret_key)
                .unwrap();

            let secret_key_parsed = SecretKey::from_bytes(&key_b64_to_bytes(secret_key).unwrap());

            let path = secret_key_path(tmp_dir_str, cluster_name);
            let secret_key_from_file = get_secret_key(&path).await.unwrap();

            assert_eq!(
                secret_key_parsed.to_bytes(),
                secret_key_from_file.unwrap().to_bytes()
            )
        }

        assert_eq!(
            Ok(None),
            get_secret_key(&format!("{tmp_dir_str}/non_existent"))
                .await
                // Can't compare secret keys so we map to bytes
                // even if we don't want one
                .map(|opt| opt.map(|pk| pk.to_bytes()))
        );
        // Will fail if we're root, but who runs this as root??
        assert!(
            get_secret_key(&format!("/root/non_existent"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn secret_key_set() {
        let tmp_dir = TempDir::new().unwrap();
        let tmp_dir_str = tmp_dir.to_str().unwrap();

        let path = format!("{tmp_dir_str}/secret");
        let key = SecretKey::generate(&mut rand::rng());

        assert!(set_secret_key(&path, &key).await.is_ok());
        let read_file = read_to_string(&path).await;
        assert!(read_file.is_ok());
        assert_eq!(read_file.unwrap(), key_bytes_to_b64(&key.to_bytes()));
    }
}
