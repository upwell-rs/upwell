//! The request seeders an axum application registers.

use std::any::{TypeId, type_name};
use std::collections::HashMap;

use upwell_core::TypeDescriptor;

use crate::middleware::MiddlewareApplier;
use crate::request_seed::SeedReader;

/// The registered request seeders: their layers and the readers of their seeds, one per
/// seeded type, and the seeder registered for each type.
#[derive(Default)]
pub(super) struct RequestSeeds {
    pub(super) layers: Vec<MiddlewareApplier>,
    pub(super) readers: Vec<SeedReader>,
    /// The type of each registered seed, in registration order.
    pub(super) types: Vec<TypeDescriptor>,
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
                self.types.push(TypeDescriptor::of::<T>(type_name::<T>()));

                true
            }
        }
    }
}

/// The two openers of the HTTP request scope: controller routes, which run the request seeders
/// and serve the controller entries, and every other request (one no route matches, a
/// WebSocket upgrade), whose scope opens only when something injects from it, without the
/// request seeds. The DI-backed middleware in `middleware` runs for both, so both take it as
/// roots: what it reaches over declared edges must be seeded by both openers, and what only a
/// controller handler reaches only by the first. Middleware that injects at run time without
/// a declared edge is not seen.
pub(super) fn request_openers(
    seeds: &RequestSeeds,
    controllers: &[crate::controller::ControllerDescriptor],
    middleware: &[TypeDescriptor],
) -> [upwell_app::OpenerSeeds; 2] {
    let request = <crate::scope::HttpRequest as upwell_core::Scope>::KEY;
    let meta = crate::request_meta::REQUEST_META_DESCRIPTOR.ty;
    let entries = super::entries::route_entries(controllers)
        .map(|entry| entry.key)
        .collect();

    [
        upwell_app::OpenerSeeds {
            opener: String::from(CONTROLLER_REQUESTS),
            scope: request,
            seeds: std::iter::once(meta)
                .chain(seeds.types.iter().copied())
                .collect(),
            entries: Some(entries),
            roots: middleware.to_vec(),
        },
        upwell_app::OpenerSeeds {
            opener: String::from(OTHER_REQUESTS),
            scope: request,
            seeds: vec![meta],
            entries: Some(Vec::new()),
            roots: middleware.to_vec(),
        },
    ]
}

/// The opener of the request scope of controller routes.
pub(super) const CONTROLLER_REQUESTS: &str = "axum controller routes";

/// The opener of the request scope of every other request: one no route matches, or a
/// WebSocket upgrade.
pub(super) const OTHER_REQUESTS: &str = "axum non-controller requests";
