# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- *(app)* add condition-aware candidate graph and future-scope preparation

## 0.19.0 - 2026-07-21

### Added

- *(di)* add provider ordering and deferred primitives

### Fixed

- *(di)* resolve transient dependencies from the building scope

## 0.16.0 - 2026-07-17

### Fixed

- *(runtime)* harden lifecycle, reloads, and filesystem safety ([#85](https://github.com/upwell-rs/upwell/pull/85))

## 0.12.0 - 2026-07-08

### Added

- *(app)* make init_tracing layer-extensible
- *(jobs)* implement job scheduling with interval and cron support

### Fixed

- *(review)* address PR #66 comments

### Other

- Added docs to all crates

## 0.7.0 - 2026-06-30

### Other

- Feature/protocol agnostic ([#20](https://github.com/upwell-rs/upwell/pull/20))
