use std::fs::File;
use std::io::Read;
use std::path::Path;

#[cfg(unix)]
type MalloptFn = unsafe extern "C" fn(libc::c_int, libc::c_int) -> libc::c_int;
#[cfg(unix)]
type MallocTrimFn = unsafe extern "C" fn(usize) -> libc::c_int;


#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(unix)]
const M_PURGE_ALL: libc::c_int = -104;
#[cfg(unix)]
const M_PURGE: libc::c_int = -101;

/// Trims unused heap memory back to the kernel safely across Glibc, Musl, and Android Bionic libc
pub fn trim_heap_memory() {
    #[cfg(unix)]
    unsafe {
        // 1. Android Bionic Scudo/Jemalloc heap purge via dynamic mallopt
        // M_PURGE_ALL = -104 (Android 12+), M_PURGE = -101 (Android 9+)
        let mallopt_sym = libc::dlsym(libc::RTLD_DEFAULT, c"mallopt".as_ptr());
        if !mallopt_sym.is_null() {
            let func: MalloptFn = std::mem::transmute(mallopt_sym);
            let _ = func(M_PURGE_ALL, 0);
            let _ = func(M_PURGE, 0);
        }

        // 2. Dynamic resolution of malloc_trim (avoids hard link-time dependency on NDK libc.so)
        let trim_sym = libc::dlsym(libc::RTLD_DEFAULT, c"malloc_trim".as_ptr());
        if !trim_sym.is_null() {
            let func: MallocTrimFn = std::mem::transmute(trim_sym);
            let _ = func(0);
        }
    }
}


/// Reads file content into a fixed stack buffer without heap allocation
pub fn read_file_to_buf<'a>(path: &Path, buf: &'a mut [u8]) -> Option<&'a str> {
    let mut file = File::open(path).ok()?;
    let n = file.read(buf).ok()?;
    std::str::from_utf8(&buf[..n]).ok()
}

