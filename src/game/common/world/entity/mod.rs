use specs::{Component, storage::BTreeStorage};
use serde::{Serialize, Deserialize};

mod player;
pub use player::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameEntity;

impl Component for GameEntity {
    type Storage = BTreeStorage<Self>;
}