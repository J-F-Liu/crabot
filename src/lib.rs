pub mod chat;
pub mod model;
pub mod model_database;
pub mod session;
pub mod settings;
pub mod setup;
pub mod system;
pub mod tools;
pub mod user;
pub mod workspace;

use std::collections::HashSet;

pub fn app_title() -> &'static str {
    concat!("Crabot v", env!("CARGO_PKG_VERSION"))
}
use std::hash::Hash;

pub trait HashSetExt<T> {
    fn set(&mut self, value: T, enabled: bool);
}

impl<T: Eq + Hash> HashSetExt<T> for HashSet<T> {
    fn set(&mut self, value: T, enabled: bool) {
        if enabled {
            self.insert(value);
        } else {
            self.remove(&value);
        }
    }
}
