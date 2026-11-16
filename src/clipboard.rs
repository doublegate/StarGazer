//! Clipboard: the system clipboard via `arboard` (X11, Wayland, macOS,
//! Windows), falling back to the OSC 52 terminal escape, which also works over
//! SSH in terminals that support it.
//!
//! The `arboard::Clipboard` is kept alive for the whole session because on
//! X11 the clipboard contents belong to the process that set them.

use std::io::Write;

pub struct Clipboard {
    system: Option<arboard::Clipboard>,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard {
    pub fn new() -> Self {
        let remote =
            std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some();
        Self {
            // Over SSH the system clipboard is the remote host's; use OSC 52.
            system: if remote {
                None
            } else {
                arboard::Clipboard::new().ok()
            },
        }
    }

    pub fn copy(&mut self, text: &str) -> Result<(), String> {
        if let Some(cb) = &mut self.system
            && cb.set_text(text.to_string()).is_ok()
        {
            return Ok(());
        }
        osc52(text).map_err(|e| e.to_string())
    }
}

fn osc52(text: &str) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()
}

/// Standard base64 (RFC 4648) with padding.
pub fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn rfc4648_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }
}
