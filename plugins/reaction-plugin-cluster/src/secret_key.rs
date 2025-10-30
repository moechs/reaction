use std::io;

use data_encoding::DecodeError;
use iroh::SecretKey;
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
};

fn secret_key_path(cluster_name: &str) -> String {
    format!("./secret_key_{cluster_name}.txt")
}

pub async fn secret_key(cluster_name: &str) -> Result<SecretKey, String> {
    if let Some(key) = get_secret_key(cluster_name).await? {
        Ok(key)
    } else {
        let key = SecretKey::generate(&mut rand::rng());
        set_secret_key(cluster_name, &key).await?;
        Ok(key)
    }
}

async fn get_secret_key(cluster_name: &str) -> Result<Option<SecretKey>, String> {
    let key = match fs::read_to_string(secret_key_path(cluster_name)).await {
        Ok(key) => Ok(key),
        Err(err) => match err.kind() {
            io::ErrorKind::NotFound => return Ok(None),
            _ => Err(format!("can't read secret key file: {err}")),
        },
    }?;
    let bytes = match key_b64_to_bytes(&key) {
        Ok(key) => Ok(key),
        Err(err) => Err(format!(
            "invalid secret key read from file: {err}. Please remove the `{}` file from plugin directory.",
            secret_key_path(cluster_name),
        )),
    }?;
    Ok(Some(SecretKey::from_bytes(&bytes)))
}

async fn set_secret_key(cluster_name: &str, key: &SecretKey) -> Result<(), String> {
    let secret_key = key_bytes_to_b64(&key.to_bytes());
    let secret_key_path = secret_key_path(cluster_name);
    File::options()
        .mode(0o600)
        .write(true)
        .create(true)
        .open(&secret_key_path)
        .await
        .map_err(|err| format!("can't open `{secret_key_path}` in plugin directory: {err}"))?
        .write_all(secret_key.as_bytes())
        .await
        .map_err(|err| format!("can't write to `{secret_key_path}` in plugin directory: {err}"))
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
