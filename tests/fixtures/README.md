# Test Fixtures

テスト用のGitリポジトリフィクスチャです。

## フィクスチャ一覧

| ディレクトリ | 説明 |
|-------------|------|
| `simple/` | 基本的なリポジトリ（2コミット） |
| `empty/` | 空のリポジトリ（コミットなし） |
| `branches/` | 複数ブランチを持つリポジトリ |

## フィクスチャの作成

### Linux / macOS

```bash
cd tests/fixtures
bash create_fixtures.sh
```

### Windows

```powershell
cd tests\fixtures
powershell -ExecutionPolicy Bypass -File create_fixtures.ps1
```

## CI設定

GitHub Actionsでの使用例:

```yaml
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Create test fixtures
        run: |
          cd tests/fixtures
          bash create_fixtures.sh

      - name: Run tests
        run: cargo test
```

Windows CIでの使用例:

```yaml
jobs:
  test:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4

      - name: Create test fixtures
        run: |
          cd tests\fixtures
          powershell -ExecutionPolicy Bypass -File create_fixtures.ps1

      - name: Run tests
        run: cargo test
```

## 注意事項

- フィクスチャは `.gitignore` に追加されているため、リポジトリにはコミットされません
- テスト実行前に必ずフィクスチャ作成スクリプトを実行してください
- スクリプトは冪等性があり、再実行しても問題ありません
