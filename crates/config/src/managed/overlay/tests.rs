use super::*;

fn overlaid(assignments: &[(&str, &str)]) -> ConfigValue {
    let mut overlay = ConfigOverlay::default();
    let mut root = ConfigValue::Table(Vec::new());

    for (path, literal) in assignments {
        overlay.set(path, literal).expect("valid assignment");
    }

    overlay.apply(&mut root);

    root
}

#[test]
fn literals_parse_as_typed_scalars() {
    let root = overlaid(&[
        ("a.flag", "false"),
        ("a.count", "-3"),
        ("a.ratio", "0.5"),
        ("a.word", "inf"),
        ("a.quoted", "\"true\""),
    ]);

    assert_eq!(root.get_path("a.flag"), Some(&ConfigValue::Bool(false)));
    assert_eq!(root.get_path("a.count"), Some(&ConfigValue::Int(-3)));
    assert_eq!(root.get_path("a.ratio"), Some(&ConfigValue::Float(0.5)));
    assert_eq!(
        root.get_path("a.word"),
        Some(&ConfigValue::parsed_str("inf").expect("literal string")),
        "non-numeric words stay strings"
    );
    assert_eq!(
        root.get_path("a.quoted"),
        Some(&ConfigValue::parsed_str("true").expect("literal string")),
        "quotes force a string"
    );
}

#[test]
fn later_assignments_win_and_paths_keep_assignment_order() {
    let mut overlay = ConfigOverlay::default();
    let mut root = ConfigValue::Table(Vec::new());

    overlay.set("a.b", "1").expect("first assignment");
    overlay.set("a.b", "2").expect("second assignment");
    overlay.apply(&mut root);

    assert_eq!(root.get_path("a.b"), Some(&ConfigValue::Int(2)));
    assert_eq!(overlay.paths(), ["a.b", "a.b"]);
}

#[test]
fn invalid_paths_and_templates_are_rejected_without_the_value() {
    let mut overlay = ConfigOverlay::default();

    for path in ["", "a..b", "a.b c", "a.$b"] {
        assert!(
            matches!(
                overlay.set(path, "1"),
                Err(ConfigOverlayError::InvalidPath { .. })
            ),
            "path {path:?} is rejected"
        );
    }

    let error = overlay
        .set("a.secret", "${UNTERMINATED")
        .expect_err("an unterminated placeholder is rejected");

    assert!(matches!(error, ConfigOverlayError::InvalidValue { .. }));
    assert!(
        !error.to_string().contains("UNTERMINATED"),
        "the error never echoes the value"
    );
    assert!(overlay.is_empty(), "rejected assignments record nothing");
}

#[test]
fn out_of_range_numbers_are_rejected_instead_of_converted() {
    let mut overlay = ConfigOverlay::default();

    for literal in [
        "170141183460469231731687303715884105728",
        "-170141183460469231731687303715884105729",
        "1e999",
    ] {
        assert!(
            matches!(
                overlay.set("a.n", literal),
                Err(ConfigOverlayError::OutOfRange { .. })
            ),
            "{literal} is rejected"
        );
    }

    assert!(overlay.is_empty());
}
