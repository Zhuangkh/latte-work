use std::{
    fs, io,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use tokio::net::{UnixListener, UnixStream};

pub(super) type Stream = UnixStream;

#[derive(Debug)]
pub(super) struct Endpoint {
    state: PathBuf,
    socket: PathBuf,
}
impl Endpoint {
    pub(super) fn prepare(state: &Path) -> io::Result<Self> {
        if !state.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "state directory missing",
            ));
        }
        let directory = crate::fs::PrivateDir::prepare(state)?;
        Self::open(directory.path())
    }
    pub(super) fn open(state: &Path) -> io::Result<Self> {
        let metadata = state.symlink_metadata()?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "invalid state directory",
            ));
        }
        let state = state.canonicalize()?;
        let endpoint = Self {
            socket: state.join("control.sock"),
            state,
        };
        endpoint.verify()?;
        Ok(endpoint)
    }
    pub(super) fn state_dir(&self) -> &Path {
        &self.state
    }
    pub(super) fn address(&self) -> String {
        self.socket.display().to_string()
    }
    pub(super) fn server_lock(&self) -> PathBuf {
        self.state.join("server.lock")
    }
    pub(super) fn upgrade_lock(&self) -> PathBuf {
        self.state.join("upgrade.lock")
    }
    pub(super) fn verify(&self) -> io::Result<()> {
        let metadata = self.state.symlink_metadata()?;
        if !metadata.is_dir()
            || metadata.uid() != nix::unistd::Uid::effective().as_raw()
            || metadata.permissions().mode() & 0o777 != 0o700
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "state directory must be user-owned mode 0700",
            ));
        }
        Ok(())
    }
    pub(super) async fn connect(&self) -> io::Result<Stream> {
        self.verify()?;
        let metadata = self.socket.symlink_metadata()?;
        if !metadata.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "control.sock is not a socket",
            ));
        }
        let stream = UnixStream::connect(&self.socket).await?;
        verify_peer(&stream)?;
        Ok(stream)
    }
    pub(super) async fn bind(&self) -> io::Result<Listener> {
        match self.socket.symlink_metadata() {
            Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(&self.socket)?,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "control.sock is occupied by a non-socket file",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(&self.socket)?;
        let metadata = self.socket.symlink_metadata()?;
        let owned = Listener {
            inner: listener,
            path: self.socket.clone(),
            identity: (metadata.dev(), metadata.ino()),
        };
        fs::set_permissions(&self.socket, fs::Permissions::from_mode(0o600))?;
        Ok(owned)
    }
}

#[derive(Debug)]
pub(super) struct Listener {
    inner: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
}
impl Listener {
    pub(super) async fn accept(&self, endpoint: &Endpoint) -> io::Result<Stream> {
        let (stream, _) = self.inner.accept().await?;
        endpoint.verify()?;
        verify_peer(&stream)?;
        Ok(stream)
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        if self
            .path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_socket() && (m.dev(), m.ino()) == self.identity)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn verify_peer(stream: &Stream) -> io::Result<()> {
    let peer = stream
        .peer_cred()
        .map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e))?;
    if peer.uid() != nix::unistd::Uid::effective().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "IPC peer belongs to another user",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn legacy_unix_socket_is_compatible_and_non_socket_is_preserved() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let root = tempfile::tempdir().unwrap();
        let endpoint = crate::ipc::Endpoint::prepare(root.path()).unwrap();
        let old = UnixListener::bind(root.path().join("control.sock")).unwrap();
        let mut client = endpoint
            .connect(std::time::Duration::from_secs(1))
            .await
            .unwrap();
        let (mut server, _) = old.accept().await.unwrap();
        client.write_all(b"hello\n").await.unwrap();
        let mut bytes = [0; 6];
        server.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"hello\n");
        drop((client, server, old));
        // A dead legacy socket is reclaimed only while holding the singleton lock.
        let listener = endpoint.bind().await.unwrap();
        assert_eq!(
            fs::metadata(root.path().join("control.sock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(listener);
        let path = root.path().join("control.sock");
        fs::write(&path, "untouched").unwrap();
        assert!(endpoint.bind().await.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "untouched");
    }
}
