# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
The project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0]

### Added

`icalkit` provides strict parsing, lossless import, explicit normalization, and transactional editing for iCalendar data.
The recurrence API expands occurrences within a time window from the caller.
Calendar-aware operations report ambiguous and nonexistent local times.
The scheduling API handles inbound and outbound iTIP messages.
The CalDAV API provides client and server operations that callers integrate with their HTTP transport and storage.

`icalkit` requires Rust 1.88 or later.
Use `--no-default-features` for `no_std` builds with `alloc`.

## Development history

See the [pre-release development history](https://github.com/P4suta/icalkit/blob/b34be44c8ad5050c4e07d3fd3e4a1f8bb9bba7f7/CHANGELOG.md#unreleased).
