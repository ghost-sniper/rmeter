# Crate documentation

Build from the workspace root with Rust and Cargo:

```sh
cargo doc --workspace --no-deps --open
cargo test --workspace --doc
```

The default landing pages and API reference are English. Each crate has a
`translations` section with Simplified Chinese, Traditional Chinese, Japanese,
French and German guides. All languages are generated in one build; no locale
feature flags or environment variables are required. The translations cover the
crate guides, not every API comment or the existing Chinese repository manuals.

Generated entry points:

| Crate | HTML file |
| --- | --- |
| `rmeter` | `target/doc/rmeter/index.html` |
| `rmeter-core` | `target/doc/rmeter_core/index.html` |
| `rmeter-component` | `target/doc/rmeter_component/index.html` |

For example, the Simplified Chinese root guide is at
`target/doc/rmeter/translations/zh_cn/index.html`. Other locale module names are
`zh_tw`, `ja`, `fr`, and `de`. These documentation-only modules are compiled under
`cfg(doc)` and do not add runtime APIs or dependencies.

## Sources

| Language | rmeter | rmeter-core | rmeter-component |
| --- | --- | --- | --- |
| English (default) | [en](en.md) | [en](../rmeter-core/docs/en.md) | [en](../rmeter-component/docs/en.md) |
| 简体中文 | [zh-CN](zh-CN.md) | [zh-CN](../rmeter-core/docs/zh-CN.md) | [zh-CN](../rmeter-component/docs/zh-CN.md) |
| 繁體中文 | [zh-TW](zh-TW.md) | [zh-TW](../rmeter-core/docs/zh-TW.md) | [zh-TW](../rmeter-component/docs/zh-TW.md) |
| 日本語 | [ja](ja.md) | [ja](../rmeter-core/docs/ja.md) | [ja](../rmeter-component/docs/ja.md) |
| Français | [fr](fr.md) | [fr](../rmeter-core/docs/fr.md) | [fr](../rmeter-component/docs/fr.md) |
| Deutsch | [de](de.md) | [de](../rmeter-core/docs/de.md) | [de](../rmeter-component/docs/de.md) |

Guide API links use rustdoc paths and resolve in the generated HTML. The
repository README files remain separate, detailed Chinese usage manuals.

## Maintaining the guides

Update all six guide files for a crate when its behavior or examples change.
Keep Rust identifiers, JSON keys and code examples identical across languages.
Use intra-doc links for APIs rather than hard-coded generated HTML paths. Runnable
examples should avoid external services; use `rust,no_run` for examples requiring
HTTP, PostgreSQL or a deployed worker so Cargo still compiles them.

To reject broken rustdoc links and other documentation warnings in PowerShell:

```powershell
$previousRustdocFlags = $env:RUSTDOCFLAGS
try {
    $env:RUSTDOCFLAGS = '-D warnings'
    cargo doc --workspace --all-features --no-deps
    if ($LASTEXITCODE -ne 0) { throw 'Documentation build failed' }
    cargo test --workspace --all-features --doc
    if ($LASTEXITCODE -ne 0) { throw 'Documentation examples failed' }
} finally {
    $env:RUSTDOCFLAGS = $previousRustdocFlags
}
```

Build outputs stay under the ignored `target/` directory. Do not commit them.
