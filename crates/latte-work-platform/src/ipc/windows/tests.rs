use super::*;
use crate::ipc::{self, Endpoint as PublicEndpoint};
use tokio::io::AsyncReadExt;
const LIMIT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn case_and_extended_path_aliases_share_endpoint_and_singleton() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = PublicEndpoint::prepare(root.path()).unwrap();
    let long = PublicEndpoint::open(endpoint.state_dir()).unwrap();
    let upper = root.path().to_str().unwrap().to_ascii_uppercase();
    let alias = PublicEndpoint::open(Path::new(&upper)).unwrap();
    assert_eq!(endpoint.address(), long.address());
    assert_eq!(endpoint.address(), alias.address());
    let _listener = endpoint.bind().await.unwrap();
    assert_eq!(
        alias.bind().await.unwrap_err().kind(),
        io::ErrorKind::AddrInUse
    );
}

#[tokio::test]
async fn busy_pipe_is_bounded_and_does_not_look_absent() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = PublicEndpoint::prepare(root.path()).unwrap();
    let _listener = endpoint.bind().await.unwrap();
    let raw = control::connect_once(&endpoint.address()).unwrap();
    let error = tokio::time::timeout(LIMIT, endpoint.connect(Duration::from_millis(60)))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(!ipc::daemon_absent(&error));
    raw.assume_flushed();
    drop(raw);
}

#[tokio::test]
async fn listener_and_split_streams_pin_control_directory() {
    tokio::time::timeout(LIMIT, async {
        let root = tempfile::tempdir().unwrap();
        let endpoint = PublicEndpoint::prepare(root.path()).unwrap();
        let control_path = endpoint.state_dir().join("control");
        let moved = endpoint.state_dir().join("moved");
        let listener = endpoint.bind().await.unwrap();
        let (client, server) = tokio::join!(endpoint.connect(LIMIT), listener.accept());
        let client = client.unwrap();
        let (reader, writer) = server.unwrap().into_split();
        drop((endpoint, listener, client));
        assert!(std::fs::rename(&control_path, &moved).is_err());
        drop(reader);
        assert!(std::fs::rename(&control_path, &moved).is_err());
        drop(writer);
        std::fs::rename(&control_path, &moved).unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cached_endpoint_rechecks_acl_before_connecting_or_locking() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = PublicEndpoint::prepare(root.path()).unwrap();
    let script = r#"$ErrorActionPreference='Stop'
$p=$env:LATTE_IPC_TEST_CONTROL
$acl=[IO.Directory]::GetAccessControl($p)
$rule=[Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new('S-1-1-0'), 'Read', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
$acl.AddAccessRule($rule)
[IO.Directory]::SetAccessControl($p,$acl)
"#;
    let mut child = tokio::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env(
            "LATTE_IPC_TEST_CONTROL",
            endpoint.state_dir().join("control"),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(15), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    let error = endpoint.connect(LIMIT).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(!ipc::daemon_absent(&error));
    assert!(endpoint.try_lock_server().is_err());
    assert!(endpoint.try_lock_upgrade().is_err());
    assert!(PublicEndpoint::prepare(root.path()).is_err());
    assert!(PublicEndpoint::open(root.path()).is_err());
}

#[tokio::test]
async fn eof_and_shutdown_leave_no_lingering_instance() {
    tokio::time::timeout(LIMIT, async {
        let root = tempfile::tempdir().unwrap();
        let endpoint = PublicEndpoint::prepare(root.path()).unwrap();
        let listener = endpoint.bind().await.unwrap();
        let (client, server) = tokio::join!(endpoint.connect(LIMIT), listener.accept());
        let mut server = server.unwrap();
        drop(client.unwrap());
        let mut bytes = [0];
        let result = server.read(&mut bytes).await;
        assert!(
            matches!(result, Ok(0))
                || result.as_ref().is_err_and(|e| matches!(
                    e.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::UnexpectedEof
                )),
            "{result:?}"
        );
        drop((server, listener));
        let _new = endpoint.bind().await.unwrap();
    })
    .await
    .unwrap();
}
