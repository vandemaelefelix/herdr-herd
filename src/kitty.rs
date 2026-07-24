//! Kitty graphics protocol escape-sequence builders. We emit these ourselves;
//! herdr forwards them to the outer terminal when its experimental
//! `[experimental] kitty_graphics = true` flag is on (see the design spec).

use crate::base64;

const CHUNK: usize = 4096; // max base64 chars per APC chunk (protocol limit)

/// Wrap `control` (and optional `payload`) in a kitty graphics APC sequence.
/// The `;` delimiter is only valid when a payload follows — omitting it for
/// an empty payload keeps `delete_all()` byte-for-byte `\x1b_Ga=d,d=A\x1b\\`,
/// with no trailing `;`.
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
/// `rows` terminal cells (`c=`/`r=`). This makes the on-screen footprint known
/// without querying the cell pixel size (which herdr does not report), so the
/// backend can size, bottom-anchor, and hit-test images exactly.
pub fn place_sized(id: u32, pid: u32, cols: u16, rows: u16) -> String {
    apc(&format!("a=p,i={id},p={pid},c={cols},r={rows},q=2"), "")
}

/// Delete placement `pid` of image `id` (removes it from screen; keeps data).
pub fn delete_placement(id: u32, pid: u32) -> String {
    apc(&format!("a=d,d=i,i={id},p={pid},q=2"), "")
}

/// Delete all images and placements (teardown / clean exit).
pub fn delete_all() -> String {
    apc("a=d,d=A", "")
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
        let ps = place_sized(7, 3, 9, 4);
        assert!(ps.contains("a=p") && ps.contains("i=7") && ps.contains("p=3"));
        assert!(
            ps.contains("c=9") && ps.contains("r=4"),
            "explicit cell footprint"
        );
        assert!(delete_placement(7, 3).contains("a=d") && delete_placement(7, 3).contains("i=7"));
        assert_eq!(delete_all(), "\x1b_Ga=d,d=A\x1b\\");
        assert!(probe_query(9).contains("a=q") && probe_query(9).contains("i=9"));
    }
}
