# upwell-example-conditional

A runnable worked example of conditional components and runtime transitions in
[Upwell](../../README.md).

## Role

An injectable `Authenticator` has an unconditional `DefaultAuthenticator` and a
`CustomAuthenticator` enabled by `#[when(AuthConfig.mode == AuthMode::Custom)]`. The example flips
`auth.mode` at runtime and shows how each consumer crosses the switch without a restart:

| Component | Holds | On a provider switch |
|---|---|---|
| `LoginGate` | `Dep<dyn Authenticator>` | keeps its instance; the live handle is rebound in place |
| `AuthBanner` | `Arc<dyn Authenticator>` | reconstructed by its ordinary factory |
| `SessionLedger` | `Arc<dyn Authenticator>` + open sessions | replaced by its `#[replace]` recipe, carrying the sessions |
| `TransitionAudit` | nothing the switch affects | retained; its hooks inspect the proposal and the outcome |

A switch whose new `auth.max_sessions` cannot hold the open sessions makes the replacement fail,
and the whole transition rolls back: every instance and the active generation stay as they were.

## Usage

```sh
cargo run -p upwell-example-conditional
```

It switches to the custom provider, attempts a switch back that rolls back, switches back, and
prints each step and the audit trail. `tests/scenario.rs` asserts the same scenario.

## Internal role

Not published. CI builds and runs it in the `Examples` job; its test runs in the extended nextest
cohort. It depends only on the `upwell` facade.

## Feature flags

None. It uses the facade's default features.
