use super::*;

fn key(text: &'static str) -> EntryKey {
    EntryKey::new(text).expect("a valid entry key")
}

#[test]
fn described_routes_are_the_entries_an_endpoint_serves() {
    let served = served([Some(key("ws/chat.send")), Some(key("ws/chat.join"))].into_iter());

    assert_eq!(served, Some(vec![key("ws/chat.send"), key("ws/chat.join")]));
}

#[test]
fn an_undescribed_route_makes_the_endpoint_serve_everything() {
    let served = served([Some(key("ws/chat.send")), None].into_iter());

    assert_eq!(served, None, "a message without an entry builds every slot");
}
