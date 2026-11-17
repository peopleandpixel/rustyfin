//! The key frames of a Matroska file's first video track, from its Cues index, for trickplay's
//! seek mode: which key frame a seek to a time lands on, and that frame's bytes, each with a
//! few ranged reads.
//!
//! A cue point gives a key frame's time, its cluster's position and (as mkvmerge and ffmpeg
//! write it) the block's position inside the cluster. ffmpeg's backward seek lands on the last
//! cue at or before the time asked for, so `MkvIndex::pick` does the same; past the last cue
//! ffmpeg reads on through the file instead, so there `pick` gives nothing. Where the cue points
//! lack the block's position (older mkvmerge), the cluster is read from its start up to the
//! video block with the cue's time, which in most such files is the cluster's first. Either
//! way the block found must carry the cue's time.
//!
//! A file whose track is compressed (header stripping), with fewer than two cue points (ffmpeg
//! ignores such an index), or that can't be read as expected within the limits below gives no
//! index, and a frame that can't be found an `INDEX_FAILED`; the caller then lets ffprobe find
//! the frames. Nothing here reads, scans or keeps more than a fixed amount.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

/// Why a key frame couldn't be read through the index. The tile then reads it with ffprobe.
pub(crate) const INDEX_FAILED: &str = "the Matroska index led to no key frame";

const EBML: u64 = 0x1A45_DFA3;
const SEGMENT: u64 = 0x1853_8067;
const SEEK_HEAD: u64 = 0x114D_9B74;
const SEEK: u64 = 0x4DBB;
const SEEK_ID: u64 = 0x53AB;
const SEEK_POSITION: u64 = 0x53AC;
const INFO: u64 = 0x1549_A966;
const TIMESTAMP_SCALE: u64 = 0x2A_D7B1;
const TRACKS: u64 = 0x1654_AE6B;
const TRACK_ENTRY: u64 = 0xAE;
const TRACK_NUMBER: u64 = 0xD7;
const TRACK_TYPE: u64 = 0x83;
const CONTENT_ENCODINGS: u64 = 0x6D80;
const CUES: u64 = 0x1C53_BB6B;
const CUE_POINT: u64 = 0xBB;
const CUE_TIME: u64 = 0xB3;
const CUE_TRACK_POSITIONS: u64 = 0xB7;
const CUE_TRACK: u64 = 0xF7;
const CUE_CLUSTER_POSITION: u64 = 0xF1;
const CUE_RELATIVE_POSITION: u64 = 0xF0;
const CLUSTER: u64 = 0x1F43_B675;
const CLUSTER_TIMESTAMP: u64 = 0xE7;
const CRC_32: u64 = 0xBF;
const VOID: u64 = 0xEC;
const SIMPLE_BLOCK: u64 = 0xA3;
const BLOCK_GROUP: u64 = 0xA0;
const BLOCK: u64 = 0xA1;

/// The most a header element's body (SeekHead, Info, Tracks, Cues) may take.
const HEADER_MOST: u64 = 16 << 20;
/// The most a key frame may take.
const FRAME_MOST: u64 = 64 << 20;
/// The top-level elements read before the first cluster, and the seek heads followed.
const TOP_LEVEL_MOST: usize = 64;
const SEEK_HEADS_MOST: usize = 4;
/// The cue points kept: a 3-hour film with a key frame a second has about 11,000.
const CUES_MOST: usize = 1 << 20;
/// How far into a cluster `find_block` reads, and how many of its elements it looks at.
const SCAN_MOST: u64 = 4 << 20;
const SCAN_ELEMENTS_MOST: usize = 1 << 16;
/// What `read_frame` reads at a block first: its header and the frame's start.
const CHUNK: u64 = 32 << 10;

/// One cue point of the video track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cue {
    /// The key frame's time, in seconds.
    pub(crate) seconds: f64,
    /// The same in the file's timestamp units, as its blocks give it.
    time: u64,
    /// Where its cluster starts in the file.
    cluster: u64,
    /// Where its block starts in the cluster's body, where the cue says.
    relative: Option<u64>,
}

/// The cue points of a Matroska file's first video track, in time order.
#[derive(Debug)]
pub(crate) struct MkvIndex {
    track: u64,
    /// Nanoseconds per timestamp unit.
    scale: u64,
    cues: Vec<Cue>,
    /// How many frames `frame` read, for tests to see the index was used.
    #[cfg(test)]
    pub(crate) frames_read: std::sync::atomic::AtomicUsize,
}

/// An element: its id, where its body starts, and its body's size (`None`: unknown, to the end
/// of its parent).
#[derive(Clone, Copy, Debug)]
struct Element {
    id: u64,
    body: u64,
    size: Option<u64>,
}

impl Element {
    /// Where the element ends, if its size is known.
    fn end(&self) -> Option<u64> {
        self.body.checked_add(self.size?)
    }
}

/// An EBML number at the start of `data`: its value (the length marker kept, as for an id, when
/// `marker`) and its length in bytes. A size of all ones (unknown) is `u64::MAX`.
fn number(data: &[u8], marker: bool) -> Option<(u64, usize)> {
    let first = *data.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    let rest = data.get(1..len)?;
    let mut value = if marker {
        u64::from(first)
    } else {
        u64::from(first) & (0xff >> len)
    };
    let mut all_ones = value == 0xff >> len;
    for b in rest {
        value = value << 8 | u64::from(*b);
        all_ones &= *b == 0xff;
    }
    Some((if !marker && all_ones { u64::MAX } else { value }, len))
}

/// The element whose header is at the start of `data`, which sits at `at` in the file.
fn element(data: &[u8], at: u64) -> Option<Element> {
    let (id, id_len) = number(data, true)?;
    let (size, size_len) = number(data.get(id_len..)?, false)?;
    Some(Element {
        id,
        body: at.checked_add((id_len + size_len) as u64)?,
        size: (size != u64::MAX).then_some(size),
    })
}

/// The children of an element whose body is `body`, one at a time, up to the first that
/// can't be read or runs past the body.
fn children(body: &[u8]) -> impl Iterator<Item = (Element, &[u8])> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let el = element(body.get(at..)?, at as u64)?;
        let start = usize::try_from(el.body).ok()?;
        let end = start.checked_add(usize::try_from(el.size?).ok()?)?;
        let data = body.get(start..end)?;
        at = end;
        Some((el, data))
    })
}

/// The body of the first child `id` of `body`.
fn child(body: &[u8], id: u64) -> Option<&[u8]> {
    children(body).find(|(e, _)| e.id == id).map(|(_, d)| d)
}

fn uint(data: &[u8]) -> Option<u64> {
    (data.len() <= 8).then(|| data.iter().fold(0u64, |v, b| v << 8 | u64::from(*b)))
}

/// Reads `len` bytes at `at`, at most `most`.
fn read(file: &File, at: u64, len: u64, most: u64) -> Option<Vec<u8>> {
    if len > most {
        return None;
    }
    let mut buf = vec![0; usize::try_from(len).ok()?];
    file.read_exact_at(&mut buf, at).ok()?;
    Some(buf)
}

/// Reads up to `len` bytes at `at`, fewer at the end of the file.
fn read_some(file: &File, at: u64, len: u64, file_len: u64) -> Option<Vec<u8>> {
    read(file, at, len.min(file_len.checked_sub(at)?), len)
}

/// The element header at `at`.
fn header(file: &File, at: u64, file_len: u64) -> Option<Element> {
    element(&read_some(file, at, 16, file_len)?, at)
}

/// The body of header element `el`, read whole.
fn body(file: &File, el: Element) -> Option<Vec<u8>> {
    read(file, el.body, el.size?, HEADER_MOST)
}

impl MkvIndex {
    /// The index of the Matroska file at `path`, read with a few ranged reads. Blocking.
    pub(crate) fn read(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        let file_len = file.metadata().ok()?.len();
        let ebml = header(&file, 0, file_len).filter(|e| e.id == EBML)?;
        let segment = header(&file, ebml.end()?, file_len).filter(|e| e.id == SEGMENT)?;
        let segment_end = segment.end().map_or(file_len, |end| end.min(file_len));
        // The top-level elements up to the first cluster, and those a seek head points at
        let (mut info, mut tracks, mut cues) = (None, None, None);
        let mut seek_heads = Vec::new();
        let mut at = segment.body;
        for _ in 0..TOP_LEVEL_MOST {
            if at >= segment_end {
                break;
            }
            let el = header(&file, at, file_len)?;
            match el.id {
                INFO => info = Some(el),
                TRACKS => tracks = Some(el),
                CUES => cues = Some(el),
                SEEK_HEAD => seek_heads.push(el),
                CLUSTER => break,
                _ => {}
            }
            at = el.end()?;
        }
        let mut followed = 0;
        while cues.is_none() || info.is_none() || tracks.is_none() {
            // A seek head may name another, at the end of the file; follow a few
            let head = *seek_heads.get(followed)?;
            followed += 1;
            if followed > SEEK_HEADS_MOST {
                return None;
            }
            let entries = body(&file, head)?;
            for (seek, entry) in children(&entries).take(TOP_LEVEL_MOST) {
                if seek.id != SEEK {
                    continue;
                }
                let (Some(id), Some(position)) =
                    (child(entry, SEEK_ID), child(entry, SEEK_POSITION))
                else {
                    continue;
                };
                let at = segment.body.checked_add(uint(position)?)?;
                let target = match uint(id)? {
                    INFO => &mut info,
                    TRACKS => &mut tracks,
                    CUES => &mut cues,
                    SEEK_HEAD => {
                        seek_heads.push(header(&file, at, file_len)?);
                        continue;
                    }
                    _ => continue,
                };
                if target.is_none() {
                    *target = Some(header(&file, at, file_len)?);
                }
            }
        }
        let (info, tracks, cues) = (info?, tracks?, cues?);
        if info.id != INFO || tracks.id != TRACKS || cues.id != CUES {
            return None;
        }
        let scale = child(&body(&file, info)?, TIMESTAMP_SCALE)
            .map_or(Some(1_000_000), uint)
            .filter(|&s| s > 0)?;
        let track = Self::video_track(&body(&file, tracks)?)?;
        let cues = Self::cue_points(&body(&file, cues)?, track, scale, segment.body)?;
        Some(Self {
            track,
            scale,
            cues,
            #[cfg(test)]
            frames_read: Default::default(),
        })
    }

    /// The number of the first video track, which ffmpeg's `V:0` is; `None` if it is
    /// compressed.
    fn video_track(tracks: &[u8]) -> Option<u64> {
        for (entry, fields) in children(tracks) {
            if entry.id != TRACK_ENTRY || child(fields, TRACK_TYPE).and_then(uint) != Some(1) {
                continue;
            }
            if child(fields, CONTENT_ENCODINGS).is_some() {
                return None;
            }
            return child(fields, TRACK_NUMBER).and_then(uint);
        }
        None
    }

    /// The cue points of `track`, in time order; `None` for fewer than two, or too many.
    fn cue_points(cues: &[u8], track: u64, scale: u64, segment: u64) -> Option<Vec<Cue>> {
        let mut out = Vec::new();
        for (point, fields) in children(cues) {
            if point.id != CUE_POINT {
                continue;
            }
            let time = child(fields, CUE_TIME).and_then(uint)?;
            for (positions, data) in children(fields) {
                if positions.id != CUE_TRACK_POSITIONS
                    || child(data, CUE_TRACK).and_then(uint) != Some(track)
                {
                    continue;
                }
                if out.len() == CUES_MOST {
                    return None;
                }
                let cluster = child(data, CUE_CLUSTER_POSITION).and_then(uint)?;
                out.push(Cue {
                    seconds: time as f64 * scale as f64 / 1e9,
                    time,
                    cluster: segment.checked_add(cluster)?,
                    relative: child(data, CUE_RELATIVE_POSITION).and_then(uint),
                });
            }
        }
        out.sort_by_key(|c| c.time);
        (out.len() >= 2).then_some(out)
    }

    /// The key frame ffmpeg's backward seek to `seconds` lands on: the last cue at or before
    /// it, in the file's timestamp units, which ffmpeg rounds the time to (from microseconds,
    /// to the nearest). `None` before the first cue and after the last, where ffmpeg doesn't go
    /// by the index.
    pub(crate) fn pick(&self, seconds: f64) -> Option<Cue> {
        if !seconds.is_finite() || seconds < 0.0 {
            return None;
        }
        let micros = (seconds * 1e6).round() as u128;
        let scale = u128::from(self.scale);
        let units = (micros * 1000 + scale / 2) / scale;
        let after = self.cues.partition_point(|c| u128::from(c.time) <= units);
        let last = self.cues.last()?;
        if after == 0 || (after == self.cues.len() && units > u128::from(last.time)) {
            return None;
        }
        Some(self.cues[after - 1])
    }

    /// The bytes of the key frame at `cue`, as ffmpeg's packet holds them. Blocking. Fails with
    /// `INDEX_FAILED` where the cue leads to no unlaced key frame block of the track at its
    /// time.
    pub(crate) fn frame(&self, file: &File, cue: Cue) -> Result<Vec<u8>, String> {
        let frame = self
            .read_frame(file, cue)
            .ok_or_else(|| format!("{INDEX_FAILED} at {:.3} s", cue.seconds));
        #[cfg(test)]
        if frame.is_ok() {
            self.frames_read
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        frame
    }

    /// Two ranged reads: the cluster's header and timestamp, then from the block on, the first
    /// `CHUNK` bytes holding its header and the frame's start; the rest of a larger frame
    /// follows on from there, which a share's read-ahead serves without another seek. Without
    /// the block's position, the cluster is read on from its start instead (`find_block`).
    fn read_frame(&self, file: &File, cue: Cue) -> Option<Vec<u8>> {
        let file_len = file.metadata().ok()?.len();
        // `chunk` holds the file from `at`, the block, on
        let (at, chunk) = match cue.relative {
            Some(relative) => {
                let head = read_some(file, cue.cluster, 64, file_len)?;
                let cluster = element(&head, cue.cluster).filter(|e| e.id == CLUSTER)?;
                // Muxers write the cluster's timestamp first, after a CRC-32 (ffmpeg) or Void
                let mut child_at = cluster.body;
                let stamp = loop {
                    let offset = usize::try_from(child_at - cue.cluster).ok()?;
                    let child = element(head.get(offset..)?, child_at)?;
                    match child.id {
                        CLUSTER_TIMESTAMP => break child,
                        CRC_32 | VOID => child_at = child.end()?,
                        _ => return None,
                    }
                };
                let start = usize::try_from(stamp.body - cue.cluster).ok()?;
                let end = start.checked_add(usize::try_from(stamp.size?).ok()?)?;
                let cluster_time = uint(head.get(start..end)?)?;
                let at = cluster.body.checked_add(relative)?;
                let chunk = read_some(file, at, CHUNK, file_len)?;
                let (_, time) = self.block(&chunk, at, cluster_time)?;
                (time == cue.time).then_some((at, chunk))?
            }
            None => self.find_block(file, cue, file_len)?,
        };
        let (block, _) = self.block(&chunk, at, 0)?;
        let body = usize::try_from(block.body - at).ok()?;
        let (track, track_len) = number(chunk.get(body..)?, false)?;
        let flags = *chunk.get(body + track_len + 2)?;
        let simple = chunk
            .first()
            .is_some_and(|&id| u64::from(id) == SIMPLE_BLOCK);
        // The track's, unlaced, and (for a SimpleBlock, which says) a key frame
        if track != self.track || flags & 0x06 != 0 || (simple && flags & 0x80 == 0) {
            return None;
        }
        let start = body + track_len + 3;
        let len = block.size?.checked_sub(track_len as u64 + 3)?;
        if len > FRAME_MOST {
            return None;
        }
        let len = usize::try_from(len).ok()?;
        let mut frame = chunk.get(start..)?.to_vec();
        frame.truncate(len);
        if frame.len() < len {
            let from = at.checked_add(chunk.len() as u64)?;
            let rest = (len - frame.len()) as u64;
            // A size that runs past the end of the file is refused before anything is allocated
            if from.checked_add(rest)? > file_len {
                return None;
            }
            frame.extend(read(file, from, rest, FRAME_MOST)?);
        }
        Some(frame)
    }

    /// The Block (a SimpleBlock, or the Block among a BlockGroup's first children) whose
    /// element starts `data`, which sits at `at`, and its time in a cluster at `cluster_time`.
    /// `None` for another element or another track.
    fn block(&self, data: &[u8], at: u64, cluster_time: u64) -> Option<(Element, u64)> {
        let mut block = element(data, at)?;
        if block.id == BLOCK_GROUP {
            let group_end = block.end()?;
            let mut child_at = block.body;
            block = loop {
                let offset = usize::try_from(child_at.checked_sub(at)?).ok()?;
                let child = element(data.get(offset..)?, child_at)?;
                if child.id == BLOCK {
                    break child;
                }
                child_at = child.end()?;
                if child_at >= group_end {
                    return None;
                }
            };
        } else if block.id != SIMPLE_BLOCK {
            return None;
        }
        let head = data.get(usize::try_from(block.body - at).ok()?..)?;
        let (track, track_len) = number(head, false)?;
        let relative = i16::from_be_bytes([*head.get(track_len)?, *head.get(track_len + 1)?]);
        let time = cluster_time.checked_add_signed(i64::from(relative))?;
        (track == self.track).then_some((block, time))
    }

    /// The video block of `cue`'s cluster at its time, read on from the cluster's start, and
    /// the file from there on as far as it was read (at least its header). Reads on in one
    /// stretch, at most `SCAN_MOST` bytes and `SCAN_ELEMENTS_MOST` elements.
    fn find_block(&self, file: &File, cue: Cue, file_len: u64) -> Option<(u64, Vec<u8>)> {
        let cluster = header(file, cue.cluster, file_len).filter(|e| e.id == CLUSTER)?;
        let end = cluster
            .end()
            .unwrap_or(file_len)
            .min(file_len)
            .min(cluster.body.saturating_add(SCAN_MOST));
        // `buf` holds the file from `buf_at` on; the element looked at is at `pos` in it
        let (mut buf, mut buf_at, mut pos) = (Vec::new(), cluster.body, 0usize);
        let mut cluster_time = None;
        for _ in 0..SCAN_ELEMENTS_MOST {
            let at = buf_at.checked_add(pos as u64)?;
            if at >= end {
                return None;
            }
            // Read on until the element's header, and a block's first bytes, are in `buf`
            while pos + 32 > buf.len() {
                let from = buf_at.checked_add(buf.len() as u64)?;
                if from >= end {
                    break;
                }
                let more = if buf.is_empty() { CHUNK } else { 256 << 10 };
                buf.extend(read(file, from, more.min(end - from), more)?);
            }
            let el = element(buf.get(pos..)?, at)?;
            match el.id {
                CLUSTER_TIMESTAMP => {
                    let start = usize::try_from(el.body - buf_at).ok()?;
                    let len = usize::try_from(el.size?).ok().filter(|&l| l <= 8)?;
                    cluster_time = uint(buf.get(start..start + len)?);
                }
                SIMPLE_BLOCK | BLOCK_GROUP => {
                    if let Some((_, time)) = self.block(&buf[pos..], at, cluster_time?)
                        && time == cue.time
                    {
                        return Some((at, buf.split_off(pos)));
                    }
                }
                // A cluster of unknown size ends where the next starts
                CLUSTER => return None,
                _ => {}
            }
            let next = el.end()?;
            // Past the buffer, the reads above go on from its end, one stretch
            while next > buf_at.checked_add(buf.len() as u64)? {
                let from = buf_at + buf.len() as u64;
                if from >= end {
                    return None;
                }
                let more = (next - from).min(256 << 10).min(end - from);
                buf.extend(read(file, from, more, 256 << 10)?);
            }
            pos = usize::try_from(next - buf_at).ok()?;
            // Drop what lies behind, now and then: each byte is moved at most once
            if pos > buf.len() / 2 {
                buf.drain(..pos);
                buf_at = next;
                pos = 0;
            }
        }
        None
    }
}

#[cfg(test)]
impl MkvIndex {
    /// The same index without the blocks' positions, as older mkvmerge writes it.
    pub(crate) fn without_positions(&self) -> Self {
        Self {
            track: self.track,
            scale: self.scale,
            frames_read: Default::default(),
            cues: self
                .cues
                .iter()
                .map(|c| Cue {
                    relative: None,
                    ..*c
                })
                .collect(),
        }
    }

    /// How many cues' blocks come after another block in their cluster: the cluster's
    /// timestamp takes at most 11 bytes before the first.
    pub(crate) fn key_frames_inside_clusters(&self) -> usize {
        self.cues
            .iter()
            .filter(|c| c.relative.is_some_and(|r| r > 11))
            .count()
    }

    /// The same index with every cluster `by` bytes off, as a damaged index would be.
    pub(crate) fn shifted(&self, by: u64) -> Self {
        Self {
            track: self.track,
            scale: self.scale,
            frames_read: Default::default(),
            cues: self
                .cues
                .iter()
                .map(|c| Cue {
                    cluster: c.cluster + by,
                    ..*c
                })
                .collect(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An element: `id` as written, an 8-byte size, `body`.
    fn el(id: u64, body: &[u8]) -> Vec<u8> {
        let id_bytes = id.to_be_bytes();
        let skip = id_bytes.iter().take_while(|&&b| b == 0).count();
        let mut out = id_bytes[skip..].to_vec();
        out.extend((body.len() as u64 | 1 << 56).to_be_bytes());
        out.extend_from_slice(body);
        out
    }

    fn uint_el(id: u64, value: u64) -> Vec<u8> {
        el(id, &value.to_be_bytes())
    }

    /// A block of `track` at `relative` to its cluster's time, key where `key`, holding `data`.
    fn simple_block(track: u8, relative: i16, key: bool, data: &[u8]) -> Vec<u8> {
        let mut body = vec![0x80 | track];
        body.extend(relative.to_be_bytes());
        body.push(if key { 0x80 } else { 0 });
        body.extend_from_slice(data);
        el(SIMPLE_BLOCK, &body)
    }

    /// The frame of video key frame `k` in `matroska`.
    pub(crate) fn frame_data(k: u64) -> Vec<u8> {
        (0..300 + k * 37).map(|i| (i * 7 + k) as u8).collect()
    }

    /// A Matroska file in memory: a video track (1) and an audio track (2), clusters 2,000
    /// units apart whose first block is audio, then video key frame k (`frame_data`) at 40
    /// units in, then a non-key video frame; cues for the key frames, with the blocks'
    /// positions where `positions`. Every size takes 8 bytes, so the layout is fixed.
    pub(crate) fn matroska(clusters: u64, positions: bool) -> Vec<u8> {
        let ebml = el(EBML, &el(0x4282, b"matroska"));
        let info = el(INFO, &uint_el(TIMESTAMP_SCALE, 1_000_000));
        let entry = |number: u64, kind: u64| {
            el(
                TRACK_ENTRY,
                &[uint_el(TRACK_NUMBER, number), uint_el(TRACK_TYPE, kind)].concat(),
            )
        };
        let tracks = el(TRACKS, &[entry(1, 1), entry(2, 2)].concat());
        let seek = |id: u64, pos: u64| {
            el(
                SEEK,
                &[uint_el(SEEK_ID, id), uint_el(SEEK_POSITION, pos)].concat(),
            )
        };
        // Two passes: the seek head's size is fixed, so the second puts the right positions
        let mut segment = Vec::new();
        for _ in 0..2 {
            let seek_head_len = el(
                SEEK_HEAD,
                &[seek(INFO, 0), seek(TRACKS, 0), seek(CUES, 0)].concat(),
            )
            .len() as u64;
            let info_at = seek_head_len;
            let tracks_at = info_at + info.len() as u64;
            let mut body = Vec::new();
            let mut cue_points = Vec::new();
            let mut at = tracks_at + tracks.len() as u64;
            for k in 0..clusters {
                let time = k * 2000;
                let audio = simple_block(2, 0, true, &[0xAA; 500]);
                let key = simple_block(1, 40, true, &frame_data(k));
                let after = simple_block(1, 80, false, &[0x55; 200]);
                let stamp = uint_el(CLUSTER_TIMESTAMP, time);
                let relative = (stamp.len() + audio.len()) as u64;
                let cluster = el(CLUSTER, &[stamp, audio, key, after].concat());
                let mut positions_el =
                    vec![uint_el(CUE_TRACK, 1), uint_el(CUE_CLUSTER_POSITION, at)];
                if positions {
                    positions_el.push(uint_el(CUE_RELATIVE_POSITION, relative));
                }
                cue_points.push(el(
                    CUE_POINT,
                    &[
                        uint_el(CUE_TIME, time + 40),
                        el(CUE_TRACK_POSITIONS, &positions_el.concat()),
                    ]
                    .concat(),
                ));
                at += cluster.len() as u64;
                body.extend(cluster);
            }
            let cues_at = at;
            let seek_head = el(
                SEEK_HEAD,
                &[
                    seek(INFO, info_at),
                    seek(TRACKS, tracks_at),
                    seek(CUES, cues_at),
                ]
                .concat(),
            );
            segment = [
                seek_head,
                info.clone(),
                tracks.clone(),
                body,
                el(CUES, &cue_points.concat()),
            ]
            .concat();
        }
        [ebml, el(SEGMENT, &segment)].concat()
    }

    fn temp_file(name: &str, data: &[u8]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("r3trickplay-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn r3trickplay_ebml_numbers_parse() {
        assert_eq!(number(&[0x81], false), Some((1, 1)));
        assert_eq!(number(&[0x40, 0x02], false), Some((2, 2)));
        assert_eq!(number(&[0x1A, 0x45, 0xDF, 0xA3], true), Some((EBML, 4)));
        assert_eq!(number(&[0xFF], false), Some((u64::MAX, 1)));
        assert_eq!(
            number(&[0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff], false),
            Some((u64::MAX, 8))
        );
        assert_eq!(number(&[0x00], false), None);
        assert_eq!(number(&[0x40], false), None);
        assert_eq!(number(&[], false), None);
    }

    #[test]
    fn r3trickplay_cue_picks_seek_backward() {
        let cue = |time: u64| Cue {
            seconds: time as f64 / 1000.0,
            time,
            cluster: 0,
            relative: Some(0),
        };
        let index = MkvIndex {
            track: 1,
            scale: 1_000_000,
            cues: vec![cue(500), cue(2000), cue(4000)],
            frames_read: Default::default(),
        };
        assert_eq!(
            index.pick(0.0),
            None,
            "before the first cue, ffmpeg doesn't use the index"
        );
        assert_eq!(index.pick(1.999), Some(cue(500)));
        assert_eq!(
            index.pick(1.9995),
            Some(cue(2000)),
            "rounded to the nearest unit"
        );
        assert_eq!(index.pick(2.0), Some(cue(2000)), "on a cue: that cue");
        assert_eq!(index.pick(4.0), Some(cue(4000)));
        assert_eq!(
            index.pick(4.001),
            None,
            "past the last cue, ffmpeg reads on"
        );
    }

    #[test]
    fn r3trickplay_index_reads_its_frames_with_and_without_positions() {
        for positions in [true, false] {
            let path = temp_file("mkv", &matroska(5, positions));
            let index = MkvIndex::read(&path).expect("an index");
            assert_eq!(index.cues.len(), 5);
            let file = File::open(&path).unwrap();
            for k in 0..4u64 {
                // The middle of the k-th 2 s: the cue at k * 2 s + 40 ms
                let cue = index.pick(k as f64 * 2.0 + 1.0).unwrap();
                assert_eq!(cue.time, k * 2000 + 40);
                assert_eq!(
                    index.frame(&file, cue).unwrap(),
                    frame_data(k),
                    "{positions} {k}"
                );
            }
            // Past the last cue (8.04 s): none; on it: its frame
            assert_eq!(index.pick(9.0), None);
            let last = index.pick(8.04).unwrap();
            assert_eq!(index.frame(&file, last).unwrap(), frame_data(4));
            // A cue whose time isn't its block's: no frame
            let wrong = Cue {
                time: 41,
                ..index.pick(1.0).unwrap()
            };
            assert!(index.frame(&file, wrong).is_err());
            // Fewer than two cue points: no index
            let _ = std::fs::remove_file(&path);
            let path = temp_file("mkv1", &matroska(1, positions));
            assert!(MkvIndex::read(&path).is_none());
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn r3trickplay_garbage_index_never_panics_and_stays_bounded() {
        // xorshift: the same bytes on every run
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..3_000 {
            let len = (next() % 64) as usize;
            let data: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let _ = children(&data).count();
            let _ = MkvIndex::video_track(&data);
            let _ = MkvIndex::cue_points(&data, 1, 1_000_000, 0);
            let _ = element(&data, 0);
        }
        // Lots of tiny elements: walked one at a time, nothing kept per element
        let voids = [0xEC, 0x80].repeat(1 << 20);
        assert_eq!(children(&voids).count(), 1 << 20);
        assert!(MkvIndex::cue_points(&voids, 1, 1_000_000, 0).is_none());
        // A real file damaged: flipped bytes, cut short, or an element made huge
        for positions in [true, false] {
            let good = matroska(4, positions);
            for round in 0..300 {
                let mut bad = good.clone();
                match round % 3 {
                    0 => {
                        for _ in 0..1 + next() % 8 {
                            let at = (next() as usize) % bad.len();
                            bad[at] ^= 1 << (next() % 8);
                        }
                    }
                    1 => bad.truncate((next() as usize) % bad.len()),
                    _ => {
                        // An 8-byte size (0x01 then 7 bytes) set to almost 2^56
                        let sizes: Vec<usize> =
                            (0..bad.len() - 8).filter(|&i| bad[i] == 0x01).collect();
                        let at = sizes[(next() as usize) % sizes.len()];
                        bad[at + 1..at + 8]
                            .copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe]);
                    }
                }
                let path = temp_file("bad", &bad);
                if let Some(index) = MkvIndex::read(&path) {
                    let file = File::open(&path).unwrap();
                    for cue in index.cues.clone() {
                        let _ = index.frame(&file, cue);
                        let _ = index.without_positions().frame(&file, cue);
                    }
                }
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    #[test]
    fn r3trickplay_cluster_scan_is_linear_and_bounded() {
        // A cluster of 2-byte Voids up to the scan's limit, no block: given up on, quickly
        let mut cluster = uint_el(CLUSTER_TIMESTAMP, 0);
        cluster.extend([0xEC, 0x80].repeat((SCAN_MOST / 2) as usize));
        let data = [el(EBML, &[]), el(SEGMENT, &el(CLUSTER, &cluster))].concat();
        let path = temp_file("voids", &data);
        let index = MkvIndex {
            track: 1,
            scale: 1_000_000,
            cues: vec![],
            frames_read: Default::default(),
        };
        let cue = Cue {
            seconds: 0.0,
            time: 0,
            cluster: (el(EBML, &[]).len() + 12) as u64,
            relative: None,
        };
        let started = std::time::Instant::now();
        assert!(index.frame(&File::open(&path).unwrap(), cue).is_err());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        let _ = std::fs::remove_file(&path);
    }
}
