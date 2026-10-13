# Golden fixtures

Byte-exact reference output. Service files preserve the retired Bash captures.
Status JSON now pins the version 2 schema: fresh session activity, separately
labeled cached daemon policy and read-only helper installation evidence.
Change a fixture together with the renderer it pins and explain why.

| File | Pins | Read by |
| --- | --- | --- |
| `user_agent.plist` | the LaunchAgent plist | `src/service/tests.rs` |
| `helper.plist` | the root LaunchDaemon plist | `src/service/tests.rs` |
| `vigil.newsyslog` | the `newsyslog.d` entry | `src/service/tests.rs` |
| `status_clean.json` | `vigil status --json`, clean state | `src/check/tests.rs` |
| `status_assertions.json` | `vigil status --json` with two assertion holders | `src/check/tests.rs` |

The JSON goldens include their schema version and compare directly. Their recent
Codex file supplies labeled uncertain fallback, not an exact working session.
Paths inside the files point at the capture sandbox
(`/private/tmp/vigil-golden-sbx`), and `helper.plist` / `vigil.newsyslog` carry
the capture machine's uid and user name — the tests substitute both.
