# zerogit-remote

Remote operations for [zerogit](https://github.com/siska-tech/zerogit):
`clone`, `fetch` and `push`.

zerogit keeps a single dependency (`miniz_oxide`); this crate adds the
network side on top of it:

| Transport | URLs | Authentication |
| --- | --- | --- |
| Local | paths, `file://` | none (the repository is read and written directly, without running Git) |
| Smart HTTP(S) | `http://`, `https://` | Basic (in the URL or with `HttpAuth`), bearer token |
| SSH | `user@host:path`, `ssh://` | keys, through the system's `ssh` client and agent (`GIT_SSH_COMMAND` is honored) |

Fetching uses Git protocol version 2 and falls back to the original
protocol when a server does not offer it; pushing uses the receive-pack
protocol. TLS is provided by rustls (the `https` feature, enabled by
default).

```rust,no_run
use std::path::Path;
use zerogit_remote::{clone, fetch, push, CloneOptions, PushOptions};

let repo = clone("https://github.com/user/repo.git", Path::new("repo"), &CloneOptions::new())?;
fetch(&repo, "origin")?;
push(&repo, "origin", &["main"], &PushOptions::new().set_upstream(true))?;
# Ok::<(), zerogit_remote::Error>(())
```

Results follow Git: references, remote and upstream configuration,
`FETCH_HEAD` and reflog messages are the same as with `git clone`,
`git fetch` and `git push`, which the tests check over the local
transport, Git's own `upload-pack`/`receive-pack`, and `git http-backend`.

Not supported: shallow and partial clones, the `git://` protocol,
credential helpers, and push options such as `--atomic`.

## License

MIT OR Apache-2.0
