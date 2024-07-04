# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
### Security

- Changed `async-tungstenite` version from "0.16" to "0.25.1" to address vulnerability [RUSTSEC-2023-0065](https://rustsec.org/advisories/RUSTSEC-2023-0065) affecting `tungstenite` versions until "0.20".