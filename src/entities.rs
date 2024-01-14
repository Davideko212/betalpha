use tokio::net::TcpStream;

use crate::{
    packet::{self, util::SendPacket},
    utils::look_to_i8_range,
    PositionAndLook,
};

#[derive(Debug, Clone)]
pub enum Type {
    Player(String),
    //Item(i16, i8),
}

pub async fn spawned_named_entity(
    stream: &mut TcpStream,
    eid: i32,
    name: &str,
    pos_and_look: &PositionAndLook,
) -> Result<(), packet::PacketError> {
    let x = (pos_and_look.x * 32.).round() as i32;
    let y = (pos_and_look.y * 32.).round() as i32;
    let z = (pos_and_look.z * 32.).round() as i32;

    let (rotation, pitch) = look_to_i8_range(pos_and_look.yaw, pos_and_look.pitch);

    packet::NamedEntitySpawnPacket {
        entity_id: eid,
        name: name.to_string(),
        x,
        y,
        z,
        rotation,
        pitch,
        current_item: 0,
    }
    .send(stream)
    .await
}

pub async fn spawn_pickup_entity(
    stream: &mut TcpStream,
    eid: i32,
    item_id: i16,
    count: i8,
    pos_and_look: &PositionAndLook,
) -> Result<(), packet::PacketError> {
    let mut pickup_entity_spawn = vec![0x15];
    pickup_entity_spawn.extend_from_slice(&eid.to_be_bytes());

    pickup_entity_spawn.extend_from_slice(&item_id.to_be_bytes());
    pickup_entity_spawn.extend_from_slice(&count.to_be_bytes());
    let x = (pos_and_look.x * 32.).round() as i32;
    let y = (pos_and_look.y * 32.).round() as i32;
    let z = (pos_and_look.z * 32.).round() as i32;

    pickup_entity_spawn.extend_from_slice(&x.to_be_bytes());
    pickup_entity_spawn.extend_from_slice(&y.to_be_bytes());
    pickup_entity_spawn.extend_from_slice(&z.to_be_bytes());
    pickup_entity_spawn.extend_from_slice(&[0, 0, 0]);

    packet::to_client_packets::PickupSpawnPacket {
        entity_id: eid,
        item_id,
        count,
        x,
        y,
        z,
        rotation: 0,
        pitch: 0,
        roll: 0,
    }
        .send(stream)
        .await
}
