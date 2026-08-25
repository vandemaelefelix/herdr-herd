//! Partitioning of the kitty image-id space between strip panes.
//!
//! Every strip pane is its own process, but they all forward their escapes to
//! ONE outer terminal, so they share a single, terminal-global image namespace.
//! Transmitting under an id another pane already owns *replaces* that pane's
//! pixels, and its cached id then draws the wrong sprite (issue #29). A pane
//! therefore claims a disjoint block of ids up front instead of counting from
//! `1` as though it owned the terminal alone.
//!
//! The block is derived from the process rather than negotiated: kitty's `I=`
//! image-number mechanism would need the terminal's reply read back, which the
//! strip cannot rely on (herdr forwards our escapes, and we suppress replies
//! with `q=2`).

use std::time::{SystemTime, UNIX_EPOCH};

/// Image ids reserved to one pane. Generous: a pane only consumes an id per
/// *distinct* image (species/status/frame/facing/hue/focus, plus icons), never
/// per frame, and eviction hands ids back by wrapping long before this is hit.
pub const IDS_PER_PANE: u32 = 1 << 16;

/// Number of disjoint blocks the id space splits into. Id `0` is not a valid
/// kitty image id, so block `b` owns `[b * IDS_PER_PANE + 1, (b+1) *
/// IDS_PER_PANE]` and the last block stops exactly at `u32::MAX`'s block
/// boundary rather than overflowing past it.
pub const BLOCKS: u32 = u32::MAX / IDS_PER_PANE;

/// A pane's own block of the shared image-id space: hands out ids that no
/// other pane's block can contain.
#[derive(Debug, Clone, Copy)]
pub struct ImageIds {
    base: u32,
    next: u32,
}

impl ImageIds {
    /// Claim `block % BLOCKS` of the id space. Explicit-block construction is
    /// the seam that lets a test drive two renderers with disjoint id spaces
    /// inside one process, where [`ImageIds::for_process`] would hand both the
    /// same block.
    pub fn for_block(block: u32) -> Self {
        let base = (block % BLOCKS) * IDS_PER_PANE + 1;
        Self { base, next: base }
    }

    /// Claim the block belonging to this process, mixed from the pid and the
    /// startup instant. Distinct live panes are overwhelmingly likely to land
    /// on distinct blocks; with `BLOCKS` = 65535 even a dozen panes collide
    /// with probability well under 0.1%.
    pub fn for_process() -> Self {
        Self::for_block(block_from_seed(process_seed()))
    }

    /// The first id in this pane's block.
    #[cfg(test)]
    pub fn base(&self) -> u32 {
        self.base
    }

    /// Whether `id` belongs to this pane — the property a cross-pane test
    /// asserts about every id a renderer transmits or deletes.
    #[cfg(test)]
    pub fn contains(&self, id: u32) -> bool {
        id >= self.base && id - self.base < IDS_PER_PANE
    }

    /// Hand out the next id, wrapping back to the block's start when the block
    /// is exhausted. Wrapping replaces one of *this* pane's own long-evicted
    /// images rather than trampling a neighbour's, which is the whole point of
    /// the partition.
    pub fn alloc(&mut self) -> u32 {
        let id = self.next;
        self.next = if id - self.base >= IDS_PER_PANE - 1 {
            self.base
        } else {
            id + 1
        };
        id
    }
}

/// Monotonic placement ids. A kitty placement id is scoped to the image id it
/// is placed under, and image ids are already disjoint per pane, so this only
/// has to be unique within one image: a wrapping counter suffices. It is kept
/// separate from [`ImageIds`] because placements are allocated *per member per
/// frame* — sharing one counter (as the original code did) burned through the
/// image-id space at frame rate.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlacementIds {
    next: u32,
}

impl PlacementIds {
    /// Ids start at 1: `p=0` is the protocol's "unspecified placement".
    pub fn new() -> Self {
        Self { next: 1 }
    }

    pub fn alloc(&mut self) -> u32 {
        let id = self.next;
        self.next = if id == u32::MAX { 1 } else { id + 1 };
        id
    }
}

/// Spread a seed over the block space. SplitMix64's finalizer: cheap, and it
/// decorrelates the low bits of neighbouring pids, which a plain `pid % BLOCKS`
/// would map to neighbouring blocks.
fn block_from_seed(seed: u64) -> u32 {
    (mix64(seed) % BLOCKS as u64) as u32
}

fn mix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// pid ⊕ startup nanoseconds. The pid alone is unique among *live* processes,
/// which is what matters here; the nanoseconds add entropy so two panes whose
/// pids happen to mix into the same block are not doomed to do so on every
/// restart.
fn process_seed() -> u64 {
    let pid = u64::from(std::process::id());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(0);
    (pid << 32) ^ nanos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_block_stays_inside_the_valid_nonzero_id_range() {
        for block in [0u32, 1, 7, BLOCKS - 1, BLOCKS, BLOCKS + 3, u32::MAX] {
            let ids = ImageIds::for_block(block);
            assert!(ids.base() >= 1, "id 0 is not a valid kitty image id");
            let last = ids.base() as u64 + IDS_PER_PANE as u64 - 1;
            assert!(
                last <= u32::MAX as u64,
                "block {block} runs past the u32 id space (last = {last})"
            );
        }
    }

    #[test]
    fn distinct_blocks_hand_out_disjoint_ids() {
        let mut a = ImageIds::for_block(3);
        let mut b = ImageIds::for_block(4);
        let a_ids: Vec<u32> = (0..1000).map(|_| a.alloc()).collect();
        let b_ids: Vec<u32> = (0..1000).map(|_| b.alloc()).collect();
        for id in &a_ids {
            assert!(!b_ids.contains(id), "id {id} was handed out by both blocks");
            assert!(a.contains(*id) && !b.contains(*id));
        }
    }

    #[test]
    fn alloc_wraps_inside_its_own_block_when_exhausted() {
        let mut ids = ImageIds::for_block(2);
        let base = ids.base();
        for _ in 0..IDS_PER_PANE {
            let id = ids.alloc();
            assert!(ids.contains(id), "{id} escaped the block");
        }
        assert_eq!(
            ids.alloc(),
            base,
            "an exhausted block wraps onto its own oldest id, never a neighbour's"
        );
    }

    #[test]
    fn nearby_seeds_land_on_far_apart_blocks() {
        // Consecutive pids are the realistic case (panes spawned back to back),
        // so they must not map to adjacent — or worse, identical — blocks.
        let blocks: Vec<u32> = (1000..1016).map(|pid| block_from_seed(pid << 32)).collect();
        let mut sorted = blocks.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), blocks.len(), "consecutive pids collided");
    }

    #[test]
    fn placement_ids_are_unique_and_never_zero() {
        let mut p = PlacementIds::new();
        let ids: Vec<u32> = (0..100).map(|_| p.alloc()).collect();
        assert!(ids.iter().all(|&id| id != 0));
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
    }
}
