use upwell_tooling_schema::ConfigAssignment;

/// Parses one `PATH=VALUE` config assignment, splitting at the first `=`.
///
/// The path must be dot-separated `[A-Za-z0-9_-]` segments; the value is passed through
/// verbatim and interpreted by the probed application.
pub(super) fn parse_assignment(argument: &str) -> Result<ConfigAssignment, String> {
    let Some((path, value)) = argument.split_once('=') else {
        return Err(String::from("expected PATH=VALUE"));
    };

    let valid = !path.is_empty()
        && path.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        });

    if !valid {
        return Err(format!(
            "invalid config path `{path}`: expected dot-separated [A-Za-z0-9_-] segments"
        ));
    }

    Ok(ConfigAssignment {
        path: path.to_string(),
        value: value.to_string(),
    })
}
