use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let repo = zerogit::Repository::open(&args[0]).unwrap();
    for path in &args[1..] {
        let start = Instant::now();
        let blame = repo.blame(path, &zerogit::BlameOptions::new()).unwrap();
        let elapsed = start.elapsed();
        for line in blame.lines() {
            println!(
                "{} {} {}",
                line.commit().to_hex(),
                line.original_line(),
                line.path().to_string_lossy().replace('\\', "/")
            );
        }
        eprintln!("{} lines {} in {:?}", path, blame.lines().len(), elapsed);
    }
}
