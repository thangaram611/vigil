# Golden fixtures

Byte-exact reference output, captured from the retired Bash implementation
before it was deleted. The Bash source is gone, so these files cannot be
regenerated — they are now the frozen reference. Change one only together with
the renderer it pins, and say why in the commit.

| File | Pins | Read by |
| --- | --- | --- |
| `user_agent.plist` | the LaunchAgent plist | `src/service/tests.rs` |
| `helper.plist` | the root LaunchDaemon plist | `src/service/tests.rs` |
| `vigil.newsyslog` | the `newsyslog.d` entry | `src/service/tests.rs` |
| `status_clean.json` | `vigil status --json`, clean state | `src/check/tests.rs` |
| `status_assertions.json` | `vigil status --json` with two assertion holders | `src/check/tests.rs` |

The JSON goldens predate the `"version": 1,` first key; the tests insert that
line before comparing. Paths inside the files point at the capture sandbox
(`/private/tmp/vigil-golden-sbx`), and `helper.plist` / `vigil.newsyslog` carry
the capture machine's uid and user name — the tests substitute both.
