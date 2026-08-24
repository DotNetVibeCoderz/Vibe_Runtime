//! A PNG writer, so a device screen can leave the machine as an image.
//!
//! # Why this is here rather than a crate
//!
//! The workspace has no external dependencies — the same reason the DAP
//! server's JSON is hand-written. A PNG is a signature, three chunks and two
//! checksums, and both checksums already exist: CRC-32 in [`rustclr_crypto`]
//! and Adler-32 in eleven lines below.
//!
//! # Stored deflate, not compressed
//!
//! `IDAT` holds a zlib stream, and zlib permits **stored** blocks: a header,
//! a length, its complement, and the bytes. That is a legal deflate stream
//! that every decoder accepts, and it costs about 0.1% over the raw size.
//!
//! Writing a real compressor would be a few hundred lines of Huffman coding to
//! save a few hundred kilobytes on files that are written once and read by a
//! browser. A 320x240 screen is 225 KB stored. The trade is not close.
//!
//! # Filter type 0 on every row
//!
//! PNG lets each row pick a filter; picking well is what a compressor needs
//! and this does not compress, so every row says `None` and carries its bytes.

use rustclr_crypto::crc32::crc32;

/// Adler-32, the checksum zlib puts at the end of its stream.
fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for byte in data {
        // 65521 is the largest prime below 2^16, which is what the algorithm
        // is defined over.
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    // The CRC covers the type and the body, and not the length.
    let mut checked = Vec::with_capacity(4 + body.len());
    checked.extend_from_slice(kind);
    checked.extend_from_slice(body);
    out.extend_from_slice(&crc32(&checked).to_be_bytes());
}

/// A zlib stream holding `data` in stored blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    // 0x78 0x01: deflate, 32 KB window, no preset dictionary, fastest level.
    let mut out = vec![0x78, 0x01];
    // A stored block's length is a `u16`, so anything larger is split.
    const MAX_BLOCK: usize = 0xFFFF;
    let mut offset = 0;
    loop {
        let end = (offset + MAX_BLOCK).min(data.len());
        let block = &data[offset..end];
        let last = end == data.len();
        out.push(u8::from(last));
        out.extend_from_slice(&(block.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        out.extend_from_slice(block);
        offset = end;
        if last {
            break;
        }
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Encode `width` x `height` RGB565 pixels as an 8-bit RGB PNG.
///
/// RGB565 is expanded by replicating the high bits into the low ones rather
/// than shifting and leaving zeros: a full-white 0xFFFF becomes 255 and not
/// 248, so a screenshot of a white screen is white.
pub fn rgb565_to_png(pixels: &[u16], width: u32, height: u32) -> Vec<u8> {
    let expected = (width as usize) * (height as usize);
    debug_assert_eq!(pixels.len(), expected, "the buffer must hold width x height pixels");

    // One filter byte per row, then three bytes per pixel.
    let mut raw = Vec::with_capacity(height as usize * (1 + 3 * width as usize));
    for y in 0..height as usize {
        raw.push(0); // filter: None
        for x in 0..width as usize {
            let p = pixels.get(y * width as usize + x).copied().unwrap_or(0);
            let r5 = ((p >> 11) & 0x1F) as u8;
            let g6 = ((p >> 5) & 0x3F) as u8;
            let b5 = (p & 0x1F) as u8;
            raw.push((r5 << 3) | (r5 >> 2));
            raw.push((g6 << 2) | (g6 >> 4));
            raw.push((b5 << 3) | (b5 >> 2));
        }
    }

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(2); // colour type: truecolour
    ihdr.push(0); // compression: deflate
    ihdr.push(0); // filter method 0
    ihdr.push(0); // no interlace
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    chunk(&mut png, b"IEND", &[]);
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vector RFC 1950 quotes for the string "Wikipedia".
    #[test]
    fn adler32_matches_its_published_vector() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        // The empty stream is 1, not 0 — `a` starts at one.
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn a_png_has_the_signature_and_the_three_chunks() {
        let png = rgb565_to_png(&[0xFFFF, 0x0000, 0xF800, 0x001F], 2, 2);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        for kind in [b"IHDR", b"IDAT", b"IEND"] {
            assert!(
                png.windows(4).any(|w| w == kind),
                "{} is missing",
                core::str::from_utf8(kind).unwrap()
            );
        }
    }

    /// The point of replicating bits rather than shifting: white must be white.
    ///
    /// A naive `(r5 << 3)` gives 248, and a screenshot of a white panel comes
    /// out faintly grey — which looks like a rendering bug in whatever drew it.
    #[test]
    fn full_white_survives_the_expansion() {
        let png = rgb565_to_png(&[0xFFFF], 1, 1);
        // Past the signature, IHDR (25 bytes), and the IDAT length/type/zlib
        // header/block header: the row is a filter byte then RGB.
        let idat = png.windows(4).position(|w| w == b"IDAT").expect("IDAT");
        let pixel = &png[idat + 4 + 2 + 5 + 1..idat + 4 + 2 + 5 + 4];
        assert_eq!(pixel, &[255, 255, 255], "0xFFFF must expand to full white");
    }

    /// A buffer larger than one stored block has to be split, and the split
    /// must not lose or duplicate a byte.
    #[test]
    fn a_stream_longer_than_one_block_is_split_correctly() {
        let data: Vec<u8> = (0..70_000u32).map(|i| i as u8).collect();
        let z = zlib_stored(&data);
        // 2 zlib header + 2 blocks x 5 header + payload + 4 adler.
        assert_eq!(z.len(), 2 + 5 + 0xFFFF + 5 + (70_000 - 0xFFFF) + 4);
        assert_eq!(&z[z.len() - 4..], &adler32(&data).to_be_bytes());
        // The first block is not the last, the second is.
        assert_eq!(z[2], 0);
        assert_eq!(z[2 + 5 + 0xFFFF], 1);
    }
}
