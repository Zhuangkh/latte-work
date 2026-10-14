//! Transient SSH password handshake with the askpass helper. The secret lives
//! only in memory and on a private local endpoint; it is never persisted.
use std::path::Path;

#[cfg(unix)]
mod imp {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, path::PathBuf};
    use tokio::{io::AsyncWriteExt, net::UnixListener, task::JoinHandle};

    pub struct Askpass {
        directory: PathBuf,
        socket: PathBuf,
        task: JoinHandle<()>,
    }

    impl Askpass {
        pub fn start(password: String) -> Result<Self, String> {
            if password.is_empty() || password.len() > 4096 || password.contains('\0') {
                return Err("SSH 密码长度无效".into());
            }
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos();
            let directory =
                std::env::temp_dir().join(format!("lw-a-{:x}-{stamp:x}", std::process::id()));
            std::fs::create_dir(&directory).map_err(|e| e.to_string())?;
            let socket = directory.join("socket");
            let listener = (|| -> std::io::Result<UnixListener> {
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
                UnixListener::bind(&socket)
            })();
            let listener = match listener {
                Ok(value) => value,
                Err(error) => {
                    let _ = std::fs::remove_file(&socket);
                    let _ = std::fs::remove_dir(&directory);
                    return Err(error.to_string());
                }
            };
            let task = tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let _ = stream.write_all(password.as_bytes()).await;
                }
            });
            Ok(Self {
                directory,
                socket,
                task,
            })
        }

        pub fn target(&self) -> &Path {
            &self.socket
        }
    }

    impl Drop for Askpass {
        fn drop(&mut self) {
            self.task.abort();
            let _ = std::fs::remove_file(&self.socket);
            let _ = std::fs::remove_dir(&self.directory);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tokio::io::AsyncReadExt;

        #[tokio::test]
        async fn askpass_uses_private_socket_and_removes_it_after_use() {
            let challenge = Askpass::start("test-password".to_owned()).unwrap();
            assert_eq!(
                std::fs::metadata(&challenge.directory)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            let directory = challenge.directory.clone();
            let socket = challenge.target().to_path_buf();
            let mut stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
            let mut secret = String::new();
            stream.read_to_string(&mut secret).await.unwrap();
            assert_eq!(secret, "test-password");
            drop(challenge);
            assert!(!socket.exists());
            assert!(!directory.exists());
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::path::PathBuf;
    use tokio::{io::AsyncWriteExt, task::JoinHandle};

    pub struct Askpass {
        target: PathBuf,
        task: JoinHandle<()>,
    }

    impl Askpass {
        pub fn start(password: String) -> Result<Self, String> {
            if password.is_empty() || password.len() > 4096 || password.contains('\0') {
                return Err("SSH 密码长度无效".into());
            }
            let pipe = latte_work_platform::pipe::PrivatePipe::bind("askpass")
                .map_err(|e| e.to_string())?;
            let target = PathBuf::from(pipe.name());
            // The helper stops at the NUL terminator, so it never depends on
            // pipe EOF semantics; passwords already reject embedded NUL bytes.
            let mut payload = password.into_bytes();
            payload.push(0);
            let task = tokio::spawn(async move {
                let mut rejected = 0u32;
                loop {
                    let mut stream = match pipe.accept_once().await {
                        Ok(stream) => stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::PermissionDenied
                                && rejected < 8 =>
                        {
                            // A foreign local process cannot claim the handshake.
                            rejected += 1;
                            continue;
                        }
                        Err(_) => break,
                    };
                    if stream.write_all(&payload).await.is_err() {
                        stream.evade_limbo();
                        break;
                    }
                    let _ = stream.flush().await;
                    stream.evade_limbo();
                }
            });
            Ok(Self { target, task })
        }

        pub fn target(&self) -> &Path {
            &self.target
        }
    }

    impl Drop for Askpass {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
}

pub use imp::Askpass;
