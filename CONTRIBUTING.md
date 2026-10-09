# Contributing

Thanks for your interest in caffeinate for Windows! Bug reports, ideas,
documentation fixes and pull requests are all welcome.

## Issues

- Search the [existing issues](https://github.com/metinata/caffeinate-win/issues)
  first; someone may have reported the same thing.
- For bugs, use the bug report form. The output of `caffeinate --version`, your
  Windows version and the exact command make a bug much easier to fix.
- caffeinate aims to behave like the macOS command. Ideas that go beyond it are
  welcome, but please open an issue to discuss them before writing code.
- Security vulnerabilities should be reported privately as described in
  [SECURITY.md](SECURITY.md), not in public issues.

## Development setup

You need:

- Windows 10 or 11 (x64 or ARM64)
- [Rust](https://rustup.rs) 1.85 or newer
- The MSVC build tools: Visual Studio or Build Tools with the
  "Desktop development with C++" workload, which includes the Windows SDK

Then:

```console
> git clone https://github.com/metinata/caffeinate-win
> cd caffeinate-win
> cargo build
> cargo run -- -d -t 5
```

## Checks

Run these before you open a pull request:

```console
> cargo fmt --all
> cargo clippy --all-targets -- -D warnings
> cargo test
```

CI runs the same checks, builds for ARM64 and checks the minimum supported Rust
version (1.85).

`.cargo/config.toml` links the C runtime statically, so that `caffeinate.exe`
runs without the Visual C++ Redistributable. A `RUSTFLAGS` environment variable
replaces that setting, and a test then fails; unset it to build.

### About the integration tests

The tests in `tests/cli.rs` run the real `caffeinate.exe` and read the system-wide
power state, so they keep your PC awake for a few seconds at a time. Other programs
that keep the PC awake, such as a playing video or a keep-awake tool (including
another `caffeinate`), hold the same state; the tests then cannot check those parts,
pass them and print a warning. For a complete run, close such programs first.
`powercfg /requests`, run from an elevated prompt, shows what is active.

## Guidelines

- **Match macOS.** Options, output and exit codes should behave like macOS
  `caffeinate` unless Windows makes that impossible. Document any difference in
  `README.md`.
- **Stay small.** caffeinate is a single executable with two small dependencies.
  Please discuss new dependencies in an issue first.
- **Explain `unsafe`.** Every `unsafe` block needs a `// SAFETY:` comment that says
  why it is sound. Clippy enforces this.
- **Test changes.** Unit tests live next to the code; end-to-end tests go in
  `tests/cli.rs`.
- **Record user-facing changes** in `CHANGELOG.md` under `[Unreleased]`.

Commit messages should start with a short summary in the imperative mood, for
example "Fix -w when the process has already exited", with details in the body
when the reason is not obvious.

## Releasing

For maintainers:

1. Set the new version in `Cargo.toml` and run `cargo check` to update `Cargo.lock`.
2. In `CHANGELOG.md`, rename `[Unreleased]` to `[X.Y.Z] - YYYY-MM-DD`, add a fresh
   empty `[Unreleased]` section above it, and update the links at the bottom.
3. Commit as "Release vX.Y.Z", then tag and push:

   ```console
   > git tag vX.Y.Z
   > git push origin main vX.Y.Z
   ```

The release workflow checks that the tag matches `Cargo.toml`, builds x86_64 and
ARM64 zips with checksums and provenance attestations, and publishes a GitHub
release with the changelog section as its notes. The release also carries
`install.ps1`, which the installation command in the README downloads from the
latest release; keep the zip names it expects (`caffeinate-<target>.zip`) stable.

## License

By contributing, you agree that your contributions are licensed under the
[MIT License](LICENSE).
