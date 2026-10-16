#[cfg(test)]
#[path = "windows/tests.rs"]
mod tests;

use crate::control;
use latte_work_windows_security::{ControlDirectory, DirectoryIdentity};
use sha2::{Digest, Sha256};
use std::{
    io,
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Debug)]
pub(super) struct Endpoint {
    state: PathBuf,
    directory: ControlDirectory,
    name: String,
    identity: DirectoryIdentity,
}
impl Endpoint {
    pub(super) fn prepare(state: &Path) -> io::Result<Self> {
        Self::from_directory(ControlDirectory::create_or_validate(
            &state.join("control"),
        )?)
    }
    pub(super) fn open(state: &Path) -> io::Result<Self> {
        Self::from_directory(ControlDirectory::open_verified(&state.join("control"))?)
    }
    fn from_directory(directory: ControlDirectory) -> io::Result<Self> {
        let identity = directory.identity()?;
        let mut hash = Sha256::new();
        hash.update(b"latte-work-control-v1\0");
        hash.update(latte_work_windows_security::current_user_sid()?.as_bytes());
        hash.update([0]);
        hash.update(identity.volume.to_le_bytes());
        hash.update(identity.file_id);
        let name = format!(r"\\.\pipe\latte-work-v1-{:x}", hash.finalize());
        let state = directory
            .path()
            .parent()
            .expect("control leaf has a parent")
            .to_owned();
        Ok(Self {
            state,
            directory,
            name,
            identity,
        })
    }
    pub(super) fn state_dir(&self) -> &Path {
        &self.state
    }
    pub(super) fn address(&self) -> String {
        self.name.clone()
    }
    pub(super) fn server_lock(&self) -> PathBuf {
        self.directory.path().join("server.lock")
    }
    pub(super) fn upgrade_lock(&self) -> PathBuf {
        self.directory.path().join("upgrade.lock")
    }
    pub(super) fn verify(&self) -> io::Result<()> {
        self.directory
            .verify()
            .map_err(|error| security_error("control directory owner/DACL", error))?;
        if self.directory.identity()? != self.identity {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "control directory identity changed",
            ));
        }
        Ok(())
    }
    pub(super) async fn connect(&self) -> io::Result<Stream> {
        self.verify()?;
        // Only pipe-busy is retried here. The outer Endpoint deadline bounds all
        // attempts; missing pipe returns immediately so the coordinator can launch.
        loop {
            match control::connect_once(&self.name) {
                Ok(raw) => {
                    let stream = Stream(raw);
                    self.verify()?;
                    return Ok(stream);
                }
                Err(error) if error.raw_os_error() == Some(231) => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    self.verify()?;
                }
                Err(error) => return Err(error),
            }
        }
    }
    pub(super) async fn bind(&self) -> io::Result<Listener> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        loop {
            self.verify()?;
            match control::listen(&self.name) {
                Ok(listener) => return Ok(Listener(listener)),
                // Tokio/Mio may retain closed instances until cancelled I/O completes.
                // We already own the singleton lock; never remove or adopt a pipe.
                Err(error)
                    if error.raw_os_error() == Some(5)
                        && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct Listener(control::Listener);
impl Listener {
    pub(super) async fn accept(&self, endpoint: &Endpoint) -> io::Result<Stream> {
        let stream = Stream(self.0.accept().await?);
        endpoint.verify()?;
        control::verify_peer(&stream.0)
            .map_err(|error| security_error("client identity (server side)", error))?;
        Ok(stream)
    }
}

// Preserve the existing byte-bridge contract: writes are not acknowledgements,
// and EOF closes the connection rather than waiting for a peer to consume data.
// Interprocess's raw pipe otherwise moves dirty handles into an unbounded linger
// pool on Drop, which could keep a replaced daemon's pipe alive after its lock.
#[derive(Debug)]
pub(super) struct Stream(control::Stream);
impl Drop for Stream {
    fn drop(&mut self) {
        self.0.assume_flushed();
    }
}
impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}
impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
fn security_error(stage: &str, error: io::Error) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("IPC security validation failed [{stage}]: {error}"),
    )
}
