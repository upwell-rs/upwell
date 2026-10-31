critical_filter := '''
  binary_id(=upwell)
  | binary_id(=cargo-upwell)
  | binary_id(=cargo-upwell::command)
  | binary_id(=cargo-upwell::completions)
  | binary_id(=cargo-upwell::init)
  | binary_id(=cargo-upwell::probe)
  | binary_id(=cargo-upwell::bin/cargo-upwell)
  | binary_id(=upwell-app)
  | binary_id(=upwell-app::third_party_plugin)
  | binary_id(=upwell-app::third_party_protocol)
  | binary_id(=upwell-axum)
  | binary_id(=upwell-axum-json-ws)
  | binary_id(=upwell-axum-macros)
  | binary_id(=upwell-axum-stomp)
  | binary_id(=upwell-client)
  | binary_id(=upwell-config)
  | binary_id(=upwell-core)
  | binary_id(=upwell-di)
  | binary_id(=upwell-dirs)
  | binary_id(=upwell-hooks)
  | binary_id(=upwell-jobs)
  | binary_id(=upwell-macros-core)
  | binary_id(=upwell-rpc)
  | binary_id(=upwell-transport)
  | binary_id(=upwell-tooling-schema)
  | binary_id(=upwell-test-utils::process)
  | binary_id(=upwell-test-utils::temp)
  | binary_id(=upwell::app_commands)
  | binary_id(=upwell::app_definition)
  | binary_id(=upwell::axum_middleware)
  | binary_id(=upwell::builtins)
  | binary_id(=upwell::by_value)
  | binary_id(=upwell::client)
  | binary_id(=upwell::config_macro)
  | binary_id(=upwell::config_reload)
  | binary_id(=upwell::config_reload_hardening)
  | binary_id(=upwell::dep_swap)
  | binary_id(=upwell::dependency_injection)
  | binary_id(=upwell::factory)
  | binary_id(=upwell::hook_abort)
  | binary_id(=upwell::hook_custom)
  | binary_id(=upwell::hook_lifecycle)
  | binary_id(=upwell::hooks)
  | binary_id(=upwell::middleware)
  | binary_id(=upwell::plugin_composition)
  | binary_id(=upwell::provider_primitives)
  | binary_id(=upwell::providers)
  | binary_id(=upwell::rpc_hardening)
  | binary_id(=upwell::scope_local_providers)
  | binary_id(=upwell::scopes)
  | binary_id(=upwell::status_codes)
  | binary_id(=upwell::streaming)
  | binary_id(=upwell::third_party_protocol)
  | binary_id(=upwell::tooling_probe_process)
  | binary_id(=upwell::type_registration)
  | binary_id(=upwell-config::namespace_and_defaults)
  | binary_id(=upwell-config::substitution)
  | binary_id(=upwell-core::resolver_set_performance)
  | binary_id(=upwell-di::memory_contracts)
  | binary_id(=upwell-example-http)
  | binary_id(=upwell-example-http::client)
  | binary_id(=upwell-example-http::extractors)
  | binary_id(=upwell-example-http::openapi)
  | binary_id(=upwell-example-http::routes)
  | binary_id(=upwell-example-http::stomp)
  | binary_id(=upwell-example-http::topic_generics)
  | binary_id(=upwell-example-http::ws)
  | binary_id(=upwell-example-daemon::bin/upwell-example-daemon)
'''

test:
    just test-critical
    just test-extended
    just test-ui
    just test-doc

test-critical:
    #!/usr/bin/env bash
    set -euo pipefail

    nextest_version="$(cargo nextest --version)"
    [[ "$nextest_version" == "cargo-nextest 0.9.140 "* ]]
    cargo nextest run \
        --workspace \
        --all-features \
        --locked \
        --profile ci-critical \
        --filterset '{{ critical_filter }}'

test-extended:
    #!/usr/bin/env bash
    set -euo pipefail

    nextest_version="$(cargo nextest --version)"
    [[ "$nextest_version" == "cargo-nextest 0.9.140 "* ]]
    cargo nextest run \
        --workspace \
        --all-features \
        --locked \
        --profile ci-extended \
        --filterset 'all() - ({{ critical_filter }})'

test-ui:
    cargo nextest run -p upwell --all-features --locked --test ui

test-doc:
    cargo test --doc --workspace --all-features --locked

test-config:
    #!/usr/bin/env bash
    set -euo pipefail

    nextest_version="$(cargo nextest --version)"
    [[ "$nextest_version" == "cargo-nextest 0.9.140 "* ]]
    cargo nextest show-config version --profile ci-critical
    cargo nextest show-config version --profile ci-extended
    cargo nextest show-config test-groups \
        --workspace \
        --all-features \
        --locked \
        --profile ci-critical >/dev/null
    cargo nextest show-config test-groups \
        --workspace \
        --all-features \
        --locked \
        --profile ci-extended >/dev/null

test-archive archive:
    #!/usr/bin/env bash
    set -euo pipefail

    nextest_version="$(cargo nextest --version)"
    [[ "$nextest_version" == "cargo-nextest 0.9.140 "* ]]
    cargo nextest archive \
        --workspace \
        --all-features \
        --locked \
        --archive-file '{{ archive }}'

test-critical-archive archive:
    just _ci-test-critical '{{ archive }}' "$PWD"

test-extended-archive archive:
    just _ci-test-extended '{{ archive }}' "$PWD"

_ci-test-critical archive workspace:
    #!/usr/bin/env bash
    set -euo pipefail

    nextest_version="$(cargo nextest --version)"
    [[ "$nextest_version" == "cargo-nextest 0.9.140 "* ]]
    cargo nextest run \
        --archive-file '{{ archive }}' \
        --workspace-remap '{{ workspace }}' \
        --profile ci-critical \
        --filterset '{{ critical_filter }}'

_ci-test-extended archive workspace:
    #!/usr/bin/env bash
    set -euo pipefail

    nextest_version="$(cargo nextest --version)"
    [[ "$nextest_version" == "cargo-nextest 0.9.140 "* ]]
    cargo nextest run \
        --archive-file '{{ archive }}' \
        --workspace-remap '{{ workspace }}' \
        --profile ci-extended \
        --filterset 'all() - ({{ critical_filter }})'

# Criterion keeps reports and saved baselines in `benchmarks/criterion` (override with
# `CRITERION_HOME`), outside `target/`, so `cargo clean` keeps the baselines. Arguments after

# `--` go to Criterion: `just bench http_upwell -- --save-baseline before`.
bench *args:
    #!/usr/bin/env bash
    set -euo pipefail

    export CRITERION_HOME="${CRITERION_HOME:-{{ justfile_directory() }}/benchmarks/criterion}"
    targets=()
    criterion=()
    forwarding=false

    for arg in {{ args }}; do
        if [[ "$forwarding" == true ]]; then
            criterion+=("$arg")
        elif [[ "$arg" == "--" ]]; then
            forwarding=true
        else
            targets+=(--bench "$arg")
        fi
    done

    cargo bench --manifest-path benchmarks/Cargo.toml --locked "${targets[@]}" -- "${criterion[@]}"

bench-lock:
    cargo metadata --manifest-path benchmarks/Cargo.toml --format-version 1 > /dev/null

test-templates:
    cargo nextest run -p cargo-upwell --test init --run-ignored only --no-tests=fail --locked
