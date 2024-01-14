use std::path::Path;

use nbt::{Blob, Value};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::utils::base36_to_base10;

use super;

pub fn load_entire_world(world_path: impl AsRef<Path>) -> World {
    open(world_path);
}
