use std::{borrow::Cow, convert::TryFrom, ops::Range};

use gltf::json::{self, validation::USize64};
use serde::Deserialize;
use vtx_meshopt_decode::ffi;

use super::Result;

const EXT: &str = "EXT_meshopt_compression";
const KHR: &str = "KHR_meshopt_compression";
const MAX_DECODED_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum Mode { Attributes, Triangles, Indices }

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum Filter {
    #[default]
    None,
    Octahedral,
    Quaternion,
    Exponential,
    Color,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    buffer: usize,
    #[serde(default)]
    byte_offset: usize,
    byte_length: usize,
    byte_stride: usize,
    count: usize,
    mode: Mode,
    #[serde(default)]
    filter: Filter,
}

struct Job {
    source: usize,
    range: Range<usize>,
    stride: usize,
    count: usize,
    size: usize,
    mode: Mode,
    filter: Filter,
}

impl Descriptor {
    fn job(self, view: &json::buffer::View, buffers: &[json::buffer::Buffer], khr: bool) -> Result<Job> {
        let size = self.count.checked_mul(self.byte_stride).ok_or("meshopt output size overflow")?;
        if size == 0 || self.byte_length == 0 || size as u64 != view.byte_length.0 ||
            view.byte_stride.is_some_and(|stride| stride.0 != self.byte_stride)
        {
            return Err("Invalid meshopt buffer view layout".into());
        }
        let valid = match self.mode {
            Mode::Attributes => (4..=256).contains(&self.byte_stride) && self.byte_stride % 4 == 0 &&
                match self.filter {
                    Filter::None | Filter::Exponential => true,
                    Filter::Octahedral => matches!(self.byte_stride, 4 | 8),
                    Filter::Quaternion => self.byte_stride == 8,
                    Filter::Color => khr && matches!(self.byte_stride, 4 | 8),
                },
            Mode::Triangles | Mode::Indices => matches!(self.byte_stride, 2 | 4) &&
                matches!(self.filter, Filter::None) &&
                (!matches!(self.mode, Mode::Triangles) || self.count % 3 == 0),
        };
        if !valid { return Err("Invalid meshopt mode, stride, or filter".into()); }

        let source = buffers.get(self.buffer).ok_or("Invalid meshopt source buffer")?;
        let end = self.byte_offset.checked_add(self.byte_length).ok_or("meshopt input range overflow")?;
        if end as u64 > source.byte_length.0 { return Err("meshopt input exceeds its source buffer".into()); }
        let fallback = buffers.get(view.buffer.value()).ok_or("Invalid meshopt fallback buffer")?;
        let start = view.byte_offset.map_or(0, |offset| offset.0);
        if start.checked_add(view.byte_length.0).is_none_or(|end| end > fallback.byte_length.0) {
            return Err("meshopt output exceeds its declared fallback buffer".into());
        }
        Ok(Job { source: self.buffer, range: self.byte_offset..end, stride: self.byte_stride,
                 count: self.count, size, mode: self.mode, filter: self.filter })
    }
}

pub struct Plan {
    pub original_buffers: usize,
    pub decoded_bytes: usize,
    pub jobs_count: usize,
    unused: Vec<bool>,
    jobs: Vec<Job>,
}

impl Plan {
    pub fn prepare(root: &mut json::Root) -> Result<Self> {
        let original_buffers = root.buffers.len();
        let mut jobs = Vec::new();
        let mut decoded_bytes = 0usize;
        for view in &mut root.buffer_views {
            let Some(extensions) = &mut view.extensions else { continue; };
            let ext = extensions.others.remove(EXT);
            let khr = extensions.others.remove(KHR);
            let (descriptor, khr) = match (ext, khr) {
                (Some(ext), None) => (ext, false),
                (None, Some(khr)) => (khr, true),
                (None, None) => continue,
                _ => return Err("glTF view declares both meshopt extensions".into()),
            };
            let descriptor: Descriptor = serde_json::from_value(descriptor)?;
            let job = descriptor.job(view, &root.buffers[..original_buffers], khr)?;
            decoded_bytes = decoded_bytes.checked_add(job.size).ok_or("meshopt output size overflow")?;
            if decoded_bytes > MAX_DECODED_BYTES { return Err("meshopt output exceeds 512 MiB".into()); }
            view.buffer = json::Index::new(u32::try_from(root.buffers.len())?);
            view.byte_offset = None;
            root.buffers.push(json::buffer::Buffer {
                byte_length: USize64(job.size as u64), uri: None, extensions: None, extras: Default::default(),
            });
            jobs.push(job);
        }
        root.extensions_required.retain(|extension| extension != EXT && extension != KHR);

        // Replaced views use appended buffers. An original buffer is needed only
        // for ordinary views or as a source of compressed bytes.
        let mut unused = vec![!jobs.is_empty(); original_buffers];
        for view in &root.buffer_views {
            if let Some(slot) = unused.get_mut(view.buffer.value()) { *slot = false; }
        }
        for job in &jobs { unused[job.source] = false; }
        for (buffer, unused) in root.buffers.iter().zip(&unused) {
            let fallback = buffer.extensions.as_ref().is_some_and(|extensions|
                [EXT, KHR].iter().any(|&name| extensions.others.get(name)
                    .and_then(|value| value.get("fallback")).and_then(|value| value.as_bool()) == Some(true)));
            if fallback && !unused { return Err("meshopt fallback is also used as a data source".into()); }
        }
        Ok(Self { original_buffers, decoded_bytes, jobs_count: jobs.len(), unused, jobs })
    }

    pub fn unused(&self, buffer: usize) -> bool { self.unused[buffer] }

    pub fn decode(&self, buffers: &[Cow<[u8]>]) -> Result<Vec<Decoded>> {
        #[cfg(feature = "rayon")]
        if self.jobs.len() > 1 && self.decoded_bytes >= 2 * 1024 * 1024 {
            use rayon::prelude::*;
            return Ok(self.jobs.par_iter().map(|job| job.decode(buffers))
                .collect::<std::result::Result<_, _>>()?);
        }
        Ok(self.jobs.iter().map(|job| job.decode(buffers)).collect::<std::result::Result<_, _>>()?)
    }
}

pub struct Decoded {
    words: Vec<u32>,
    size: usize,
}

impl Decoded {
    pub fn as_bytes(&self) -> &[u8] { &bytemuck::cast_slice(&self.words)[..self.size] }
}

impl Job {
    fn decode(&self, buffers: &[Cow<[u8]>]) -> std::result::Result<Decoded, &'static str> {
        if !cfg!(target_endian = "little") { return Err("meshopt decoding requires a little-endian host"); }
        let source = buffers.get(self.source).and_then(|buffer| buffer.get(self.range.clone()))
            .ok_or("Truncated meshopt input")?;
        // The filters operate on aligned 16- or 32-bit values.
        let mut words = vec![0u32; self.size.div_ceil(4)];
        let output = words.as_mut_ptr().cast();
        // SAFETY: the descriptor validates codec and filter constraints. The
        // source range is checked and the aligned destination holds count * stride bytes.
        unsafe {
            let status = match self.mode {
                Mode::Attributes => ffi::meshopt_decodeVertexBuffer(output, self.count, self.stride, source.as_ptr(), source.len()),
                Mode::Triangles => ffi::meshopt_decodeIndexBuffer(output, self.count, self.stride, source.as_ptr(), source.len()),
                Mode::Indices => ffi::meshopt_decodeIndexSequence(output, self.count, self.stride, source.as_ptr(), source.len()),
            };
            if status != 0 { return Err("Invalid meshopt compressed data"); }
            match self.filter {
                Filter::None => {}
                Filter::Octahedral => ffi::meshopt_decodeFilterOct(output, self.count, self.stride),
                Filter::Quaternion => ffi::meshopt_decodeFilterQuat(output, self.count, self.stride),
                Filter::Exponential => ffi::meshopt_decodeFilterExp(output, self.count, self.stride),
                Filter::Color => ffi::meshopt_decodeFilterColor(output, self.count, self.stride),
            }
        }
        Ok(Decoded { words, size: self.size })
    }
}
