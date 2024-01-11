use tokio::{io::AsyncWriteExt, net::TcpStream};

use crate::PositionAndLook;

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
) {
    let mut named_entity_spawn = vec![0x14];
    named_entity_spawn.extend_from_slice(&eid.to_be_bytes());

    named_entity_spawn.extend_from_slice(&(name.len() as i16).to_be_bytes());
    named_entity_spawn.extend_from_slice(name.as_bytes());
    let x = (pos_and_look.x * 32.).round() as i32;
    let y = (pos_and_look.y * 32.).round() as i32;
    let z = (pos_and_look.z * 32.).round() as i32;

    named_entity_spawn.extend_from_slice(&x.to_be_bytes());
    named_entity_spawn.extend_from_slice(&y.to_be_bytes());
    named_entity_spawn.extend_from_slice(&z.to_be_bytes());
    named_entity_spawn.extend_from_slice(&[0, 0, 0, 0]);

    stream.write_all(&named_entity_spawn).await.unwrap();
    stream.flush().await.unwrap();
}

pub async fn spawn_pickup_entity(
    stream: &mut TcpStream,
    eid: i32,
    item_id: i16,
    count: i8,
    pos_and_look: &PositionAndLook,
) {
    let mut pickup_entity_spawn = vec![0x15];
    pickup_entity_spawn.extend_from_slice(&eid.to_be_bytes());

    pickup_entity_spawn.extend_from_slice(&item_id.to_be_bytes());
    pickup_entity_spawn.extend_from_slice(&count.to_be_bytes());
    let x = ((pos_and_look.x + 0.5) * 32.).round() as i32;
    let y = (pos_and_look.y * 32.).round() as i32;
    let z = ((pos_and_look.z + 0.5) * 32.).round() as i32;

    pickup_entity_spawn.extend_from_slice(&x.to_be_bytes());
    pickup_entity_spawn.extend_from_slice(&y.to_be_bytes());
    pickup_entity_spawn.extend_from_slice(&z.to_be_bytes());
    pickup_entity_spawn.extend_from_slice(&[0, 0, 0]);

    stream.write_all(&pickup_entity_spawn).await.unwrap();
    stream.flush().await.unwrap();
}
