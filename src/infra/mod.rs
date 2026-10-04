//! Infrastructure utilities (hashing, compression, filesystem).

pub mod compression;
pub mod fs;
pub mod hash;
pub(crate) mod lock;

pub use compression::{compress, decompress, decompress_exact, decompress_prefix};
pub use fs::{read_file, write_file_atomic};
pub use hash::{crc32, hash_object};
pub(crate) use lock::{write_locked, LockFile};
