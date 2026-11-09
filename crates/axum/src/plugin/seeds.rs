//! The request seeders an axum application registers.

use std::any::{TypeId, type_name};
use std::collections::HashMap;

use crate::middleware::MiddlewareApplier;
use crate::request_seed::SeedReader;

/// The registered request seeders: their layers and the readers of their seeds, one per
/// seeded type, and the seeder registered for each type.
#[derive(Default)]
pub(super) struct RequestSeeds {
    pub(super) layers: Vec<MiddlewareApplier>,
    pub(super) readers: Vec<SeedReader>,
    seeders: HashMap<TypeId, RegisteredSeeder>,
    pub(super) conflict: Option<crate::Error>,
}

/// The seeder registered for one seeded type.
struct RegisteredSeeder {
    seeder: TypeId,
    name: &'static str,
}

impl RequestSeeds {
    /// Records the seeder `M` of `T`, returning whether its layer and reader must be added: not
    /// when `M` is already registered, and not when another seeder of `T` is, which fails
    /// preparation instead.
    pub(super) fn register<T: 'static, M: 'static>(&mut self) -> bool {
        let seeder = TypeId::of::<M>();
        let name = type_name::<M>();

        match self.seeders.get(&TypeId::of::<T>()) {
            Some(registered) if registered.seeder == seeder => false,

            Some(registered) => {
                self.conflict
                    .get_or_insert(crate::Error::DuplicateRequestSeeder {
                        seed: type_name::<T>(),
                        first: registered.name,
                        second: name,
                    });

                false
            }

            None => {
                self.seeders
                    .insert(TypeId::of::<T>(), RegisteredSeeder { seeder, name });

                true
            }
        }
    }
}
