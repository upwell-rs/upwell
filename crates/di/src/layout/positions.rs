//! Where an opening stores the slots it fills in place, so the block an open scope allocates
//! is sized by what its opening fills, not by every slot of its layout.

use super::{SlotIdx, SlotLayout};

/// The place, in an open scope's in-place block, of each slot its opening fills: the slots its
/// plan builds and the scope's seed slots. A slot without a place is built on demand into its
/// own cell. Planned once per entry plan and shared by every instance opened with it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SlotPositions {
    places: Box<[Option<u32>]>,
    len: usize,
}

impl SlotPositions {
    /// The places of the slots `builds` lists and the seed slots of `layout`, in slot order.
    pub(crate) fn of(layout: &SlotLayout, builds: &[bool]) -> Self {
        let mut len = 0;
        let places = layout
            .slots()
            .iter()
            .zip(builds)
            .map(|(spec, builds)| {
                let filled = *builds || spec.factory.is_none();
                let place = filled.then(|| u32::try_from(len).expect("a layout has few slots"));

                len += usize::from(filled);

                place
            })
            .collect();

        Self { places, len }
    }

    /// The number of places.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// The place of `slot`, if its opening fills it in place.
    pub(crate) fn place(&self, slot: SlotIdx) -> Option<usize> {
        self.places[slot.index()].map(|place| place as usize)
    }
}
