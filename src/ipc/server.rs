#[cfg(unix)]
use crate::error::CleanerError;
use crate::error::Result;
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
#[cfg(unix)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
const MAX_CONCURRENT_IPC_WORKERS: usize = 4;

pub struct IpcServer {
    #[cfg(unix)]
    listeners: Vec<UnixListener>,
    #[cfg(unix)]
    active_workers: Arc<AtomicUsize>,
}

#[cfg(unix)]
struct WorkerGuard(Arc<AtomicUsize>);

#[cfg(unix)]
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
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
                return Err(CleanerError::Ipc(
                    "Failed to bind any IPC socket".to_string(),
                ));
            }

            Ok(Self {
                listeners,
                active_workers: Arc::new(AtomicUsize::new(0)),
            })
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
                let caller_uid = match self.get_peer_uid(&stream) {
                    Some(uid) if self.is_uid_allowed(uid) => uid,
                    _ => {
                        log::warn!("Unauthorized IPC connection rejected (unknown UID or no SO_PEERCRED)");
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(3)));
                        let _ = send_message(
                            &mut stream,
                            &Response::Error("Unauthorized caller UID".to_string()),
                        );
                        continue;
                    }
                };

                // Check active worker concurrency limit to protect against thread exhaustion DoS
                let current_workers = self.active_workers.load(Ordering::Relaxed);
                if current_workers >= MAX_CONCURRENT_IPC_WORKERS {
                    log::warn!(
                        "IPC connection rejected: max worker limit reached ({})",
                        MAX_CONCURRENT_IPC_WORKERS
                    );
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(3)));
                    let _ = send_message(
                        &mut stream,
                        &Response::Error("Daemon IPC busy, please retry shortly".to_string()),
                    );
                    continue;
                }

                self.active_workers.fetch_add(1, Ordering::SeqCst);
                let worker_guard = WorkerGuard(self.active_workers.clone());
                let handler_clone = handler.clone();

                let _ = std::thread::Builder::new()
                    .name("ipc-worker".to_string())
                    .stack_size(256 * 1024)
                    .spawn(move || {
                        let _guard = worker_guard;
                        let _ = stream.set_nonblocking(false);
                        // Tiered timeouts: 3s header read, 15s payload write
                        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(15)));

                        match read_message::<_, Command>(&mut stream) {
                            Ok(cmd) => {
                                log::debug!("Received IPC command: {:?} from UID {}", cmd, caller_uid);

                                // Granular RBAC Authorization check per command
                                if !is_command_authorized(caller_uid, &cmd) {
                                    log::warn!(
                                        "UID {} unauthorized for requested command {:?}",
                                        caller_uid,
                                        cmd
                                    );
                                    let _ = send_message(
                                        &mut stream,
                                        &Response::Error(
                                            "Permission denied: Insufficient privileges for this command"
                                                .to_string(),
                                        ),
                                    );
                                    return;
                                }

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
    fn get_peer_uid(&self, stream: &UnixStream) -> Option<u32> {
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
            Some(cred.uid)
        } else {
            None
        }
    }

    #[cfg(unix)]
    fn is_uid_allowed(&self, uid: u32) -> bool {
        let current_uid = unsafe { libc::getuid() };
        // Allow same UID, root (0), system (1000), shell/adb (2000)
        uid == current_uid || uid == 0 || uid == 1000 || uid == 2000
    }
}

#[cfg(unix)]
pub fn is_command_authorized(caller_uid: u32, cmd: &Command) -> bool {
    // UID 0 (root) is fully authorized for all operations
    if caller_uid == 0 {
        return true;
    }

    match cmd {
        Command::GetStatus | Command::GetStats | Command::Ping | Command::Cancel => {
            // Read-only queries and cancel allowed for system (1000) and shell (2000)
            caller_uid == 1000 || caller_uid == 2000
        }
        Command::TriggerClean(params) if !params.deep && !params.trim => {
            // Non-deep clean allowed for system (1000)
            caller_uid == 1000
        }
        // Deep clean, trim, reload config, stop daemon strictly require root (0)
        _ => false,
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
