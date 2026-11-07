use upwell_core::EntryKey;

use super::EntryDescriptor;
use super::validate::{EntryError, validate_keys, validate_scopes};
use crate::scope::PreparedScopeTopology;

/// A dense, process-local index of one prepared entry.
///
/// Stable only within one prepared application. Tooling and diagnostics use the entry's
/// [`EntryKey`] instead.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EntryId(u32);

impl EntryId {
    /// The entry's position in its [`EntryTable`].
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// The validated entry points of a prepared application, in stable key order.
///
/// [`EntryId`]s index this table densely. The table is fixed for the application's
/// lifetime; every candidate runtime generation revalidates the entries' targets against
/// its own graph.
#[derive(Clone, Debug, Default)]
pub struct EntryTable {
    entries: Box<[EntryDescriptor]>,
}

/// `entries` in the order an [`EntryTable`] holds them, so a position is the
/// [`EntryId::index`] of the entry there.
pub(crate) fn in_table_order(
    entries: &[EntryDescriptor],
) -> impl Iterator<Item = &EntryDescriptor> {
    let mut sorted: Vec<&EntryDescriptor> = entries.iter().collect();

    sorted.sort_by_key(|entry| entry.key);

    sorted.into_iter()
}

impl EntryTable {
    pub(crate) fn new(
        entries: &[EntryDescriptor],
        topology: &PreparedScopeTopology,
    ) -> Result<Self, EntryError> {
        if u32::try_from(entries.len()).is_err() {
            return Err(EntryError::TooManyEntries {
                count: entries.len(),
            });
        }

        let entries: Vec<EntryDescriptor> = in_table_order(entries).copied().collect();

        validate_keys(&entries)?;
        validate_scopes(&entries, topology)?;

        Ok(Self {
            entries: entries.into_boxed_slice(),
        })
    }

    /// Returns every entry in stable key order, so a position is its [`EntryId::index`].
    pub fn entries(&self) -> &[EntryDescriptor] {
        &self.entries
    }

    /// Returns the number of prepared entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the application registered no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the entry with dense identity `id`.
    pub fn get(&self, id: EntryId) -> Option<&EntryDescriptor> {
        self.entries.get(id.index())
    }

    /// Returns the dense identity of the entry with stable identity `key`.
    pub fn id(&self, key: &EntryKey) -> Option<EntryId> {
        self.entries
            .binary_search_by_key(key, |entry| entry.key)
            .ok()
            .map(Self::id_at)
    }

    /// Returns every entry with its dense identity, in stable key order.
    pub fn iter(&self) -> impl Iterator<Item = (EntryId, &EntryDescriptor)> {
        self.entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (Self::id_at(index), entry))
    }

    fn id_at(index: usize) -> EntryId {
        let index = u32::try_from(index).expect("entry tables hold at most u32::MAX entries");

        EntryId(index)
    }
}
