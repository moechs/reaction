use std::{
    ffi::{CStr, CString},
    thread,
};

use libnftables1_sys::Nftables;
use nftables::batch::Batch;
use tokio::sync::{mpsc, oneshot};

pub fn nftables_thread() -> NftClient {
    let (tx, mut rx) = mpsc::channel(10);

    thread::spawn(move || {
        let mut conn = Nftables::new();

        while let Some(NftCommand { json, ret }) = rx.blocking_recv() {
            let (rc, output, error) = conn.run_cmd(json.as_ptr());
            let res = match rc {
                0 => to_rust_string(output)
                    .ok_or_else(|| "unknown ok (rc = 0 but no output buffer)".into()),
                _ => to_rust_string(error)
                    .map(|err| format!("error (rc = {rc}: {err})"))
                    .ok_or_else(|| format!("unknown error (rc = {rc} but no error buffer)")),
            };
            ret.send(res);
        }
    });

    NftClient { tx }
}

fn to_rust_string(c_ptr: *const i8) -> Option<String> {
    if c_ptr.is_null() {
        None
    } else {
        Some(
            unsafe { CStr::from_ptr(c_ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

pub struct NftClient {
    tx: mpsc::Sender<NftCommand>,
}

impl NftClient {
    pub async fn send(&self, batch: Batch<'_>) -> Result<String, String> {
        // convert JSON to CString
        let mut json = serde_json::to_vec(&batch.to_nftables())
            .map_err(|err| format!("couldn't build json to send to nftables: {err}"))?;
        json.push('\0' as u8);
        let json = CString::from_vec_with_nul(json)
            .map_err(|err| format!("invalid json with null char: {err}"))?;
        // Send command
        let (tx, rx) = oneshot::channel();
        let command = NftCommand { json, ret: tx };
        self.tx.send(command).await;
        // Wait for result
        rx.await
            .map_err(|err| format!("nftables thread has quit: {err}"))?
    }
}

struct NftCommand {
    json: CString,
    ret: oneshot::Sender<Result<String, String>>,
}
