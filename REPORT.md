# Report: kitty cross-pane correctness

Branch `fix/kitty-cross-pane`, 3 commits, not pushed.

Gate: **273 tests pass**, `cargo clippy --all-targets -- -D warnings` clean,
`cargo fmt --check` clean, `cargo build --release --features dev-marker` builds.

---

## What changed, per issue

### #29 — image ids collide across panes

New module **`src/kitty_ids.rs`**. The u32 id space is cut into 65535 blocks of
65536 ids; a pane claims one block, derived from its pid mixed with its startup
nanoseconds (SplitMix64 finalizer). `ImageIds::alloc` hands out ids inside that
block and wraps to the block's own start when exhausted, so an exhausted pane
overwrites its own oldest image rather than a neighbour's.

`KittyRenderer` gained `with_image_ids(scale, out, ImageIds)`. Production
`new()` calls `ImageIds::for_process()`; the explicit-block constructor is the
seam that lets one test process stand in for two panes.

**I did not adopt kitty's `I=` image-number mechanism** (the issue's alternative
suggestion). `I=` requires reading the terminal's allocated id back, and the
strip has no reliable reply channel: herdr forwards our escapes, we set `q=2` to
suppress replies, and the render loop has no reader for them.

**The issue's stated fix is not sufficient on its own.** Seeding `next_id` from
a pid offset would still have failed, because *placement* ids came off the same
counter and are allocated per member per frame (~60/second at 12 fps with 5
members). Any offset-based block would be walked through in minutes and into the
next pane's block. So placement ids moved to their own `PlacementIds` counter.
That is safe: a placement id is scoped to its image id in the protocol, and
image ids are now disjoint per pane.

### #28 — `a=d,d=A` is terminal-global

`kitty::delete_all()` is **removed from the crate**, not merely avoided, and
replaced by `kitty::delete_image(id)` → `a=d,d=I,i=<id>,q=2`. Leaving an unused
`delete_all` builder around invites exactly this regression back.

`KittyRenderer::free_all_images()` iterates `cache` + `icon_cache` and emits one
`d=I` per owned id, then clears the caches and both placement maps. It is called
from the resize purge and from `teardown`. Nothing in the crate can now emit a
delete that is not scoped to an id this process owns.

### #30 — image data never freed, no eviction

Cache values became `Cached { id, last_used }`, where `last_used` is a frame
counter. At the end of every frame, `evict_stale_images` frees (with `d=I`) any
image not placed for `IMAGE_TTL_FRAMES` (720 ≈ 60s at the loop's ~12 fps), plus
the oldest entries above `MAX_CACHED_IMAGES` (256) as a backstop for bursts.
Images placed on the current frame are never evictable, so eviction can only
reclaim things that are off screen.

**Eviction counts frames, not `now_ms`.** `now_ms` was the obvious clock, but
`render::run_loop` pins it to `0` forever under `reduced_motion`, which would
have silently disabled eviction for anyone using that setting.

### #46 — `member_scale = 7` oversamples

Default is now **4** (`src/config.rs`), README updated. A member displays in
~7x4 cells (~56x68 screen px) from a 16x19 sprite-pixel crop window; scale 4
transmits ~64x76 px for that, roughly 1:1 with a little headroom.

---

## Tests added, and what each one pins

Every cross-pane test drives **two `KittyRenderer` instances against separate
sinks** with distinct id blocks, which is the only way to observe these
properties from one process.

`src/kitty_render.rs`:

| Test | Property |
|---|---|
| `two_panes_never_transmit_under_the_same_image_id` | Two panes drawing the same herd emit disjoint `i=` sets, and each pane's ids fall only inside its own block (so disjointness holds for every future id, not just these). |
| `a_pane_tearing_down_leaves_the_other_panes_cached_images_intact` | Pane A's teardown frees no id belonging to pane B, emits no `d=A`, and B's next frame is a pure re-place: no `a=t`, and every id it places is one it transmitted before A quit. This is #28's exact failure mode. |
| `a_resize_in_one_pane_frees_only_that_panes_images` | Same, for the far more common trigger. |
| `no_command_in_a_panes_whole_lifecycle_is_terminal_global` | Blanket scan over transmit → status change → focus change → resize → departure → teardown: no `d=A`, and every id transmitted, placed or freed is inside this pane's block. |
| `placements_do_not_consume_the_panes_image_ids` | 40 frames advance the image-id counter by <8, i.e. placements no longer burn image ids. |
| `an_image_that_stops_being_placed_is_freed_and_retransmitted_later` | An unplaced image's *data* is freed by id after the TTL, and the cache drops it so the member re-transmits rather than placing a dead id. |
| `an_image_still_on_screen_is_never_freed_however_long_it_is_drawn` | Eviction can never take a live image. |
| `evict_from_frees_stale_entries_and_keeps_the_current_frames` | TTL boundary. |
| `evict_from_caps_the_cache_without_touching_this_frames_images` | The size cap reclaims oldest-first and never this frame's image. |
| `the_default_scale_is_not_oversampled_against_the_displayed_footprint` | #46 as a ratio, not a constant: transmitted px ≤ 1.5x displayed px. |

`src/kitty_ids.rs`: `every_block_stays_inside_the_valid_nonzero_id_range`,
`distinct_blocks_hand_out_disjoint_ids`,
`alloc_wraps_inside_its_own_block_when_exhausted`,
`nearby_seeds_land_on_far_apart_blocks` (consecutive pids must not collide),
`placement_ids_are_unique_and_never_zero`.

`src/kitty.rs`: `delete_image_frees_data_with_uppercase_i_and_names_one_id`.

### Red-then-green, verified by re-introducing each bug

Each new test was confirmed to actually fail against the old behaviour, by
temporarily reverting the fix and re-running:

- Both renderers forced onto block 0 → `two_panes_never_transmit_under_the_same_image_id`,
  `a_pane_tearing_down_leaves_the_other_panes_cached_images_intact`,
  `a_resize_in_one_pane_frees_only_that_panes_images` all fail.
- `free_all_images` writing `\x1b_Ga=d,d=A\x1b\\` → those two plus
  `no_command_in_a_panes_whole_lifecycle_is_terminal_global`,
  `resize_purges_and_retransmits`, `teardown_frees_this_panes_own_ids...` fail.
- `evict_stale_images` call commented out →
  `an_image_that_stops_being_placed_is_freed_and_retransmitted_later` fails.
- `member_scale` back to 7 → `the_default_scale_is_not_oversampled...` fails.

---

## Existing tests I changed, and why

No test was weakened to make a new one pass, but four had the old behaviour
written into them and could not both survive and describe the fix:

1. **`kitty_render::resize_purges_and_retransmits`** asserted
   `out.contains("a=d,d=A")` — literally the bug #28 says to remove. Its intent
   (a resize purges and retransmits) is intact and now *stronger*: it asserts
   the freed ids are exactly the ids this pane transmitted, and that `d=A` never
   appears.
2. **`kitty_render::teardown_deletes_all_images`** asserted byte-equality with
   `\x1b_Ga=d,d=A\x1b\\`. Renamed to
   `teardown_frees_this_panes_own_ids_and_never_the_whole_terminal` and
   rewritten to draw first, then assert one `d=I` per transmitted id and no
   `d=A`.
3. **`kitty::place_and_delete_reference_ids`** lost its
   `assert_eq!(delete_all(), …)` line, because `delete_all` no longer exists. A
   new test covers `delete_image` in its place.
4. **`config::renderer_defaults_to_auto_and_scale_to_seven`** → `…_to_four`,
   plus three `member_scale: 7` literals in default-value assertions. These pin
   a constant the issue asks to change.

Also: `icon_crop_never_pans_into_the_bitmap_across_the_full_wave` used
`let scale = 7; // production default`. I changed it to 4 so the comment stays
true. `crop_rect` is linear in scale, so this is not a weaker check.

## What the issues got wrong

- **#29's proposed fix is incomplete** — see above; a pid-derived `next_id`
  offset alone would be exhausted within minutes because placement ids shared
  the counter. The issue does not mention this.
- **#46's "scale 7 transmits 112x168 px"** mixes two rectangles. The transmitted
  canvas is 182x168 px (26x24 sprite px including motion/hat padding); the
  *displayed* crop window is 112x133 px. The conclusion (2-3x oversampled) is
  unchanged, and #30's separate figure of ~122 KB per padded sheep is exactly
  right (182·168·4 = 122,304 B).
- Everything else in the four issues checked out against the source.

## Deliberately left out

- **The resize purge itself.** Images are placed with an explicit `c=`/`r=` cell
  footprint, so a resize does not actually require new pixel data; with `d=A`
  gone, the original justification ("the terminal may have dropped our images")
  is weak. Dropping the purge would remove the retransmission burst #46
  measures, but that is a behaviour change beyond these four issues and an
  existing test pins it. Worth a follow-up issue.
- **`caps::RealCaps` probes with a hardcoded `id: 0x7E51`** in every pane. `a=q`
  stores no image so it does not collide in the image namespace, but the reply
  match is by that shared id. Out of scope; untouched.
- **Making the TTL / cache cap configurable.** Both are consts.
- **Abnormal exit still leaks.** A SIGKILLed pane never runs `teardown`, so its
  images stay resident until the terminal is reset. Pre-existing; `d=A` was
  never a safe fix for it.

## What is unproven

- **No live multi-pane verification.** This session runs inside herdr
  (`HERDR_ENV=1`) and `scripts/herd-test.sh` must be started from a plain
  terminal tab, so I could not run it. Everything here is proven at the
  escape-sequence level against `Vec<u8>` sinks, not against a real terminal.
  To check by hand: `sh scripts/herd-test.sh` from a normal tab, open two or
  more strips, then resize the window and quit one pane — the others must keep
  their sheep.
- **Block collisions are unlikely, not impossible.** Two live panes whose
  (pid, start-time) mix into the same block still share ids. ~1 in 65535 per
  pair, versus 100% before. A collision-free scheme needs a reply channel we
  do not have.
- **`ImageIds::for_process()` is not covered by a test** that observes two real
  processes — it cannot be, from one test binary. Only `block_from_seed`'s
  spreading behaviour is tested.
- **The 1:1 scale claim assumes a cell size.** The crate cannot query the real
  cell size through herdr; the oversampling test assumes a generous 10x22 px
  cell, which is the lenient direction. Whether scale 4 looks right on Felix's
  Ghostty is a visual call that still needs eyes on it.
