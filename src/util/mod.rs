pub mod io_fast;
pub mod logger;

pub use io_fast::{format_bytes, read_file_to_buf, trim_heap_memory};
pub use logger::init_logger;
