# Changelog

All notable changes to roadcase are recorded here. The four crates (`roadcase`,
`roadcase-bundle`, `roadcase-update`, `roadcase-repo`) share one version.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and roadcase
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Entries are written by
the release pull request from the [conventional commits](https://www.conventionalcommits.org)
since the last release.

## [Unreleased]

## [0.1.2](https://github.com/transcodeworks/roadcase/compare/v0.1.1...v0.1.2) - 2026-11-18

### Fixed

- *(macos)* size disk images explicitly with copy headroom ([#7](https://github.com/transcodeworks/roadcase/pull/7))

### Fixed

- Size macOS disk images explicitly with headroom so `hdiutil` does not run out of space while copying an application.

## [0.1.1](https://github.com/transcodeworks/roadcase/compare/v0.1.0...v0.1.1) - 2026-11-17

### Added

- *(cli)* roadcase release takes the system it runs on when none is named

## [0.1.0](https://github.com/transcodeworks/roadcase/releases/tag/v0.1.0) - 2026-11-17

### Added

- roadcase, packaging, signing and updates for desktop applications
