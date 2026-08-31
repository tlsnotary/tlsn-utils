# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Return a parse error instead of panicking when a chunked HTTP body declares
  data beyond the available input or contains a non-UTF-8 trailer name.
- Accept valid UTF-8 whose multibyte code points cross HTTP chunk boundaries.

### Added

`UnboundedMemorySink` and `UnboundedMemoryStream` mirroring the `MemorySink` and `MemoryStream` wrappers of `futures::mpsc::{Sender, Receiver}` for `futures::mpsc::{UnboundedSender, UnboundedReceiver}`.
