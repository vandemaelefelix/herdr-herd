//! Kitty graphics protocol escape-sequence builders. We emit these ourselves;
//! herdr forwards them to the outer terminal when its experimental
//! `[experimental] kitty_graphics = true` flag is on (see the design spec).

use crate::base64;

const CHUNK: usize = 4096; // max base64 chars per APC chunk (protocol limit)

/// Wrap `control` (and optional `payload`) in a kitty graphics APC sequence.
/// The `;` delimiter is only valid when a payload follows — omitting it for an
/// empty payload keeps a payload-less command (`delete_image`, `place`) as bare
/// `\x1b_G<control>\x1b\\`, with no trailing `;`.
fn apc(control: &str, payload: &str) -> String {
    if payload.is_empty() {
        format!("\x1b_G{control}\x1b\\")
    } else {
        format!("\x1b_G{control};{payload}\x1b\\")
    }
}

/// Transmit RGBA (`f=32`) image data under image id `id`, without displaying it
/// (`a=t`). `q=2` suppresses the terminal's success/failure replies.
pub fn transmit_rgba(id: u32, w: usize, h: usize, rgba: &[u8]) -> String {
    let b64 = base64::encode(rgba);
    let chunks: Vec<&str> = if b64.is_empty() {
        vec![""]
    } else {
        (0..b64.len())
            .step_by(CHUNK)
            .map(|i| &b64[i..(i + CHUNK).min(b64.len())])
            .collect()
    };
    let mut out = String::new();
    for (idx, chunk) in chunks.iter().enumerate() {
        let last = idx == chunks.len() - 1;
        let control = if idx == 0 {
            format!(
                "a=t,f=32,s={w},v={h},i={id},q=2,m={}",
                if last { 0 } else { 1 }
            )
        } else {
            format!("m={}", if last { 0 } else { 1 })
        };
        out.push_str(&apc(&control, chunk));
    }
    out
}

/// Place transmitted image `id` as placement `pid` at the current cursor.
pub fn place(id: u32, pid: u32) -> String {
    apc(&format!("a=p,i={id},p={pid},q=2"), "")
}

/// Place transmitted image `id` as placement `pid`, scaled to exactly `cols` x
/// `rows` terminal cells (`c=`/`r=`), at stacking index `z` (`z=`; higher =
/// on top). The explicit cell footprint makes the on-screen size known without
/// querying the cell pixel size (which herdr does not report); the explicit `z`
/// makes overlap stacking deterministic and match our draw z-order, so hover
/// hit-testing agrees with what is visually in front.
pub fn place_sized(id: u32, pid: u32, cols: u16, rows: u16, z: i32) -> String {
    apc(
        &format!("a=p,i={id},p={pid},c={cols},r={rows},z={z},q=2"),
        "",
    )
}

/// A source-image crop rectangle, in raster pixels (kitty's `x=`/`y=`/`w=`/`h=`
/// placement keys): show only this `w`x`h` region starting at `(x, y)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Place transmitted image `id` as placement `pid`, showing only `crop`'s
/// source rectangle, stretched to `cols`x`rows` terminal cells exactly like
/// [`place_sized`]. This is how motion is animated without retransmitting a
/// new image every frame: the source image is transmitted once, padded with
/// a transparent margin, and each frame crops a different sub-window of it —
/// panning the "camera" over a static, larger canvas.
pub fn place_cropped(id: u32, pid: u32, crop: Crop, cols: u16, rows: u16, z: i32) -> String {
    let Crop { x, y, w, h } = crop;
    apc(
        &format!("a=p,i={id},p={pid},x={x},y={y},w={w},h={h},c={cols},r={rows},z={z},q=2"),
        "",
    )
}

/// Delete placement `pid` of image `id` (removes it from screen; keeps data).
pub fn delete_placement(id: u32, pid: u32) -> String {
    apc(&format!("a=d,d=i,i={id},p={pid},q=2"), "")
}

/// Delete image `id` outright: uppercase `d=I` frees the image *data* and every
/// placement of it, unlike `delete_placement`'s lowercase `d=i`, which only
/// takes the placement off screen and leaks the data (issue #30).
///
/// This is deliberately the only delete that frees data. The protocol's
/// terminal-global `a=d,d=A` is never emitted: every strip pane forwards its
/// escapes to one outer terminal, so `d=A` from one pane destroys every other
/// pane's images while their caches still map to the dead ids, leaving them
/// permanently blank (issue #28). Deletes must name ids this process owns.
pub fn delete_image(id: u32) -> String {
    apc(&format!("a=d,d=I,i={id},q=2"), "")
}

/// The capability-probe query: transmit+query a 1x1 image under `id` (`a=q`).
pub fn probe_query(id: u32) -> String {
    let b64 = base64::encode(&[0u8, 0, 0]); // 1x1 RGB
    apc(&format!("a=q,i={id},f=24,s=1,v=1"), &b64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transmit_wraps_in_apc_and_sets_dimensions() {
        // 1x1 opaque red pixel
        let s = transmit_rgba(7, 1, 1, &[255, 0, 0, 255]);
        assert!(s.starts_with("\x1b_G"));
        assert!(s.ends_with("\x1b\\"));
        assert!(s.contains("a=t")); // transmit only (no display)
        assert!(s.contains("f=32"));
        assert!(s.contains("s=1"));
        assert!(s.contains("v=1"));
        assert!(s.contains("i=7"));
    }

    #[test]
    fn large_payload_is_chunked_with_m_flags() {
        // 40x40 RGBA = 6400 bytes -> base64 ~8536 chars -> >1 chunk of 4096.
        let s = transmit_rgba(1, 40, 40, &vec![1u8; 40 * 40 * 4]);
        assert!(s.matches("\x1b_G").count() >= 2, "multiple APC chunks");
        assert!(s.contains("m=1"), "non-final chunks set m=1");
        assert!(s.contains("m=0"), "final chunk sets m=0");
    }

    #[test]
    fn place_and_delete_reference_ids() {
        assert!(
            place(7, 3).contains("a=p")
                && place(7, 3).contains("i=7")
                && place(7, 3).contains("p=3")
        );
        let ps = place_sized(7, 3, 9, 4, 5);
        assert!(ps.contains("a=p") && ps.contains("i=7") && ps.contains("p=3"));
        assert!(
            ps.contains("c=9") && ps.contains("r=4") && ps.contains("z=5"),
            "explicit cell footprint + stacking index"
        );
        assert!(delete_placement(7, 3).contains("a=d") && delete_placement(7, 3).contains("i=7"));
        assert!(probe_query(9).contains("a=q") && probe_query(9).contains("i=9"));
    }

    #[test]
    fn delete_image_frees_data_with_uppercase_i_and_names_one_id() {
        // Lowercase `d=i` (delete_placement) keeps the data; only uppercase
        // `d=I` frees it. And the id is always named: an unscoped delete would
        // reach into other panes' images.
        let d = delete_image(4242);
        assert_eq!(d, "\x1b_Ga=d,d=I,i=4242,q=2\x1b\\");
        assert!(
            !d.contains("d=A"),
            "the terminal-global delete must never be emitted"
        );
    }

    #[test]
    fn place_cropped_carries_the_source_rectangle_and_cell_footprint() {
        let pc = place_cropped(
            7,
            3,
            Crop {
                x: 12,
                y: 8,
                w: 40,
                h: 28,
            },
            9,
            4,
            5,
        );
        assert!(pc.contains("a=p") && pc.contains("i=7") && pc.contains("p=3"));
        assert!(
            pc.contains("x=12") && pc.contains("y=8") && pc.contains("w=40") && pc.contains("h=28"),
            "source crop rectangle"
        );
        assert!(
            pc.contains("c=9") && pc.contains("r=4") && pc.contains("z=5"),
            "destination cell footprint + stacking index, same as place_sized"
        );
    }
}
