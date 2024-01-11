use std::sync::{atomic::AtomicBool, Arc};

use tokio::{
    net::TcpStream,
    sync::{broadcast, RwLock},
};

use crate::{
    packet::{self, util::SendPacket},
    world::BlockUpdate,
    State,
};

pub async fn block_updates(
    logged_in: Arc<AtomicBool>,
    mut rx_block_updates: broadcast::Receiver<BlockUpdate>,
    state: Arc<RwLock<State>>,
    stream: Arc<RwLock<TcpStream>>,
) {
    loop {
        if !logged_in.load(std::sync::atomic::Ordering::Relaxed) {
            continue;
        }

        if let Ok(block_update) = rx_block_updates.recv().await {
            let (x, y, z, id, meta);

            println!("block update");

            // check distance etc of player

            match block_update {
                BlockUpdate::Place(block_info) => {
                    x = block_info.x;
                    y = block_info.y;
                    z = block_info.z;
                    id = block_info.item_id as i8;
                    // meta = block_info.face
                    meta = 0;
                }
                BlockUpdate::Break((break_x, break_y, break_z)) => {
                    x = break_x;
                    y = break_y;
                    z = break_z;
                    id = 0;
                    meta = 0;
                }
            }
            packet::BlockChangePacket {
                x,
                y,
                z,
                block_type: id,
                block_metadata: meta,
            }
            .send(&mut *stream.write().await)
            .await
            .unwrap();
        }
    }
}
