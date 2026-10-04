# Test Fixtures

テスト用のGitリポジトリフィクスチャです。

## フィクスチャ一覧

| ディレクトリ | 説明 |
|-------------|------|
| `simple/` | 基本的なリポジトリ（2コミット） |
| `empty/` | 空のリポジトリ（コミットなし） |
| `branches/` | 複数ブランチを持つリポジトリ |
| `remotes/` | リモート追跡ブランチ（origin、upstream、ネスト名）を持つリポジトリ |
| `tags/` | 軽量タグと注釈付きタグを持つリポジトリ |
| `diff/` | 追加・削除・変更（ネストしたパスを含む）の2コミット |
| `rename/` | 完全一致リネームの2コミット |
| `merge/` | `--no-ff` のマージコミットを持つリポジトリ |

すべて `main` ブランチで作成されます。ユーザー・システムのGit設定（既定ブランチ名、`core.autocrlf` など）は無視し、作成者と日時も固定するため、どの環境でも同じ内容になります。

## フィクスチャの作成

フィクスチャの定義は `create_fixtures.sh` に一本化しています。

### Linux / macOS

```bash
cd tests/fixtures
bash create_fixtures.sh
```

### Windows

Git for Windows 付属のbashで `create_fixtures.sh` を実行します。

```powershell
powershell -ExecutionPolicy Bypass -File tests\fixtures\create_fixtures.ps1
```

## CI設定

`.github/workflows/ci.yml` では、全OSで `bash create_fixtures.sh` を実行してからテストします。

## 注意事項

- フィクスチャは `.gitignore` に追加されているため、リポジトリにはコミットされません
- テスト実行前に必ずフィクスチャ作成スクリプトを実行してください
- スクリプトは冪等性があり、再実行しても問題ありません
