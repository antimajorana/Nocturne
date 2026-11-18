# onenote.rs OneNote fixture corpus

This folder recognizes the work of the [`onenote.rs` Rust OneNote parser](https://github.com/msiemens/onenote.rs) and the fixture contributors credited by that project. Only test data is copied; no parser source code is included.

The full upstream `crates/parser/tests/samples/` fixture set at commit `de3a64e8159c684b137ded89c3fef3c17272ec1d` is represented under `samples/`, preserving relative paths and source bytes. It contains 32 `.one` section files and 20 `.onetoc2` files (omitting the two >100 MB upstream samples). `samples/SHA256SUMS.txt` records the SHA-256 of every sample. Paths with non-ASCII characters are percent-encoded as UTF-8 in the manifest.

The onenote.rs repository is MPL-2.0. `LICENSE-MPL-2.0.txt` is included for samples without a more specific notice. The `joplin/` subtree has its own AGPL-3.0-or-later notice and full license. The `onenote-2016/` subtree has an MIT notice from OfficeDev/Interop-TestSuites. Each subfolder README describes its source and terms.

The corpus tests exercise all 32 bundled `.one` files and report ignored `.onetoc2` files in both HTML and CMGFM modes. Unsupported or skipped samples are recorded as such; their presence does not claim compatibility.

Source revision: https://github.com/msiemens/onenote.rs/tree/de3a64e8159c684b137ded89c3fef3c17272ec1d
