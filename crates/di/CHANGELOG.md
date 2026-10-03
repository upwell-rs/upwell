# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- *(di)* add immutable conditional graph transition planning and stable factory recipe IDs

## 0.20.0 - 2026-07-22

### Added

- *(di)* dual linkme/inventory registration backend for factories & hooks

## 0.19.0 - 2026-07-21

### Added

- *(di)* add provider ordering and deferred primitives

### Fixed

- *(di)* group deferred candidates by scope identity, not rank
- *(di)* walk deferred candidates in scope-chain order
- *(di)* make deferred candidate selection scope-aware
- *(di)* reject ambiguous deferred candidates
- *(di)* distinguish qualified no-match from ambiguity
- *(di)* wait for complete local provider sets before fallback
- *(di)* select scope-local providers in build ordering
- *(di)* select providers before computing build-order waits
- *(di)* resolve transient dependencies from the building scope
- *(di)* resolve review findings in provider primitives
- *(di)* hydrate deferred dependencies after build

## 0.17.2 - 2026-07-18

### Other

- *(di)* isolate memory contract measurements

## 0.17.1 - 2026-07-17

### Other

- *(benchmarks)* address automated review findings
- *(benchmarks)* expand suite across DI, config, RPC, serde, and WS

## 0.16.0 - 2026-07-17

### Fixed

- *(runtime)* harden lifecycle, reloads, and filesystem safety ([#85](https://github.com/upwell-rs/upwell/pull/85))

## 0.14.2 - 2026-07-17

### Other

- *(di)* harden hot paths and add controlled benchmarks

## 0.12.0 - 2026-07-08

### Added

- *(jobs)* implement job scheduling with interval and cron support

### Other

- Added docs to all crates

## 0.7.0 - 2026-06-30

### Other

- Feature/protocol agnostic ([#20](https://github.com/upwell-rs/upwell/pull/20))
