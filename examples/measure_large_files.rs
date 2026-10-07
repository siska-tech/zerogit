//! Measures the memory zerogit uses with a large file: the peak of heap
//! memory while reading a blob as a stream (`Repository::blob_reader`), or
//! while adding, committing and checking out a large file.
//!
//! ```text
//! cargo run --release --example measure_large_files -- <repository> <revision>
//! cargo run --release --example measure_large_files -- --write <new directory> <megabytes>
//! ```
//!
//! The peak is counted by a global allocator wrapper, so it covers every
//! heap allocation of the process (not memory-mapped files or the stack).

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use zerogit::{Repository, RestoreOptions};

/// The system allocator, keeping track of the bytes in use and their peak.
struct Counting;

static IN_USE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            let now = IN_USE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        IN_USE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs `f` and reports its time and the peak of heap memory above what
/// was in use before.
fn measure<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let before = IN_USE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let start = Instant::now();
    let result = f();
    let elapsed = start.elapsed();
    let peak = PEAK.load(Ordering::Relaxed) - before;
    println!(
        "{:<28} {:>9.1?}   peak heap {:>8.1} MB",
        label,
        elapsed,
        peak as f64 / 1_000_000.0
    );
    result
}

const USAGE: &str = "usage: measure_large_files <repository> <revision>\n       \
     measure_large_files --write <new directory> <megabytes>";

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next().expect(USAGE);
    if first == "--write" {
        let dir = args.next().expect(USAGE);
        let megabytes: usize = args.next().expect(USAGE).parse().expect(USAGE);
        write_file(&dir, megabytes);
        return;
    }
    let dir = first;
    let revision = args.next().expect(USAGE);
    let repo = Repository::open(&dir).unwrap();
    let size = measure("blob_reader + copy", || {
        let mut reader = repo.blob_reader(&revision).unwrap();
        println!(
            "{} bytes, {}",
            reader.size(),
            if reader.is_streamed() {
                "streamed"
            } else {
                "rebuilt in memory"
            }
        );
        std::io::copy(&mut reader, &mut std::io::sink()).unwrap()
    });
    println!("read {} bytes", size);
    // For comparison, the whole blob in memory (within the read limits).
    measure("blob (whole, for comparison)", || {
        match repo.blob(&revision) {
            Ok(blob) => blob.size(),
            Err(e) => {
                println!("blob(): {}", e);
                0
            }
        }
    });
}

/// Creates a repository in `dir` with a file of `megabytes` MB, then adds
/// and commits it, and writes it back with a restore and a checkout.
fn write_file(dir: &str, megabytes: usize) {
    let repo = Repository::init(dir).unwrap();
    let path = std::path::Path::new(dir).join("big.bin");
    {
        // Poorly compressible bytes, written a megabyte at a time.
        let mut file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut chunk = vec![0u8; 1 << 20];
        for _ in 0..megabytes {
            for byte in chunk.iter_mut() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = (state >> 24) as u8;
            }
            file.write_all(&chunk).unwrap();
        }
    }
    // A small file keeps the index from becoming empty later.
    std::fs::write(std::path::Path::new(dir).join("small.txt"), "small\n").unwrap();
    repo.add("small.txt").unwrap();
    measure("add", || repo.add("big.bin").unwrap());
    let commit = measure("commit", || {
        repo.create_commit("Big file", "Measure", "measure@example.com")
            .unwrap()
    });
    println!("commit {}", commit);
    std::fs::remove_file(&path).unwrap();
    measure("restore", || {
        repo.restore(&["big.bin"], &RestoreOptions::new()).unwrap()
    });
    repo.create_branch("other", None).unwrap();
    std::fs::remove_file(&path).unwrap();
    repo.add("big.bin").unwrap();
    repo.create_commit("Remove it", "Measure", "measure@example.com")
        .unwrap();
    measure("checkout (file appears)", || {
        repo.checkout("other").unwrap()
    });
    measure("checkout (file goes)", || repo.checkout("main").unwrap());
    measure("checkout (file appears)", || {
        repo.checkout("other").unwrap()
    });
    println!(
        "{} bytes in the work tree",
        std::fs::metadata(&path).unwrap().len()
    );
}
