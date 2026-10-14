use interprocess::os::windows::{
    named_pipe::{
        PipeListenerOptions,
        pipe_mode::Bytes,
        tokio::{DuplexPipeStream, PipeListener},
    },
    security_descriptor::SecurityDescriptor,
};
use std::{
    fs::OpenOptions,
    io,
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsHandle, OwnedHandle},
    },
};
use widestring::U16CString;
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, SECURITY_IDENTIFICATION};

pub(crate) type Listener = PipeListener<Bytes, Bytes>;
pub(crate) type Stream = DuplexPipeStream<Bytes>;

pub(crate) fn listen(name: &str) -> io::Result<Listener> {
    validate_name(name)?;
    let sid = latte_work_windows_security::current_user_sid()?;
    // Explicit owner, no group: avoids interprocess 2.4.5's group-copy bug.
    let sddl = U16CString::from_str(format!("O:{sid}D:P(A;;GA;;;{sid})"))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let descriptor = SecurityDescriptor::deserialize(&sddl)?;
    PipeListenerOptions::new()
        .path(name)
        .security_descriptor(Some(descriptor))
        .accept_remote(false)
        .inheritable(false)
        .create_tokio_duplex::<Bytes>()
}

fn validate_name(name: &str) -> io::Result<()> {
    let suffix = name
        .strip_prefix(r"\\.\pipe\latte-work-")
        .unwrap_or_default();
    if suffix.is_empty()
        || suffix.len() > 160
        || !suffix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a local Latte control pipe name",
        ));
    }
    Ok(())
}

pub(crate) fn connect_once(name: &str) -> io::Result<Stream> {
    validate_name(name)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(name)?;
    let handle: OwnedHandle = file.into();
    Stream::try_from(handle).map_err(|error| io::Error::other(error.to_string()))
}

pub(crate) fn verify_peer(stream: &Stream) -> io::Result<u32> {
    latte_work_windows_security::verify_pipe_peer(stream.as_handle())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn remote_and_file_names_are_rejected_before_opening() {
        for path in [
            r"\\remote\pipe\latte-work-test",
            r"C:\latte-work-test",
            r"\\.\pipe\latte-work-test\child",
        ] {
            assert_eq!(
                listen(path).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
            assert_eq!(
                connect_once(path).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }
}
