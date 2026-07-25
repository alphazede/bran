//! Process-local, ephemeral result storage; this is neither chat history nor filesystem persistence.

use std::collections::BTreeMap;
use std::fmt;

/// The only content-address algorithm accepted by this store.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ResultIdAlgorithm {
    Sha256,
}

impl ResultIdAlgorithm {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
        }
    }
}

/// A SHA-256 content identifier. Its value is always lowercase hexadecimal.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ResultId {
    algorithm: ResultIdAlgorithm,
    value: String,
}

impl ResultId {
    pub fn sha256(bytes: &[u8]) -> Self {
        let digest = sha256(bytes);
        let mut value = String::with_capacity(64);
        for byte in digest {
            use fmt::Write as _;
            write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
        }
        Self {
            algorithm: ResultIdAlgorithm::Sha256,
            value,
        }
    }

    pub const fn algorithm_kind(&self) -> ResultIdAlgorithm {
        self.algorithm
    }

    pub const fn algorithm(&self) -> &'static str {
        self.algorithm.as_str()
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn parse(value: &str) -> Result<Self, ResultStoreError> {
        let Some(digest) = value.strip_prefix("sha256:") else {
            return Err(ResultStoreError::InvalidId);
        };
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(ResultStoreError::InvalidId);
        }
        Ok(Self {
            algorithm: ResultIdAlgorithm::Sha256,
            value: digest.to_owned(),
        })
    }
}

impl fmt::Display for ResultId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.algorithm())?;
        f.write_str(":")?;
        f.write_str(self.value())
    }
}

/// Positive capacity and lifetime limits for [`MemoryResultStore`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResultStoreLimits {
    pub max_entries: usize,
    pub max_total_bytes: usize,
    pub max_item_bytes: usize,
    pub max_age_ticks: u64,
}

impl ResultStoreLimits {
    pub const fn new(
        max_entries: usize,
        max_total_bytes: usize,
        max_item_bytes: usize,
        max_age_ticks: u64,
    ) -> Result<Self, ResultStoreError> {
        if max_entries == 0
            || max_total_bytes == 0
            || max_item_bytes == 0
            || max_age_ticks == 0
            || max_item_bytes > max_total_bytes
        {
            return Err(ResultStoreError::InvalidLimits);
        }
        Ok(Self {
            max_entries,
            max_total_bytes,
            max_item_bytes,
            max_age_ticks,
        })
    }
}

/// Content-free failures from the in-memory store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResultStoreError {
    InvalidLimits,
    InvalidId,
    Unavailable,
    Corrupt,
    EmptyInput,
    ItemTooLarge,
    NotFound,
    ContentIdCollision,
    ArithmeticOverflow,
    BatchCapacityExceeded,
}

/// Bounded content-addressed result persistence.
///
/// Implementations must preserve bytes exactly, derive every returned ID as
/// SHA-256 over those bytes, publish batches atomically, and apply the exposed
/// capacity and `max_age_ticks` limits on both writes and reads.
pub trait ResultStore {
    fn limits(&self) -> ResultStoreLimits;

    fn put_batch(
        &mut self,
        payloads: &[&[u8]],
        now_tick: u64,
    ) -> Result<Vec<ResultId>, ResultStoreError>;

    fn get(&mut self, id: &ResultId, now_tick: u64) -> Result<Vec<u8>, ResultStoreError>;
}

/// Aggregate state only; it never includes stored content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResultStoreReceipt {
    pub entry_count: usize,
    pub total_bytes: usize,
}

/// The content identifier and aggregate receipt returned by [`MemoryResultStore::put`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PutResult {
    pub id: ResultId,
    pub receipt: ResultStoreReceipt,
}

/// A process-local result store with deterministic oldest-first eviction.
///
/// Storage and the `(created_tick, insertion_order, id)` index are `BTreeMap`s:
/// lookup/removal are `O(log n)` and each eviction is `O(log n)`.
#[derive(Debug, Clone)]
pub struct MemoryResultStore {
    limits: ResultStoreLimits,
    entries: BTreeMap<ResultId, Entry>,
    age_order: BTreeMap<(u64, u64, ResultId), ()>,
    total_bytes: usize,
    next_order: u64,
}

#[derive(Debug, Clone)]
struct Entry {
    bytes: Vec<u8>,
    created_tick: u64,
    order: u64,
}

impl MemoryResultStore {
    pub fn new(
        max_entries: usize,
        max_total_bytes: usize,
        max_item_bytes: usize,
        max_age_ticks: u64,
    ) -> Result<Self, ResultStoreError> {
        Self::with_limits(ResultStoreLimits::new(
            max_entries,
            max_total_bytes,
            max_item_bytes,
            max_age_ticks,
        )?)
    }

    pub const fn with_limits(limits: ResultStoreLimits) -> Result<Self, ResultStoreError> {
        if limits.max_entries == 0
            || limits.max_total_bytes == 0
            || limits.max_item_bytes == 0
            || limits.max_age_ticks == 0
            || limits.max_item_bytes > limits.max_total_bytes
        {
            return Err(ResultStoreError::InvalidLimits);
        }
        Ok(Self {
            limits,
            entries: BTreeMap::new(),
            age_order: BTreeMap::new(),
            total_bytes: 0,
            next_order: 0,
        })
    }

    pub const fn limits(&self) -> ResultStoreLimits {
        self.limits
    }

    pub fn receipt(&self) -> ResultStoreReceipt {
        ResultStoreReceipt {
            entry_count: self.entries.len(),
            total_bytes: self.total_bytes,
        }
    }

    pub fn put(
        &mut self,
        bytes: impl AsRef<[u8]>,
        now_tick: u64,
    ) -> Result<PutResult, ResultStoreError> {
        let bytes = bytes.as_ref();
        if bytes.is_empty() {
            return Err(ResultStoreError::EmptyInput);
        }
        if bytes.len() > self.limits.max_item_bytes {
            return Err(ResultStoreError::ItemTooLarge);
        }

        self.evict_expired(now_tick);
        let id = ResultId::sha256(bytes);
        if let Some(existing) = self.entries.get(&id) {
            if existing.bytes != bytes {
                return Err(ResultStoreError::ContentIdCollision);
            }
            return Ok(PutResult {
                id,
                receipt: self.receipt(),
            });
        }

        while self.entries.len() >= self.limits.max_entries
            || bytes.len() > self.limits.max_total_bytes.saturating_sub(self.total_bytes)
        {
            self.remove_oldest()
                .ok_or(ResultStoreError::ArithmeticOverflow)?;
        }

        let order = self.next_order;
        self.next_order = self
            .next_order
            .checked_add(1)
            .ok_or(ResultStoreError::ArithmeticOverflow)?;
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes.len())
            .ok_or(ResultStoreError::ArithmeticOverflow)?;
        self.age_order.insert((now_tick, order, id.clone()), ());
        self.entries.insert(
            id.clone(),
            Entry {
                bytes: bytes.to_vec(),
                created_tick: now_tick,
                order,
            },
        );
        Ok(PutResult {
            id,
            receipt: self.receipt(),
        })
    }

    /// Smallest atomic batch API: stage on clone, commit only on success.
    /// Fails batch (no publish) if any returned id was evicted by later batch members.
    pub fn put_batch(
        &mut self,
        payloads: impl IntoIterator<Item = impl AsRef<[u8]>>,
        now_tick: u64,
    ) -> Result<Vec<ResultId>, ResultStoreError> {
        let mut trial = self.clone();
        let mut ids = vec![];
        for payload in payloads {
            ids.push(trial.put(payload, now_tick)?.id);
        }
        if ids.is_empty() {
            return Err(ResultStoreError::EmptyInput);
        }
        if ids.iter().any(|id| !trial.entries.contains_key(id)) {
            return Err(ResultStoreError::BatchCapacityExceeded);
        }
        *self = trial;
        Ok(ids)
    }

    pub fn get(&mut self, id: &ResultId, now_tick: u64) -> Result<Vec<u8>, ResultStoreError> {
        self.evict_expired(now_tick);
        self.entries
            .get(id)
            .map(|entry| entry.bytes.clone())
            .ok_or(ResultStoreError::NotFound)
    }

    pub fn remove(&mut self, id: &ResultId) -> Result<ResultStoreReceipt, ResultStoreError> {
        self.remove_id(id).ok_or(ResultStoreError::NotFound)?;
        Ok(self.receipt())
    }

    pub fn clear(&mut self) -> ResultStoreReceipt {
        self.entries.clear();
        self.age_order.clear();
        self.total_bytes = 0;
        self.receipt()
    }

    fn evict_expired(&mut self, now_tick: u64) -> (usize, usize) {
        let mut entries = 0;
        let mut bytes = 0;
        while let Some((created_tick, _, id)) =
            self.age_order.first_key_value().map(|(key, _)| key.clone())
        {
            if now_tick.saturating_sub(created_tick) < self.limits.max_age_ticks {
                break;
            }
            if let Some(removed) = self.remove_id(&id) {
                entries += 1;
                bytes += removed;
            }
        }
        (entries, bytes)
    }

    fn remove_oldest(&mut self) -> Option<(ResultId, usize)> {
        let (_, _, id) = self.age_order.first_key_value()?.0.clone();
        let bytes = self.remove_id(&id)?;
        Some((id, bytes))
    }

    fn remove_id(&mut self, id: &ResultId) -> Option<usize> {
        let entry = self.entries.remove(id)?;
        self.age_order
            .remove(&(entry.created_tick, entry.order, id.clone()));
        self.total_bytes = self.total_bytes.checked_sub(entry.bytes.len())?;
        Some(entry.bytes.len())
    }
}

impl ResultStore for MemoryResultStore {
    fn limits(&self) -> ResultStoreLimits {
        MemoryResultStore::limits(self)
    }

    fn put_batch(
        &mut self,
        payloads: &[&[u8]],
        now_tick: u64,
    ) -> Result<Vec<ResultId>, ResultStoreError> {
        MemoryResultStore::put_batch(self, payloads.iter().copied(), now_tick)
    }

    fn get(&mut self, id: &ResultId, now_tick: u64) -> Result<Vec<u8>, ResultStoreError> {
        MemoryResultStore::get(self, id, now_tick)
    }
}

fn sha256(input: &[u8]) -> [u8; 32] {
    let mut state = [
        0x6a09e667_u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut chunks = input.chunks_exact(64);
    for chunk in &mut chunks {
        sha256_block(&mut state, chunk.try_into().expect("chunk size is 64"));
    }
    let tail = chunks.remainder();
    let mut final_blocks = [0_u8; 128];
    final_blocks[..tail.len()].copy_from_slice(tail);
    final_blocks[tail.len()] = 0x80;
    let final_len = if tail.len() < 56 { 64 } else { 128 };
    final_blocks[final_len - 8..final_len]
        .copy_from_slice(&(input.len() as u64).wrapping_mul(8).to_be_bytes());
    for block in final_blocks[..final_len].chunks_exact(64) {
        sha256_block(&mut state, block.try_into().expect("chunk size is 64"));
    }

    let mut digest = [0_u8; 32];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

fn sha256_block(state: &mut [u32; 8], block: &[u8; 64]) {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut words = [0_u32; 64];
    for (index, chunk) in block.chunks_exact(4).enumerate() {
        words[index] = u32::from_be_bytes(chunk.try_into().expect("chunk size is 4"));
    }
    for index in 16..64 {
        let s0 = words[index - 15].rotate_right(7)
            ^ words[index - 15].rotate_right(18)
            ^ (words[index - 15] >> 3);
        let s1 = words[index - 2].rotate_right(17)
            ^ words[index - 2].rotate_right(19)
            ^ (words[index - 2] >> 10);
        words[index] = words[index - 16]
            .wrapping_add(s0)
            .wrapping_add(words[index - 7])
            .wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for index in 0..64 {
        let choice = (e & f) ^ ((!e) & g);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let next_a = h
            .wrapping_add(sum1)
            .wrapping_add(choice)
            .wrapping_add(K[index])
            .wrapping_add(words[index])
            .wrapping_add(sum0)
            .wrapping_add(majority);
        let next_e = d
            .wrapping_add(h)
            .wrapping_add(sum1)
            .wrapping_add(choice)
            .wrapping_add(K[index])
            .wrapping_add(words[index]);
        h = g;
        g = f;
        f = e;
        e = next_e;
        d = c;
        c = b;
        b = a;
        a = next_a;
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}
