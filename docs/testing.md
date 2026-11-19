# How oxidows is checked against Windows

The apps replace closed-source Windows programs, so there is no original source to diff against.
Instead the real Windows on the CI runner is the oracle: the same input goes through the Windows
API and through our code, and a difference fails the build. The idea comes from
[rust-anything](https://github.com/erkamyaman/rust-anything) (record the original's behavior,
replay it against the port, bench, then gate).

| Check | Where | Fails CI when |
| --- | --- | --- |
| Parity with Windows | `crates/explorer-sys/tests/oracle.rs` | the portable name order (vs `StrCmpLogicalW`), the Size column (vs `StrFormatKBSizeW`), short sizes like "1.50 KB" (vs `StrFormatByteSizeW`) or file time text (vs `FileTimeToSystemTime`) differs; also the comparer the app installs |
| Divergence report | same file, `divergence_report` | never; prints where the portable name order still differs (punctuation, non-ASCII names) |
| Bench vs Windows | `crates/explorer-sys/benches/vs_windows.rs` | sorting 100k entries takes over 5 s; the table is in the job summary |
| Gate: format, lints | `cargo fmt --check`, `cargo clippy -D warnings` | any warning |
| Gate: no panics in logic | `#![deny(clippy::unwrap_used, clippy::expect_used)]` in `explorer-core` and `taskmgr-core` (tests exempt) | an `unwrap()` or `expect()` in library code |
| Gate: no unsafe in logic | `#![forbid(unsafe_code)]` in the same two crates | any `unsafe` |

Name order is the one thing the code doesn't copy: punctuation, accents and the user's language follow
Windows' collation tables, so on Windows the app installs `StrCmpLogicalW` itself
(`explorer_sys::use_windows_name_order`, called first thing in `oxi-explorer`'s `main`). The portable
`natural_cmp_portable` is what unit tests and other platforms use.

## Adding a case

Put the edge case in the corpus of the matching test in `oracle.rs` (names, sizes, timestamps).
For a new function that mirrors a Windows one, copy a test: call the Windows function through
`windows-sys`, call ours, and collect every mismatch into the assertion message so one CI run
shows them all.

Run locally on Windows with `cargo test -p explorer-sys --test oracle` and
`cargo bench -p explorer-sys --bench vs_windows`.
