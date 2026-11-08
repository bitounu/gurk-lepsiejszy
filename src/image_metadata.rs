use anyhow::{Context, Result, bail, ensure};

const PNG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Remove metadata without decoding or recompressing the image.
/// Unsupported formats are returned unchanged. Recognized malformed containers fail closed.
pub fn clean(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.starts_with(&[0xff, 0xd8]) {
        clean_jpeg(bytes)
    } else if bytes.starts_with(PNG) {
        clean_png(bytes)
    } else {
        Ok(bytes.to_vec())
    }
}

fn clean_jpeg(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(bytes.len());
    output.extend_from_slice(&bytes[..2]);
    let mut pos = 2;
    let mut entropy = false;
    let mut has_orientation = false;
    loop {
        if entropy {
            let start = pos;
            loop {
                ensure!(pos < bytes.len(), "JPEG has no end marker");
                if bytes[pos] != 0xff {
                    pos += 1;
                    continue;
                }
                let marker_start = pos;
                while bytes.get(pos) == Some(&0xff) {
                    pos += 1;
                }
                let marker = *bytes.get(pos).context("Truncated JPEG scan marker")?;
                if marker == 0 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
                    pos += 1;
                    continue;
                }
                output.extend_from_slice(&bytes[start..marker_start]);
                pos = marker_start;
                entropy = false;
                break;
            }
        }
        let start = pos;
        ensure!(
            bytes.get(pos) == Some(&0xff),
            "Invalid JPEG marker boundary"
        );
        while bytes.get(pos) == Some(&0xff) {
            pos += 1;
        }
        let marker = *bytes.get(pos).context("Truncated JPEG marker")?;
        pos += 1;
        match marker {
            0xd9 => {
                output.extend_from_slice(&bytes[start..pos]);
                return Ok(output);
            }
            0 | 0xd8 => bail!("Unexpected JPEG marker"),
            0x01 | 0xd0..=0xd7 => {
                output.extend_from_slice(&bytes[start..pos]);
                continue;
            }
            _ => {}
        }
        let length_bytes = bytes
            .get(pos..pos + 2)
            .context("Truncated JPEG segment length")?;
        let length = u16::from_be_bytes([length_bytes[0], length_bytes[1]]) as usize;
        ensure!(length >= 2, "Invalid JPEG segment length");
        let end = pos.checked_add(length).context("JPEG segment overflow")?;
        let payload = bytes.get(pos + 2..end).context("Truncated JPEG segment")?;
        match marker {
            0xe0 if payload.starts_with(b"JFIF\0") => {
                ensure!(payload.len() >= 14, "Truncated JFIF header");
                let thumb_len = usize::from(payload[12]) * usize::from(payload[13]) * 3;
                ensure!(payload.len() >= 14 + thumb_len, "Truncated JFIF thumbnail");
                let mut minimal = payload[..14].to_vec();
                minimal[12] = 0;
                minimal[13] = 0;
                jpeg_segment(&mut output, marker, &minimal);
            }
            0xe1 if payload.starts_with(b"Exif\0\0") => {
                if let Some(value) = orientation(&payload[6..])? {
                    ensure!(!has_orientation, "Multiple JPEG EXIF orientation records");
                    has_orientation = true;
                    let mut minimal = b"Exif\0\0".to_vec();
                    minimal.extend(orientation_tiff(value, true));
                    jpeg_segment(&mut output, marker, &minimal);
                }
            }
            0xe2 if payload.starts_with(b"ICC_PROFILE\0") => {
                ensure!(
                    payload.len() >= 14 && payload[12] > 0 && payload[13] >= payload[12],
                    "Invalid JPEG ICC header"
                );
                output.extend_from_slice(&bytes[start..end]);
            }
            0xee if payload.starts_with(b"Adobe") => output.extend_from_slice(&bytes[start..end]),
            0xe0..=0xef | 0xfe => {}
            _ => output.extend_from_slice(&bytes[start..end]),
        }
        pos = end;
        if marker == 0xda {
            entropy = true;
        }
        // DNL is allowed inside an entropy-coded scan and the scan resumes afterwards.
        if marker == 0xdc {
            entropy = true;
        }
    }
}

fn jpeg_segment(output: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    output.extend_from_slice(&[0xff, marker]);
    output.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    output.extend_from_slice(payload);
}

/// Read only IFD0 Orientation; never follow metadata/thumbnail/GPS pointers.
fn orientation(tiff: &[u8]) -> Result<Option<u16>> {
    ensure!(tiff.len() >= 8, "Truncated EXIF TIFF header");
    let little = match &tiff[..2] {
        b"II" => true,
        b"MM" => false,
        _ => bail!("Invalid EXIF byte order"),
    };
    let read16 = |offset: usize| -> Result<u16> {
        let s = tiff
            .get(offset..offset.checked_add(2).context("EXIF offset overflow")?)
            .context("Truncated EXIF value")?;
        Ok(if little {
            u16::from_le_bytes([s[0], s[1]])
        } else {
            u16::from_be_bytes([s[0], s[1]])
        })
    };
    let read32 = |offset: usize| -> Result<u32> {
        let s = tiff
            .get(offset..offset.checked_add(4).context("EXIF offset overflow")?)
            .context("Truncated EXIF offset")?;
        Ok(if little {
            u32::from_le_bytes([s[0], s[1], s[2], s[3]])
        } else {
            u32::from_be_bytes([s[0], s[1], s[2], s[3]])
        })
    };
    ensure!(read16(2)? == 42, "Invalid EXIF TIFF magic");
    let offset = usize::try_from(read32(4)?)?;
    if offset == 0 {
        return Ok(None);
    }
    ensure!(offset >= 8, "Invalid EXIF IFD offset");
    let count = usize::from(read16(offset)?);
    let end = offset
        .checked_add(2)
        .and_then(|n| n.checked_add(count * 12))
        .and_then(|n| n.checked_add(4))
        .context("EXIF IFD overflow")?;
    ensure!(end <= tiff.len(), "Truncated EXIF IFD");
    let mut result = None;
    for i in 0..count {
        let entry = offset + 2 + i * 12;
        if read16(entry)? == 0x0112 {
            ensure!(result.is_none(), "Duplicate EXIF orientation");
            ensure!(
                read16(entry + 2)? == 3 && read32(entry + 4)? == 1,
                "Invalid EXIF orientation type"
            );
            let value = read16(entry + 8)?;
            ensure!((1..=8).contains(&value), "Invalid EXIF orientation value");
            result = Some(value);
        }
    }
    Ok(result)
}

fn orientation_tiff(value: u16, little: bool) -> Vec<u8> {
    let mut output = if little {
        b"II".to_vec()
    } else {
        b"MM".to_vec()
    };
    let u16_bytes = |v: u16| {
        if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    };
    let u32_bytes = |v: u32| {
        if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    };
    output.extend(u16_bytes(42));
    output.extend(u32_bytes(8));
    output.extend(u16_bytes(1));
    output.extend(u16_bytes(0x0112));
    output.extend(u16_bytes(3));
    output.extend(u32_bytes(1));
    output.extend(u16_bytes(value));
    output.extend([0, 0]);
    output.extend(u32_bytes(0));
    output
}

fn clean_png(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut output = PNG.to_vec();
    let mut pos: usize = 8;
    let mut header = false;
    let mut image_data = false;
    let mut exif = false;
    loop {
        let prefix = bytes
            .get(pos..pos.checked_add(8).context("PNG offset overflow")?)
            .context("Truncated PNG chunk header")?;
        let length = u32::from_be_bytes(prefix[..4].try_into()?) as usize;
        ensure!(length <= 0x7fff_ffff, "Invalid PNG chunk length");
        let kind: &[u8; 4] = prefix[4..8].try_into()?;
        ensure!(
            kind.iter().all(u8::is_ascii_alphabetic) && kind[2].is_ascii_uppercase(),
            "Invalid PNG chunk type"
        );
        let end = pos
            .checked_add(12)
            .and_then(|n| n.checked_add(length))
            .context("PNG chunk overflow")?;
        let chunk = bytes.get(pos..end).context("Truncated PNG chunk")?;
        ensure!(
            crc(&chunk[4..8 + length]) == u32::from_be_bytes(chunk[8 + length..].try_into()?),
            "PNG checksum mismatch"
        );
        let payload = &chunk[8..8 + length];
        if !header {
            ensure!(kind == b"IHDR" && length == 13, "PNG must begin with IHDR");
            ensure!(
                payload[..4] != [0; 4] && payload[4..8] != [0; 4],
                "Invalid PNG dimensions"
            );
            header = true;
        } else {
            ensure!(kind != b"IHDR", "Duplicate PNG IHDR");
        }
        match kind {
            b"IEND" => {
                ensure!(
                    length == 0 && image_data,
                    "Invalid PNG IEND or missing IDAT"
                );
                output.extend_from_slice(chunk);
                return Ok(output);
            }
            b"IDAT" => {
                image_data = true;
                output.extend_from_slice(chunk);
            }
            b"eXIf" => {
                ensure!(!exif, "Duplicate PNG EXIF");
                exif = true;
                if let Some(value) = orientation(payload)? {
                    png_chunk(&mut output, b"eXIf", &orientation_tiff(value, true));
                }
            }
            // Explicit rendering and animation allowlist. Unknown ancillary chunks may
            // carry private application metadata, so are intentionally discarded.
            b"cHRM" | b"gAMA" | b"iCCP" | b"sBIT" | b"sRGB" | b"bKGD" | b"hIST" | b"tRNS"
            | b"pHYs" | b"sPLT" | b"acTL" | b"fcTL" | b"fdAT" | b"cICP" | b"mDCv" | b"cLLi" => {
                output.extend_from_slice(chunk)
            }
            b"IHDR" | b"PLTE" => output.extend_from_slice(chunk),
            _ if kind[0].is_ascii_uppercase() => bail!("Unknown critical PNG chunk"),
            _ => {}
        }
        pos = end;
    }
}

fn png_chunk(output: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    output.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    let start = output.len();
    output.extend_from_slice(kind);
    output.extend_from_slice(payload);
    let checksum = crc(&output[start..]);
    output.extend_from_slice(&checksum.to_be_bytes());
}

// PNG's IEEE CRC-32, accelerated without decoding any pixel data.
fn crc(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0xff, marker];
        v.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(payload);
        v
    }
    fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = (payload.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v.extend_from_slice(&crc(&v[4..]).to_be_bytes());
        v
    }
    #[test]
    fn jpeg_preserves_progressive_entropy_and_rendering_segments() {
        let mut expected = vec![0xff, 0xd8];
        expected.extend(segment(0xe2, b"ICC_PROFILE\0\x01\x01profile"));
        expected.extend(segment(0xee, b"Adobe\0rendering"));
        expected.extend(segment(0xda, b"scan"));
        expected.extend([1, 0xff, 0, 2, 0xff, 0xd0, 3, 0xff, 0x01, 4]);
        expected.extend(segment(0xc4, b"table"));
        expected.extend(segment(0xda, b"scan2"));
        expected.extend([4, 0xff, 0, 5, 0xff, 0xd9]);
        let mut input = vec![0xff, 0xd8];
        input.extend(segment(0xe1, b"Exif\0\0II\x2a\0\x08\0\0\0\0\0\0\0\0\0"));
        input.extend(segment(0xe2, b"MPF\0private"));
        input.extend(segment(0xfe, b"private"));
        input.extend_from_slice(&expected[2..]);
        input.extend(b"private movie trailer");
        assert_eq!(clean(&input).unwrap(), expected);
    }
    #[test]
    fn orientation_rebuilt_without_private_fields() {
        for little in [true, false] {
            let exif = orientation_tiff(6, little);
            let mut input = vec![0xff, 0xd8];
            let mut payload = b"Exif\0\0".to_vec();
            payload.extend(exif);
            payload.extend(b"private GPS thumbnail");
            input.extend(segment(0xe1, &payload));
            input.extend([0xff, 0xd9]);
            let output = clean(&input).unwrap();
            assert!(!output.windows(7).any(|w| w == b"private"));
            assert_eq!(orientation(&output[12..output.len() - 2]).unwrap(), Some(6));
        }
    }
    #[test]
    fn png_preserves_render_chunks_and_removes_text_and_trailer() {
        let mut expected = PNG.to_vec();
        expected.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]));
        expected.extend(chunk(b"iCCP", b"render profile"));
        expected.extend(chunk(b"IDAT", b"compressed bytes"));
        expected.extend(chunk(b"IEND", b""));
        let mut input = expected[..expected.len() - 12].to_vec();
        input.extend(chunk(b"tEXt", b"private"));
        input.extend(chunk(b"eXIf", &orientation_tiff(8, true)));
        input.extend(chunk(b"IEND", b""));
        input.extend(b"private trailer");
        let output = clean(&input).unwrap();
        assert!(output.windows(16).any(|w| w == b"compressed bytes"));
        assert!(!output.windows(7).any(|w| w == b"private"));
        assert!(output.ends_with(&chunk(b"IEND", b"")));
        assert!(output.windows(4).any(|w| w == b"eXIf"));
    }
    #[test]
    fn jfif_thumbnail_removed_and_dnl_scan_continues() {
        let mut jfif = b"JFIF\0\x01\x02\0\0\x01\0\x01\x01\x01".to_vec();
        jfif.extend([10, 20, 30]);
        let mut input = vec![0xff, 0xd8];
        input.extend(segment(0xe0, &jfif));
        input.extend(segment(0xda, b"scan"));
        input.extend([1, 2]);
        input.extend(segment(0xdc, &[0, 1]));
        input.extend([3, 4, 0xff, 0xd9]);
        let mut expected = vec![0xff, 0xd8];
        jfif.truncate(14);
        jfif[12] = 0;
        jfif[13] = 0;
        expected.extend(segment(0xe0, &jfif));
        expected.extend_from_slice(&input[23..]);
        assert_eq!(clean(&input).unwrap(), expected);
    }
    #[test]
    fn png_animation_rendering_and_idat_are_byte_identical() {
        let mut input = PNG.to_vec();
        input.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]));
        for kind in [b"acTL", b"fcTL", b"tRNS", b"cICP", b"IDAT", b"fdAT"] {
            input.extend(chunk(kind, b"exact original bytes"));
        }
        input.extend(chunk(b"IEND", b""));
        assert_eq!(clean(&input).unwrap(), input);
        assert_eq!(crc(b"123456789"), 0xcbf4_3926);
    }
    #[test]
    fn corrupt_exif_offsets_and_orientation_are_rejected() {
        let mut tiff = orientation_tiff(6, true);
        tiff[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(orientation(&tiff).is_err());
        let mut tiff = orientation_tiff(6, false);
        tiff[14..18].copy_from_slice(&2u32.to_be_bytes());
        assert!(orientation(&tiff).is_err());
        assert!(orientation(&orientation_tiff(9, true)).is_err());
        let valid = orientation_tiff(6, true);
        for end in 0..valid.len() {
            assert!(orientation(&valid[..end]).is_err());
        }
    }
    #[test]
    fn rejects_unknown_critical_png_and_duplicate_jpeg_orientation() {
        let mut png = PNG.to_vec();
        png.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]));
        png.extend(chunk(b"ABCD", b"unknown rendering"));
        png.extend(chunk(b"IDAT", b"data"));
        png.extend(chunk(b"IEND", b""));
        assert!(clean(&png).is_err());
        let mut jpeg = vec![0xff, 0xd8];
        let mut exif = b"Exif\0\0".to_vec();
        exif.extend(orientation_tiff(6, true));
        jpeg.extend(segment(0xe1, &exif));
        jpeg.extend(segment(0xe1, &exif));
        jpeg.extend([0xff, 0xd9]);
        assert!(clean(&jpeg).is_err());
    }
    #[test]
    fn repeated_cleaning_is_idempotent_and_multi_part_icc_survives() {
        let mut jpeg = vec![0xff, 0xd8];
        jpeg.extend(segment(0xe2, b"ICC_PROFILE\0\x01\x02first"));
        jpeg.extend(segment(0xe2, b"ICC_PROFILE\0\x02\x02second"));
        jpeg.extend(segment(0xfe, b"private"));
        jpeg.extend([0xff, 0xd9]);
        let first = clean(&jpeg).unwrap();
        assert_eq!(clean(&first).unwrap(), first);
        assert!(first.windows(5).any(|w| w == b"first"));
        assert!(first.windows(6).any(|w| w == b"second"));
        let mut png = PNG.to_vec();
        png.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]));
        png.extend(chunk(b"eXIf", &orientation_tiff(8, false)));
        png.extend(chunk(b"tEXt", b"private"));
        png.extend(chunk(b"IDAT", b"data"));
        png.extend(chunk(b"IEND", b""));
        let first = clean(&png).unwrap();
        assert_eq!(clean(&first).unwrap(), first);
    }
    #[test]
    fn malformed_recognized_files_fail_and_unknown_passes() {
        assert!(clean(&[0xff, 0xd8, 0xff, 0xe1, 0xff, 0xff]).is_err());
        assert!(clean(&[0xff, 0xd8, 0xff, 0xda, 0, 2, 1]).is_err());
        let mut png = PNG.to_vec();
        png.extend(chunk(b"IHDR", &[0; 13]));
        png[20] ^= 1;
        assert!(clean(&png).is_err());
        assert!(clean(PNG).is_err());
        assert_eq!(clean(b"unsupported").unwrap(), b"unsupported");
    }
}
