//! Bounded saved-image fallback. This never claims to decode CAD geometry.
use super::Result;
use std::{convert::TryInto, fs, io::{Read, Cursor}, path::Path};
const MAX_IMAGE: u64 = 16 * 1024 * 1024;

pub fn load(path: &Path) -> Result<Vec<u8>> {
    if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("f3d")) {
        return fusion(path);
    }
    let mut signature = [0; 8];
    let ole = fs::File::open(path)?.read_exact(&mut signature).is_ok()
        && signature == [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
    // OLE previews have an indexed stream. Avoid scanning the whole legacy
    // drawing for the unrelated modern compressed-stream header first.
    if !ole {
        if let Ok(bytes) = crate::solidworks::saved_preview_bytes(path) { return Ok(bytes); }
    }
    let mut file = cfb::open(path).map_err(|_| "This drawing has no readable saved preview. Native drawing geometry is not supported.")?;
    for name in ["/PreviewPNG", "/Preview"] {
        if let Ok(mut stream) = file.open_stream(name) {
            let mut bytes = Vec::new();
            stream.by_ref().take(MAX_IMAGE + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_IMAGE { continue; }
            if image::guess_format(&bytes).is_ok() { return Ok(bytes); }
            // Legacy Preview streams can contain a headerless Windows DIB.
            if bytes.len() >= 40 {
                let header = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
                if [40, 108, 124].contains(&header) && bytes.len() >= header {
                    let depth = u16::from_le_bytes(bytes[14..16].try_into().unwrap());
                    let used = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
                    let palette = if depth <= 8 { if used == 0 { 1usize << depth } else { used } } else { 0 };
                    let compression = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
                    let masks = if header == 40 && compression == 3 { 12 } else { 0 };
                    let offset = 14 + header + palette.saturating_mul(4) + masks;
                    if offset <= bytes.len() + 14 {
                        let mut bmp = b"BM".to_vec();
                        bmp.extend_from_slice(&((bytes.len()+14) as u32).to_le_bytes());
                        bmp.extend_from_slice(&[0;4]);bmp.extend_from_slice(&(offset as u32).to_le_bytes());
                        bmp.extend(bytes);return Ok(bmp);
                    }
                }
            }
        }
    }
    Err("This drawing has no usable saved image. Save it with preview graphics enabled; full drawing import is not available.".into())
}

fn fusion(path: &Path) -> Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(fs::File::open(path)?)?;
    if archive.len() > 20_000 { return Err("Fusion archive contains too many entries".into()); }
    let mut candidates = Vec::new();
    for i in 0..archive.len() {
        let item=archive.by_index(i)?;let name=item.name()?.to_ascii_lowercase();
        if !(name.ends_with(".png") || name.ends_with(".jpg") || name.ends_with(".jpeg")) ||
            !["preview", "thumbnail"].iter().any(|s| name.contains(s)) ||
            item.size()==0 || item.size()>MAX_IMAGE || item.compressed_size()>MAX_IMAGE { continue; }
        let rank = i32::from(name.contains("fusionassetname"))*8 + i32::from(name.contains("[active]"))*4 +
            i32::from(name.contains("large"))*2 - i32::from(name.contains("toolpath"))*8;
        candidates.push((rank,item.size(),i));
    }
    candidates.sort_unstable_by(|a,b|b.cmp(a));
    for (_,_,i) in candidates {
        let mut bytes=Vec::new();archive.by_index(i)?.take(MAX_IMAGE+1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 <= MAX_IMAGE && dimensions(&bytes).is_ok() { return Ok(bytes); }
    }
    Err("This Fusion archive has no usable saved preview. Its proprietary 3D geometry requires an additional translator.".into())
}

pub fn dimensions(bytes: &[u8]) -> Result<[u32;2]> {
    let (w,h)=image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?.into_dimensions()?;
    if w==0 || h==0 || w>8192 || h>8192 || u64::from(w)*u64::from(h)>16_000_000 {
        return Err("Saved image dimensions exceed the display limit".into());
    }
    Ok([w,h])
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn rejects_invalid_and_oversized_saved_images() {
        assert!(dimensions(b"not an image").is_err());
        let mut png=b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&9000u32.to_be_bytes());png.extend_from_slice(&9000u32.to_be_bytes());
        assert!(dimensions(&png).is_err());
    }
}
