# Runtime Transition Hooks

A runtime transition is one config reload that may change the component graph. For example,
setting `auth.mode = "custom"` can enable an application's own `Authenticator` provider in place
of the default one. Every reload runs as one serialized transaction. That transaction stages the
new configuration and plans which components must change. It then builds a candidate generation
next to the active one, and either publishes the candidate or drops it whole.

Components and operators take part through `#[hook(Kind)]` methods: they can inspect a pending
change, veto it, observe how it ended, and learn when they have been displaced. This guide covers
the phases in order, how hooks are routed, and what each hook can see.

The snippets below are abridged from the facade test
[`tests/transition_lifecycle`](../tests/transition_lifecycle/app.rs), which compiles them through
`upwell` and switches the effective `Authenticator` provider in both directions through every
phase. Helpers such as `current_freeze` or `metrics::record_reload` stand in for application
code.

## Phases

A reload whose source changed runs these phases in order. An unchanged source is a no-op: no
hooks run, and no generation moves.

| # | Phase | Hook kind | Runs on | Can reject |
|---|---|---|---|:-:|
| 1 | Stage: re-read config, find changed bindings | – | – | ✓ (config load error) |
| 2 | Evaluate conditions, plan the transition | – | – | ✓ (condition or plan error) |
| 3 | **Proposal** | `TransitionProposal` | active generation | ✓ |
| 4 | Construct the candidate root | – (component factories) | candidate | ✓ (factory error) |
| 5 | **Config reload** | `ConfigReload` | candidate generation | ✓ |
| 6 | **Decision** | `TransitionDecision` | candidate generation | ✓ |
| 7 | Commit: publish the candidate, commit config | – | – | – |
| 8 | **Retirement** (committed only) | `ComponentRetired` | displaced generation | – |
| 9 | **Committed** / **Aborted** | `TransitionCommitted` / `TransitionAborted` | published / active generation | – |

Phases 3–7 run inside the transaction. A rejection at any of them leaves the active generation,
the config tree, its live slots, and the config generation unchanged, and drops the candidate.
Side effects outside the framework (I/O, external systems, mutations made through retained
instances) are not rolled back.

Phases 8–9 run after the transaction has released its locks, before `reload_config()` or the
trigger returns. The outcome is final by then: these hooks cannot roll anything back.

`ConfigReload` hooks run exactly once per attempt. The decision phase reads their accepted
outcomes from `PendingTransition::config_hooks()` instead of running them again.

## Hook kinds

### `TransitionProposal`: inspect before anything is built

```rust
#[methods]
impl AuthGate {
    #[hook(TransitionProposal)]
    async fn propose(&self, pending: PendingTransition) -> TransitionVerdict {
        if let Some(gate) = pending.component(AuthGate::ID) {
            tracing::info!(action = ?gate.action, reasons = ?gate.reasons, "auth gate is changing");
        }

        TransitionVerdict::Accept
    }
}
```

`PendingTransition` carries the attempt and base generation IDs, the changed binding paths, and
every affected component. For each component it gives the stable ID, the planner action (`Add`,
`Replace`, `RebindLive`, `Remove`), the selected factory, and the reason paths explaining the
action. A hook may also take `#[config("path")] next: CfgNext<T>` to read the staged typed value
of a binding it declares. The receiver (`&self`) is the **active** instance, because no candidate
exists yet. A rejection constructs nothing.

### `TransitionDecision`: veto the constructed candidate

```rust
#[methods]
impl ChangeFreeze {
    #[hook(TransitionDecision, always)]
    async fn decide(&self, pending: PendingTransition) -> TransitionVerdict {
        match current_freeze() {
            Some(reason) => TransitionVerdict::Reject(reason),
            None => TransitionVerdict::Accept,
        }
    }
}
```

Decision runs after construction and the config-reload hooks, immediately before commit. Its
receiver is the **candidate** instance. A rejection drops the whole candidate.

### `ConfigReload`: accept or reject new config values

This hook kind is unchanged. It runs against the candidate generation, between proposal and
decision. Its contract (`HookOutcome`, an `Err` aborting the reload, routing by changed binding
paths) is documented on `ConfigReload` in
[`upwell-config`](../crates/config/src/managed/reload.rs).

### `ComponentRetired`: learn you have been displaced

```rust
#[methods]
impl CustomAuthenticator {
    #[hook(ComponentRetired)]
    async fn retired(&self, retirement: ComponentRetirement) {
        self.stop_accepting_new_sessions();
    }
}
```

After a commit, every singleton the commit **replaced or removed** receives `ComponentRetired`
once. Its `&self` is the displaced instance itself, resolved through the old generation's root,
never its replacement. `ComponentRetirement` names the component and gives its
`RetirementReason` (`Replaced` or `Removed`), the attempt, and both generation IDs. Retained,
rebound, and newly added components never retire.

Retirement is logical displacement, not destruction. Pinned `RuntimeView`s, open scopes, and other
holders may keep using the instance after retirement, until they release it. A retirement hook
should therefore stop taking new work or schedule cleanup. It must not invalidate state that a
pinned caller may still read. Components retire in `RuntimeTransitionReport::displaced()` order:
a consumer retires before the components it holds a fixed dependency on. Observing when the old
generation is finally dropped is tracked in #311.

### `TransitionCommitted` / `TransitionAborted`: observe the outcome

```rust
#[methods]
impl TransitionAudit {
    #[hook(TransitionCommitted)]
    async fn committed(&self, report: RuntimeTransitionReport) {
        metrics::record_reload(report.attempt(), report.elapsed(), report.displaced().len());
    }

    #[hook(TransitionAborted)]
    async fn aborted(&self, report: RuntimeTransitionReport) {
        tracing::warn!(outcome = ?report.outcome(), "reload rejected");
    }
}
```

Each committed or rejected attempt fires exactly one of the two, with its final report. Committed
observers run on the **published** generation, after every retirement. Aborted observers run on
the generation that stayed active. No-op attempts, cancelled attempts (whose future was dropped),
and reloads rejected as reentrant fire neither.

## Roles and routing

- **Participants** (`#[hook(Kind)]` on `TransitionProposal`/`TransitionDecision`) return a
  `TransitionVerdict`. They run sequentially in registration order, and the first `Reject`,
  error, or panic ends the attempt.
- **Observers** (`#[hook(Kind, observe)]`) return `()` or `Result<(), E>`. They run concurrently,
  and only once every participant has accepted. Their failures are recorded in the report and
  traced, and never reject. Every `TransitionCommitted`, `TransitionAborted` and
  `ComponentRetired` hook is an observer.
- **Targeted** (the default): proposal hooks run only on affected components present in the
  active generation (replaced, rebound, or removed). Decision hooks run only on affected
  components present in the candidate (added, replaced, or rebound). A retirement hook runs only
  for its own component's retirement.
- **`always`** (`#[hook(Kind, always)]`): the hook runs on every transition that reaches its phase,
  whether or not the plan touches its component. Use it for operator policies and audits. An
  `always` retirement hook observes every component's retirement.
- Committed and aborted observers run on every attempt that reaches them; targeting does not
  apply.

Panics in any hook are caught and reported without their payload.

## Reports and redaction

Every attempt that acquires the transition writer produces one `RuntimeTransitionReport`. A commit
or no-op returns it from `reload_config()`. A rejection carries it inside
`AppError::TransitionRejected`. The report holds:

- the attempt and generation IDs, and the reload `cause()`;
- the terminal outcome (rejecting phase and failure category);
- changed binding paths and type names, and the affected components with actions and reasons;
- the displaced components, config-reload hook outcomes, and transition hook outcomes;
- per-phase timings.

Reports and hook contexts never contain current or proposed config values, deserializer output,
error messages, or panic payloads. That applies to `Debug`, `Display`, serialized output (with
the `serde` feature), and the structured `upwell::transition` tracing events. The only free text
they carry is chosen by hook authors as `&'static str`: the reason a participant passes to
`TransitionVerdict::Reject`, and the reason a config-reload hook passes to
`HookOutcome::RestartRequired`. The only way to read
staged values is `CfgNext<T>`, and only for a binding the hook declares. The unredacted error
behind a rejection is reachable only through `TransitionRejection::error()` and the error source
chain.

## Reentrancy

Proposal, config-reload and decision hooks, and candidate factories, run while the reload holds
its locks. Calling `reload_config()` or `RuntimeReloader::reload()` for the same runtime from
inside them returns `AppError::ReentrantReload` immediately, and the running attempt is unaffected.
Retirement and outcome observers run after the locks are released, so they may request a
follow-up reload. That reload runs as a new attempt.

## Trigger sources

Each report records what requested it as a `ReloadCause`:

| Cause | Source |
|---|---|
| `Requested` | `AppRuntime::reload_config()` or the injectable `RuntimeReloader` |
| `Signal` | `SIGHUP` |
| `FileWatch` | a change to a watched config file |

All three run the same transaction and fire the same hooks.

## Lifecycle hooks

`Startup` gates run before the application serves, and reload triggers start only afterwards.
Reload triggers stop before `Shutdown` observers run. Triggered transitions therefore happen
strictly between the two. A direct `reload_config()` call runs whenever it is made.

`Startup` is a one-time process event: it fires once, on the generation built at startup.
Components a later reload adds do not receive it; use their factory or `#[init]` instead.
`Shutdown` fires on the generation that is current when shutdown begins, so the live instances
receive it rather than instances a reload has already retired. Reload triggers are stopped by
then, but a direct `reload_config()` from another task is not coordinated with shutdown: if it
commits while `Shutdown` observers run, they still address the generation pinned when shutdown
began. Targeted `Shutdown` hooks pair with startup by component type: a component whose
`Startup` gate failed or never ran (because a reload added it) is skipped, while one a reload
replaced still shuts down through its replacement. `#[hook(Shutdown, always)]` hooks run
regardless.
