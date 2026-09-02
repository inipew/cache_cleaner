#[cfg(unix)]
use std::os::unix::io::FromRawFd;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::path::Path;

use crate::config::SOCKET_PATH;
use crate::error::{CleanerError, Result};
#[cfg(unix)]
use crate::ipc::protocol::{read_message, send_message};
use crate::ipc::protocol::{Command, Response};

pub struct IpcClient;

impl IpcClient {
    pub fn connect_and_send(command: &Command) -> Result<Response> {
        #[cfg(unix)]
        {
            let mut stream = Self::try_connect()?;
            let _ = stream.set_nonblocking(false);
            let timeout_secs = match command {
                Command::TriggerClean(_) => 60,
                _ => 10,
            };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(timeout_secs)));
            let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(timeout_secs)));
            send_message(&mut stream, command)?;
            let response: Response = read_message(&mut stream)?;
            Ok(response)
        }
        #[cfg(not(unix))]
        {
            let _ = command;
            Err(CleanerError::Ipc(
                "IPC UNIX sockets are not supported on this platform".to_string(),
            ))
        }
    }

    #[cfg(unix)]
    fn try_connect() -> Result<UnixStream> {
        // 1. Try abstract namespace socket @cleaner_daemon
        if let Ok(stream) = connect_abstract_unix("cleaner_daemon") {
            return Ok(stream);
        }

        // 2. Try the dao filesystem socket: /data/adb/cleaner/run/daemon
        //
        // NOTE: We intentionally do NOT fall back to /tmp/cleaner.sock. /tmp is world-writable
        // (and sticky), so an attacker could pre-create a socket there and man-in-the-middle IPC
        // requests (reading status/commands or sending forged responses). Only the root-owned,
        // permission-controlled filesystem socket is trusted.
        let socket_paths: &[&str] = &[SOCKET_PATH];

        for path in socket_paths {
            if Path::new(path).exists() {
                if let Ok(stream) = UnixStream::connect(path) {
                    return Ok(stream);
                }
            }
        }

        Err(CleanerError::Ipc(format!(
            "Could not connect to cleaner daemon at {}. Is the daemon running?",
            SOCKET_PATH
        )))
    }
}

#[cfg(unix)]
fn connect_abstract_unix(name: &str) -> std::io::Result<UnixStream> {
    let sock = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if sock < 0 {
        return Err(std::io::Error::last_os_error());
    }

    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;

    let bytes = name.as_bytes();
    let max_len = addr.sun_path.len() - 2;
    let copy_len = bytes.len().min(max_len);
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr() as *const libc::c_char,
            addr.sun_path.as_mut_ptr().add(1),
            copy_len,
        );
    }

    let addr_len = (std::mem::size_of::<libc::sa_family_t>() + 1 + copy_len) as libc::socklen_t;

    let res = unsafe {
        libc::connect(
            sock,
            &addr as *const libc::sockaddr_un as *const libc::sockaddr,
            addr_len,
        )
    };

    if res < 0 {
        unsafe { libc::close(sock) };
        return Err(std::io::Error::last_os_error());
    }

    Ok(unsafe { UnixStream::from_raw_fd(sock) })
}
