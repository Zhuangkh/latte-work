//! Local byte-stream IPC with endpoint-scoped locks and authenticated connections.
//! No application requests are sent or replayed by this layer.
use crate::lock::FileLock;
use std::{
    fs::TryLockError,
    io,
    path::Path,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, ReadHalf, WriteHalf};

#[cfg(unix)]
#[path = "ipc/unix.rs"]
mod backend;
#[cfg(windows)]
#[path = "ipc/windows.rs"]
mod backend;

/// One state directory's IPC identity. Windows retains a verified directory guard.
#[derive(Clone, Debug)]
pub struct Endpoint(Arc<backend::Endpoint>);
impl Endpoint {
    /// Explicit initialization. State directory must already exist. Unix retains
    /// control.sock; Windows creates or verifies the private control subdirectory.
    pub fn prepare(state: &Path) -> io::Result<Self> {
        backend::Endpoint::prepare(state).map(|e| Self(Arc::new(e)))
    }

    /// Read-only endpoint discovery; missing directories are never created.
    pub fn open(state: &Path) -> io::Result<Self> {
        backend::Endpoint::open(state).map(|e| Self(Arc::new(e)))
    }

    pub fn state_dir(&self) -> &Path {
        self.0.state_dir()
    }

    /// Diagnostic address only; never used as a credential or parsed back into a path.
    pub fn address(&self) -> String {
        self.0.address()
    }

    /// One bounded connection attempt. Only absence/refusal authorizes daemon startup;
    /// permission/peer/directory failures and busy timeouts must propagate to the caller.
    pub async fn connect(&self, timeout: Duration) -> io::Result<Stream> {
        let inner = tokio::time::timeout(timeout, self.0.connect())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "IPC connection timed out"))??;
        Ok(Stream {
            inner,
            _server: None,
            _endpoint: self.clone(),
        })
    }

    /// Acquires the authoritative singleton lock without retries or deleting paths.
    pub fn try_lock_server(&self) -> Result<ServerLock, TryLockError> {
        self.0.verify().map_err(TryLockError::Error)?;
        let lock = FileLock::try_acquire(&self.0.server_lock())?;
        Ok(ServerLock {
            _lock: lock,
            endpoint: self.clone(),
        })
    }

    pub fn try_lock_upgrade(&self) -> Result<UpgradeLock, TryLockError> {
        self.0.verify().map_err(TryLockError::Error)?;
        let lock = FileLock::try_acquire(&self.0.upgrade_lock())?;
        Ok(UpgradeLock {
            _lock: lock,
            _endpoint: self.clone(),
        })
    }

    /// Locks then binds. Stale Unix socket reclamation is reachable only through
    /// the acquired ServerLock, never from a client connection attempt.
    pub async fn bind(&self) -> io::Result<Listener> {
        self.try_lock_server()
            .map_err(|error| match error {
                TryLockError::WouldBlock => io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "server already owns this endpoint",
                ),
                TryLockError::Error(error) => error,
            })?
            .listen()
            .await
    }
}

/// Only these connection errors indicate an absent daemon. Timeouts/security
/// failures do not justify launching another process or repairing the endpoint.
pub fn daemon_absent(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

#[derive(Debug)]
pub struct UpgradeLock {
    _lock: FileLock,
    _endpoint: Endpoint,
}

#[derive(Debug)]
pub struct ServerLock {
    _lock: FileLock,
    endpoint: Endpoint,
}
impl ServerLock {
    pub async fn listen(self) -> io::Result<Listener> {
        self.endpoint.0.verify()?;
        let inner = self.endpoint.0.bind().await?;
        Ok(Listener {
            inner,
            lease: Arc::new(self),
        })
    }
}

/// Drop closes the listener and removes only its own Unix socket. Accepted streams
/// retain the singleton lease; stop/reap connections before expecting lock release.
#[derive(Debug)]
pub struct Listener {
    inner: backend::Listener,
    lease: Arc<ServerLock>,
}
impl Listener {
    /// Security rejection closes that connection and returns PermissionDenied;
    /// callers may continue accepting. No unverified stream reaches protocol code.
    pub async fn accept(&self) -> io::Result<Stream> {
        let inner = self.inner.accept(&self.lease.endpoint.0).await?;
        Ok(Stream {
            inner,
            _server: Some(self.lease.clone()),
            _endpoint: self.lease.endpoint.clone(),
        })
    }
}

/// Owns its directory guard and (for accepted connections) server lease until all
/// split halves drop. Half-close is not portable; dropping both halves closes IPC.
#[derive(Debug)]
pub struct Stream {
    inner: backend::Stream,
    _server: Option<Arc<ServerLock>>,
    _endpoint: Endpoint,
}
impl Stream {
    pub fn into_split(self) -> (ReadHalf<Self>, WriteHalf<Self>) {
        tokio::io::split(self)
    }
}
impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}
impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests;
