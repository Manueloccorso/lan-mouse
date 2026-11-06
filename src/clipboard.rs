use std::{collections::BTreeMap, process::Stdio, rc::Rc};

use async_trait::async_trait;
use lan_mouse_proto::{CLIPBOARD_CHUNK_SIZE, ProtoEvent};
use thiserror::Error;
use tokio::{io::AsyncWriteExt, process::Command};

pub(crate) use lan_mouse_proto::MAX_CLIPBOARD_SIZE;

#[derive(Debug, Error)]
pub(crate) enum ClipboardError {
    #[error("failed to run {command}: {source}")]
    Command {
        command: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("{command} exited unsuccessfully: {status}")]
    Status {
        command: &'static str,
        status: String,
    },
    #[error("clipboard output is not valid UTF-8")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("clipboard text exceeds the {MAX_CLIPBOARD_SIZE} byte limit")]
    TooLarge,
}

#[async_trait(?Send)]
pub(crate) trait Clipboard: 'static {
    async fn read_text(&self) -> Result<Option<String>, ClipboardError>;
    async fn write_text(&self, text: &str) -> Result<(), ClipboardError>;
}

pub(crate) type ClipboardHandle = Rc<dyn Clipboard>;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct WaylandClipboard;

#[async_trait(?Send)]
impl Clipboard for WaylandClipboard {
    async fn read_text(&self) -> Result<Option<String>, ClipboardError> {
        let output = Command::new("wl-paste")
            .args(["--no-newline", "--type", "text/plain"])
            .output()
            .await
            .map_err(|source| ClipboardError::Command {
                command: "wl-paste",
                source,
            })?;

        if !output.status.success() {
            // wl-paste exits unsuccessfully when no clipboard selection is
            // available. This is normal and should not be noisy.
            log::debug!("wl-paste did not return clipboard text");
            return Ok(None);
        }

        let text = String::from_utf8(output.stdout)?;
        if text.len() > MAX_CLIPBOARD_SIZE {
            return Err(ClipboardError::TooLarge);
        }
        Ok(Some(text))
    }

    async fn write_text(&self, text: &str) -> Result<(), ClipboardError> {
        if text.len() > MAX_CLIPBOARD_SIZE {
            return Err(ClipboardError::TooLarge);
        }

        let mut child = Command::new("wl-copy")
            .args(["--type", "text/plain"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| ClipboardError::Command {
                command: "wl-copy",
                source,
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .await
                .map_err(|source| ClipboardError::Command {
                    command: "wl-copy",
                    source,
                })?;
        }

        let status = child
            .wait()
            .await
            .map_err(|source| ClipboardError::Command {
                command: "wl-copy",
                source,
            })?;
        if !status.success() {
            return Err(ClipboardError::Status {
                command: "wl-copy",
                status: status.to_string(),
            });
        }
        Ok(())
    }
}

pub(crate) fn wayland() -> ClipboardHandle {
    Rc::new(WaylandClipboard)
}

pub(crate) fn transfer_events(text: &str, transfer_id: u32) -> Vec<ProtoEvent> {
    let bytes = text.as_bytes();
    if bytes.len() > MAX_CLIPBOARD_SIZE {
        return Vec::new();
    }
    let total_chunks = bytes.len().div_ceil(CLIPBOARD_CHUNK_SIZE).max(1);
    let mut events = Vec::with_capacity(total_chunks + 1);
    events.push(ProtoEvent::ClipboardStart {
        transfer_id,
        total_bytes: bytes.len() as u32,
    });
    for (chunk, data) in bytes.chunks(CLIPBOARD_CHUNK_SIZE).enumerate() {
        let mut chunk_data = [0; CLIPBOARD_CHUNK_SIZE];
        chunk_data[..data.len()].copy_from_slice(data);
        events.push(ProtoEvent::ClipboardChunk {
            transfer_id,
            chunk: chunk as u16,
            total_chunks: total_chunks as u16,
            len: data.len() as u16,
            data: chunk_data,
        });
    }
    if bytes.is_empty() {
        events.push(ProtoEvent::ClipboardChunk {
            transfer_id,
            chunk: 0,
            total_chunks: 1,
            len: 0,
            data: [0; CLIPBOARD_CHUNK_SIZE],
        });
    }
    events
}

#[derive(Debug, Default)]
pub(crate) struct ClipboardTransfer {
    transfer_id: Option<u32>,
    total_bytes: usize,
    total_chunks: usize,
    chunks: BTreeMap<u16, Vec<u8>>,
}

impl ClipboardTransfer {
    pub(crate) fn receive(&mut self, event: &ProtoEvent) -> Option<(u32, String)> {
        match event {
            ProtoEvent::ClipboardStart {
                transfer_id,
                total_bytes,
            } if (*total_bytes as usize) <= MAX_CLIPBOARD_SIZE => {
                self.transfer_id = Some(*transfer_id);
                self.total_bytes = *total_bytes as usize;
                self.total_chunks = 0;
                self.chunks.clear();
            }
            ProtoEvent::ClipboardStart { .. } => self.reset(),
            ProtoEvent::ClipboardChunk {
                transfer_id,
                chunk,
                total_chunks,
                len,
                data,
            } => {
                if self.transfer_id != Some(*transfer_id)
                    || *total_chunks == 0
                    || usize::from(*len) > CLIPBOARD_CHUNK_SIZE
                    || usize::from(*chunk) >= usize::from(*total_chunks)
                    || usize::from(*total_chunks)
                        != self.total_bytes.div_ceil(CLIPBOARD_CHUNK_SIZE).max(1)
                {
                    return None;
                }
                self.total_chunks = usize::from(*total_chunks);
                self.chunks
                    .entry(*chunk)
                    .or_insert_with(|| data[..usize::from(*len)].to_vec());
                if self.chunks.len() != self.total_chunks {
                    return None;
                }
                let mut bytes = Vec::with_capacity(self.total_bytes);
                for index in 0..self.total_chunks {
                    bytes.extend_from_slice(self.chunks.get(&(index as u16))?);
                }
                if bytes.len() != self.total_bytes {
                    self.reset();
                    return None;
                }
                let text = String::from_utf8(bytes).ok()?;
                let id = self.transfer_id.take()?;
                self.chunks.clear();
                return Some((id, text));
            }
            _ => {}
        }
        None
    }

    fn reset(&mut self) {
        self.transfer_id = None;
        self.total_bytes = 0;
        self.total_chunks = 0;
        self.chunks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_round_trips_empty_text() {
        let events = transfer_events("", 7);
        let mut transfer = ClipboardTransfer::default();
        assert_eq!(transfer.receive(&events[0]), None);
        assert_eq!(transfer.receive(&events[1]), Some((7, String::new())));
    }

    #[test]
    fn transfer_round_trips_chunked_utf8() {
        let text = "héllo ".repeat(300);
        let events = transfer_events(&text, 11);
        let mut transfer = ClipboardTransfer::default();
        assert_eq!(transfer.receive(&events[0]), None);
        let mut result = None;
        for event in &events[1..] {
            result = transfer.receive(event).or(result);
        }
        assert_eq!(result, Some((11, text)));
    }

    #[test]
    fn oversized_transfer_is_not_created() {
        let text = "x".repeat(MAX_CLIPBOARD_SIZE + 1);
        assert!(transfer_events(&text, 1).is_empty());
    }
}
