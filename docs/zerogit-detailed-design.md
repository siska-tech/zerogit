# zerogit 詳細設計書

## 1. Infrastructure Layer

### 1.1 SHA-1 ハッシュ（`infra::hash`）

#### 1.1.1 内部データ構造

```rust
/// SHA-1計算の内部状態
struct Sha1State {
    /// 中間ハッシュ値 (5 x 32bit)
    h: [u32; 5],
    /// 処理済みバイト数
    len: u64,
    /// 未処理のバッファ（64バイトブロック）
    buffer: [u8; 64],
    /// バッファ内の有効バイト数
    buffer_len: usize,
}

impl Default for Sha1State {
    fn default() -> Self {
        Self {
            // RFC 3174 初期値
            h: [
                0x67452301,
                0xEFCDAB89,
                0x98BADCFE,
                0x10325476,
                0xC3D2E1F0,
            ],
            len: 0,
            buffer: [0u8; 64],
            buffer_len: 0,
        }
    }
}
```

#### 1.1.2 内部関数

```rust
/// 64バイトブロックを処理
fn process_block(state: &mut Sha1State, block: &[u8; 64])

/// ワード拡張（16ワード → 80ワード）
fn expand_words(block: &[u8; 64]) -> [u32; 80]

/// ラウンド関数 f(t, B, C, D)
fn round_f(t: usize, b: u32, c: u32, d: u32) -> u32

/// ラウンド定数 K(t)
fn round_k(t: usize) -> u32

/// 左ローテート
fn rotl(value: u32, shift: u32) -> u32
```

#### 1.1.3 アルゴリズム

```
SHA-1 メインアルゴリズム:

1. 初期化
   state.h = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0]

2. データ処理（64バイトブロック単位）
   FOR each 64-byte block:
       words[0..16] = block を 32bit big-endian で解釈
       FOR t = 16 to 79:
           words[t] = rotl(words[t-3] ^ words[t-8] ^ words[t-14] ^ words[t-16], 1)
       
       a, b, c, d, e = state.h[0..5]
       
       FOR t = 0 to 79:
           temp = rotl(a, 5) + f(t, b, c, d) + e + K(t) + words[t]
           e = d
           d = c
           c = rotl(b, 30)
           b = a
           a = temp
       
       state.h[0..5] += [a, b, c, d, e]

3. パディング
   - 0x80 を追加
   - 長さが 448 mod 512 になるまで 0x00 を追加
   - 元のメッセージ長（ビット）を 64bit big-endian で追加

4. 最終ハッシュ
   h[0] || h[1] || h[2] || h[3] || h[4] (big-endian連結)
```

**ラウンド関数:**

```
t =  0..19: f(B, C, D) = (B AND C) OR ((NOT B) AND D),  K = 0x5A827999
t = 20..39: f(B, C, D) = B XOR C XOR D,                 K = 0x6ED9EBA1
t = 40..59: f(B, C, D) = (B AND C) OR (B AND D) OR (C AND D), K = 0x8F1BBCDC
t = 60..79: f(B, C, D) = B XOR C XOR D,                 K = 0xCA62C1D6
```

#### 1.1.4 公開API実装

```rust
/// バイト列のSHA-1ハッシュを計算
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut state = Sha1State::default();
    
    // 64バイトブロック単位で処理
    let chunks = data.chunks_exact(64);
    let remainder = chunks.remainder();
    
    for chunk in chunks {
        let block: [u8; 64] = chunk.try_into().unwrap();
        process_block(&mut state, &block);
        state.len += 64;
    }
    
    // 残りをバッファにコピー
    state.buffer[..remainder.len()].copy_from_slice(remainder);
    state.buffer_len = remainder.len();
    state.len += remainder.len() as u64;
    
    // パディングと最終処理
    finalize(&mut state)
}

/// Gitオブジェクト形式でハッシュ計算
pub fn hash_object(kind: &str, content: &[u8]) -> Oid {
    let header = format!("{} {}\0", kind, content.len());
    let mut data = header.into_bytes();
    data.extend_from_slice(content);
    Oid(sha1(&data))
}
```

---

### 1.2 圧縮・解凍（`infra::compression`）

#### 1.2.1 内部構造

```rust
use miniz_oxide::inflate::decompress_to_vec_zlib;
use miniz_oxide::deflate::compress_to_vec_zlib;

/// 解凍結果
pub struct DecompressedData {
    pub data: Vec<u8>,
    pub original_size: usize,
}
```

#### 1.2.2 内部関数

```rust
/// zlibヘッダーの検証
fn validate_zlib_header(data: &[u8]) -> Result<()> {
    if data.len() < 2 {
        return Err(Error::DecompressionFailed);
    }
    
    let cmf = data[0];
    let flg = data[1];
    
    // CM (Compression Method) = 8 (deflate)
    if (cmf & 0x0F) != 8 {
        return Err(Error::DecompressionFailed);
    }
    
    // FCHECK: (CMF * 256 + FLG) % 31 == 0
    if ((cmf as u16) * 256 + (flg as u16)) % 31 != 0 {
        return Err(Error::DecompressionFailed);
    }
    
    Ok(())
}
```

#### 1.2.3 公開API実装

```rust
/// zlib形式のデータを解凍
pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    validate_zlib_header(data)?;
    
    decompress_to_vec_zlib(data)
        .map_err(|_| Error::DecompressionFailed)
}

/// データをzlib形式で圧縮（Phase 2）
pub fn compress(data: &[u8]) -> Vec<u8> {
    compress_to_vec_zlib(data, 6) // compression level 6
}
```

---

### 1.3 ファイルシステム（`infra::fs`）

#### 1.3.1 内部関数

```rust
/// パスの正規化（..や.を解決）
fn normalize_path(path: &Path) -> Result<PathBuf>

/// セキュリティチェック（パストラバーサル防止）
fn validate_path_security(base: &Path, target: &Path) -> Result<()>

/// ディレクトリの再帰的走査（.gitを除外）
fn walk_dir_recursive(
    dir: &Path,
    ignore_patterns: &[&str],
    callback: &mut dyn FnMut(&Path) -> Result<()>,
) -> Result<()>
```

#### 1.3.2 公開API

```rust
/// ファイル読み取り
pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(Error::Io)
}

/// ファイル書き込み（アトミック）
pub fn write_file_atomic(path: &Path, content: &[u8]) -> Result<()> {
    let temp_path = path.with_extension("tmp");
    std::fs::write(&temp_path, content)?;
    std::fs::rename(&temp_path, path)?;
    Ok(())
}

/// ワーキングツリーのファイル一覧取得
pub fn list_working_tree(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    walk_dir_recursive(root, &[".git"], &mut |path| {
        if path.is_file() {
            files.push(path.to_path_buf());
        }
        Ok(())
    })?;
    Ok(files)
}
```

---

## 2. Objects Layer

### 2.1 オブジェクトID（`objects::oid`）

#### 2.1.1 内部実装

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Oid(pub(crate) [u8; 20]);

impl Oid {
    /// 16進数文字を4bitに変換
    fn hex_char_to_nibble(c: u8) -> Result<u8> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err(Error::InvalidOid(format!("invalid hex char: {}", c as char))),
        }
    }
    
    /// 短縮形式からの検索（プレフィックス検索）
    pub(crate) fn find_by_prefix(
        store: &ObjectStore,
        prefix: &str,
    ) -> Result<Oid> {
        if prefix.len() < 4 {
            return Err(Error::InvalidOid("prefix too short (min 4)".into()));
        }
        
        let candidates = store.find_objects_by_prefix(prefix)?;
        
        match candidates.len() {
            0 => Err(Error::ObjectNotFound(/* ... */)),
            1 => Ok(candidates[0]),
            _ => Err(Error::AmbiguousOid(prefix.to_string())),
        }
    }
}
```

#### 2.1.2 トレイト実装

```rust
impl std::fmt::Display for Oid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl std::fmt::Debug for Oid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Oid({})", self.short())
    }
}

impl std::str::FromStr for Oid {
    type Err = Error;
    
    fn from_str(s: &str) -> Result<Self> {
        Oid::from_hex(s)
    }
}
```

---

### 2.2 オブジェクトストア（`objects::store`）

#### 2.2.1 内部構造

```rust
/// Looseオブジェクトストア
pub(crate) struct LooseObjectStore {
    /// .git/objects パス
    objects_dir: PathBuf,
}

/// オブジェクト読み取り結果
struct RawObject {
    kind: ObjectKind,
    size: usize,
    content: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
enum ObjectKind {
    Blob,
    Tree,
    Commit,
    Tag,
}
```

#### 2.2.2 内部関数

```rust
impl LooseObjectStore {
    /// OidからファイルパスへX変換
    /// 例: abc123... → .git/objects/ab/c123...
    fn oid_to_path(&self, oid: &Oid) -> PathBuf {
        let hex = oid.to_hex();
        self.objects_dir
            .join(&hex[0..2])
            .join(&hex[2..])
    }
    
    /// 生オブジェクト読み取り
    fn read_raw(&self, oid: &Oid) -> Result<RawObject> {
        let path = self.oid_to_path(oid);
        let compressed = fs::read_file(&path)?;
        let decompressed = compression::decompress(&compressed)?;
        
        self.parse_raw_object(&decompressed, oid)
    }
    
    /// 生オブジェクトのパース
    fn parse_raw_object(&self, data: &[u8], oid: &Oid) -> Result<RawObject> {
        // "type size\0content" 形式をパース
        let null_pos = data.iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error::InvalidObject {
                oid: *oid,
                reason: "missing null terminator".into(),
            })?;
        
        let header = std::str::from_utf8(&data[..null_pos])
            .map_err(|_| Error::InvalidObject {
                oid: *oid,
                reason: "invalid header encoding".into(),
            })?;
        
        let mut parts = header.split(' ');
        let kind_str = parts.next().ok_or_else(/* ... */)?;
        let size_str = parts.next().ok_or_else(/* ... */)?;
        
        let kind = match kind_str {
            "blob" => ObjectKind::Blob,
            "tree" => ObjectKind::Tree,
            "commit" => ObjectKind::Commit,
            "tag" => ObjectKind::Tag,
            _ => return Err(Error::InvalidObject { /* ... */ }),
        };
        
        let size: usize = size_str.parse()
            .map_err(|_| Error::InvalidObject { /* ... */ })?;
        
        let content = data[null_pos + 1..].to_vec();
        
        if content.len() != size {
            return Err(Error::InvalidObject {
                oid: *oid,
                reason: format!("size mismatch: header={}, actual={}", size, content.len()),
            });
        }
        
        Ok(RawObject { kind, size, content })
    }
    
    /// プレフィックスでオブジェクト検索
    fn find_objects_by_prefix(&self, prefix: &str) -> Result<Vec<Oid>> {
        let dir_prefix = &prefix[0..2];
        let file_prefix = &prefix[2..];
        
        let dir_path = self.objects_dir.join(dir_prefix);
        if !dir_path.exists() {
            return Ok(Vec::new());
        }
        
        let mut matches = Vec::new();
        for entry in std::fs::read_dir(&dir_path)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            
            if name_str.starts_with(file_prefix) {
                let full_hex = format!("{}{}", dir_prefix, name_str);
                matches.push(Oid::from_hex(&full_hex)?);
            }
        }
        
        Ok(matches)
    }
}
```

#### 2.2.3 公開API実装

```rust
impl LooseObjectStore {
    pub fn read(&self, oid: &Oid) -> Result<Object> {
        let raw = self.read_raw(oid)?;
        
        match raw.kind {
            ObjectKind::Blob => Ok(Object::Blob(Blob::parse(&raw.content)?)),
            ObjectKind::Tree => Ok(Object::Tree(Tree::parse(&raw.content)?)),
            ObjectKind::Commit => Ok(Object::Commit(Commit::parse(&raw.content, *oid)?)),
            ObjectKind::Tag => Ok(Object::Tag(Tag::parse(&raw.content)?)),
        }
    }
    
    pub fn exists(&self, oid: &Oid) -> bool {
        self.oid_to_path(oid).exists()
    }
    
    /// Phase 2: オブジェクト書き込み
    pub fn write(&self, kind: &str, content: &[u8]) -> Result<Oid> {
        let oid = hash::hash_object(kind, content);
        
        if self.exists(&oid) {
            return Ok(oid); // 既存なら書き込み不要
        }
        
        let path = self.oid_to_path(&oid);
        
        // ディレクトリ作成
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        // ヘッダー付きで圧縮
        let header = format!("{} {}\0", kind, content.len());
        let mut data = header.into_bytes();
        data.extend_from_slice(content);
        let compressed = compression::compress(&data);
        
        fs::write_file_atomic(&path, &compressed)?;
        
        Ok(oid)
    }
}
```

---

### 2.3 Blobパース（`objects::blob`）

#### 2.3.1 実装

```rust
#[derive(Debug, Clone)]
pub struct Blob {
    content: Vec<u8>,
}

impl Blob {
    /// 生コンテンツからBlobを構築
    pub(crate) fn parse(content: &[u8]) -> Result<Self> {
        Ok(Self {
            content: content.to_vec(),
        })
    }
    
    /// Phase 2: コンテンツからBlob作成
    pub fn new(content: Vec<u8>) -> Self {
        Self { content }
    }
}
```

---

### 2.4 Treeパース（`objects::tree`）

#### 2.4.1 内部構造

```rust
#[derive(Debug, Clone)]
pub struct Tree {
    entries: Vec<TreeEntry>,
}

#[derive(Debug, Clone)]
pub struct TreeEntry {
    mode: FileMode,
    name: String,
    oid: Oid,
}
```

#### 2.4.2 パースアルゴリズム

```
Tree形式:
  [mode] SP [name] NUL [20-byte SHA]
  [mode] SP [name] NUL [20-byte SHA]
  ...

パース手順:
1. WHILE データが残っている:
   a. SPまで読む → mode (ASCII数字)
   b. NULまで読む → name (UTF-8)
   c. 20バイト読む → oid (バイナリ)
   d. TreeEntryを構築してリストに追加

2. エントリをソート（名前でバイト順）
```

```rust
impl Tree {
    pub(crate) fn parse(content: &[u8]) -> Result<Self> {
        let mut entries = Vec::new();
        let mut pos = 0;
        
        while pos < content.len() {
            // mode (SPまで)
            let space_pos = content[pos..]
                .iter()
                .position(|&b| b == b' ')
                .ok_or_else(|| Error::InvalidObject { /* ... */ })?;
            
            let mode_bytes = &content[pos..pos + space_pos];
            let mode = Self::parse_mode(mode_bytes)?;
            pos += space_pos + 1;
            
            // name (NULまで)
            let null_pos = content[pos..]
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| Error::InvalidObject { /* ... */ })?;
            
            let name = std::str::from_utf8(&content[pos..pos + null_pos])
                .map_err(|_| Error::InvalidObject { /* ... */ })?
                .to_string();
            pos += null_pos + 1;
            
            // oid (20バイト)
            if pos + 20 > content.len() {
                return Err(Error::InvalidObject { /* ... */ });
            }
            let oid = Oid::from_bytes(&content[pos..pos + 20])?;
            pos += 20;
            
            entries.push(TreeEntry { mode, name, oid });
        }
        
        Ok(Self { entries })
    }
    
    fn parse_mode(bytes: &[u8]) -> Result<FileMode> {
        let mode_str = std::str::from_utf8(bytes)
            .map_err(|_| Error::InvalidObject { /* ... */ })?;
        let mode_num: u32 = mode_str.parse()
            .map_err(|_| Error::InvalidObject { /* ... */ })?;
        
        match mode_num {
            0o100644 => Ok(FileMode::Regular),
            0o100755 => Ok(FileMode::Executable),
            0o120000 => Ok(FileMode::Symlink),
            0o160000 => Ok(FileMode::Submodule),
            0o040000 | 0o40000 => Ok(FileMode::Tree),
            _ => Err(Error::InvalidObject { /* ... */ }),
        }
    }
}
```

---

### 2.5 Commitパース（`objects::commit`）

#### 2.5.1 内部構造

```rust
#[derive(Debug, Clone)]
pub struct Commit {
    oid: Oid,
    tree: Oid,
    parents: Vec<Oid>,
    author: Signature,
    committer: Signature,
    message: String,
}

#[derive(Debug, Clone)]
pub struct Signature {
    name: String,
    email: String,
    time: i64,
    offset: i32,
}
```

#### 2.5.2 パースアルゴリズム

```
Commit形式:
  tree <tree-sha>
  parent <parent-sha>      (0個以上)
  author <n> <<email>> <timestamp> <offset>
  committer <n> <<email>> <timestamp> <offset>
  [gpgsig ...]             (オプション)
  
  <message>

パース手順:
1. 行単位で読み取り
2. "tree " で始まる → tree SHA
3. "parent " で始まる → parent SHA (複数可)
4. "author " で始まる → Signature パース
5. "committer " で始まる → Signature パース
6. 空行 → 残りはメッセージ
```

```rust
impl Commit {
    pub(crate) fn parse(content: &[u8], oid: Oid) -> Result<Self> {
        let text = std::str::from_utf8(content)
            .map_err(|_| Error::InvalidObject { /* ... */ })?;
        
        let mut lines = text.lines().peekable();
        let mut tree: Option<Oid> = None;
        let mut parents = Vec::new();
        let mut author: Option<Signature> = None;
        let mut committer: Option<Signature> = None;
        
        // ヘッダー部分
        while let Some(line) = lines.next() {
            if line.is_empty() {
                break; // メッセージ開始
            }
            
            if let Some(sha) = line.strip_prefix("tree ") {
                tree = Some(Oid::from_hex(sha)?);
            } else if let Some(sha) = line.strip_prefix("parent ") {
                parents.push(Oid::from_hex(sha)?);
            } else if let Some(sig_str) = line.strip_prefix("author ") {
                author = Some(Self::parse_signature(sig_str)?);
            } else if let Some(sig_str) = line.strip_prefix("committer ") {
                committer = Some(Self::parse_signature(sig_str)?);
            } else if line.starts_with("gpgsig ") {
                // GPG署名をスキップ
                while let Some(l) = lines.peek() {
                    if l.starts_with(' ') {
                        lines.next();
                    } else {
                        break;
                    }
                }
            }
            // 他の未知ヘッダーは無視
        }
        
        // メッセージ部分
        let message: String = lines.collect::<Vec<_>>().join("\n");
        
        Ok(Self {
            oid,
            tree: tree.ok_or_else(|| Error::InvalidObject { /* ... */ })?,
            parents,
            author: author.ok_or_else(|| Error::InvalidObject { /* ... */ })?,
            committer: committer.ok_or_else(|| Error::InvalidObject { /* ... */ })?,
            message,
        })
    }
    
    /// Signatureパース
    /// 形式: "Name <email@example.com> 1700000000 +0900"
    fn parse_signature(s: &str) -> Result<Signature> {
        // 後ろからパース（タイムゾーン → タイムスタンプ → 残りが名前+email）
        let parts: Vec<&str> = s.rsplitn(3, ' ').collect();
        if parts.len() != 3 {
            return Err(Error::InvalidObject { /* ... */ });
        }
        
        let offset = Self::parse_offset(parts[0])?;
        let time: i64 = parts[1].parse()
            .map_err(|_| Error::InvalidObject { /* ... */ })?;
        let name_email = parts[2];
        
        // "Name <email>" をパース
        let lt_pos = name_email.rfind('<')
            .ok_or_else(|| Error::InvalidObject { /* ... */ })?;
        let gt_pos = name_email.rfind('>')
            .ok_or_else(|| Error::InvalidObject { /* ... */ })?;
        
        let name = name_email[..lt_pos].trim().to_string();
        let email = name_email[lt_pos + 1..gt_pos].to_string();
        
        Ok(Signature { name, email, time, offset })
    }
    
    /// タイムゾーンオフセットパース
    /// "+0900" → 540, "-0500" → -300
    fn parse_offset(s: &str) -> Result<i32> {
        let sign = match s.chars().next() {
            Some('+') => 1,
            Some('-') => -1,
            _ => return Err(Error::InvalidObject { /* ... */ }),
        };
        
        let hours: i32 = s[1..3].parse()
            .map_err(|_| Error::InvalidObject { /* ... */ })?;
        let mins: i32 = s[3..5].parse()
            .map_err(|_| Error::InvalidObject { /* ... */ })?;
        
        Ok(sign * (hours * 60 + mins))
    }
}
```

---

## 3. Refs Layer

### 3.1 参照解決（`refs::resolver`）

#### 3.1.1 内部構造

```rust
pub(crate) struct RefStore {
    git_dir: PathBuf,
}

/// 参照の種類
enum RefValue {
    /// 直接的なSHA
    Direct(Oid),
    /// シンボリック参照
    Symbolic(String),
}
```

#### 3.1.2 内部関数

```rust
impl RefStore {
    /// 参照ファイルを読み取り
    fn read_ref_file(&self, name: &str) -> Result<RefValue> {
        let path = self.git_dir.join(name);
        let content = fs::read_file(&path)?;
        let text = std::str::from_utf8(&content)
            .map_err(|_| Error::InvalidRef { /* ... */ })?
            .trim();
        
        if let Some(target) = text.strip_prefix("ref: ") {
            Ok(RefValue::Symbolic(target.to_string()))
        } else {
            Ok(RefValue::Direct(Oid::from_hex(text)?))
        }
    }
    
    /// シンボリック参照を再帰的に解決
    fn resolve_recursive(&self, name: &str, depth: usize) -> Result<Oid> {
        if depth > 10 {
            return Err(Error::InvalidRef {
                name: name.to_string(),
                reason: "too many symbolic references".into(),
            });
        }
        
        match self.read_ref_file(name)? {
            RefValue::Direct(oid) => Ok(oid),
            RefValue::Symbolic(target) => self.resolve_recursive(&target, depth + 1),
        }
    }
}
```

#### 3.1.3 公開API実装

```rust
impl RefStore {
    /// HEAD取得
    pub fn head(&self) -> Result<Head> {
        let head_path = self.git_dir.join("HEAD");
        if !head_path.exists() {
            return Err(Error::RefNotFound("HEAD".into()));
        }
        
        match self.read_ref_file("HEAD")? {
            RefValue::Direct(oid) => Ok(Head::Detached(oid)),
            RefValue::Symbolic(target) => {
                let branch_name = target
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&target);
                
                let oid = self.resolve_recursive(&target, 0)?;
                
                Ok(Head::Branch(Branch {
                    name: branch_name.to_string(),
                    oid,
                }))
            }
        }
    }
    
    /// ブランチ一覧取得
    pub fn branches(&self) -> Result<Vec<Branch>> {
        let heads_dir = self.git_dir.join("refs/heads");
        if !heads_dir.exists() {
            return Ok(Vec::new());
        }
        
        let mut branches = Vec::new();
        self.collect_refs_recursive(&heads_dir, "", &mut branches)?;
        
        branches.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(branches)
    }
    
    /// 再帰的にrefs収集（ネストしたブランチ対応）
    fn collect_refs_recursive(
        &self,
        dir: &Path,
        prefix: &str,
        branches: &mut Vec<Branch>,
    ) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            
            let full_name = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", prefix, name)
            };
            
            if path.is_dir() {
                self.collect_refs_recursive(&path, &full_name, branches)?;
            } else {
                let ref_path = format!("refs/heads/{}", full_name);
                let oid = self.resolve_recursive(&ref_path, 0)?;
                branches.push(Branch { name: full_name, oid });
            }
        }
        Ok(())
    }
    
    /// 参照を解決（SHA or ブランチ名 → Oid）
    pub fn resolve(&self, refspec: &str) -> Result<Oid> {
        // 完全なSHA
        if refspec.len() == 40 && refspec.chars().all(|c| c.is_ascii_hexdigit()) {
            return Oid::from_hex(refspec);
        }
        
        // refs/heads/xxx
        let full_ref = if refspec.starts_with("refs/") {
            refspec.to_string()
        } else {
            format!("refs/heads/{}", refspec)
        };
        
        self.resolve_recursive(&full_ref, 0)
    }
}
```

---

## 4. Index Layer

### 4.1 インデックス読み取り（`index::reader`）

#### 4.1.1 内部構造

```rust
/// Indexファイルヘッダー
struct IndexHeader {
    signature: [u8; 4],  // "DIRC"
    version: u32,
    entry_count: u32,
}

/// Index エントリ（v2形式）
struct RawIndexEntry {
    ctime_sec: u32,
    ctime_nsec: u32,
    mtime_sec: u32,
    mtime_nsec: u32,
    dev: u32,
    ino: u32,
    mode: u32,
    uid: u32,
    gid: u32,
    size: u32,
    oid: [u8; 20],
    flags: u16,
    // extended_flags: u16,  // v3以降
    name: String,  // 可変長、NULパディング
}
```

#### 4.1.2 パースアルゴリズム

```
Index v2 形式:
  [4] "DIRC" (signature)
  [4] version (big-endian, 2/3/4)
  [4] entry count (big-endian)
  
  [entries...]
    [4] ctime seconds
    [4] ctime nanoseconds
    [4] mtime seconds
    [4] mtime nanoseconds
    [4] dev
    [4] ino
    [4] mode
    [4] uid
    [4] gid
    [4] file size
    [20] SHA-1
    [2] flags (name length in lower 12 bits)
    [?] name (NUL-terminated)
    [1-8] padding to 8-byte boundary
  
  [extensions...]  (オプション)
  [20] checksum (SHA-1 of all preceding content)

パース手順:
1. ヘッダー読み取り（12バイト）
2. シグネチャ検証 ("DIRC")
3. バージョン検証 (2, 3, 4)
4. エントリを順次パース
5. チェックサム検証（オプション）
```

```rust
impl Index {
    pub(crate) fn parse(data: &[u8]) -> Result<Self> {
        let mut cursor = Cursor::new(data);
        
        // ヘッダー
        let header = Self::parse_header(&mut cursor)?;
        
        // エントリ
        let mut entries = Vec::with_capacity(header.entry_count as usize);
        for _ in 0..header.entry_count {
            let entry = Self::parse_entry(&mut cursor, header.version)?;
            entries.push(entry);
        }
        
        Ok(Self {
            version: header.version,
            entries,
        })
    }
    
    fn parse_header(cursor: &mut Cursor<&[u8]>) -> Result<IndexHeader> {
        let mut sig = [0u8; 4];
        cursor.read_exact(&mut sig)?;
        
        if &sig != b"DIRC" {
            return Err(Error::InvalidIndex {
                version: 0,
                reason: "invalid signature".into(),
            });
        }
        
        let version = cursor.read_u32::<BigEndian>()?;
        if version < 2 || version > 4 {
            return Err(Error::InvalidIndex {
                version,
                reason: format!("unsupported version: {}", version),
            });
        }
        
        let entry_count = cursor.read_u32::<BigEndian>()?;
        
        Ok(IndexHeader { signature: sig, version, entry_count })
    }
    
    fn parse_entry(cursor: &mut Cursor<&[u8]>, version: u32) -> Result<IndexEntry> {
        let entry_start = cursor.position();
        
        let ctime = cursor.read_u32::<BigEndian>()? as u64;
        let _ctime_nsec = cursor.read_u32::<BigEndian>()?;
        let mtime = cursor.read_u32::<BigEndian>()? as u64;
        let _mtime_nsec = cursor.read_u32::<BigEndian>()?;
        let _dev = cursor.read_u32::<BigEndian>()?;
        let _ino = cursor.read_u32::<BigEndian>()?;
        let mode = cursor.read_u32::<BigEndian>()?;
        let _uid = cursor.read_u32::<BigEndian>()?;
        let _gid = cursor.read_u32::<BigEndian>()?;
        let size = cursor.read_u32::<BigEndian>()?;
        
        let mut oid_bytes = [0u8; 20];
        cursor.read_exact(&mut oid_bytes)?;
        let oid = Oid::from_bytes(&oid_bytes)?;
        
        let flags = cursor.read_u16::<BigEndian>()?;
        let name_len = (flags & 0x0FFF) as usize;
        
        // v3以降: extended flags
        if version >= 3 && (flags & 0x4000) != 0 {
            let _extended = cursor.read_u16::<BigEndian>()?;
        }
        
        // 名前読み取り
        let mut name_buf = vec![0u8; name_len];
        cursor.read_exact(&mut name_buf)?;
        let name = String::from_utf8(name_buf)
            .map_err(|_| Error::InvalidIndex { /* ... */ })?;
        
        // パディングをスキップ（8バイト境界）
        let entry_size = cursor.position() - entry_start;
        let padding = (8 - (entry_size % 8)) % 8;
        cursor.seek(SeekFrom::Current(padding as i64))?;
        
        Ok(IndexEntry {
            oid,
            path: PathBuf::from(name),
            mode: Self::parse_mode(mode)?,
            size,
            mtime,
            ctime,
        })
    }
    
    fn parse_mode(mode: u32) -> Result<FileMode> {
        match mode {
            0o100644 => Ok(FileMode::Regular),
            0o100755 => Ok(FileMode::Executable),
            0o120000 => Ok(FileMode::Symlink),
            0o160000 => Ok(FileMode::Submodule),
            _ => Err(Error::InvalidIndex { /* ... */ }),
        }
    }
}
```

---

## 5. Repository Layer

### 5.1 メイン構造体（`repository`）

#### 5.1.1 内部構造

```rust
pub struct Repository {
    /// リポジトリルートパス
    path: PathBuf,
    /// .gitディレクトリパス
    git_dir: PathBuf,
    /// オブジェクトストア
    objects: LooseObjectStore,
    /// 参照ストア
    refs: RefStore,
}
```

#### 5.1.2 内部関数

```rust
impl Repository {
    /// .gitディレクトリの検証
    fn validate_git_dir(git_dir: &Path) -> Result<()> {
        // 必須ディレクトリ/ファイルの存在確認
        let required = ["objects", "refs", "HEAD"];
        
        for item in required {
            let path = git_dir.join(item);
            if !path.exists() {
                return Err(Error::NotARepository(git_dir.to_path_buf()));
            }
        }
        
        Ok(())
    }
    
    /// 短縮SHAからOidを解決
    fn resolve_short_oid(&self, id: &str) -> Result<Oid> {
        if id.len() == 40 {
            return Oid::from_hex(id);
        }
        
        if id.len() < 4 {
            return Err(Error::InvalidOid(format!(
                "SHA prefix too short: {} (minimum 4 characters)",
                id
            )));
        }
        
        Oid::find_by_prefix(&self.objects, id)
    }
}
```

#### 5.1.3 公開API実装

```rust
impl Repository {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().canonicalize()
            .map_err(|_| Error::NotARepository(path.as_ref().to_path_buf()))?;
        
        let git_dir = if path.ends_with(".git") {
            path.clone()
        } else {
            path.join(".git")
        };
        
        Self::validate_git_dir(&git_dir)?;
        
        let repo_path = if git_dir.ends_with(".git") {
            git_dir.parent().unwrap().to_path_buf()
        } else {
            git_dir.clone()
        };
        
        Ok(Self {
            path: repo_path,
            git_dir: git_dir.clone(),
            objects: LooseObjectStore::new(git_dir.join("objects")),
            refs: RefStore::new(git_dir),
        })
    }
    
    pub fn discover<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut current = path.as_ref().canonicalize()
            .map_err(|_| Error::NotARepository(path.as_ref().to_path_buf()))?;
        
        loop {
            let git_dir = current.join(".git");
            if git_dir.is_dir() {
                return Self::open(&current);
            }
            
            if !current.pop() {
                return Err(Error::NotARepository(path.as_ref().to_path_buf()));
            }
        }
    }
    
    pub fn commit(&self, id: &str) -> Result<Commit> {
        let oid = self.resolve_short_oid(id)?;
        let obj = self.objects.read(&oid)?;
        
        obj.into_commit().map_err(|_| Error::TypeMismatch {
            expected: "commit",
            actual: obj.kind(),
        })
    }
    
    pub fn log(&self) -> Result<LogIterator<'_>> {
        let head = self.refs.head()?;
        self.log_from(&head.oid().to_hex())
    }
    
    pub fn log_from(&self, id: &str) -> Result<LogIterator<'_>> {
        let start_oid = self.resolve_short_oid(id)?;
        Ok(LogIterator::new(self, start_oid))
    }
}
```

---

### 5.2 ログイテレータ（`log`）

#### 5.2.1 内部構造

```rust
pub struct LogIterator<'a> {
    repo: &'a Repository,
    /// 処理待ちコミット（優先度キュー：時刻降順）
    pending: BinaryHeap<PendingCommit>,
    /// 処理済みコミット
    seen: HashSet<Oid>,
}

/// 優先度キュー用のラッパー
struct PendingCommit {
    oid: Oid,
    time: i64,
}

impl Ord for PendingCommit {
    fn cmp(&self, other: &Self) -> Ordering {
        // 時刻降順（新しい順）
        self.time.cmp(&other.time)
    }
}
```

#### 5.2.2 イテレータ実装

```rust
impl<'a> LogIterator<'a> {
    pub(crate) fn new(repo: &'a Repository, start: Oid) -> Self {
        let mut pending = BinaryHeap::new();
        pending.push(PendingCommit { oid: start, time: i64::MAX });
        
        Self {
            repo,
            pending,
            seen: HashSet::new(),
        }
    }
}

impl Iterator for LogIterator<'_> {
    type Item = Result<Commit>;
    
    fn next(&mut self) -> Option<Self::Item> {
        while let Some(PendingCommit { oid, .. }) = self.pending.pop() {
            // 重複スキップ（マージで同じコミットに到達する場合）
            if self.seen.contains(&oid) {
                continue;
            }
            self.seen.insert(oid);
            
            // コミット読み取り
            let commit = match self.repo.commit(&oid.to_hex()) {
                Ok(c) => c,
                Err(e) => return Some(Err(e)),
            };
            
            // 親コミットをキューに追加
            for parent_oid in commit.parents() {
                if !self.seen.contains(parent_oid) {
                    // 親の時刻を取得（エラー時は0として扱う）
                    let time = self.repo.commit(&parent_oid.to_hex())
                        .map(|c| c.author().time())
                        .unwrap_or(0);
                    
                    self.pending.push(PendingCommit {
                        oid: *parent_oid,
                        time,
                    });
                }
            }
            
            return Some(Ok(commit));
        }
        
        None
    }
}
```

---

### 5.3 ステータス（`status`）

#### 5.3.1 内部構造

```rust
/// 比較用の正規化されたエントリ
struct NormalizedEntry {
    path: PathBuf,
    oid: Option<Oid>,
    mode: Option<FileMode>,
}

/// 三方比較の結果
enum DiffResult {
    Unmodified,
    Added,
    Deleted,
    Modified,
}
```

#### 5.3.2 アルゴリズム

```
Status計算手順:

1. データ収集
   - HEAD tree → HashMap<Path, (Oid, Mode)>
   - Index → HashMap<Path, (Oid, Mode, Mtime)>
   - Working tree → HashMap<Path, Mtime>

2. 三方比較
   FOR each unique path in (HEAD ∪ Index ∪ Working):
       head_entry = HEAD[path]
       index_entry = Index[path]
       work_entry = Working[path]
       
       IF work_entry exists AND index_entry not exists:
           status = Untracked
       ELSE IF work_entry not exists AND index_entry exists:
           status = Deleted
       ELSE IF index_entry.oid != head_entry.oid:
           status = Staged*
       ELSE IF file_changed(work_entry, index_entry):
           status = Modified
       ELSE:
           status = Clean (skip)

3. 結果をソートして返却
```

```rust
impl Repository {
    pub fn status(&self) -> Result<Vec<StatusEntry>> {
        // HEAD tree取得
        let head_tree = self.get_head_tree()?;
        let head_entries = self.flatten_tree(&head_tree, PathBuf::new())?;
        
        // Index読み取り
        let index = self.index()?;
        let index_map: HashMap<_, _> = index.entries()
            .iter()
            .map(|e| (e.path().to_path_buf(), e))
            .collect();
        
        // ワーキングツリー
        let work_files = fs::list_working_tree(&self.path)?;
        let work_set: HashSet<_> = work_files.iter()
            .map(|p| p.strip_prefix(&self.path).unwrap().to_path_buf())
            .collect();
        
        let mut result = Vec::new();
        
        // 全パスを収集
        let mut all_paths: HashSet<PathBuf> = HashSet::new();
        all_paths.extend(head_entries.keys().cloned());
        all_paths.extend(index_map.keys().cloned());
        all_paths.extend(work_set.iter().cloned());
        
        for path in all_paths {
            let status = self.compute_status(
                &path,
                head_entries.get(&path),
                index_map.get(&path).copied(),
                work_set.contains(&path),
            )?;
            
            if let Some(status) = status {
                result.push(StatusEntry { path, status });
            }
        }
        
        result.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(result)
    }
    
    fn compute_status(
        &self,
        path: &Path,
        head: Option<&(Oid, FileMode)>,
        index: Option<&IndexEntry>,
        in_worktree: bool,
    ) -> Result<Option<FileStatus>> {
        match (head, index, in_worktree) {
            // Untracked: ワーキングにあるがIndexにない
            (_, None, true) => Ok(Some(FileStatus::Untracked)),
            
            // Deleted: Indexにあるがワーキングにない
            (_, Some(_), false) => Ok(Some(FileStatus::Deleted)),
            
            // Added (staged): Indexにあるが HEADにない
            (None, Some(_), _) => Ok(Some(FileStatus::Added)),
            
            // Staged Deleted: HEADにあるがIndexにない（かつワーキングにもない）
            (Some(_), None, false) => Ok(Some(FileStatus::StagedDeleted)),
            
            // 変更チェック
            (Some((head_oid, _)), Some(idx), true) => {
                if idx.oid() != head_oid {
                    // Index ≠ HEAD → Staged
                    Ok(Some(FileStatus::StagedModified))
                } else if self.file_modified(path, idx)? {
                    // Working ≠ Index → Modified
                    Ok(Some(FileStatus::Modified))
                } else {
                    // 変更なし
                    Ok(None)
                }
            }
            
            _ => Ok(None),
        }
    }
    
    /// ワーキングツリーのファイルがIndexと異なるか
    fn file_modified(&self, path: &Path, index_entry: &IndexEntry) -> Result<bool> {
        let full_path = self.path.join(path);
        let metadata = std::fs::metadata(&full_path)?;
        
        // サイズチェック（高速）
        if metadata.len() as u32 != index_entry.size() {
            return Ok(true);
        }
        
        // mtime チェック（高速）
        let mtime = metadata.modified()
            .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs())
            .unwrap_or(0);
        
        if mtime != index_entry.mtime() {
            // mtimeが違う → 内容を確認
            let content = fs::read_file(&full_path)?;
            let oid = hash::hash_object("blob", &content);
            return Ok(oid != *index_entry.oid());
        }
        
        Ok(false)
    }
    
    /// Treeをフラット化
    fn flatten_tree(
        &self,
        tree: &Tree,
        prefix: PathBuf,
    ) -> Result<HashMap<PathBuf, (Oid, FileMode)>> {
        let mut result = HashMap::new();
        
        for entry in tree.entries() {
            let path = prefix.join(entry.name());
            
            if entry.is_tree() {
                let subtree = self.tree(&entry.oid().to_hex())?;
                result.extend(self.flatten_tree(&subtree, path)?);
            } else {
                result.insert(path, (*entry.oid(), entry.mode()));
            }
        }
        
        Ok(result)
    }
}
```

---

## 6. エラー処理の詳細

### 6.1 エラー伝播フロー

```
[infra層]                    [domain層]                    [API層]
     │                            │                            │
     │ std::io::Error             │                            │
     │ ─────────────────────────> │                            │
     │     From<io::Error>        │                            │
     │                            │ Error::InvalidObject       │
     │                            │ ─────────────────────────> │
     │                            │     そのまま伝播           │
     │                            │                            │
     │ miniz_oxide::Error         │                            │
     │ ─────────────────────────> │                            │
     │  Error::DecompressionFailed│                            │
```

### 6.2 エラーコンテキスト付加

```rust
/// コンテキスト付きエラー拡張
trait ResultExt<T> {
    fn with_context<F, S>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> S,
        S: Into<String>;
}

impl<T> ResultExt<T> for Result<T> {
    fn with_context<F, S>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> S,
        S: Into<String>,
    {
        self.map_err(|e| {
            // エラーにコンテキストを追加
            match e {
                Error::Io(io_err) => Error::Io(std::io::Error::new(
                    io_err.kind(),
                    format!("{}: {}", f().into(), io_err),
                )),
                other => other,
            }
        })
    }
}

// 使用例
fn read_object(oid: &Oid) -> Result<Object> {
    let path = oid_to_path(oid);
    let data = fs::read_file(&path)
        .with_context(|| format!("reading object {}", oid.short()))?;
    // ...
}
```

### 6.3 エラー回復戦略

| エラー種別 | 回復可能性 | 戦略 |
|------------|------------|------|
| `Io` (NotFound) | 状況依存 | 上位で判断（存在確認 vs 必須読み取り） |
| `Io` (PermissionDenied) | 不可 | そのまま伝播 |
| `ObjectNotFound` | 部分的 | 短縮SHA検索を試みる |
| `InvalidObject` | 不可 | そのまま伝播（データ破損） |
| `DecompressionFailed` | 不可 | そのまま伝播（データ破損） |
| `InvalidOid` | 不可 | 入力検証エラーとして伝播 |

### 6.4 パニック回避

```rust
// ❌ 避けるべきパターン
fn get_object(id: &str) -> Object {
    let oid = Oid::from_hex(id).unwrap(); // パニックの可能性
    self.objects.read(&oid).expect("object must exist")
}

// ✅ 推奨パターン
fn get_object(id: &str) -> Result<Object> {
    let oid = Oid::from_hex(id)?;
    self.objects.read(&oid)
}
```

### 6.5 エラーメッセージ設計

```rust
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // 具体的な情報を含める
            Error::ObjectNotFound(oid) => {
                write!(f, "object not found: {}", oid.short())
            }
            
            // 原因を明確に
            Error::InvalidObject { oid, reason } => {
                write!(f, "invalid object {}: {}", oid.short(), reason)
            }
            
            // 次のアクションを示唆
            Error::NotARepository(path) => {
                write!(
                    f,
                    "not a git repository (or any parent up to mount point {})",
                    path.display()
                )
            }
            
            // I/Oエラーはそのまま
            Error::Io(e) => write!(f, "I/O error: {}", e),
            
            // ...
        }
    }
}
```

---

## 7. Phase 2 詳細設計（概要）

### 7.1 Index書き込み

```rust
impl Index {
    /// Indexをファイルに書き込み
    pub fn write(&self, path: &Path) -> Result<()> {
        let mut buffer = Vec::new();
        
        // ヘッダー
        buffer.extend_from_slice(b"DIRC");
        buffer.extend_from_slice(&self.version.to_be_bytes());
        buffer.extend_from_slice(&(self.entries.len() as u32).to_be_bytes());
        
        // エントリ
        for entry in &self.entries {
            self.write_entry(&mut buffer, entry)?;
        }
        
        // チェックサム
        let checksum = hash::sha1(&buffer);
        buffer.extend_from_slice(&checksum);
        
        // アトミック書き込み
        fs::write_file_atomic(path, &buffer)
    }
}
```

### 7.2 コミット作成

```rust
impl Repository {
    pub fn create_commit(
        &self,
        message: &str,
        author: Option<&Signature>,
        committer: Option<&Signature>,
    ) -> Result<Oid> {
        // 1. Indexからtreeを構築
        let tree_oid = self.build_tree_from_index()?;
        
        // 2. 親コミット取得
        let parents = match self.head() {
            Ok(head) => vec![*head.oid()],
            Err(Error::RefNotFound(_)) => vec![], // 初期コミット
            Err(e) => return Err(e),
        };
        
        // 3. Signature解決
        let author = author
            .cloned()
            .or_else(|| self.config_signature())
            .ok_or(Error::ConfigNotFound("user.name/email".into()))?;
        let committer = committer.cloned().unwrap_or_else(|| author.clone());
        
        // 4. コミットオブジェクト作成
        let commit_content = self.format_commit(
            &tree_oid,
            &parents,
            &author,
            &committer,
            message,
        );
        
        let commit_oid = self.objects.write("commit", commit_content.as_bytes())?;
        
        // 5. HEAD更新
        self.update_head(&commit_oid)?;
        
        Ok(commit_oid)
    }
}
```

---

## 8. 設計上の注意事項

### 8.1 スレッドセーフティ

- `Repository` は `Send + Sync` ではない（内部で可変状態を持つ可能性）
- 並列処理が必要な場合は外部で同期を取る
- Phase 3 以降で `Arc<Repository>` 対応を検討

### 8.2 メモリ効率

- 大きなBlob は遅延読み込み（`LazyBlob` パターン）
- LogIterator はストリーミング処理
- Tree の flatten は必要時のみ実行

### 8.3 拡張ポイント

```rust
/// 将来の拡張: Packfileサポート
trait ObjectReader {
    fn read(&self, oid: &Oid) -> Result<Object>;
    fn exists(&self, oid: &Oid) -> bool;
}

// LooseObjectStore は ObjectReader を実装
// PackfileStore は ObjectReader を実装（Phase 3）
// ChainedStore は複数の ObjectReader を連結
```
