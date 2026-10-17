//! The public plugin and protocol contracts declare their members in canonical order.

fn assert_source_order(source: &str, markers: &[&str]) {
    let mut remainder = source;

    for marker in markers {
        let position = remainder
            .find(marker)
            .unwrap_or_else(|| panic!("missing source marker `{marker}`"));

        remainder = &remainder[position + marker.len()..];
    }
}

fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let source = source
        .split_once(start)
        .unwrap_or_else(|| panic!("missing source boundary `{start}`"))
        .1;
    source
        .split_once(end)
        .unwrap_or_else(|| panic!("missing source boundary `{end}`"))
        .0
}

#[test]
fn public_contract_members_follow_canonical_order() {
    let protocol_source = include_str!("../../protocol.rs");
    let plugin_source = include_str!("../../plugin.rs");
    let definition = source_between(
        protocol_source,
        "pub trait ProtocolDefinition",
        "pub trait PreparedProtocol",
    );
    let prepared = source_between(
        protocol_source,
        "pub trait PreparedProtocol",
        "pub struct PreBuildContext",
    );
    let plugin = source_between(
        plugin_source,
        "pub trait Plugin:",
        "pub trait PluginWithOptions",
    );

    assert_source_order(
        definition,
        &[
            "type Prepared:",
            "type Error:",
            "const ID:",
            "const SCOPE_TOPOLOGY:",
            "fn register(",
            "fn prepare(",
            "fn auto_discover(",
            "fn register_plugins(",
            "fn pre_build(",
        ],
    );
    assert_source_order(
        prepared,
        &["type Runtime:", "type Error:", "fn build(", "fn tooling("],
    );
    assert_source_order(
        plugin,
        &[
            "const ID:",
            "const RELATIONS:",
            "fn contribute(",
            "fn auto_discover(",
            "fn cli(",
        ],
    );
}
