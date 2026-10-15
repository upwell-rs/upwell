# Conditional Component Semantics

**Status**: Implementation contract proposed by #204; descriptor and runtime integration remain in #210, #207, and #206.
**Date**: 2026-08-20

## V1 boundary

Runtime activation is limited to factory-backed singleton components. Conditions on manual
instances, runtime seeds, transient or scoped components, factories, individual provider mappings,
and protocol-captured roles are rejected until those roles have an explicit transition contract.
Already-open scopes remain pinned to their opening generation. A future root-based scope opening
uses the current generation; a child of an old parent remains on the parent's generation.

## Condition algebra

Each component has zero or one root condition. No condition means `true`. The closed expression
model is:

- a typed boolean config fact;
- equality between a typed config fact and a scalar boolean, integer, string, or enum-token literal;
- eligibility of a component by stable component ID;
- eligibility of one exact provider mapping by stable mapping ID;
- a trusted config callback over a statically declared set of typed facts;
- a trusted availability callback over statically declared component or provider-mapping inputs;
- `all`, `any`, and `not` composition.

Every expression node has an owner-scoped stable condition ID and source metadata. References use
stable IDs rather than `TypeId`, pointer identity, or vector positions. Duplicate IDs, missing
references, invalid scalar kinds, and missing config facts are validation errors, not `false`.
`all([])` is true and `any([])` is false. Evaluation visits every child in stable ID order so
short-circuiting cannot change explanation output.

Config facts are statically registered beneath a typed config binding. Conditions consume candidate
config only after source merging, defaults, placeholder resolution, deserialization, and validation.
There is no float coercion, string-to-boolean coercion, arbitrary `PartialEq`, or callback context
with undeclared framework inputs in v1. Callback descriptors are static function pointers rather
than capturing closures and follow the trusted-code contract described below.

## Eligibility and provider selection

The following states are distinct:

- A component is **eligible** when its condition evaluates to true.
- A provider mapping is **eligible** when its concrete component is eligible.
- A provider is **selected** when the existing scope-aware qualifier, primary, ordering, and
  cardinality rules select an eligible mapping for a consumer edge.
- A component is **active** when a committed runtime generation contains its instance.

Provider conditions are therefore derived from their concrete component. V1 cannot independently
condition one trait mapping from a component that provides several traits. Provider-presence
predicates mean that one exact mapping is eligible, not that it is selected or visible in every
scope. Eligibility filters the candidate set before the existing provider selection and graph
validation run.

The default/custom `Authenticator` model uses no second fallback algorithm. The unconditional
default is selected while it is the sole eligible provider. When the conditionally eligible custom
provider is also present, the existing unique-primary rule selects it.

## Determinism and cycles

Availability references form a static directed graph between condition-owning components.
Component references add an edge to that component; provider-mapping references add an edge to the
mapping's concrete component. Validation rejects every self-cycle and strongly connected component,
including positive, negated, and currently unreachable logical branches. V1 does not run a
fixed-point solver.

Cycle diagnostics start from the lowest stable component ID and traverse outgoing edges ordered by
target component ID and condition ID. They report component IDs, condition IDs, predicate kinds,
and negation polarity. Descriptor order cannot change validation, cycle witnesses, eligibility
outcomes, or condition decision reports. Existing explicit registration, lifecycle, and provider
ordering remain authoritative outside condition evaluation.

Evaluation proceeds in five phases:

1. Resolve effective static registrations and validate stable IDs, references, scalar kinds, and the
   singleton boundary.
2. Build the complete availability graph and reject cycles before consulting current fact values.
3. Evaluate components in dependency-first topological order, with stable component and condition
   IDs breaking otherwise equivalent ordering.
4. Derive provider-mapping eligibility from the completed component decisions.
5. Filter the candidate catalog, then run the existing provider selection, dependency, scope, cycle,
   and constructability validation unchanged.

Inactive components do not require their ordinary construction dependencies to be satisfiable in the
effective graph. Their static condition and factory metadata must still be structurally valid. If a
later fact change activates one, candidate graph validation must succeed before publication.

## Worked examples

### Config-gated provider fallback

`custom-authenticator` has `ConfigBool(auth.custom.enabled)`. `default-authenticator` is
unconditional. The custom mapping is the unique primary when both components are eligible. With the
fact false, only the default mapping is eligible and the existing sole-provider rule selects it. With
the fact true, the existing primary rule selects custom. A missing or non-boolean fact rejects the
candidate rather than silently selecting the default.

### Component-presence gate

`audit-exporter` has `All([ConfigBool(audit.enabled),
ComponentEligible(audit-transport)])`. The transport decision is evaluated first. The exporter is
eligible only when both decisions are true; condition evaluation does not construct either component.
If the exporter becomes eligible but has an unsatisfied ordinary dependency, downstream candidate
graph validation rejects the transition.

### Provider-mapping-presence gate

`secured-routes` may reference `ProviderMappingEligible(authenticator/custom)`. That predicate is
true whenever the custom provider's concrete component is eligible, even if a qualified consumer or
scope would select another provider. Selection and scope visibility remain downstream decisions.

### Invalid availability cycle

If `component-a` references `ComponentEligible(component-b)` and `component-b` references any
provider mapping implemented by `component-a`, the static graph contains `a -> b -> a`. Preparation
rejects the cycle with both component and condition IDs even when a config branch would currently
make one side unreachable.

## Redaction

Decision reports may contain stable IDs, config fact paths, predicate kinds, logical operators,
boolean outcomes, and dependency paths. They never contain current values, expected equality
literals, hashes of either value, arbitrary deserializer output, or panic payloads. The rule applies
to `Debug`, `Display`, errors, serialization, tooling, hooks, and tracing.

## Attempts, generations, and publication

A transition attempt ID identifies every requested attempt, including a no-op, rejection, failure,
or stale proposal. A runtime generation ID identifies only a state-changing commit. Initial runtime
state is generation zero. No-op, failed, rejected, and stale attempts do not create semantic runtime
generations. This deliberately supersedes the current config-reload counter, which remains an
attempt-local implementation detail until #206 integrates the sole transition coordinator.

One immutable runtime generation owns the committed config, condition decisions, effective graph,
singleton and provider bindings, root registry state, and future-scope plan. One atomic pointer
publishes that aggregate. A proposal records its exact base generation and commits only through a
pointer-identity compare-and-swap; a stale base cannot overwrite newer state.

A pinned runtime view loads the generation pointer once and performs every cross-handle read through
that immutable generation. Separate unpinned live-handle calls may straddle publication and therefore
do not promise aggregate consistency. Existing independent `Cfg` and `Dep` cells cannot themselves
serve as the transaction boundary.

## Feasibility result

The private executable model under `upwell-app::runtime::generation_spike` proves that:

- one pointer publication changes config, component, provider, and future-scope-plan views together;
- an old pinned view remains internally consistent after publication;
- a generation-aware live trait binding follows default/custom switching in both directions while a
  fixed `Arc` snapshot remains safe;
- unchanged immutable component instances can be shared between generations without a generation
  back-reference;
- a scope opening pins one generation across an asynchronous suspension;
- pointer-identity compare-and-swap rejects stale proposals; and
- an old generation is reclaimed after its final pin is dropped.

The model intentionally does not claim that today's `Dep<dyn Trait>`, provider erasure,
`ConfigReloader`, `AppRuntime`, or protocol captures already satisfy these properties. Production
integration and end-to-end proofs belong to later epic issues.

## Descriptor catalog result

Issue #210 adds the static vocabulary and pure registry seam needed before runtime integration:

- `ComponentDescriptor` owns an optional static root condition; independently conditional factories
  and provider mappings remain structurally unsupported in v1.
- Config fact descriptors and supplied fact snapshots are separate types. Snapshots are validated
  completely against the catalog before expression evaluation and redact their scalar values from
  debug output.
- `ConditionCatalog` validates stable IDs, references, scalar kinds, singleton/factory ownership,
  manual overrides, complete provider ordering, and every availability cycle without constructing
  or erasing components.
- Evaluation returns an explicitly eligible registry. `evaluate_validated` additionally applies the
  existing provider selection and rank-based DI graph validation; application scope topology remains
  the preparation layer's responsibility.
- Provider mapping identities are derived from the validated concrete component ID, provided trait
  type, and qualifier, independent of descriptor order.
- Condition dependencies are queryable as config, component, and exact provider-mapping facts for
  later invalidation and explanation work.
- `DependencyObservation` records fixed `Snapshot` versus generation-aware `Live` edges independently
  of eager/lazy/deferred/fresh resolution timing. `Arc`, lazy, deferred, and fresh handles are
  snapshots; `Dep` and `Cfg` are live.

This issue does not define user-facing condition macro syntax, typed config fact extraction,
condition tooling facets, protocol-role validation, graph diffs, transition strategies, or runtime
publication. Those remain owned by #208, #205, #207, #209, and #206 respectively.

Condition descriptors may also use metadata-complete config or availability callbacks. Their
contexts expose only statically declared scalar facts or eligibility inputs through the framework;
the declared inputs, not callback execution paths, remain authoritative for validation, cycle
detection, invalidation, tooling, and deterministic input ordering. Callbacks receive no resolver,
component, factory, selected-provider, filesystem, or mutation capability from Upwell. They remain
ordinary trusted Rust code, however, so implementations contractually must be deterministic,
non-blocking, panic-free, and free of external side effects. Upwell cannot sandbox direct access to
environment, filesystem, or global process state.

The DI engine emits subscriber-neutral structured tracing for debugging graph decisions. Candidate
condition node outcomes and eligibility, provider selection reasons/stages, construction-edge
inclusion or exclusion, build positions, and exact cycle members/edges use the
`upwell::di::condition`, `upwell::di::selection`, and `upwell::di::graph` targets. Detailed outcomes
are `TRACE` diagnostics, validated candidate summaries are `DEBUG`, and cycle paths are `ERROR`.
Events contain stable IDs and categories but never current config values or expected equality
literals. A deferred edge is reported as an intentional construction-cycle break; unsupported eager
cycles remain rejected and report exact cyclic members separately from components blocked behind
them.
