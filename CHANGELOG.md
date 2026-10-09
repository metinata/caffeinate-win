# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-10-09

### Added

- `caffeinate` command for Windows with the macOS options `-d`, `-i`, `-m`, `-s`,
  `-u`, `-t` and `-w`, plus `-h`/`--help` and `-V`/`--version`. As on macOS, `-u`
  turns the display on and lasts 5 seconds without `-t`, and `-t` counts only
  time the system is awake.
- Running a utility while the system is kept awake, with `cmd.exe`-style
  `PATHEXT` lookup (so `npm` finds `npm.cmd` and `.\gradlew` finds `gradlew.bat`).
  Arguments are passed on exactly as written, and the exit code is passed back;
  `126` and `127` report a utility that could not be started or was not found.
- Windows PowerShell started from PowerShell 7 gets the module path PowerShell 7
  would give it, so it does not try to load PowerShell 7's modules.
- Ctrl+C goes to a console utility, which decides whether to exit; with a GUI
  utility, it ends caffeinate and leaves the program running.
- If caffeinate is killed or its window is closed, the utility ends too.
- The command line is shown as the reason in `powercfg /requests`.
- Version information embedded in `caffeinate.exe`, which needs no Visual C++
  runtime.
- Release builds for x86_64 and ARM64 with checksums and build provenance
  attestations, and a PowerShell installer that sets caffeinate up for the
  current user.

[Unreleased]: https://github.com/metinata/caffeinate-win/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/metinata/caffeinate-win/releases/tag/v0.1.0
