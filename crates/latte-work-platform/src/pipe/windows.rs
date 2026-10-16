//! One-shot private pipes. The listener reuses the control-channel DACL and
//! authenticates the same user before handing out a stream, so a
//! different local process cannot read a transient handshake secret.
use crate::control;
use std::{
    io,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub type PrivateStream = control::Stream;

/// A bound local pipe with a random name and a user-only DACL.
pub struct PrivatePipe {
    name: String,
    listener: control::Listener,
}

impl PrivatePipe {
    /// Bind `\\.\pipe\latte-work-<tag>-<unique>`. `tag` is ASCII alphanumeric or `-`.
    pub fn bind(tag: &str) -> io::Result<Self> {
        if tag.is_empty()
            || tag.len() > 32
            || !tag
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pipe tag must be lowercase ASCII alphanumeric or '-'",
            ));
        }
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default()
            ^ u128::from(std::process::id())
            ^ u128::from(NEXT.fetch_add(1, Ordering::Relaxed));
        let name = format!(r"\\.\pipe\latte-work-{tag}-{unique:x}");
        let listener = control::listen(&name)?;
        Ok(Self { name, listener })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Accept one authenticated client. The stream is returned only after the
    /// kernel-reported peer token matches the current user.
    pub async fn accept_once(&self) -> io::Result<PrivateStream> {
        let stream = self.listener.accept().await?;
        control::verify_peer(&stream)?;
        Ok(stream)
    }
}

/// Synchronous connect for helper processes that run before the async runtime.
pub fn connect_file_once(name: &str) -> io::Result<std::fs::File> {
    control::connect_file_once(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn a_flushed_pipe_closes_the_client_with_eof() {
        use std::sync::mpsc;
        let pipe = PrivatePipe::bind("roundtrip").unwrap();
        let name = pipe.name().to_owned();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut file = connect_file_once(&name).unwrap();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            let _ = tx.send(data);
        });
        let mut stream = pipe.accept_once().await.unwrap();
        stream.write_all(b"secret").await.unwrap();
        stream.flush().await.unwrap();
        stream.evade_limbo();
        let data = tokio::task::spawn_blocking(move || {
            rx.recv_timeout(std::time::Duration::from_secs(10))
        })
        .await
        .unwrap()
        .expect("client did not see EOF");
        assert_eq!(data, b"secret");
    }
}
