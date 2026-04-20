use std::{
    ffi::{CStr, CString},
    thread,
};

use libnftables1_sys::{Nftables, nft_ctx_input_set_flags, nft_ctx_output_set_flags};
use nftables::batch::Batch;
use tokio::sync::{mpsc, oneshot};

/// A client with a dedicated server thread to libnftables.
/// Calling [`Default::default()`] spawns a new server thread.
/// Cloning just creates a new client to the same server thread.
#[derive(Clone)]
pub struct NftClient {
    tx: mpsc::Sender<NftCommand>,
}

impl Default for NftClient {
    fn default() -> Self {
        let (tx, mut rx) = mpsc::channel(10);

        thread::spawn(move || {
            let mut conn = Nftables::new();
            conn.set_output_json();

            while let Some(NftCommand { json, ret }) = rx.blocking_recv() {
                let (rc, output, error) = conn.run_cmd(json.as_ptr());
                let res = match rc {
                    0 => to_rust_string(output)
                        .ok_or_else(|| "unknown ok (rc = 0 but no output buffer)".into()),
                    _ => Err(to_rust_string(error)
                        .map(|err| format!("error (rc = {rc}: {err})"))
                        .unwrap_or_else(|| {
                            format!("unknown error (rc = {rc} but no error buffer)")
                        })),
                };
                let _ = ret.send(res);
            }
        });

        NftClient { tx }
    }
}

impl NftClient {
    /// Send a batch to nftables.
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
        self.tx
            .send(command)
            .await
            .map_err(|err| format!("nftables thread has quit, can't send command: {err}"))?;

        // Wait for result
        rx.await
            .map_err(|_| format!("nftables thread has quit, no response for command"))?
    }
}

struct NftCommand {
    json: CString,
    ret: oneshot::Sender<Result<String, String>>,
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
