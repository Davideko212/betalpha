use std::cell::RefCell;
use std::collections::{HashMap};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use nbt::{Blob, from_gzip_reader};
use serde_json::{Result, Value};
use nbt::ser::Compound;
use serde_json::ser::State;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use crate::packet;
use crate::packet::PacketError;
use crate::packet::util::SendPacket;
use crate::utils::base36_from_i64;
use crate::world::util::{read_nbt_bool, read_nbt_byte_array, read_nbt_i32, read_nbt_i64};

mod util {
    pub fn read_nbt_i64(blob: &nbt::Blob, name: &'static str) -> std::io::Result<i64> {
        if let nbt::Value::Long(v) = blob.get(name).ok_or(std::io::Error::new(std::io::ErrorKind::NotFound, "Field does not exist!"))? {
            return Ok(*v);
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Field has wrong type!"))
        }
    }

    pub fn read_nbt_i32(blob: &nbt::Blob, name: &'static str) -> std::io::Result<i32> {
        if let nbt::Value::Int(v) = blob.get(name).ok_or(std::io::Error::new(std::io::ErrorKind::NotFound, "Field does not exist!"))? {
            return Ok(*v);
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Field has wrong type!"))
        }
    }

    pub fn read_nbt_byte(blob: &nbt::Blob, name: &'static str) -> std::io::Result<i8> {
        if let nbt::Value::Byte(v) = blob.get(name).ok_or(std::io::Error::new(std::io::ErrorKind::NotFound, "Field does not exist!"))? {
            return Ok(*v);
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Field has wrong type!"))
        }
    }

    pub fn read_nbt_bool(blob: &nbt::Blob, name: &'static str) -> std::io::Result<bool> {
        read_nbt_byte(blob, name).map(|v| v > 0)
    }

    pub fn read_nbt_byte_array(blob: &nbt::Blob, name: &'static str) -> std::io::Result<Vec<u8>> {
        if let nbt::Value::ByteArray(v) = blob.get(name).ok_or(std::io::Error::new(std::io::ErrorKind::NotFound, "Field does not exist!"))? {
            return Ok(unsafe {
                let slice = std::ptr::slice_from_raw_parts(v.as_ptr() as *const u8, v.len());
                Vec::from(slice.as_ref().unwrap())
            });
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Field has wrong type!"))
        }
    }
}

pub struct World {
    path: PathBuf,
    chunks: HashMap<u64, Arc<Mutex<Chunk>>>,
    seed: i64,
    spawn: [i32; 3],
    time: u64,
    size_on_disk: u64,
    last_played: u64,
}

impl World {
    pub fn open(world_path: &Path) -> std::io::Result<Self> {
        let (seed, spawn, time, size_on_disk, last_played) = {
            let mut file = std::fs::File::open(world_path.join("level.dat"))?;
            let blob: Blob = from_gzip_reader(&mut file)?;
            let data: Value = serde_json::from_str(&serde_json::to_string(blob.get("Data").unwrap()).unwrap()).unwrap();

            let seed = data["RandomSeed"].as_i64().unwrap();
            let spawn = [data["SpawnX"].as_i64().unwrap() as i32, data["SpawnY"].as_i64().unwrap() as i32, data["SpawnZ"].as_i64().unwrap() as i32];
            let time = data["Time"].as_u64().unwrap();
            let size_on_disk = data["SizeOnDisk"].as_u64().unwrap();
            let last_played = data["LastPlayed"].as_u64().unwrap();

            (seed, spawn, time, size_on_disk, last_played)
        };

        Ok(Self {
            path: world_path.to_path_buf(),
            chunks: HashMap::with_capacity(u16::MAX as usize),
            seed,
            spawn,
            time,
            size_on_disk,
            last_played,
        })
    }

    pub fn close(self) -> std::io::Result<()> {
        let mut file = std::fs::File::open(self.path.join("level.dat"))?;
        let mut blob = Blob::new();
        let mut map: HashMap<String, nbt::Value> = HashMap::new();

        map.insert("RandomSeed".to_string(), nbt::Value::Long(self.seed));
        map.insert("SpawnX".to_string(), nbt::Value::Int(self.spawn[0]));
        map.insert("SpawnY".to_string(), nbt::Value::Int(self.spawn[1]));
        map.insert("SpawnZ".to_string(), nbt::Value::Int(self.spawn[2]));
        map.insert("Time".to_string(), nbt::Value::Long(self.time as i64));
        let size = fs_extra::dir::get_size(self.path).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        map.insert("SizeOnDisk".to_string(), nbt::Value::Long(size as i64));
        map.insert("LastPlayed".to_string(), nbt::Value::Long(std::time::UNIX_EPOCH.elapsed().unwrap().as_secs() as i64));
        blob.insert("Data", nbt::Value::Compound(map)).expect("TODO: panic message");

        blob.to_gzip_writer(&mut file)?;
        Ok(())
    }

    /// Gets a chunk from loaded chunks or loads the chunk into memory.
    ///
    /// returns: Result<Rc<RefCell<Chunk>, Global>, Error>
    pub fn get_chunk(&mut self, x: i32, z: i32) -> std::io::Result<Arc<Mutex<Chunk>>> {
        //println!("meow: {x} {z}");
        //let key = ((x << 16) & z) as u64;
        let mut hasher = DefaultHasher::new();
        x.hash(&mut hasher);
        let key = ((hasher.finish() as i16 % i16::MAX) as i32 * (z + i16::MAX as i32)) as u64; // how not to hash properly 101
        if let Some(chunk) = self.chunks.get(&key) {
            Ok(chunk.clone())
        } else {
            let chunk = Chunk::load(&self.path, x, z)?;
            self.chunks.insert(key, Arc::new(Mutex::new(chunk)));
            self.chunks.get(&key).cloned().ok_or(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Chunk is not loaded!"))
        }
    }

    /// Saves a chunk to disk and unloads it from memory.
    ///
    /// Errors if chunk is still borrowed.
    ///
    /// returns: Result<(), Error>
    pub fn unload_chunk(&mut self, x: i32, z: i32) -> std::io::Result<()> {
        //let key = (x as u64) << 4 | z as u64;
        let mut hasher = DefaultHasher::new();
        x.hash(&mut hasher);
        let key = ((hasher.finish() as i16 % i16::MAX) as i32 * (z + i16::MAX as i32)) as u64; // how not to hash properly 101

        if let Some(chunk) = self.chunks.remove(&key) {
            match chunk.try_lock() {
                Ok(mut chunk) => { chunk.save(&self.path) }
                Err(e) => {
                    self.chunks.insert(key, chunk.clone());
                    Err(std::io::Error::new(std::io::ErrorKind::Other, e))
                }
            }
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Chunk is not loaded!"))
        }
    }

    pub fn get_spawn(&mut self) -> [i32; 3] {
        self.spawn
    }
}

pub struct Chunk {
    chunk_x: i32,
    chunk_z: i32,
    terrain_populated: bool,
    last_update: u64,
    blocks: Vec<u8>,
    data: Vec<u8>,
    block_light: Vec<u8>,
    sky_light: Vec<u8>,
    height_map: Vec<u8>,
}

impl Chunk {
    pub fn load(world_path: &Path, x: i32, z: i32) -> std::io::Result<Self> {
        let (high_level, low_level) = (base36_from_i64((x as u8 % 64) as i64), base36_from_i64((z as u8 % 64) as i64));
        let file_name = format!("c.{x}.{z}.dat");
        let file_path = world_path.join(high_level.clone()).join(low_level.clone()).join(file_name);

        let (terrain_populated, last_update, blocks, data, block_light, sky_light, height_map) = {
            let mut file = std::fs::File::open(file_path)?;
            let blob = Blob::from_gzip_reader(&mut file)?;
            let level: Value = serde_json::from_str(&serde_json::to_string(blob.get("Level").unwrap()).unwrap()).unwrap();

            let terrain_populated = level["TerrainPopulated"].as_i64().unwrap() != 0;
            let last_update = level["LastUpdate"].as_i64().unwrap() as u64;
            let blocks: Vec<u8> = level["Blocks"].as_array().unwrap().iter().map(|x| x.as_i64().unwrap() as u8).collect();
            let data: Vec<u8> = level["Data"].as_array().unwrap().iter().map(|x| x.as_i64().unwrap() as u8).collect();
            let block_light: Vec<u8> = level["BlockLight"].as_array().unwrap().iter().map(|x| x.as_i64().unwrap() as u8).collect();
            let sky_light: Vec<u8> = level["SkyLight"].as_array().unwrap().iter().map(|x| x.as_i64().unwrap() as u8).collect();
            let height_map: Vec<u8> = level["HeightMap"].as_array().unwrap().iter().map(|x| x.as_i64().unwrap() as u8).collect();
            (terrain_populated, last_update, blocks, data, block_light, sky_light, height_map)
        };

        Ok(Self {
            chunk_x: x,
            chunk_z: z,
            terrain_populated,
            last_update,
            blocks,
            data,
            block_light,
            sky_light,
            height_map,
        })
    }

    /// Returns the BlockID at the coordinates specified or `None` if the index is out of bounds.
    ///
    /// # Arguments
    ///
    /// * `x`: chunk local x
    /// * `y`: chunk local y
    /// * `z`: chunk local z
    ///
    /// returns: Option<u8>
    pub fn get_block(&self, x: u8, y: u8, z: u8) -> Option<u8> {
        let index = (y as i32 + ( (z as i32) * 128 + ( (x as i32) * 128 * 16 ) )) as usize;
        self.blocks.get(index).copied()
    }

    /// Overwrites the BlockID at the coordinates specified and returns the old BlockID or `None` if the index is out of bounds.
    ///
    /// # Arguments
    ///
    /// * `x`: chunk local x
    /// * `y`: chunk local y
    /// * `z`: chunk local z
    ///
    /// returns: Option<u8>
    pub fn set_block(&mut self, x: u8, y: u8, z: u8, block_id: u8) -> Option<u8> {
        let index = (y as i32 + ( (z as i32) * 128 + ( (x as i32) * 128 * 16 ) )) as usize;
        self.blocks.get_mut(index).map(|v| {
            let tmp = *v;
            *v = block_id;
            tmp
        })
    }

    pub fn save(&mut self, world_path: &Path) -> std::io::Result<()> {
        let (high_level, low_level) = (base36_from_i64(self.chunk_x as i64), base36_from_i64(self.chunk_z as i64));
        let file_name = format!("c.{}.{}.dat", self.chunk_x, self.chunk_z);
        let file_path = world_path.join(high_level).join(low_level).join(file_name);

        {
            let vu8_vi8 = |x: &Vec<u8>| -> Vec<i8> {
                unsafe {
                    let slice = std::ptr::slice_from_raw_parts(x.as_ptr() as *const i8, x.len());
                    Vec::from(slice.as_ref().unwrap())
                }
            };

            let mut file = std::fs::File::open(file_path)?;
            let mut blob = Blob::new();
            let mut map: HashMap<String, nbt::Value> = HashMap::new();
            map.insert("TerrainPopulated".to_string(), nbt::Value::Byte(self.terrain_populated as i8));
            map.insert("LastUpdate".to_string(), nbt::Value::Long(self.last_update as i64));
            map.insert("Blocks".to_string(), nbt::Value::ByteArray(vu8_vi8(&self.blocks)));
            map.insert("Data".to_string(), nbt::Value::ByteArray(vu8_vi8(&self.data)));
            map.insert("BlockLight".to_string(), nbt::Value::ByteArray(vu8_vi8(&self.block_light)));
            map.insert("SkyLight".to_string(), nbt::Value::ByteArray(vu8_vi8(&self.sky_light)));
            map.insert("HeightMap".to_string(), nbt::Value::ByteArray(vu8_vi8(&self.height_map)));
            blob.insert("Data", nbt::Value::Compound(map)).expect("TODO: panic message");

            blob.to_gzip_writer(&mut file)?;
        }

        Ok(())
    }
}

pub async fn send_chunk(chunk: &Chunk, stream: &mut TcpStream) -> Result<()> {
    packet::PreChunkPacket {
        x: chunk.chunk_x,
        z: chunk.chunk_z,
        mode: true,
    }
        .send(stream)
        .await.unwrap();

    // let mut map_chunk = vec![0x33];
    let x = chunk.chunk_x * 16;
    let y = 0i16;
    let z = chunk.chunk_z * 16;

    let mut to_compress = chunk.blocks.clone();
    to_compress.extend_from_slice(&chunk.data);
    to_compress.extend_from_slice(&chunk.block_light);
    to_compress.extend_from_slice(&chunk.sky_light);

    unsafe {
        let mut len = libz_sys::compressBound(to_compress.len().try_into().unwrap());
        let mut compressed_bytes = vec![0u8; len as usize];
        libz_sys::compress(
            compressed_bytes.as_mut_ptr(),
            &mut len,
            to_compress.as_ptr(),
            to_compress.len().try_into().unwrap(),
        );

        packet::MapChunkPacket {
            x,
            y,
            z,
            size_x: 15,
            size_y: 127,
            size_z: 15,
            compressed_size: len as i32,
            compressed_data: compressed_bytes[..len as usize].to_vec(),
        }
            .send(stream)
            .await.unwrap();
    }

    Ok(())
}