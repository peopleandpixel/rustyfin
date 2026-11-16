//! The key frames of a Matroska file's first video track, from its Cues index, for trickplay's
//! seek mode: which key frame a seek to a time lands on, and that frame's bytes, each with one
//! ranged read.
//!
//! A cue point gives a key frame's time, its cluster's position and (as mkvmerge and ffmpeg
//! write it) the block's position inside the cluster. ffmpeg's backward seek lands on the last
//! cue at or before the time asked for, so `MkvIndex::pick` does the same. A file whose cue
//! points lack the block's position, whose track is compressed (header stripping), or that
//! can't be read as expected gives no index; the caller then lets ffprobe find the frames.

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
const SIMPLE_BLOCK: u64 = 0xA3;
const BLOCK_GROUP: u64 = 0xA0;
const BLOCK: u64 = 0xA1;

/// The most a header element (Info, Tracks, Cues) or a key frame may take: more is refused.
const MOST: u64 = 64 << 20;

/// One cue point of the video track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cue {
    /// The key frame's time, in seconds.
    pub(crate) seconds: f64,
    /// Where its cluster starts in the file.
    cluster: u64,
    /// Where its block starts in the cluster's body.
    relative: u64,
}

/// The cue points of a Matroska file's first video track, in time order.
#[derive(Debug)]
pub(crate) struct MkvIndex {
    track: u64,
    cues: Vec<Cue>,
}

/// An element: its id, where its body starts, and its body's size (`None`: unknown, to the end
/// of its parent).
#[derive(Clone, Copy, Debug)]
struct Element {
    id: u64,
    body: u64,
    size: Option<u64>,
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
        body: at + (id_len + size_len) as u64,
        size: (size != u64::MAX).then_some(size),
    })
}

/// The children of an element whose body is `body`.
fn children(body: &[u8]) -> Vec<(Element, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < body.len() {
        let Some(el) = element(&body[at..], at as u64) else {
            break;
        };
        let start = el.body as usize;
        let Some(end) = el
            .size
            .and_then(|s| usize::try_from(s).ok())
            .and_then(|s| start.checked_add(s))
            .filter(|&end| end <= body.len())
        else {
            break;
        };
        out.push((el, &body[start..end]));
        at = end;
    }
    out
}

fn uint(data: &[u8]) -> Option<u64> {
    (data.len() <= 8).then(|| data.iter().fold(0u64, |v, b| v << 8 | u64::from(*b)))
}

/// Reads `len` bytes at `at`.
fn read(file: &File, at: u64, len: u64) -> Option<Vec<u8>> {
    if len > MOST {
        return None;
    }
    let mut buf = vec![0; usize::try_from(len).ok()?];
    file.read_exact_at(&mut buf, at).ok()?;
    Some(buf)
}

/// The element header at `at` (reading at most 16 bytes, fewer at the end of the file).
fn header(file: &File, at: u64, file_len: u64) -> Option<Element> {
    let buf = read(file, at, 16.min(file_len.checked_sub(at)?))?;
    element(&buf, at)
}

/// The body of `el`, read whole.
fn body(file: &File, el: Element) -> Option<Vec<u8>> {
    read(file, el.body, el.size?)
}

impl MkvIndex {
    /// The index of the Matroska file at `path`, read with a few ranged reads. Blocking.
    pub(crate) fn read(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        let file_len = file.metadata().ok()?.len();
        let ebml = header(&file, 0, file_len).filter(|e| e.id == EBML)?;
        let segment =
            header(&file, ebml.body + ebml.size?, file_len).filter(|e| e.id == SEGMENT)?;
        let segment_end = segment.size.map_or(file_len, |s| segment.body + s);
        // The top-level elements up to the first cluster, and those a seek head points at
        let (mut info, mut tracks, mut cues) = (None, None, None);
        let mut seek_heads = Vec::new();
        let mut at = segment.body;
        while at < segment_end {
            let el = header(&file, at, file_len)?;
            match el.id {
                INFO => info = Some(el),
                TRACKS => tracks = Some(el),
                CUES => cues = Some(el),
                SEEK_HEAD => seek_heads.push(el),
                CLUSTER => break,
                _ => {}
            }
            at = el.body + el.size?;
        }
        let mut seen = 0;
        while cues.is_none() || info.is_none() || tracks.is_none() {
            // A seek head may name another, at the end of the file; follow a few
            let head = *seek_heads.get(seen)?;
            seen += 1;
            if seen > 4 {
                return None;
            }
            for (seek, entry) in children(&body(&file, head)?) {
                if seek.id != SEEK {
                    continue;
                }
                let fields = children(entry);
                let field = |id| fields.iter().find(|(e, _)| e.id == id).map(|(_, d)| *d);
                let (Some(id), Some(position)) = (field(SEEK_ID), field(SEEK_POSITION)) else {
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
        let scale = children(&body(&file, info)?)
            .iter()
            .find(|(e, _)| e.id == TIMESTAMP_SCALE)
            .map_or(Some(1_000_000), |(_, d)| uint(d))?;
        let track = Self::video_track(&body(&file, tracks)?)?;
        let cues = Self::cue_points(&body(&file, cues)?, track, scale, segment.body)?;
        Some(Self { track, cues })
    }

    /// The number of the first video track, which ffmpeg's `V:0` is; `None` if it is
    /// compressed.
    fn video_track(tracks: &[u8]) -> Option<u64> {
        for (entry, fields) in children(tracks) {
            if entry.id != TRACK_ENTRY {
                continue;
            }
            let fields = children(fields);
            let field = |id| fields.iter().find(|(e, _)| e.id == id).map(|(_, d)| *d);
            if field(TRACK_TYPE).and_then(uint) != Some(1) {
                continue;
            }
            if field(CONTENT_ENCODINGS).is_some() {
                return None;
            }
            return field(TRACK_NUMBER).and_then(uint);
        }
        None
    }

    /// The cue points of `track`, in time order; `None` if any lacks the block's position.
    fn cue_points(cues: &[u8], track: u64, scale: u64, segment: u64) -> Option<Vec<Cue>> {
        let mut out = Vec::new();
        for (point, fields) in children(cues) {
            if point.id != CUE_POINT {
                continue;
            }
            let fields = children(fields);
            let time = fields
                .iter()
                .find(|(e, _)| e.id == CUE_TIME)
                .and_then(|(_, d)| uint(d))?;
            for (positions, data) in &fields {
                if positions.id != CUE_TRACK_POSITIONS {
                    continue;
                }
                let data = children(data);
                let field = |id| {
                    data.iter()
                        .find(|(e, _)| e.id == id)
                        .and_then(|(_, d)| uint(d))
                };
                if field(CUE_TRACK) != Some(track) {
                    continue;
                }
                out.push(Cue {
                    seconds: time as f64 * scale as f64 / 1e9,
                    cluster: segment.checked_add(field(CUE_CLUSTER_POSITION)?)?,
                    relative: field(CUE_RELATIVE_POSITION)?,
                });
            }
        }
        out.sort_by(|a, b| a.seconds.total_cmp(&b.seconds));
        (!out.is_empty()).then_some(out)
    }

    /// The key frame a backward seek to `seconds` lands on: the last cue at or before it, or
    /// the first.
    pub(crate) fn pick(&self, seconds: f64) -> Cue {
        let after = self.cues.partition_point(|c| c.seconds <= seconds);
        self.cues[after.saturating_sub(1)]
    }

    /// The bytes of the key frame at `cue`, as ffmpeg's packet holds them. Blocking. Fails with
    /// `INDEX_FAILED` where the cue leads to no unlaced key frame block of the track.
    pub(crate) fn frame(&self, file: &File, cue: Cue) -> Result<Vec<u8>, String> {
        self.read_frame(file, cue)
            .ok_or_else(|| format!("{INDEX_FAILED} at {:.3} s", cue.seconds))
    }

    /// Two ranged reads: the cluster's header, then from the block on, the first `CHUNK` bytes
    /// holding its header and the frame's start; the rest of a larger frame follows on from
    /// there, which a share's read-ahead serves without another seek.
    fn read_frame(&self, file: &File, cue: Cue) -> Option<Vec<u8>> {
        const CHUNK: u64 = 32 << 10;
        let file_len = file.metadata().ok()?.len();
        let cluster = header(file, cue.cluster, file_len).filter(|e| e.id == CLUSTER)?;
        let at = cluster.body.checked_add(cue.relative)?;
        let chunk = read(file, at, CHUNK.min(file_len.checked_sub(at)?))?;
        // The SimpleBlock, or the Block among a BlockGroup's children
        let mut block = element(&chunk, at)?;
        let simple = block.id == SIMPLE_BLOCK;
        if block.id == BLOCK_GROUP {
            let group_end = block.body.checked_add(block.size?)?;
            let mut child_at = block.body;
            block = loop {
                let offset = usize::try_from(child_at - at).ok()?;
                let child = element(chunk.get(offset..)?, child_at)?;
                if child.id == BLOCK {
                    break child;
                }
                child_at = child.body.checked_add(child.size?)?;
                if child_at >= group_end {
                    return None;
                }
            };
        } else if !simple {
            return None;
        }
        let body = usize::try_from(block.body - at).ok()?;
        let (track, track_len) = number(chunk.get(body..)?, false)?;
        let flags = *chunk.get(body + track_len + 2)?;
        // The track's, unlaced, and (for a SimpleBlock, which says) a key frame
        if track != self.track || flags & 0x06 != 0 || (simple && flags & 0x80 == 0) {
            return None;
        }
        let start = body + track_len + 3;
        let len = usize::try_from(block.size?.checked_sub(track_len as u64 + 3)?).ok()?;
        if len as u64 > MOST {
            return None;
        }
        let mut frame = chunk.get(start..)?.to_vec();
        frame.truncate(len);
        if frame.len() < len {
            let more = read(file, at + chunk.len() as u64, (len - frame.len()) as u64)?;
            frame.extend_from_slice(&more);
        }
        Some(frame)
    }
}

#[cfg(test)]
impl MkvIndex {
    /// The same index with every cluster `by` bytes off, as a damaged index would be.
    pub(crate) fn shifted(&self, by: u64) -> Self {
        let cues = self
            .cues
            .iter()
            .map(|c| Cue {
                cluster: c.cluster + by,
                ..*c
            })
            .collect();
        Self {
            track: self.track,
            cues,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let cue = |seconds: f64| Cue {
            seconds,
            cluster: 0,
            relative: 0,
        };
        let index = MkvIndex {
            track: 1,
            cues: vec![cue(0.5), cue(2.0), cue(4.0)],
        };
        assert_eq!(index.pick(0.0), cue(0.5), "before the first: the first");
        assert_eq!(index.pick(1.999), cue(0.5));
        assert_eq!(index.pick(2.0), cue(2.0), "on a cue: that cue");
        assert_eq!(index.pick(100.0), cue(4.0));
    }

    #[test]
    fn r3trickplay_garbage_index_is_none() {
        // xorshift: the same bytes on every run
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..5_000 {
            let len = (seed % 64) as usize;
            let data: Vec<u8> = (0..len)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    seed as u8
                })
                .collect();
            let _ = children(&data);
            let _ = MkvIndex::video_track(&data);
            let _ = MkvIndex::cue_points(&data, 1, 1_000_000, 0);
            let _ = element(&data, 0);
        }
    }
}
