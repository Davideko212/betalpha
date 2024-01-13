use std::{
    future::Future,
    io::Cursor,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Arc,
    },
};
use std::ops::Deref;
use std::os::raw::c_ulong;
use std::sync::atomic::AtomicI64;

use bytes::{Buf, BytesMut};

use global_handlers::{collection_center, Animation, CollectionCenter};
use packet::{Deserialize, PlayerBlockPlacementPacket};
use procedures::{
    login,
    passive::{player_look, player_position, player_position_and_look},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{
        broadcast::{self, error::TryRecvError},
        mpsc::{self, Sender},
        RwLock,
    },
};
use regex::Regex;
use rand::{Rng, thread_rng};
use world::{load_demo::load_entire_world, BlockUpdate, Chunk};

// if other clients want to interact with this client
mod global_handlers;
mod movement;
mod packet;
mod utils;
mod world;

// if the server (instantly) reacts to client activity
mod procedures;

use crate::packet::util::*;
use crate::packet::PacketError;

// mod byte_man;
// pub use byte_man::*;

mod entities;

fn base36_to_base10(input: i8) -> i32 {
    let mut result = 0;
    let mut base = 1;
    let mut num = input.abs() as i32;

    while num > 0 {
        let digit = num % 10;
        result += digit * base;
        num /= 10;
        base *= 36;
    }

    result * if input.is_negative() { -1 } else { 1 }
}

#[test]
fn test_base_conv() {
    assert_eq!(base36_to_base10(18), 44);
    println!("base: {}", base36_to_base10(-127));
    println!("{}", (12 << 4));
}

use crate::entities::{spawn_pickup_entity, spawned_named_entity};

pub struct Chunk {
    chunk_x: i32,
    chunk_z: i32,
    blocks: Vec<u8>,
    data: Vec<u8>,
    sky_light: Vec<u8>,
    block_light: Vec<u8>,
    height_map: Vec<u8>,
}

#[derive(Copy, Clone, Debug)]
pub struct Block {
    x: i32,
    y: i8,
    z: i32,
    block_type: i8,
    metadata: i8,
}

#[derive(Copy, Clone, Debug)]
pub struct Item {
    item_type: i16,
    count: i8,
    life: i16,
}

pub struct Inventory {
    main: [Item; 36],
    equipped: [Item; 4],
    crafting: [Item; 4],
}

impl Default for Item {
    fn default() -> Item { // empty item slot
        Item {
            item_type: -1,
            count: 0,
            life: 0,
        }
    }
}

type PacketHandler = Box<
    dyn FnOnce(
        &mut Cursor<&[u8]>,
        &mut TcpStream,
    ) -> Pin<Box<dyn Future<Output=Result<(), Error>>>>,
>;

#[inline]
pub async fn incomplete(_buf: &mut Cursor<&[u8]>, _stream: &mut TcpStream) -> Result<(), Error> {
    Err(Error::Incomplete)
}

fn force_boxed<T>(f: fn(&mut Cursor<&[u8]>, &mut TcpStream) -> T) -> PacketHandler
    where
        T: Future<Output=Result<(), Error>> + 'static,
{
    Box::new(move |buf, stream| Box::pin(f(buf, stream)))
}

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("0.0.0.0:25565").await.unwrap();

    // force_boxed::<_>(keep_alive);
    // let mut packet_handlers: Vec<PacketHandler> = vec![force_boxed(incomplete)];
    // packet_handlers[0x00] = keep_alive;

    // let mut chunks = Vec::new();

    let chunks = load_entire_world("./World2/");
    let chunks = &*Box::leak(chunks.into_boxed_slice());
    let (tx_pos_and_look, rx_pos_and_look) =
        mpsc::channel::<(i32, PositionAndLook, Option<String>)>(256);
    let (tx_pos_and_look_update, _pos_and_look_update_rx) = broadcast::channel(256);

    let (tx_destroy_self_entity, rx_entity_destroy) = mpsc::channel::<i32>(100);
    let (tx_destroy_entities, _) = broadcast::channel(256);
    let (tx_block_updates, mut rx_block_updates) = mpsc::channel::<Block>(100);
    let (tx_block_server, _) = broadcast::channel::<Block>(256);
    let (tx_item_updates, mut rx_item_updates) = mpsc::channel::<(i32, Item, PositionAndLook)>(100);
    let (tx_item_server, _) = broadcast::channel::<(i32, Item, PositionAndLook)>(256);

    let (tx_animation, rx_animation) = mpsc::channel::<(i32, Animation)>(100);
    let (tx_broadcast_animations, _) = broadcast::channel::<(i32, Animation)>(100);

    let (tx_block_update, rx_block_updates) = mpsc::channel::<BlockUpdate>(100);
    let (tx_broadcast_block_updates, _) = broadcast::channel::<BlockUpdate>(100);

    // several maps - avoid cloning of username (remove username from state -> username lookup ?)
    let entity_positions = std::collections::HashMap::new();
    let entity_username = std::collections::HashMap::new();

    let pos_and_look_update_tx_inner = pos_and_look_update_tx.clone();
    let tx_destroy_entities_inner = tx_destroy_entities.clone();
    let tx_block_server_inner = tx_block_server.clone();
    let tx_item_server_inner = tx_item_server.clone();
    let tx_destroy_items_inner = tx_destroy_entities.clone();

    tokio::task::spawn(async move {
        loop {
            incr_time();

            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });

    // player entities
    tokio::spawn(async move {
        loop {
            // receive position updates, log in (username)
            if let Ok((eid, pos_and_look, username)) = pos_and_look_rx.try_recv() {
                let prev_pos_and_look = entity_positions.insert(eid, pos_and_look);
                if let Some(username) = username {
                    entity_username.insert(eid, username);
                }

                // if a player logs in (prev pos is none), not moving entities should be sent
                if prev_pos_and_look.is_none() {
                    for (eid, pos_and_look) in &entity_positions {
                        pos_and_look_update_tx_inner
                            .send((
                                *eid,
                                entities::Type::Player(entity_username[eid].clone()),
                                *pos_and_look,
                                None,
                            ))
                            .unwrap();
                    }
                }

                pos_and_look_update_tx_inner
                    .send((
                        eid,
                        entities::Type::Player(entity_username[&eid].clone()),
                        pos_and_look,
                        prev_pos_and_look,
                    ))
                    .unwrap();
            }

            // ITEMS
            // receive position updates
            if let Ok((eid, item, pos_and_look)) = rx_item_updates.try_recv() {

                /*tx_item_server_inner
                    .send((
                        eid,
                        entities::Type::Player(entity_username[&eid].clone()),
                        pos_and_look,
                        prev_pos_and_look,
                    ))
                    .unwrap();*/
                tx_item_server_inner
                    .send((
                        eid,
                        item,
                        pos_and_look
                    ))
                    .unwrap();
            }

            if let Ok(block) = rx_block_updates.try_recv() {
                tx_block_server_inner.send(block).unwrap();
            }

            if let Ok(eid) = rx_entity_destroy.try_recv() {
                entity_positions.remove(&eid);
                entity_username.remove(&eid);

                tx_destroy_entities_inner.send(eid).unwrap();
                tx_destroy_items_inner.send(eid).unwrap();
            }

            tokio::time::sleep(std::time::Duration::from_secs_f64(0.0001)).await;
        }
    });
    tokio::spawn(collection_center(
        entity_username,
        entity_positions,
        CollectionCenter {
            rx_pos_and_look,
            tx_pos_and_look_update: tx_pos_and_look_update.clone(),
            rx_entity_destroy,
            tx_destroy_entities: tx_destroy_entities.clone(),
            rx_animation,
            tx_broadcast_animations: tx_broadcast_animations.clone(),
            rx_block_updates,
            tx_broadcast_block_updates: tx_broadcast_block_updates.clone(),
        },
    ));

    loop {
        let mut channels = Channels {
            tx_player_pos_and_look: tx_pos_and_look.clone(),
            rx_entity_movement: tx_pos_and_look_update.clone().subscribe(),
            tx_destroy_self_entity: tx_destroy_self_entity.clone(),
            rx_destroy_entities: tx_destroy_entities.clone().subscribe(),
            tx_block_updates: tx_block_updates.clone(),
            rx_block_server: tx_block_server.clone().subscribe(),
            tx_item_updates: tx_item_updates.clone(),
            rx_item_server: tx_item_server.clone().subscribe(),
            tx_animation: tx_animation.clone(),
            rx_global_animations: tx_broadcast_animations.subscribe(),
        };

        let stream = listener.accept().await.unwrap();
        tokio::spawn(async move {
            let rx_entity_movement = &mut channels.rx_entity_movement;
            let rx_destroy_entities = &mut channels.rx_destroy_entities;

            // used to clear the previous buffered moves ..
            while rx_entity_movement.try_recv().err() != Some(TryRecvError::Empty) {}
            while rx_destroy_entities.try_recv().err() != Some(TryRecvError::Empty) {}

            handle_client(stream.0, chunks, channels).await;
        });
    }
}

pub struct Channels {
    tx_player_pos_and_look: mpsc::Sender<(i32, PositionAndLook, Option<String>)>,
    rx_entity_movement: broadcast::Receiver<(
        i32,
        entities::Type,
        PositionAndLook,
        Option<PositionAndLook>,
    )>,
    tx_destroy_self_entity: mpsc::Sender<i32>,
    rx_destroy_entities: broadcast::Receiver<i32>,
    tx_block_updates: mpsc::Sender<Block>,
    rx_block_server: broadcast::Receiver<Block>,
    tx_item_updates: mpsc::Sender<(i32, Item, PositionAndLook)>,
    rx_item_server: broadcast::Receiver<(i32, Item, PositionAndLook)>,
    tx_animation: mpsc::Sender<(i32, Animation)>,
    rx_global_animations: broadcast::Receiver<(i32, Animation)>,
}

static TIME: AtomicI64 = AtomicI64::new(0);
const SIZE: usize = 1024 * 8;
static EIDCounter: AtomicI32 = AtomicI32::new(300000);

pub enum Error {
    Incomplete,
}

pub async fn keep_alive(
    _buf: &mut Cursor<&[u8]>,
    stream: &mut TcpStream,
) -> Result<(), PacketError> {
    let packet = vec![0];
    stream.write_all(&packet).await.unwrap();
    stream.flush().await.unwrap();
    Ok(())
}

fn get_id() -> i32 {
    static COUNTER: AtomicI32 = AtomicI32::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

// TODO: add checking with peak (faster) [I won't do it]
// TODO: use Arc rwlock
async fn parse_packet(
    stream: &mut TcpStream,
    buf: &BytesMut,
    chunks: &[Chunk],
    state: &RwLock<State>,
    tx_player_pos_and_look: &Sender<(i32, PositionAndLook, Option<String>)>,
    tx_disconnect: &Sender<i32>,
    tx_blocks: &Sender<Block>,
    tx_items: &Sender<(i32, Item, PositionAndLook)>,
    tx_entity: &Sender<(i32, PositionAndLook, Option<String>)>,
    tx_disconnect: &Sender<i32>,
    tx_animation: &Sender<(i32, Animation)>,
    logged_in: &AtomicBool,
    chat: &mut Vec<String>,
) -> Result<usize, PacketError> {
    let mut buf = Cursor::new(&buf[..]);

    // let packet_id = get_u8(&mut buf)?;
    // println!("packet_id: {packet_id}");

    // println!("buf: {buf:?}");

    // some packets may accumulate, therefore process all of them (happened especially for 0x0A)
    while let Ok(packet_id) = get_u8(&mut buf) {
        match packet_id {
            0 => keep_alive(&mut buf, stream).await?,
            1 => login(stream, &mut buf, &chunks, logged_in, state, tx_entity).await?,
            // Handshake
            0x02 => {
                // skip(&mut buf, 1)?;
                let username = get_string(&mut buf)?;
                stream.write_all(&[2, 0, 1, b'-']).await.unwrap();
                stream.flush().await.unwrap();
                println!("ch: {username:?}");
            }
            3 => {
                let message = get_string(&mut buf)?;
                chat.push(message);
                stream.flush().await.unwrap();
                //println!("chat_message: {message:?}")
            }
            0x05 => {
                let inv_type = get_i32(&mut buf)?; // TODO: USE THIS!
                let count = get_u16(&mut buf)? as i16;

                for slot in 0..count {
                    let item_id = get_u16(&mut buf)? as i16;
                    if item_id == -1 {
                        state.write().await.inventory.main[slot as usize] = Item::default();
                    } else {
                        let count = get_i8(&mut buf)?;
                        let uses = get_u16(&mut buf)? as i16;
                        state.write().await.inventory.main[slot as usize] = Item {
                            item_type: item_id,
                            count,
                            life: uses,
                        }
                    }
                }

                stream.flush().await.unwrap();
            }
            0x0A => {
                let on_ground = get_u8(&mut buf)? != 0;
                // println!("on_ground: {on_ground}");

                let outer_state;
                {
                    let mut state = state.write().await;

                    state.position_and_look.on_ground = on_ground;
                    outer_state = (state.entity_id, state.position_and_look);

                    tx_player_pos_and_look
                        .send((outer_state.0, outer_state.1, None))
                        .await
                        .unwrap();
                }
            }

            0x0B => {
                let x = get_f64(&mut buf)?;
                let y = get_f64(&mut buf)?;
                let stance = get_f64(&mut buf)?;
                let z = get_f64(&mut buf)?;
                let on_ground = get_u8(&mut buf)? != 0;

                let outer_state;
                {
                    let mut state = state.write().await;

                    state.position_and_look.x = x;
                    state.position_and_look.y = y;
                    state.position_and_look.z = z;
                    //state.position_and_look.yaw = yaw;
                    //state.position_and_look.pitch = pitch;
                    state.position_and_look.on_ground = on_ground;
                    outer_state = (state.entity_id, state.position_and_look);

                    tx_player_pos_and_look
                        .send((outer_state.0, outer_state.1, None))
                        .await
                        .unwrap();
                    // println!("{yaw} {pitch} {on_ground}");
                }
            }

            0x0C => {
                let yaw = get_f32(&mut buf)?;
                let pitch = get_f32(&mut buf)?;
                let on_ground = get_u8(&mut buf)? != 0;

                let outer_state;
                {
                    let mut state = state.write().await;
                    state.position_and_look.yaw = yaw;
                    state.position_and_look.pitch = pitch;
                    state.position_and_look.on_ground = on_ground;
                    outer_state = (state.entity_id, state.position_and_look);
                }
                tx_player_pos_and_look
                    .send((outer_state.0, outer_state.1, None))
                    .await
                    .unwrap();
                // println!("{yaw} {pitch} {on_ground}");
            }

            0x0D => {
                let x = get_f64(&mut buf)?;
                let y = get_f64(&mut buf)?;
                let _stance = get_f64(&mut buf)?;
                let z = get_f64(&mut buf)?;
                let yaw = get_f32(&mut buf)?;
                let pitch = get_f32(&mut buf)?;
                let on_ground = get_u8(&mut buf)? != 0;
                let outer_state;
                {
                    let mut state = state.write().await;
                    state.position_and_look.x = x;
                    state.position_and_look.y = y;
                    state.position_and_look.z = z;
                    state.position_and_look.yaw = yaw;
                    state.position_and_look.pitch = pitch;
                    state.position_and_look.on_ground = on_ground;
                    outer_state = (state.entity_id, state.position_and_look);
                }
                tx_player_pos_and_look
                    .send((outer_state.0, outer_state.1, None))
                    .await
                    .unwrap();

                // println!("{x} {y} {stance} {z} {yaw} {pitch} {on_ground}");
            }
            0x0E => {
                let status = get_i8(&mut buf)?;
                let x = get_i32(&mut buf)?;
                let y = get_i8(&mut buf)?;
                let z = get_i32(&mut buf)?;
                let face = get_i8(&mut buf)?;
                //println!("digging: {status} {x} {y} {z} {face}");
                //println!("{}", get_block_id(chunks, x, y, z));
                if status == 3 {
                    //destroy_block(chunks, x, y, z);
                    let block = Block {
                        x,
                        y,
                        z,
                        block_type: 0x00,
                        metadata: 0x00,
                    };
                    let pos = PositionAndLook {
                        x: x as f64 + 0.5,
                        y: y as f64,
                        z: z as f64 + 0.5,
                        yaw: 0.0,
                        pitch: 0.0,
                        on_ground: true,
                    };
                    let item = Item {
                        item_type: get_block_id(chunks, x, y, z) as i16,
                        count: 1,
                        life: 0,
                    };

                    tx_blocks.send(block).await.unwrap();
                    tx_items
                        .send((get_eidcounter(), item, pos))
                        .await
                        .unwrap();
                    incr_eidcounter();
                    println!("destroyed!");
                }
            }
            0x0F => {
                let id = get_u16(&mut buf)?;
                let x = get_i32(&mut buf)?;
                let y = get_i8(&mut buf)?;
                let z = get_i32(&mut buf)?;
                let direction = get_i8(&mut buf)?; // TODO: actually use the direction

                let block = Block {
                    x,
                    y,
                    z,
                    block_type: id as i8,
                    metadata: 0x00,
                };
                tx_blocks.send(block).await.unwrap();
            }
            0x10 => {
                let _unused = get_i32(&mut buf)?;
                let id = get_u16(&mut buf)? as i16;
            }
            0x12 => {
                let pid = get_i32(&mut buf)?;
                let arm_swinging = get_u8(&mut buf)? > 0;
                println!("{pid} {arm_swinging}")
            0x0B => player_position(&mut buf, state, tx_entity, tx_animation).await?,
            0x0C => player_look(&mut buf, state, tx_entity).await?,
            0x0D => player_position_and_look(&mut buf, state, tx_entity, tx_animation).await?,

            0x12 => {
                let pid = get_i32(&mut buf)?;
                let animation = get_u8(&mut buf)?;
                // println!("animation: {animation}");
                tx_animation
                    .send((pid, Animation::from(animation)))
                    .await
                    .unwrap();
                // println!("{pid} {arm_swinging}")
            }
            0xff => {
                // player.should_disconnect = true;
                let reason = get_string(&mut buf)?;
                println!("disconnect: {reason}");
                tx_disconnect
                    .send(state.read().await.entity_id)
                    .await
                    .unwrap();
            }

            0x0E => {
                let data = packet::PlayerDiggingPacket::nested_deserialize(&mut buf)?;
                // block broken
                if data.status == 3 {
                    tx_block_update
                        .send(BlockUpdate::Break((data.x, data.y, data.z)))
                        .await
                        .unwrap();
                }
            }

            0x0F => {
                let data = packet::PlayerBlockPlacementPacket::nested_deserialize(&mut buf)?;
                println!("place: {data:?}");
                if data.item_id != -1 {
                    tx_block_update
                        .send(BlockUpdate::Place(data))
                        .await
                        .unwrap();
                }
            }

            0x10 => {
                let data = packet::HoldingChangePacket::nested_deserialize(&mut buf)?;
            }

            // client inv
            0x05 => {
                let data = packet::PlayerInventoryPacket::nested_deserialize(&mut buf)?;
            }
            
            0x07 => {
                let data = packet::UseEntityPacket::nested_deserialize(&mut buf)?;
            }


            _ => {
                println!("packet_id: {packet_id}");
                return Err(PacketError::NotEnoughBytes);
            }
        }
    }
    Ok(buf.position() as usize)
}

pub struct State {
    entity_id: i32,
    username: String,
    logged_in: bool,
    stance: f64,
    on_ground: bool,
    position_and_look: PositionAndLook,
    inventory: Inventory,
    health: i8,
    is_crouching: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct PositionAndLook {
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    on_ground: bool,
}

async fn handle_client(stream: TcpStream, chunks: &[Chunk], channels: Channels) {
    let mut buf = BytesMut::with_capacity(SIZE);

    let Channels {
        tx_player_pos_and_look,
        rx_entity_movement,
        tx_destroy_self_entity,
        mut rx_destroy_entities,
        tx_block_updates,
        mut rx_block_server,
        tx_item_updates,
        mut rx_item_server,
        tx_animation,
        rx_global_animations: rx_global_animation,
    } = channels;

    let stream = Arc::new(RwLock::new(stream));
    let keep_alive_stream = stream.clone();
    let pos_update_stream = stream.clone();
    let chat_stream = stream.clone();
    let mob_stream = stream.clone();
    let tick_stream = stream.clone();
    let chat_msg_vec = Arc::new(RwLock::new(Vec::new()));
    let chat_msg_vec_clone = chat_msg_vec.clone();
    let user = Arc::new(RwLock::new(String::from("")));
    let entity_destroy_stream = stream.clone();
    let block_change_stream = stream.clone();
    let item_pos_update_stream = stream.clone();
    let item_pickup_stream = stream.clone();
    let dropped_items = Arc::new(RwLock::new(Vec::<(i32, Item, PositionAndLook)>::new()));
    let dropped_items_clone = dropped_items.clone();
    let health_stream = stream.clone();

    let state = Arc::new(RwLock::new(State {
        entity_id: 0,
        username: "".to_string(),
        logged_in: false,
        stance: 0.,
        on_ground: true,
        is_crouching: false,
        position_and_look: PositionAndLook {
            x: 0.,
            y: 0.,
            z: 0.,
            yaw: 0.,
            pitch: 0.,
            on_ground: true,
        },
        inventory: Inventory {
            main: [Item::default(); 36],
            equipped: [Item::default(); 4],
            crafting: [Item::default(); 4],
        },
        health: 20
    }));

    let logged_in = Arc::new(AtomicBool::new(false));

    let logged_in_inner = logged_in.clone();
    let state_pos_update = state.clone();
    let item_pos_update = state.clone();
    let health_update = state.clone();

    // spawn or update entities
    tokio::task::spawn(async move {
        let mut seen_before = HashSet::new();
        loop {
            // single core servers
            if !logged_in_inner.load(Ordering::Relaxed) {
                continue;
            }
            let Ok((eid, ty, now, prev)) = rx_entity_movement.recv().await else {
                continue;
            };

            if eid == state_pos_update.read().await.entity_id {
                continue;
            }

            // TODO: add eid is in reach check, unload/destroy entity
            // FIXME: could potentially receive a lot of data / entity information that is instantly discarded

            // println!(
            //     "i am: {}, moved: {eid} {now:?}, prev: {prev:?}",
            //     state_pos_update.read().await.entity_id
            // );

            if !seen_before.contains(&eid) {
                let mut pos_update_stream = pos_update_stream.write().await;

                match ty {
                    entities::Type::Player(name) => {
                        spawned_named_entity(&mut pos_update_stream, eid, &name, &now).await
                    }
                    /*entities::Type::Item(item_id, count) => {
                        println!("hewwo {}", eid);
                        spawn_pickup_entity(&mut pos_update_stream, eid, item_id, count, &now).await;
                    }*/
                };

                let mut entity_spawn = vec![0x1E];
                entity_spawn.extend_from_slice(&eid.to_be_bytes());

                pos_update_stream.write_all(&entity_spawn).await.unwrap();
                pos_update_stream.flush().await.unwrap();
            }

            seen_before.insert(eid);

            if let Some(prev) = prev {
                // check if travelled blocks is > 4 (teleport)

                let x = ((now.x - prev.x) * 32.).round() as i8;
                let y = ((now.y - prev.y) * 32.).round() as i8;
                let z = ((now.z - prev.z) * 32.).round() as i8;
                let yawf = ((now.yaw / 360.) * 255.) % 255.;
                let pitch = (((now.pitch / 360.) * 255.) % 255.) as i8;

                let mut yaw = yawf as i8;
                if yawf < -128. {
                    yaw = 127 - (yawf + 128.).abs() as i8
                }
                if yawf > 128. {
                    yaw = -128 + (yawf - 128.).abs() as i8
                }

                // println!("yaw: {yawf} {} pitch: {pitch}", yaw);

                let mut entity_look_and_move = vec![0x21];
                entity_look_and_move.extend_from_slice(&eid.to_be_bytes());
                entity_look_and_move.extend_from_slice(&[
                    x.to_be_bytes()[0],
                    y.to_be_bytes()[0],
                    z.to_be_bytes()[0],
                    yaw.to_be_bytes()[0],
                    pitch.to_be_bytes()[0],
                ]);

                let mut pos_update_stream = pos_update_stream.write().await;
                pos_update_stream
                    .write_all(&entity_look_and_move)
                    .await
                    .unwrap();
                pos_update_stream.flush().await.unwrap();
            }
        }
    });
    // spawn or update entities
    tokio::task::spawn(global_handlers::spawn_entities(
        logged_in.clone(),
        state.clone(),
        rx_entity_movement,
        stream.clone(),
    ));

    // destroy entities
    tokio::task::spawn(global_handlers::destroy_entities(
        rx_destroy_entities,
        stream.clone(),
    ));

    // animations
    tokio::task::spawn(global_handlers::animations(
        logged_in.clone(),
        rx_global_animation,
        state.clone(),
        stream.clone(),
    ));

    tokio::task::spawn(global_handlers::block_updates(
        logged_in.clone(),
        rx_global_block_update,
        state.clone(),
        stream.clone(),
    ));

    // keep alive
    tokio::task::spawn(async move {
        loop {
            let packet = vec![0];
            keep_alive_stream
                .write()
                .await
                .write_all(&packet)
                .await
                .unwrap();
            keep_alive_stream.write().await.flush().await.unwrap();

            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });

    // chat
    tokio::task::spawn(async move {
        loop {
            let read_vec = chat_msg_vec_clone.read().await.to_vec();
            for msg in read_vec {
                //let username =  &state_pos_update.read().await.username;
                send_chat_msg(chat_stream.clone(), &"".to_string(), msg).await;
            }
            chat_msg_vec_clone.write().await.clear();

            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });

    // mob spawning
    tokio::task::spawn(async move {
        loop {
            /*for chunk in chunks.iter() {
                let mut rng = thread_rng();
                if rng.gen_range(0..10) != 0 {
                    continue;
                }

                // only passive mobs for now
                // pig, sheep, cow, chicken <--> 90-93
                let mobtype = rng.gen_range(90..94);
                /*
                5) Chose a completely random location L1 within the chunk.
                6) If L1 is inside a rock or other solid area, bail out of the spawn function completely. Ignore the remaining chunks.
                7) If L1 is legal it will be the location of a mob pack, pick 6 locations roughly normally distributed around L1, L(i)

                Individual Spawn Loop:
                8) Check that current L(i) is unoccupied, has spawnable ground below, and an empty space above.
                9) If check fails, get next L(i) and go back to 8).
                10) Check that L(i) is 24m+ from (any) player. Also check they are 24m+ from the spawn point. If not, get next L(i) and go back to 8).
                11) Prepare mob for spawning (position, orientation, etc).
                12) LGT(i) = light level of L(i)
                13) (passive mob) If LGT(i) > 8, spawn mob at L(i).
                14) Get next L(i) and go back to 8).
                15) When done with all L(i), go back to 2).
                */
            }*/

            //mob_spawn(mob_stream.clone()).await;
            //tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            //mob_destroy(mob_stream.clone()).await;
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });

    // ticks (very scuffed)
    tokio::task::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        loop {
            let mut tick_vec = vec![0x04];
            let time = get_time();
            tick_vec.extend_from_slice(&time.to_be_bytes());

            tick_stream.write().await.write_all(&tick_vec).await.unwrap();
            tick_stream.write().await.flush().await.unwrap();
            //println!("time: {}", time);

            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });

    // block updates
    tokio::task::spawn(async move {
        loop {
            if let Ok(block) = rx_block_server.recv().await {
                let mut block_change = vec![0x35];
                block_change.extend_from_slice(&block.x.to_be_bytes());
                block_change.extend_from_slice(&block.y.to_be_bytes());
                block_change.extend_from_slice(&block.z.to_be_bytes());
                block_change.extend_from_slice(&block.block_type.to_be_bytes());
                block_change.extend_from_slice(&block.metadata.to_be_bytes());

                let mut change_block_stream = block_change_stream.write().await;
                change_block_stream
                    .write_all(&block_change)
                    .await
                    .unwrap();
                change_block_stream.flush().await.unwrap();
            }
        }
    });

    // item updates
    tokio::task::spawn(async move {
        //let mut seen_before = HashSet::new();
        loop {
            let Ok((eid, item, pos)) = rx_item_server.recv().await else {
                continue;
            };

            //if !seen_before.contains(&eid) {
                let mut pos_update_stream = item_pos_update_stream.write().await;


                spawn_pickup_entity(&mut pos_update_stream, eid, item.item_type, item.count, &pos).await;
                dropped_items.write().await.push((eid, item, pos));

                let mut entity_spawn = vec![0x1E];
                entity_spawn.extend_from_slice(&eid.to_be_bytes());

                pos_update_stream.write_all(&entity_spawn).await.unwrap();
                pos_update_stream.flush().await.unwrap();
            //}

            //seen_before.insert(eid);
        }
    });

    // item pickup
    tokio::task::spawn(async move {
        loop {
            let mut pickup_stream = item_pickup_stream.write().await;
            let temp = item_pos_update.read().await;
            let player_state = temp.deref();
            let mut to_delete = Vec::<usize>::new();

            for (i, item) in dropped_items_clone.write().await.to_vec().iter().enumerate() {
                //println!("x: {:?} z: {:?}", (player_state.position_and_look.x - item.2.x).abs(), (player_state.position_and_look.z - item.2.z).abs());
                if ((player_state.position_and_look.x - item.2.x).abs() <= 1.) &&
                    ((player_state.position_and_look.z - item.2.z).abs() <= 1.) { // TODO: also check y
                    // collect item
                    let mut collect_item = vec![0x16];
                    collect_item.extend_from_slice(&item.0.to_be_bytes());
                    collect_item.extend_from_slice(&player_state.entity_id.to_be_bytes());
                    pickup_stream.write_all(&collect_item).await.unwrap();

                    let mut add = vec![0x11];
                    add.extend_from_slice(&item.1.item_type.to_be_bytes());
                    add.extend_from_slice(&item.1.count.to_be_bytes());
                    add.extend_from_slice(&item.1.life.to_be_bytes());
                    pickup_stream.write_all(&add).await.unwrap();

                    let mut destroy_entity = vec![0x1E];
                    destroy_entity.extend_from_slice(&item.0.to_be_bytes());
                    pickup_stream.write_all(&destroy_entity).await.unwrap();

                    to_delete.push(i);
                }
            }

            for i in to_delete {
                dropped_items_clone.write().await.remove(i);
            }

            pickup_stream.flush().await.unwrap();

            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });

    let mut last_ground_pos = health_update.clone().read().await.position_and_look;
    let mut was_airborne = false;
    // fall damage
    tokio::task::spawn(async move {
        loop {
            let mut health_update_stream = health_stream.write().await;
            let temp = health_update.clone();
            let player_state = temp.write().await;
            println!("falling: {}", !player_state.position_and_look.on_ground);

            if player_state.position_and_look.on_ground {
                let diff = last_ground_pos.y - player_state.position_and_look.y;
                if was_airborne && diff > 3. {
                    let mut update_health = vec![0x08];
                    update_health.extend_from_slice(&((player_state.health as f64 - diff + 3.) as i8).to_be_bytes());
                    health_update_stream.write_all(&update_health).await.unwrap();
                }

                last_ground_pos = player_state.position_and_look;
            } else {
                was_airborne = true;
            }

            health_update_stream.flush().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });

    loop {
        if let Ok(n) = parse_packet(
            &mut *stream.write().await,
            &buf,
            chunks,
            &state,
            &tx_player_pos_and_look,
            &tx_destroy_self_entity,
            &tx_block_updates,
            &tx_item_updates,
            &tx_animation,
            &logged_in,
            &mut *chat_msg_vec.write().await,
        )
            .await
        {
            buf.advance(n);
        }

        if stream.write().await.read_buf(&mut buf).await.unwrap() == 0 {
            println!("break");
            break;
        }

        // println!("{player:?}")
    }

    tx_destroy_self_entity
        .send(state.read().await.entity_id)
        .await
        .unwrap();
}

async fn send_chat_msg(chat_stream: Arc<RwLock<TcpStream>>, user: &String, msg: String) {
    let re = Regex::new(r"( )+").unwrap();
    let formatted = format!("<{}> {}", user, re.replace_all(&msg, " "));

    let mut chat_packet = vec![3];
    chat_packet.extend_from_slice(&(formatted.len() as u16).to_be_bytes());
    chat_packet.extend_from_slice(formatted.as_bytes());
    println!("chat_packet: {chat_packet:?}");
    chat_stream.write().await.write_all(&chat_packet).await.unwrap();
    chat_stream.write().await.flush().await.unwrap();
}

async fn mob_spawn(mob_stream: Arc<RwLock<TcpStream>>) {
    let mut init_packet = vec![0x1E];
    init_packet.extend_from_slice(&420_i32.to_be_bytes()); // EID (420)
    println!("init_packet: {init_packet:?}");
    mob_stream.write().await.write_all(&init_packet).await.unwrap();
    mob_stream.write().await.flush().await.unwrap();

    /*let mut spawn_packet = vec![0x18];
    spawn_packet.extend_from_slice(&420_i32.to_be_bytes()); // EID (420)
    spawn_packet.extend_from_slice(&90_u8.to_be_bytes()); // Type (pig)
    spawn_packet.extend_from_slice(&0_i32.to_be_bytes()); // x (0)
    spawn_packet.extend_from_slice(&75_i32.to_be_bytes()); // y (75)
    spawn_packet.extend_from_slice(&(-5_i32).to_be_bytes()); // z (-5)
    spawn_packet.extend_from_slice(&0_u8.to_be_bytes()); // yaw (0)
    spawn_packet.extend_from_slice(&0_u8.to_be_bytes()); // pitch (0)
    println!("spawn_packet: {spawn_packet:?}");
    mob_stream.write().await.write_all(&spawn_packet).await.unwrap();
    mob_stream.write().await.flush().await.unwrap();*/
}

async fn mob_destroy(mob_stream: Arc<RwLock<TcpStream>>) {
    let mut destroy_packet = vec![0x1D];
    destroy_packet.extend_from_slice(&420_i32.to_be_bytes()); // EID (420)
    mob_stream.write().await.write_all(&destroy_packet).await.unwrap();
    mob_stream.write().await.flush().await.unwrap();
}

fn incr_time() {
    TIME.fetch_add(1, Ordering::SeqCst);
}

fn get_time() -> i64 {
    TIME.load(Ordering::SeqCst)
}

fn incr_eidcounter() {
    EIDCounter.fetch_add(1, Ordering::SeqCst);
}

fn get_eidcounter() -> i32 {
    EIDCounter.load(Ordering::SeqCst)
}

fn get_chunk_from_block(chunks: &[Chunk], x: i32, z: i32) -> &Chunk {
    chunks.iter().find(|c| (c.chunk_x == (x >> 4))
        && (c.chunk_z == (z >> 4))).unwrap()
}

fn get_block_id(chunks: &[Chunk], x: i32, y: i8, z: i32) -> i8 {
    let chunk = get_chunk_from_block(chunks, x, z);
    println!("chunk: {} {}", chunk.chunk_x, chunk.chunk_z);
    let chunk_x: i32 = {
        if x >= 0 {
            x % 16
        } else {
            (16 + (x % 16)) % 16
        }
    };
    let chunk_z: i32 = {
        if z >= 0 {
            z % 16
        } else {
            (16 + (z % 16)) % 16
        }
    };
    let mut index = (y as i32 + ( chunk_z * 128 + ( chunk_x * 128 * 16 ) )) as usize;
    //println!("{index} / {}", chunk.blocks.len());
    *chunk.blocks.get(index).unwrap() as i8
}

fn destroy_block(chunks: &[Chunk], x: i32, y: i8, z: i32) {
    let chunk = get_chunk_from_block(chunks, x, z);
    //println!("chunk: {} {}", chunk.chunk_x, chunk.chunk_z);
    let chunk_x: i32 = {
        if x >= 0 {
            x % 16
        } else {
            16 + (x % 16)
        }
    };
    let chunk_z: i32 = {
        if z >= 0 {
            z % 16
        } else {
            16 + (z % 16)
        }
    };
    let mut index = (y as i32 + ( chunk_z * 128 + ( chunk_x * 128 * 16 ) )) as usize;
    //println!("{index}");
    //chunk.blocks[index % usize::MAX] = 0_u8;
}