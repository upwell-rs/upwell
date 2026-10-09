/// Stable identity of one committed application runtime generation.
///
/// Transition attempts and uncommitted candidates do not allocate generation IDs.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeGenerationId(u64);

impl RuntimeGenerationId {
    /// The initial runtime generation.
    pub const INITIAL: Self = Self(0);

    /// Creates an identity from its monotonic sequence value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the monotonic sequence value.
    pub const fn get(self) -> u64 {
        self.0
    }
}
