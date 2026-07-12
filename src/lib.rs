pub mod model;
pub mod settings;
pub mod setup;
pub mod system;
pub mod tools;
pub mod user;
pub mod workspace;

use std::collections::HashSet;
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
