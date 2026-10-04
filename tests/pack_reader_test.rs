//! Pack reading compared with `git cat-file` on Git-generated packs.

use std::path::{Path, PathBuf};
use std::{fs, process::Command};
use tempfile::TempDir;
use zerogit::objects::pack::PackFile;
use zerogit::objects::ObjectType;
use zerogit::{Oid, Repository};

fn git(dir: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // Background auto-maintenance would repack concurrently with the test.
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "maintenance.auto")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "gc.auto")
        .env("GIT_CONFIG_VALUE_1", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

/// Builds history with many revisions of the same files so Git creates deep delta chains.
fn repository() -> TempDir {
    let temp = TempDir::new().unwrap();
    let repo = Repository::init(temp.path()).unwrap();
    let mut lines: Vec<String> = (0..200)
        .map(|i| format!("line {} of the document", i))
        .collect();
    for revision in 0..30 {
        lines[(revision * 7) % 200] = format!("revision {} edited this line", revision);
        lines.push(format!("appended in revision {}", revision));
        fs::write(temp.path().join("document.txt"), lines.join("\n")).unwrap();
        fs::write(
            temp.path().join("notes.md"),
            format!("# Notes\n\n{}\n", "note\n".repeat(revision + 1)),
        )
        .unwrap();
        repo.add("document.txt").unwrap();
        repo.add("notes.md").unwrap();
        repo.create_commit(
            &format!("Revision {}", revision),
            "Test",
            "test@example.com",
        )
        .unwrap();
    }
    git(temp.path(), &["tag", "-a", "v1", "-m", "Annotated tag"]);
    temp
}

fn repack(dir: &Path, ofs_delta: bool) -> PathBuf {
    let setting = format!("repack.useDeltaBaseOffset={}", ofs_delta);
    git(
        dir,
        &[
            "-c",
            &setting,
            "repack",
            "-a",
            "-d",
            "-f",
            "--depth=50",
            "--window=50",
        ],
    );
    fs::read_dir(dir.join(".git/objects/pack"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "pack"))
        .unwrap()
}

/// Parses `git cat-file --batch-all-objects --batch` into (oid, type, content).
fn all_objects(dir: &Path) -> Vec<(Oid, String, Vec<u8>)> {
    let output = git(dir, &["cat-file", "--batch-all-objects", "--batch"]);
    let mut objects = Vec::new();
    let mut rest = output.as_slice();
    while !rest.is_empty() {
        let newline = rest.iter().position(|b| *b == b'\n').unwrap();
        let header = std::str::from_utf8(&rest[..newline]).unwrap();
        let mut fields = header.split(' ');
        let oid = Oid::from_hex(fields.next().unwrap()).unwrap();
        let kind = fields.next().unwrap().to_owned();
        let size: usize = fields.next().unwrap().parse().unwrap();
        let start = newline + 1;
        objects.push((oid, kind, rest[start..start + size].to_vec()));
        rest = &rest[start + size + 1..];
    }
    objects
}

/// Returns the maximum delta depth reported by `git verify-pack -v`.
fn max_depth(dir: &Path, pack: &Path) -> usize {
    let idx = pack.with_extension("idx");
    let relative = idx
        .strip_prefix(dir)
        .unwrap_or(&idx)
        .to_str()
        .unwrap()
        .to_owned();
    let output = String::from_utf8(git(dir, &["verify-pack", "-v", &relative])).unwrap();
    output
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            (fields.len() == 7).then(|| fields[5].parse().unwrap())
        })
        .max()
        .unwrap_or(0)
}

/// Counts entries of the given pack type code (6 = OFS_DELTA, 7 = REF_DELTA).
fn count_entry_type(pack: &PackFile, type_code: u8) -> usize {
    let data = fs::read(pack.path()).unwrap();
    pack.index()
        .entries()
        .iter()
        .filter(|entry| (data[entry.offset as usize] >> 4) & 7 == type_code)
        .count()
}

fn check_pack(ofs_delta: bool) {
    let temp = repository();
    let path = repack(temp.path(), ofs_delta);
    let pack = PackFile::open(&path).unwrap();
    let objects = all_objects(temp.path());
    assert_eq!(objects.len(), pack.index().len());
    assert!(max_depth(temp.path(), &path) >= 3);
    let (used, unused) = if ofs_delta { (6, 7) } else { (7, 6) };
    assert!(count_entry_type(&pack, used) > 10);
    assert_eq!(count_entry_type(&pack, unused), 0);

    let mut kinds = std::collections::BTreeSet::new();
    for (oid, kind, content) in &objects {
        let object = pack.read(oid).unwrap().unwrap();
        assert_eq!(
            object.object_type,
            ObjectType::parse(kind).unwrap(),
            "{}",
            oid
        );
        assert_eq!(&object.content, content, "{}", oid);
        kinds.insert(kind.clone());
    }
    assert_eq!(kinds.len(), 4, "{:?}", kinds);
    pack.verify().unwrap();
}

#[test]
fn ofs_delta_pack_matches_cat_file() {
    check_pack(true);
}

#[test]
fn ref_delta_pack_matches_cat_file() {
    check_pack(false);
}

#[test]
fn corrupted_git_pack_is_rejected_without_panicking() {
    let temp = repository();
    let git_pack = repack(temp.path(), true);
    // Git writes packs read-only, so damage fresh copies instead.
    let copy = TempDir::new().unwrap();
    let path = copy.path().join("copy.pack");
    let original = fs::read(&git_pack).unwrap();
    fs::write(&path, &original).unwrap();
    fs::write(
        path.with_extension("idx"),
        fs::read(git_pack.with_extension("idx")).unwrap(),
    )
    .unwrap();
    let pack = PackFile::open(&path).unwrap();
    let mut offsets: Vec<u64> = pack.index().entries().iter().map(|e| e.offset).collect();
    offsets.sort_unstable();
    drop(pack);

    // Damage a byte inside each of several entries; reads touching it must fail.
    for &offset in offsets.iter().step_by(17) {
        let mut data = original.clone();
        data[offset as usize + 3] ^= 0x55;
        fs::write(&path, &data).unwrap();
        let pack = PackFile::open(&path).unwrap();
        assert!(pack.verify().is_err());
        let errors = pack
            .index()
            .entries()
            .iter()
            .filter(|entry| pack.read(&entry.oid).is_err())
            .count();
        assert!(errors >= 1);
    }
    fs::write(&path, &original).unwrap();
    PackFile::open(&path).unwrap().verify().unwrap();
}
