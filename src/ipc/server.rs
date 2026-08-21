use crate::error::Result;
#[cfg(unix)]
use crate::error::CleanerError;
#[cfg(unix)]
use crate::ipc::protocol::{read_message, send_message, Command, Response};
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(unix)]
use std::path::Path;

pub struct IpcServer {
    #[cfg(unix)]
    listeners: Vec<UnixListener>,
}

impl IpcServer {
    pub fn bind(socket_path: &str, abstract_name: &str) -> Result<Self> {
        #[cfg(unix)]
        {
            let mut listeners = Vec::new();

            // 1. Bind native abstract namespace socket on Linux/Android
            if !abstract_name.is_empty() {
                match bind_abstract_unix(abstract_name) {
                    Ok(l) => {
                        log::info!("IPC listening on abstract socket: @{}", abstract_name);
                        listeners.push(l);
                    }
                    Err(e) => {
                        log::warn!("Failed to bind abstract socket @{}: {}", abstract_name, e);
                    }
                }
            }

            // 2. Bind filesystem socket (/dev/socket/cleaner_daemon or /data/local/tmp/cleaner.sock)
            if !socket_path.is_empty() {
                if let Some(parent) = Path::new(socket_path).parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::remove_file(socket_path);

                match UnixListener::bind(socket_path) {
                    Ok(l) => {
                        let _ = l.set_nonblocking(true);
                        log::info!("IPC listening on filesystem socket: {}", socket_path);
                        listeners.push(l);
                    }
                    Err(e) => {
                        log::warn!("Failed to bind filesystem socket {}: {}", socket_path, e);
                    }
                }
            }

            if listeners.is_empty() {
                return Err(CleanerError::Ipc("Failed to bind any IPC socket".to_string()));
            }

            Ok(Self { listeners })
        }
        #[cfg(not(unix))]
        {
            let _ = socket_path;
            let _ = abstract_name;
            Ok(Self {})
        }
    }

    #[cfg(unix)]
    pub fn get_raw_fds(&self) -> Vec<std::os::unix::io::RawFd> {
        self.listeners.iter().map(|l| l.as_raw_fd()).collect()
    }

    #[cfg(unix)]
    pub fn accept_and_handle<F>(&self, handler: std::sync::Arc<F>)
    where
        F: Fn(Command) -> Response + Send + Sync + 'static,
    {
        for listener in &self.listeners {
            while let Ok((mut stream, _)) = listener.accept() {
                if !self.is_peer_authorized(&stream) {
                    log::warn!("Unauthorized IPC connection rejected");
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
                    let _ = send_message(
                        &mut stream,
                        &Response::Error("Unauthorized caller UID".to_string()),
                    );
                    continue;
                }

                let handler_clone = handler.clone();
                let _ = std::thread::Builder::new()
                    .name("ipc-worker".to_string())
                    .spawn(move || {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(120)));
                        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(120)));

                        match read_message::<_, Command>(&mut stream) {
                            Ok(cmd) => {
                                log::debug!("Received IPC command: {:?}", cmd);
                                let resp = handler_clone(cmd);
                                let _ = send_message(&mut stream, &resp);
                            }
                            Err(e) => {
                                log::debug!("Error handling IPC request: {}", e);
                            }
                        }
                    });
            }
        }
    }

    #[cfg(unix)]
    fn is_peer_authorized(&self, stream: &UnixStream) -> bool {
        let raw_fd = stream.as_raw_fd();
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;

        let res = unsafe {
            libc::getsockopt(
                raw_fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };

        if res == 0 {
            let uid = cred.uid;
            let current_uid = unsafe { libc::getuid() };
            log::debug!("IPC peer connected with UID: {} (daemon UID: {})", uid, current_uid);
            // Allow same UID, root (0), system (1000), shell/adb (2000)
            uid == current_uid || uid == 0 || uid == 1000 || uid == 2000
        } else {
            // Fail-closed for security
            log::warn!("Failed to retrieve SO_PEERCRED from peer, rejecting connection");
            false
        }
    }
}


#[cfg(unix)]
fn bind_abstract_unix(name: &str) -> std::io::Result<UnixListener> {
    let sock = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
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
        libc::bind(
            sock,
            &addr as *const libc::sockaddr_un as *const libc::sockaddr,
            addr_len,
        )
    };

    if res < 0 {
        unsafe { libc::close(sock) };
        return Err(std::io::Error::last_os_error());
    }

    let listen_res = unsafe { libc::listen(sock, 16) };
    if listen_res < 0 {
        unsafe { libc::close(sock) };
        return Err(std::io::Error::last_os_error());
    }

    Ok(unsafe { UnixListener::from_raw_fd(sock) })
}
