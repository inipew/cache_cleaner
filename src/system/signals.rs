#[cfg(unix)]
use std::os::unix::io::RawFd;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalEvent {
    Shutdown,
    Reload,
    Other(i32),
}

#[cfg(unix)]
pub struct SignalWatcher {
    fd: RawFd,
}

#[cfg(unix)]
impl SignalWatcher {
    /// Blocks SIGINT, SIGTERM, SIGHUP, SIGQUIT from regular thread delivery
    /// and creates a signalfd to receive them asynchronously via epoll.
    pub fn create() -> std::io::Result<Self> {
        let mut mask: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGINT);
            libc::sigaddset(&mut mask, libc::SIGTERM);
            libc::sigaddset(&mut mask, libc::SIGHUP);
            libc::sigaddset(&mut mask, libc::SIGQUIT);

            // Block signals in this and all future spawned threads
            let res = libc::pthread_sigmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut());
            if res != 0 {
                return Err(std::io::Error::from_raw_os_error(res));
            }
        }

        let sfd = unsafe { libc::signalfd(-1, &mask, libc::SFD_NONBLOCK | libc::SFD_CLOEXEC) };
        if sfd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        log::info!("SignalFD watcher created (FD: {})", sfd);
        Ok(Self { fd: sfd })
    }

    pub fn fd(&self) -> RawFd {
        self.fd
    }

    /// Read pending signals without blocking
    pub fn read_events(&self) -> Vec<SignalEvent> {
        let mut events = Vec::new();
        let mut info: libc::signalfd_siginfo = unsafe { std::mem::zeroed() };
        let info_size = std::mem::size_of::<libc::signalfd_siginfo>();

        loop {
            let n =
                unsafe { libc::read(self.fd, &mut info as *mut _ as *mut libc::c_void, info_size) };

            if n != info_size as isize {
                break;
            }

            let signo = info.ssi_signo as i32;
            let event = match signo {
                libc::SIGINT | libc::SIGTERM | libc::SIGQUIT => {
                    log::info!("Received shutdown signal: {}", signo);
                    SignalEvent::Shutdown
                }
                libc::SIGHUP => {
                    log::info!("Received SIGHUP signal (Config reload request)");
                    SignalEvent::Reload
                }
                other => {
                    log::debug!("Received unhandled signal: {}", other);
                    SignalEvent::Other(other)
                }
            };
            events.push(event);
        }

        events
    }
}

#[cfg(unix)]
impl Drop for SignalWatcher {
    fn drop(&mut self) {
        if self.fd >= 0 {
            unsafe { libc::close(self.fd) };
        }
    }
}

#[cfg(not(unix))]
pub struct SignalWatcher;

#[cfg(not(unix))]
impl SignalWatcher {
    pub fn create() -> std::io::Result<Self> {
        Ok(Self)
    }
}
