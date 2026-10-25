use super::*;

const OWNER: &str = "users";

#[test]
fn entry_keys_join_constant_parts() {
    const KEY: EntryKey = crate::entry_key!("http/", OWNER, ".", "list");

    assert_eq!(KEY.as_str(), "http/users.list");
    assert_eq!(KEY.namespace(), "http");
    assert_eq!(KEY.local_path(), "users.list");
}
