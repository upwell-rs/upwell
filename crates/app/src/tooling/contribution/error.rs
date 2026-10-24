/// A structural error in one owner-scoped tooling contribution set.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ToolingContributionError {
    /// A resource, facet, or relationship endpoint has no owner-local identity.
    #[error("tooling contribution contains an empty owner-local identity")]
    EmptyId,
    /// An extension attempted to use a framework/core identity as an owner-local identity.
    #[error("tooling contribution identity '{id}' is reserved by the framework")]
    ReservedId {
        /// Invalid identity.
        id: String,
    },
    /// The collector owner is not a protocol or plugin resource identity.
    #[error("tooling contribution owner '{id}' is not a protocol or plugin identity")]
    InvalidOwner {
        /// Invalid owner identity.
        id: String,
    },
    /// A structurally valid protocol or plugin owner was not projected.
    #[error("tooling contribution owner '{id}' does not exist in the prepared projection")]
    UnknownOwner {
        /// Missing owner identity.
        id: String,
    },
    /// A contributed resource has no display name.
    #[error("tooling resource '{id}' has no name")]
    EmptyResourceName {
        /// Owner-local resource identity.
        id: String,
    },
    /// A contributed facet has no positive schema version.
    #[error("tooling facet '{id}' has schema version zero")]
    InvalidFacetVersion {
        /// Owner-local facet identity.
        id: String,
    },
    /// One collector declared the same owner-local resource more than once.
    #[error("tooling resource '{id}' is declared more than once")]
    DuplicateResource {
        /// Duplicated owner-local identity.
        id: String,
    },
    /// One collector declared the same fully qualified facet more than once.
    #[error("tooling facet '{id}' is declared more than once")]
    DuplicateFacet {
        /// Duplicated fully qualified identity.
        id: String,
    },
    /// One collector declared the same relationship more than once.
    #[error("tooling relationship from '{from}' to '{to}' is declared more than once")]
    DuplicateRelationship {
        /// Qualified source identity.
        from: String,
        /// Qualified target identity.
        to: String,
    },
    /// A relationship or resource facet refers to an undeclared owner-local resource.
    #[error("tooling contribution refers to undeclared owner-local resource '{id}'")]
    UnknownEndpoint {
        /// Missing owner-local identity.
        id: String,
    },
    /// A supported extension relationship uses endpoints incompatible with its semantics.
    #[error("tooling contribution relationship has invalid owner-local endpoints")]
    InvalidRelationshipEndpoints,
    /// A protocol did not provide its required declarative presentation.
    #[error("protocol '{id}' did not provide tooling display metadata")]
    MissingProtocolDisplay {
        /// Protocol resource identity.
        id: String,
    },
    /// One collector declared presentation for the same owner-local resource more than once.
    #[error("tooling resource '{id}' has more than one display declaration")]
    DuplicateDisplay {
        /// Duplicated owner-local identity.
        id: String,
    },
    /// One collector declared the source of the same owner-local resource more than once.
    #[error("tooling resource '{id}' has more than one source declaration")]
    DuplicateSource {
        /// Duplicated owner-local identity.
        id: String,
    },
}
