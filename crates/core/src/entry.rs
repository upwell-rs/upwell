//! Stable entry-point identities.

crate::namespaced_id_type!(
    /// A stable namespaced identity of one entry point.
    ///
    /// An entry is a unit of work that runs inside a scope: an HTTP route, an RPC method,
    /// a WebSocket handler, a plugin entry, or an entry into a user-defined scope. Tooling
    /// and diagnostics key on this identity; the prepared application maps it to a dense
    /// process-local index.
    pub struct EntryKey,
    "entry"
);

/// The segment an entry owner (a service or controller) contributes to its entries' keys:
/// its stable ID normalized to `snake_case`. Emitted by the owner's macro, read by the macros
/// that register its entries.
#[doc(hidden)]
pub trait EntryKeyOwner {
    /// The owner's key segment.
    const KEY_SEGMENT: &'static str;
}

/// Builds an [`EntryKey`] at compile time by joining string constants, so a macro can derive
/// an entry's key from its owner's key segment (`"http/", <owner segment>, ".", "handler"`).
///
/// Every part must be a constant expression. Compilation fails when the joined text is not a
/// valid namespaced identifier.
#[doc(hidden)]
#[macro_export]
macro_rules! entry_key {
    ($($part:expr),+ $(,)?) => {{
        const PARTS: &[&str] = &[$($part),+];
        const LEN: usize = $crate::entry::joined_len(PARTS);
        const BYTES: [u8; LEN] = $crate::entry::join::<LEN>(PARTS);
        const KEY: $crate::EntryKey = $crate::entry::key_from_bytes(&BYTES);

        KEY
    }};
}

/// The byte length of `parts` joined, for [`entry_key!`].
#[doc(hidden)]
pub const fn joined_len(parts: &[&str]) -> usize {
    let mut len = 0;
    let mut index = 0;

    while index < parts.len() {
        len += parts[index].len();
        index += 1;
    }

    len
}

/// The bytes of `parts` joined, for [`entry_key!`]. `N` must be [`joined_len`] of `parts`.
#[doc(hidden)]
pub const fn join<const N: usize>(parts: &[&str]) -> [u8; N] {
    let mut bytes = [0; N];
    let mut offset = 0;
    let mut index = 0;

    while index < parts.len() {
        let part = parts[index].as_bytes();
        let mut byte = 0;

        while byte < part.len() {
            bytes[offset] = part[byte];
            offset += 1;
            byte += 1;
        }

        index += 1;
    }

    bytes
}

/// Validates joined key bytes, for [`entry_key!`].
///
/// A valid key is ASCII, so non-ASCII bytes are rejected before the bytes are read as text.
#[doc(hidden)]
pub const fn key_from_bytes(bytes: &'static [u8]) -> EntryKey {
    if !bytes.is_ascii() {
        panic!(
            "an entry key joined from constant parts is not a valid namespaced identifier; \
             each segment must use lowercase ASCII letters, digits, '.', '_' or '-'"
        );
    }

    // SAFETY: the bytes were just checked to be ASCII, which is always valid UTF-8.
    let value = unsafe { core::str::from_utf8_unchecked(bytes) };

    match EntryKey::new(value) {
        Ok(key) => key,
        Err(_) => panic!(
            "an entry key joined from constant parts is not a valid namespaced identifier; \
             each segment must use lowercase ASCII letters, digits, '.', '_' or '-'"
        ),
    }
}

#[cfg(test)]
mod tests;
