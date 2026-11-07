use std::borrow::Cow;

use base64_simd::STANDARD;
use memchr::{memchr, memmem};
use serde::Deserialize;
use serde_json::value::RawValue;

const MIN_DOCUMENT: usize = 1024 * 1024;
const MIN_PAYLOAD: usize = 64 * 1024;

#[derive(Deserialize)]
struct Resources<'a> {
    #[serde(default, borrow)]
    buffers: Vec<Buffer<'a>>,
}

#[derive(Deserialize)]
struct Buffer<'a> {
    #[serde(borrow)]
    uri: Option<&'a RawValue>,
}

pub fn base64_payload(uri: &str) -> Option<&str> {
    let (header, payload) = uri.strip_prefix("data:")?.split_once(',')?;
    header.ends_with(";base64").then_some(payload)
}

pub fn decode(payload: &str) -> super::Result<Vec<u8>> {
    #[cfg(feature = "rayon")]
    if payload.len() >= 16 * 1024 * 1024 && payload.len() % 4 == 0 {
        use base64_simd::AsOut;
        use rayon::prelude::*;

        // Split on quartet boundaries and allow a short decode only in the final block.
        const INPUT_BLOCK: usize = 4 * 1024 * 1024;
        const OUTPUT_BLOCK: usize = INPUT_BLOCK / 4 * 3;
        let payload = payload.as_bytes();
        let padding = usize::from(payload.ends_with(b"=")) + usize::from(payload.ends_with(b"=="));
        let mut output = vec![0; payload.len() / 4 * 3 - padding];
        payload.par_chunks(INPUT_BLOCK).zip(output.par_chunks_mut(OUTPUT_BLOCK))
            .try_for_each(|(input, output)| -> Result<(), &'static str> {
                let written = STANDARD.decode(input, output.as_out()).map_err(|_| "Invalid glTF base64")?.len();
                if written != output.len() { return Err("Unexpected padding inside glTF base64"); }
                Ok(())
            })?;
        return Ok(output);
    }
    Ok(STANDARD.decode_to_vec(payload)?)
}

fn borrowed_payload(uri: &RawValue) -> Option<&str> {
    let uri = uri.get().strip_prefix('"')?.strip_suffix('"')?;
    let payload = base64_payload(uri)?;
    if payload.len() < MIN_PAYLOAD || memchr(b'\\', uri.as_bytes()).is_some() {
        return None;
    }
    Some(payload)
}

/// Keep the complete JSON structure for glTF validation, but leave large,
/// literal base64 payloads in the original byte slice. Escaped strings use
/// the ordinary glTF deserializer so its JSON unescaping still applies.
pub fn prepare(json: &[u8]) -> (Cow<'_, [u8]>, Vec<Option<&str>>) {
    let original = || (Cow::Borrowed(json), Vec::new());
    if json.len() < MIN_DOCUMENT || memmem::find(json, b"\"data:").is_none() {
        return original();
    }
    let Ok(resources) = serde_json::from_slice::<Resources>(json) else { return original(); };
    let payloads: Vec<_> = resources.buffers.iter()
        .map(|buffer| buffer.uri.and_then(borrowed_payload)).collect();
    let ranges: Vec<_> = payloads.iter().flatten().map(|payload| {
        // RawValue borrows its token from json, and payload is a slice of that token.
        let start = payload.as_ptr() as usize - json.as_ptr() as usize;
        start..start + payload.len()
    }).collect();
    if ranges.is_empty() { return original(); }

    // The borrowed buffer URI tokens are in their original JSON array order.
    let removed = ranges.iter().map(|range| range.len()).sum::<usize>();
    let mut compact = Vec::with_capacity(json.len() - removed);
    let mut copied = 0;
    for range in ranges {
        compact.extend_from_slice(&json[copied..range.start]);
        copied = range.end;
    }
    compact.extend_from_slice(&json[copied..]);
    (Cow::Owned(compact), payloads)
}
