use super::*;
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const LIMIT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn absence_does_not_create_control_paths_or_authorize_security_retries() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    assert!(Endpoint::open(&missing).is_err());
    assert!(!missing.exists());
    let endpoint = Endpoint::prepare(root.path()).unwrap();
    let error = endpoint.connect(LIMIT).await.unwrap_err();
    assert!(daemon_absent(&error));
    for kind in [
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::InvalidInput,
        io::ErrorKind::TimedOut,
        io::ErrorKind::Other,
    ] {
        assert!(!daemon_absent(&io::Error::from(kind)));
    }
}

#[tokio::test]
async fn endpoints_isolate_state_and_reuse_identity_across_connections() {
    tokio::time::timeout(LIMIT, async {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::prepare(a.path()).unwrap();
        let reopened = Endpoint::open(a.path()).unwrap();
        assert_eq!(endpoint.address(), reopened.address());
        let other = Endpoint::prepare(b.path()).unwrap();
        assert_ne!(endpoint.address(), other.address());
        let listener = endpoint.bind().await.unwrap();
        let other_listener = other.bind().await.unwrap();
        assert_eq!(
            reopened.bind().await.unwrap_err().kind(),
            io::ErrorKind::AddrInUse
        );
        for _ in 0..3 {
            let (client, server) = tokio::join!(reopened.connect(LIMIT), listener.accept());
            let mut client = client.unwrap();
            let mut server = server.unwrap();
            client
                .write_all("{\"request\":\"你好\"}\n".as_bytes())
                .await
                .unwrap();
            let mut bytes = vec![0; "{\"request\":\"你好\"}\n".len()];
            server.read_exact(&mut bytes).await.unwrap();
            assert_eq!(bytes, "{\"request\":\"你好\"}\n".as_bytes());
            server.write_all(b"ok\n").await.unwrap();
            let mut reply = [0; 3];
            client.read_exact(&mut reply).await.unwrap();
            assert_eq!(&reply, b"ok\n");
        }
        drop((listener, other_listener));
        let _rebound = endpoint.bind().await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn live_split_connections_retain_server_lock_after_listener_closes() {
    tokio::time::timeout(LIMIT, async {
        let root = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::prepare(root.path()).unwrap();
        let upgrade = endpoint.try_lock_upgrade().unwrap();
        assert!(matches!(
            endpoint.try_lock_upgrade(),
            Err(TryLockError::WouldBlock)
        ));
        drop(upgrade);
        let listener = endpoint.bind().await.unwrap();
        let (client, server) = tokio::join!(endpoint.connect(LIMIT), listener.accept());
        let client = client.unwrap();
        let (reader, writer) = server.unwrap().into_split();
        drop(listener);
        assert!(matches!(
            endpoint.try_lock_server(),
            Err(TryLockError::WouldBlock)
        ));
        drop(reader);
        assert!(matches!(
            endpoint.try_lock_server(),
            Err(TryLockError::WouldBlock)
        ));
        drop(writer);
        drop(client);
        let guard = endpoint.try_lock_server().unwrap();
        drop(guard);
        let _replacement = endpoint.bind().await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn multiple_child_processes_use_the_same_authenticated_endpoint() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::prepare(root.path()).unwrap();
    let listener = endpoint.bind().await.unwrap();
    let mut children = Vec::new();
    for _ in 0..4 {
        children.push(
            tokio::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "ipc::tests::ipc_child", "--nocapture"])
                .env("LATTE_IPC_TEST_STATE", root.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap(),
        );
    }
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut workers = tokio::task::JoinSet::new();
        for _ in 0..4 {
            let stream = listener.accept().await.unwrap();
            workers.spawn(async move {
                let (mut read, mut write) = stream.into_split();
                // Larger than pipe buffers: exercises concurrent full duplex and split lifetime.
                let expected: Vec<_> = (0..32_768).map(|i| (i % 251) as u8).collect();
                let mut received = vec![0; expected.len()];
                let (sent, result) =
                    tokio::join!(write.write_all(&expected), read.read_exact(&mut received));
                sent.unwrap();
                result.unwrap();
                assert_eq!(received, expected);
                write.write_all(b"done").await.unwrap();
                let mut ack = [0; 4];
                // Explicit ack ensures the final application bytes were consumed before drop.
                read.read_exact(&mut ack).await.unwrap();
                assert_eq!(&ack, b"ack!");
            });
        }
        while let Some(result) = workers.join_next().await {
            result.unwrap();
        }
        for child in &mut children {
            assert!(child.wait().await.unwrap().success());
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn ipc_child() {
    let Some(state) = std::env::var_os("LATTE_IPC_TEST_STATE") else {
        return;
    };
    tokio::time::timeout(LIMIT, async {
        let endpoint = Endpoint::open(Path::new(&state)).unwrap();
        let stream = endpoint.connect(LIMIT).await.unwrap();
        let (mut read, mut write) = stream.into_split();
        let payload: Vec<_> = (0..32_768).map(|i| (i % 251) as u8).collect();
        let mut received = vec![0; payload.len()];
        let (sent, result) =
            tokio::join!(write.write_all(&payload), read.read_exact(&mut received));
        sent.unwrap();
        result.unwrap();
        assert_eq!(received, payload);
        let mut done = [0; 4];
        read.read_exact(&mut done).await.unwrap();
        assert_eq!(&done, b"done");
        write.write_all(b"ack!").await.unwrap();
    })
    .await
    .unwrap();
}
