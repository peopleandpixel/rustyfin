//! Raw H.264 and HEVC streams made of a video's key frames alone, for trickplay's seek mode.
//!
//! ffprobe can seek many times in one opening of a file (`-read_intervals`) and print each packet
//! it lands on (`-show_data`). `KeyFrameStream` turns those packets, which Matroska and MP4 store
//! with length prefixes (`avcC`, `hvcC`), into an Annex B stream that one ffmpeg decodes, a key
//! frame after another, in the order they came.
//!
//! An HEVC CRA key frame (x265's open GOP) is decoded as a random access point, but its picture
//! order count (POC) still follows the one before, and the decoder makes up the frames its
//! reference set names. A CRA's references all precede it, so `KeyFrameStream` rewrites each
//! CRA's POC to count up a quarter of the POC range from the last one: no frame then lands on a
//! made-up one, and frames come out in the order they went in.
//!
//! H.264 takes IDR frames, and the non-IDR key frames of open GOPs (as on Blu-rays) whose slices
//! are all I slices: those refer to no other picture, so they decode alone exactly. Only the
//! decoder's bookkeeping follows the picture before, which here isn't there: so each one's
//! `frame_num` is rewritten to follow the last (no gap in the reference frames) and its POC to
//! count up as HEVC's does. A key frame with a P or B slice, a field, or data partitioning is
//! refused.

use std::collections::HashMap;

/// Why a key frame was refused: it doesn't decode on its own, in order, in a raw stream. The
/// concat demuxer, which keeps the container's times, may still read it.
pub(crate) const NOT_ALONE: &str = "a key frame that doesn't decode alone";

/// Adds the bytes of one line of ffprobe's `-show_data` hex dump to `out`, as
/// `00000010: 0227 e584 0006 5d3c  .'....]<`. False, and nothing added, when it isn't one.
/// It allocates nothing beyond `out`, as a 4K key frame takes thousands of lines.
pub(crate) fn hex_line(line: &[u8], out: &mut Vec<u8>) -> bool {
    if line.len() < 10 || line[8] != b':' || !line[..8].iter().all(u8::is_ascii_hexdigit) {
        return false;
    }
    let start = out.len();
    let mut high: Option<u8> = None;
    let mut rest = &line[10..];
    // The hex columns end where the text column starts, two spaces on
    while let [c, tail @ ..] = rest {
        rest = tail;
        if *c == b' ' {
            if tail.first() == Some(&b' ') {
                break;
            }
            continue;
        }
        let Some(nibble) = hex_digit(*c) else {
            out.truncate(start);
            return false;
        };
        match high.take() {
            Some(h) => out.push(h << 4 | nibble),
            None => high = Some(nibble),
        }
    }
    if high.is_some() {
        out.truncate(start);
        return false;
    }
    true
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The bytes of a whole `-show_data` hex dump, such as a stream's `extradata`.
pub(crate) fn hex_dump(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for line in text.lines() {
        hex_line(line.trim_start().as_bytes(), &mut out);
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Codec {
    H264,
    Hevc,
}

impl Codec {
    /// The codec of ffprobe's `codec_name`, where its key frames can be read as a raw stream.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "h264" => Some(Self::H264),
            "hevc" => Some(Self::Hevc),
            _ => None,
        }
    }

    /// ffmpeg's name for the raw stream format.
    pub(crate) fn format(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
        }
    }
}

const START: [u8; 4] = [0, 0, 0, 1];

#[cfg(test)]
thread_local! {
    /// Set by a test: the back half of each frame's slice data is scrambled on its way out, as
    /// a damaged file or a bad rewrite would leave it.
    pub(crate) static DAMAGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Turns key frame packets into a raw Annex B stream; see the module docs.
pub(crate) struct KeyFrameStream {
    codec: Codec,
    /// The bytes of each NAL unit's length prefix.
    length_size: usize,
    /// The parameter sets of the decoder configuration, sent before the first frame.
    header: Option<Vec<u8>>,
    hevc: HevcPoc,
    h264: H264Count,
}

impl KeyFrameStream {
    /// A stream for `codec` with the container's decoder configuration `extradata` (`avcC` or
    /// `hvcC`). `None` when the configuration isn't one of those.
    pub(crate) fn new(codec: Codec, extradata: &[u8]) -> Option<Self> {
        let (length_size, sets) = match codec {
            Codec::H264 => avcc(extradata)?,
            Codec::Hevc => hvcc(extradata)?,
        };
        let mut stream = Self {
            codec,
            length_size,
            header: None,
            hevc: HevcPoc::default(),
            h264: H264Count::default(),
        };
        let mut header = Vec::new();
        for set in sets {
            let set = stream.nal(set.to_vec()).ok()?;
            header.extend_from_slice(&START);
            header.extend_from_slice(&set);
        }
        stream.header = Some(header);
        Some(stream)
    }

    /// Adds key frame `packet` to `out` as Annex B. Fails with `NOT_ALONE` where it isn't a
    /// key frame this stream can decode on its own.
    pub(crate) fn push(&mut self, packet: &[u8], out: &mut Vec<u8>) -> Result<(), String> {
        if let Some(header) = self.header.take() {
            out.extend_from_slice(&header);
        }
        let mut rest = packet;
        let mut pictures = 0;
        while !rest.is_empty() {
            let Some(len) = rest
                .get(..self.length_size)
                .map(|b| b.iter().fold(0usize, |n, b| n << 8 | usize::from(*b)))
            else {
                return Err("a packet ends inside a NAL unit's length".to_string());
            };
            let Some(nal) = rest.get(self.length_size..self.length_size + len) else {
                return Err("a packet ends inside a NAL unit".to_string());
            };
            rest = &rest[self.length_size + len..];
            if nal.is_empty() {
                continue;
            }
            // HEVC layers above the base (MV-HEVC's second view, a Dolby Vision enhancement
            // layer) are left out: the base layer decodes alone, and theirs would reuse its
            // parameter set ids and keep their own, unrewritten, POCs
            if self.codec == Codec::Hevc && hevc_layer(nal) != 0 {
                continue;
            }
            match self.codec {
                Codec::H264 => match nal[0] & 0x1f {
                    // Whether a non-IDR slice is an I slice is the slice header's to say
                    1 | 5 => pictures += 1,
                    2..=4 => {
                        return Err(format!("{NOT_ALONE}: an H.264 frame in data partitions"));
                    }
                    _ => {}
                },
                Codec::Hevc => match (nal[0] >> 1) & 0x3f {
                    16..=21 => pictures += 1,
                    0..=15 | 22..=31 => {
                        return Err(format!("{NOT_ALONE}: an HEVC frame that isn't IRAP"));
                    }
                    _ => {}
                },
            }
            #[allow(unused_mut)]
            let mut nal = self.nal(nal.to_vec())?;
            #[cfg(test)]
            if DAMAGE.get() && pictures > 0 {
                let half = nal.len() / 2;
                for b in &mut nal[half..] {
                    *b ^= 0x5a;
                }
            }
            out.extend_from_slice(&START);
            out.extend_from_slice(&nal);
        }
        if pictures == 0 {
            return Err(format!("{NOT_ALONE}: a packet without a frame"));
        }
        Ok(())
    }

    /// One NAL unit as it goes out: HEVC parameter sets are noted and CRA POCs rewritten.
    fn nal(&mut self, nal: Vec<u8>) -> Result<Vec<u8>, String> {
        match self.codec {
            Codec::H264 => self.h264.nal(nal),
            Codec::Hevc => self.hevc.nal(nal),
        }
    }
}

/// An HEVC NAL unit's `nuh_layer_id`; 0, the base layer, for a unit too short to have one.
fn hevc_layer(nal: &[u8]) -> u8 {
    match nal {
        [a, b, ..] => (a & 1) << 5 | b >> 3,
        _ => 0,
    }
}

/// The length size and parameter sets of an `avcC` record.
fn avcc(data: &[u8]) -> Option<(usize, Vec<&[u8]>)> {
    if data.len() < 7 || data[0] != 1 {
        return None;
    }
    let length_size = usize::from(data[4] & 3) + 1;
    let mut sets = Vec::new();
    let mut at = 5;
    for count_mask in [0x1f, 0xff] {
        let count = data.get(at)? & count_mask;
        at += 1;
        for _ in 0..count {
            let len = usize::from(u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]));
            sets.push(data.get(at + 2..at + 2 + len)?);
            at += 2 + len;
        }
    }
    Some((length_size, sets))
}

/// The length size and parameter sets of an `hvcC` record.
fn hvcc(data: &[u8]) -> Option<(usize, Vec<&[u8]>)> {
    if data.len() < 23 || data[0] != 1 {
        return None;
    }
    let length_size = usize::from(data[21] & 3) + 1;
    let mut sets = Vec::new();
    let mut at = 23;
    for _ in 0..data[22] {
        at += 1;
        let count = u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]);
        at += 2;
        for _ in 0..count {
            let len = usize::from(u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]));
            let set = data.get(at + 2..at + 2 + len)?;
            if hevc_layer(set) == 0 {
                sets.push(set);
            }
            at += 2 + len;
        }
    }
    Some((length_size, sets))
}

/// What a slice header needs from an H.264 SPS.
#[derive(Clone, Copy)]
struct H264Sps {
    frame_num_bits: u32,
    /// `pic_order_cnt_type`: 0 sends a POC LSB in each slice header; 1 and 2 derive the POC
    /// from `frame_num`.
    poc_type: u32,
    poc_lsb_bits: u32,
    /// For `pic_order_cnt_type` 1: no POC deltas in the slice header.
    delta_pic_order_always_zero: bool,
    frames_only: bool,
    separate_colour_planes: bool,
}

/// What a slice header needs from an H.264 PPS.
#[derive(Clone, Copy)]
struct H264Pps {
    sps: u32,
    /// `bottom_field_pic_order_in_frame_present_flag`: a bottom field POC delta in the header.
    bottom_field_poc: bool,
    redundant_pic_cnt: bool,
}

/// The H.264 parameter sets seen so far, and the `frame_num` and POC LSB the last picture was
/// given.
#[derive(Default)]
struct H264Count {
    sps: HashMap<u32, H264Sps>,
    pps: HashMap<u32, H264Pps>,
    frame_num: u32,
    lsb: u32,
}

impl H264Count {
    fn nal(&mut self, nal: Vec<u8>) -> Result<Vec<u8>, String> {
        let kind = nal.first().copied().unwrap_or_default() & 0x1f;
        let bad = |what: &str| format!("{NOT_ALONE}: its {what} can't be read");
        let payload = nal.get(1..).unwrap_or_default();
        match kind {
            7 => {
                let (id, sps) = parse_h264_sps(&unescape(payload)).ok_or_else(|| bad("SPS"))?;
                self.sps.insert(id, sps);
                Ok(nal)
            }
            8 => {
                let (id, pps) = parse_h264_pps(&unescape(payload)).ok_or_else(|| bad("PPS"))?;
                self.pps.insert(id, pps);
                Ok(nal)
            }
            1 | 5 => self.slice(nal, kind == 5),
            _ => Ok(nal),
        }
    }

    /// Takes an IDR slice's `frame_num` and POC LSB as the count's; gives a non-IDR I slice the
    /// next `frame_num` and a POC LSB a quarter of the range on. Refuses any other slice, a
    /// redundant one, and one that marks reference frames itself (memory management control
    /// operations): an MMCO 5 resets the decoder's `frame_num` and POC, and the others name
    /// pictures by a `frame_num` difference that the rewrite changes.
    fn slice(&mut self, nal: Vec<u8>, idr: bool) -> Result<Vec<u8>, String> {
        let refuse = |what: &str| format!("{NOT_ALONE}: an H.264 {what}");
        let unread = || format!("{NOT_ALONE}: its slice header can't be read");
        if nal.first().is_some_and(|h| h >> 5 & 3 == 0) {
            return Err(refuse("key frame no other frame refers to"));
        }
        let mut rbsp = unescape(nal.get(1..).unwrap_or_default());
        let mut r = Bits::new(&rbsp);
        let first = r.ue().ok_or_else(unread)? == 0;
        // I (2) and SI (4), or the same that hold for every slice of the picture (7, 9)
        let slice_type = r.ue().filter(|&t| t <= 9).ok_or_else(unread)?;
        if !matches!(slice_type % 5, 2 | 4) {
            return Err(refuse("key frame with a P or B slice"));
        }
        let pps = r
            .ue()
            .and_then(|id| self.pps.get(&id))
            .copied()
            .ok_or_else(unread)?;
        let sps = self.sps.get(&pps.sps).copied().ok_or_else(unread)?;
        if sps.separate_colour_planes {
            r.skip(2).ok_or_else(unread)?;
        }
        let frame_num_at = r.at;
        let frame_num = r.u(sps.frame_num_bits).ok_or_else(unread)?;
        if !sps.frames_only && r.flag().ok_or_else(unread)? {
            return Err(refuse("key frame coded as fields"));
        }
        if idr {
            r.ue().ok_or_else(unread)?; // idr_pic_id
        }
        let lsb_at = r.at;
        let lsb = if sps.poc_type == 0 {
            Some(r.u(sps.poc_lsb_bits).ok_or_else(unread)?)
        } else {
            None
        };
        // The rest of an I slice's header up to dec_ref_pic_marking (7.3.3): POC deltas,
        // redundant_pic_cnt; an I slice has no reference lists or weights
        if sps.poc_type == 0 && pps.bottom_field_poc {
            r.se().ok_or_else(unread)?;
        }
        if sps.poc_type == 1 && !sps.delta_pic_order_always_zero {
            r.se().ok_or_else(unread)?;
            if pps.bottom_field_poc {
                r.se().ok_or_else(unread)?;
            }
        }
        if pps.redundant_pic_cnt && r.ue().ok_or_else(unread)? > 0 {
            return Err(refuse("redundant picture"));
        }
        // dec_ref_pic_marking, as nal_ref_idc isn't 0
        if idr {
            r.skip(2).ok_or_else(unread)?;
        } else if r.flag().ok_or_else(unread)? {
            return Err(refuse("key frame that marks reference frames itself"));
        }
        if idr {
            if first {
                self.frame_num = frame_num;
                self.lsb = lsb.unwrap_or_default();
            }
            return Ok(nal);
        }
        if first {
            self.frame_num = (self.frame_num + 1) % (1 << sps.frame_num_bits);
            let range = 1u32 << sps.poc_lsb_bits;
            self.lsb = (self.lsb + range / 4) % range;
        }
        set_bits(&mut rbsp, frame_num_at, sps.frame_num_bits, self.frame_num);
        if lsb.is_some() {
            set_bits(&mut rbsp, lsb_at, sps.poc_lsb_bits, self.lsb);
        }
        let mut out = nal[..1].to_vec();
        escape(&rbsp, &mut out);
        Ok(out)
    }
}

/// Writes the low `n` bits of `value` into `data` from bit `at` on, most significant first.
/// The bits are there: they were just read.
fn set_bits(data: &mut [u8], at: usize, n: u32, value: u32) {
    for i in 0..n {
        let bit = (value >> (n - 1 - i)) & 1 == 1;
        let p = at + i as usize;
        let mask = 0x80 >> (p % 8);
        if bit {
            data[p / 8] |= mask;
        } else {
            data[p / 8] &= !mask;
        }
    }
}

/// The profiles whose SPS has the chroma format, bit depths and scaling matrices.
const H264_HIGH_PROFILES: [u32; 13] =
    [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

fn parse_h264_sps(rbsp: &[u8]) -> Option<(u32, H264Sps)> {
    let mut r = Bits::new(rbsp);
    let profile = r.u(8)?;
    r.skip(16)?; // constraint flags, level
    let id = Some(r.ue()?).filter(|&id| id <= 31)?;
    let mut separate_colour_planes = false;
    if H264_HIGH_PROFILES.contains(&profile) {
        let chroma = r.ue()?;
        if chroma == 3 {
            separate_colour_planes = r.flag()?;
        }
        r.ue()?;
        r.ue()?; // bit depths
        r.skip(1)?;
        if r.flag()? {
            for i in 0..if chroma == 3 { 12 } else { 8 } {
                if r.flag()? {
                    skip_scaling_list(&mut r, if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    // log2_max_frame_num_minus4 and log2_max_pic_order_cnt_lsb_minus4: 0 to 12
    let frame_num_bits = Some(r.ue()?).filter(|&n| n <= 12)? + 4;
    let poc_type = r.ue()?;
    let mut poc_lsb_bits = 4;
    let mut delta_pic_order_always_zero = false;
    match poc_type {
        0 => poc_lsb_bits = Some(r.ue()?).filter(|&n| n <= 12)? + 4,
        1 => {
            delta_pic_order_always_zero = r.flag()?;
            r.se()?;
            r.se()?;
            for _ in 0..Some(r.ue()?).filter(|&n| n <= 255)? {
                r.se()?;
            }
        }
        2 => {}
        _ => return None,
    }
    r.ue()?; // max_num_ref_frames
    r.skip(1)?;
    r.ue()?;
    r.ue()?; // size in macroblocks
    let frames_only = r.flag()?;
    Some((
        id,
        H264Sps {
            frame_num_bits,
            poc_type,
            poc_lsb_bits,
            delta_pic_order_always_zero,
            frames_only,
            separate_colour_planes,
        },
    ))
}

fn skip_scaling_list(r: &mut Bits<'_>, size: usize) -> Option<()> {
    let (mut last, mut next) = (8i64, 8i64);
    for _ in 0..size {
        if next != 0 {
            next = (last + r.se()?).rem_euclid(256);
        }
        if next != 0 {
            last = next;
        }
    }
    Some(())
}

/// An H.264 PPS's id and what a slice header needs of it. `None` for slice groups (FMO, of the
/// Baseline profile only), whose map isn't read.
fn parse_h264_pps(rbsp: &[u8]) -> Option<(u32, H264Pps)> {
    let mut r = Bits::new(rbsp);
    let id = Some(r.ue()?).filter(|&id| id <= 255)?;
    let sps = Some(r.ue()?).filter(|&id| id <= 31)?;
    r.skip(1)?; // entropy_coding_mode_flag
    let bottom_field_poc = r.flag()?;
    if r.ue()? != 0 {
        return None;
    }
    r.ue()?;
    r.ue()?; // default reference list sizes
    r.skip(3)?; // weighted prediction
    r.se()?;
    r.se()?;
    r.se()?; // initial QPs, chroma QP offset
    r.skip(2)?; // deblocking_filter_control_present_flag, constrained_intra_pred_flag
    let redundant_pic_cnt = r.flag()?;
    Some((
        id,
        H264Pps {
            sps,
            bottom_field_poc,
            redundant_pic_cnt,
        },
    ))
}

/// What a slice header needs from an HEVC SPS.
#[derive(Clone, Copy)]
struct Sps {
    poc_lsb_bits: u32,
    separate_colour_planes: bool,
    address_bits: u32,
}

/// What a slice header needs from an HEVC PPS.
#[derive(Clone, Copy)]
struct Pps {
    sps: u32,
    dependent_slices: bool,
    output_flag: bool,
    extra_bits: u32,
}

/// The parameter sets seen so far, and the POC LSB the last picture was given.
#[derive(Default)]
struct HevcPoc {
    sps: HashMap<u32, Sps>,
    pps: HashMap<u32, Pps>,
    lsb: u32,
}

impl HevcPoc {
    fn nal(&mut self, nal: Vec<u8>) -> Result<Vec<u8>, String> {
        let kind = (nal.first().copied().unwrap_or_default() >> 1) & 0x3f;
        let bad = |what: &str| format!("{NOT_ALONE}: its {what} can't be read");
        let payload = nal.get(2..).unwrap_or_default();
        match kind {
            33 => {
                let (id, sps) = parse_sps(&unescape(payload)).ok_or_else(|| bad("SPS"))?;
                self.sps.insert(id, sps);
                Ok(nal)
            }
            34 => {
                let (id, pps) = parse_pps(&unescape(payload)).ok_or_else(|| bad("PPS"))?;
                self.pps.insert(id, pps);
                Ok(nal)
            }
            16..=21 => self.slice(nal, kind).ok_or_else(|| bad("slice header")),
            _ => Ok(nal),
        }
    }

    /// Gives an IRAP slice its POC LSB: 0 after an IDR, which resets the count, else a quarter
    /// of the range on from the last picture's.
    fn slice(&mut self, nal: Vec<u8>, kind: u8) -> Option<Vec<u8>> {
        let mut rbsp = unescape(nal.get(2..)?);
        let mut r = Bits::new(&rbsp);
        let first = r.flag()?;
        r.skip(1)?; // no_output_of_prior_pics_flag, as every IRAP slice has
        let pps = *self.pps.get(&r.ue()?)?;
        let sps = *self.sps.get(&pps.sps)?;
        if kind == 19 || kind == 20 {
            // IDR: no POC LSB in the header
            if first {
                self.lsb = 0;
            }
            return Some(nal);
        }
        if !first {
            if pps.dependent_slices && r.flag()? {
                // A dependent slice segment takes its header from the one before
                return Some(nal);
            }
            r.skip(sps.address_bits)?;
        }
        r.skip(pps.extra_bits)?;
        r.ue()?; // slice_type
        if pps.output_flag {
            r.skip(1)?;
        }
        if sps.separate_colour_planes {
            r.skip(2)?;
        }
        let range = 1u32 << sps.poc_lsb_bits;
        if first {
            self.lsb = (self.lsb + range / 4) % range;
        }
        let at = r.at;
        r.u(sps.poc_lsb_bits)?;
        set_bits(&mut rbsp, at, sps.poc_lsb_bits, self.lsb);
        let mut out = nal.get(..2)?.to_vec();
        escape(&rbsp, &mut out);
        Some(out)
    }
}

fn parse_sps(rbsp: &[u8]) -> Option<(u32, Sps)> {
    let mut r = Bits::new(rbsp);
    r.skip(4)?; // sps_video_parameter_set_id
    let sub_layers = r.u(3)?;
    r.skip(1)?;
    // profile_tier_level: the general profile and level, then each sub-layer's
    r.skip(96)?;
    let mut present = Vec::new();
    for _ in 0..sub_layers {
        present.push((r.flag()?, r.flag()?));
    }
    if sub_layers > 0 {
        r.skip(2 * (8 - sub_layers))?;
    }
    for (profile, level) in present {
        if profile {
            r.skip(88)?;
        }
        if level {
            r.skip(8)?;
        }
    }
    let id = r.ue()?;
    let separate_colour_planes = r.ue()? == 3 && r.flag()?;
    let width = r.ue()?;
    let height = r.ue()?;
    if r.flag()? {
        for _ in 0..4 {
            r.ue()?;
        }
    }
    r.ue()?;
    r.ue()?; // bit depths
    // log2_max_pic_order_cnt_lsb_minus4: 0 to 12
    let poc_lsb_bits = Some(r.ue()?).filter(|&n| n <= 12)? + 4;
    let all_layers = r.flag()?;
    for _ in if all_layers { 0 } else { sub_layers }..=sub_layers {
        r.ue()?;
        r.ue()?;
        r.ue()?;
    }
    // The CTB size, log2: 4 to 6
    let min_cb = r.ue()?.checked_add(3)?;
    let ctb = min_cb.checked_add(r.ue()?)?;
    if !(4..=6).contains(&ctb) {
        return None;
    }
    let side = |n: u32| n.div_ceil(1 << ctb);
    let ctbs = u64::from(side(width)) * u64::from(side(height));
    let address_bits = 64 - ctbs.saturating_sub(1).leading_zeros();
    Some((
        id,
        Sps {
            poc_lsb_bits,
            separate_colour_planes,
            address_bits,
        },
    ))
}

fn parse_pps(rbsp: &[u8]) -> Option<(u32, Pps)> {
    let mut r = Bits::new(rbsp);
    let id = r.ue()?;
    let pps = Pps {
        sps: r.ue()?,
        dependent_slices: r.flag()?,
        output_flag: r.flag()?,
        extra_bits: r.u(3)?,
    };
    Some((id, pps))
}

/// A NAL unit's payload without its emulation prevention bytes.
fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &b in data {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Adds `rbsp` to `out` with emulation prevention bytes where it would hold a start code.
fn escape(rbsp: &[u8], out: &mut Vec<u8>) {
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
}

/// Reads bits, most significant first.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn bit(&mut self) -> Option<u32> {
        let byte = self.data.get(self.at / 8)?;
        let bit = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Some(u32::from(bit))
    }

    fn flag(&mut self) -> Option<bool> {
        Some(self.bit()? == 1)
    }

    fn u(&mut self, n: u32) -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            v = v << 1 | self.bit()?;
        }
        Some(v)
    }

    fn skip(&mut self, n: u32) -> Option<()> {
        self.at += n as usize;
        (self.at <= self.data.len() * 8).then_some(())
    }

    /// A signed Exp-Golomb number.
    fn se(&mut self) -> Option<i64> {
        let k = i64::from(self.ue()?);
        Some(if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) })
    }

    /// An unsigned Exp-Golomb number.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.u(zeros)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r3trickplay_hex_dump_lines_parse() {
        let mut out = Vec::new();
        assert!(hex_line(
            b"00000000: 0164 0029 ffe1 0018 6764 0029 acd9 4078  .d.)....gd.)..@x",
            &mut out
        ));
        assert!(hex_line(
            b"00000020: 0100 0668 e938 6cb2 2c                   ...h.8l.,",
            &mut out
        ));
        assert_eq!(out.len(), 25);
        assert_eq!(&out[..4], &[0x01, 0x64, 0x00, 0x29]);
        assert_eq!(
            &out[16..],
            &[0x01, 0x00, 0x06, 0x68, 0xe9, 0x38, 0x6c, 0xb2, 0x2c]
        );
        assert!(!hex_line(b"pts_time=2.920000", &mut out));
        assert!(!hex_line(b"data=", &mut out));
        // Not hex, an odd digit, and a multi-byte character where the hex starts
        assert!(!hex_line(b"00000030: 01zz", &mut out));
        assert!(!hex_line(b"00000030: 012", &mut out));
        assert!(!hex_line("00000030:\u{e9}0102".as_bytes(), &mut out));
        assert_eq!(out.len(), 25);
    }

    /// Writes bits, most significant first, into an RBSP with its stop bit.
    #[derive(Default)]
    struct BitsOut(Vec<bool>);

    impl BitsOut {
        fn u(&mut self, n: u32, v: u32) -> &mut Self {
            self.0.extend((0..n).rev().map(|i| v >> i & 1 == 1));
            self
        }
        fn ue(&mut self, v: u32) -> &mut Self {
            let n = 32 - (v + 1).leading_zeros();
            self.u(n - 1, 0).u(n, v + 1)
        }
        fn se(&mut self, v: i32) -> &mut Self {
            let k = if v > 0 { 2 * v - 1 } else { -2 * v };
            self.ue(k as u32)
        }
        /// The NAL unit: `header`, then these bits, the stop bit, padding, escaped.
        fn nal(&mut self, header: u8) -> Vec<u8> {
            self.0.push(true);
            while !self.0.len().is_multiple_of(8) {
                self.0.push(false);
            }
            let rbsp: Vec<u8> = self
                .0
                .chunks(8)
                .map(|b| b.iter().fold(0, |v, &bit| v << 1 | u8::from(bit)))
                .collect();
            let mut out = vec![header];
            escape(&rbsp, &mut out);
            out
        }
    }

    /// The H.264 coding choices `h264_stream` and `h264_slice` follow.
    #[derive(Clone, Copy, Default)]
    struct Coding {
        poc_type: u32,
        fields: bool,
        bottom_field_poc: bool,
        redundant: bool,
    }

    /// A Baseline profile stream with 4-bit frame_num and POC LSB, as `coding` says.
    fn h264_stream(coding: Coding) -> KeyFrameStream {
        let mut sps = BitsOut::default();
        sps.u(8, 66)
            .u(8, 0)
            .u(8, 30)
            .ue(0)
            .ue(0)
            .ue(coding.poc_type);
        match coding.poc_type {
            0 => {
                sps.ue(0);
            }
            1 => {
                // delta_pic_order_always_zero_flag 0, offsets, one frame in the cycle
                sps.u(1, 0).se(0).se(0).ue(1).se(2);
            }
            _ => {}
        }
        sps.ue(1)
            .u(1, 0)
            .ue(0)
            .ue(0)
            .u(1, u32::from(!coding.fields));
        if coding.fields {
            sps.u(1, 0);
        }
        sps.u(3, 0); // direct_8x8_inference, cropping, VUI
        let mut pps = BitsOut::default();
        pps.ue(0)
            .ue(0)
            .u(1, 0)
            .u(1, u32::from(coding.bottom_field_poc));
        pps.ue(0).ue(0).ue(0).u(3, 0).se(0).se(0).se(0).u(2, 0);
        pps.u(1, u32::from(coding.redundant));
        let (sps, pps) = (sps.nal(0x67), pps.nal(0x68));
        let mut config = vec![1, 66, 0, 30, 0xff, 0xe1];
        config.extend((sps.len() as u16).to_be_bytes());
        config.extend(&sps);
        config.push(1);
        config.extend((pps.len() as u16).to_be_bytes());
        config.extend(&pps);
        KeyFrameStream::new(Codec::H264, &config).unwrap()
    }

    /// An all-I slice NAL unit (IDR where `idr`) with `frame_num` and POC LSB `lsb`, a field
    /// where `field`, `redundant_pic_cnt`, and MMCO 5 where `mmco`, then a body that a rewrite
    /// must leave as it is.
    fn h264_slice(
        coding: Coding,
        idr: bool,
        (frame_num, lsb): (u32, u32),
        (field, redundant, mmco): (bool, u32, bool),
    ) -> Vec<u8> {
        let mut s = BitsOut::default();
        s.ue(0).ue(7).ue(0).u(4, frame_num);
        if coding.fields {
            s.u(1, u32::from(field));
        }
        if idr {
            s.ue(3);
        }
        if coding.poc_type == 0 {
            s.u(4, lsb);
            if coding.bottom_field_poc {
                s.se(-1);
            }
        }
        if coding.poc_type == 1 {
            s.se(3);
            if coding.bottom_field_poc {
                s.se(1);
            }
        }
        if coding.redundant {
            s.ue(redundant);
        }
        if idr {
            s.u(2, 0);
        } else if mmco {
            s.u(1, 1).ue(5).ue(0);
        } else {
            s.u(1, 0);
        }
        s.se(-2).u(16, 0xa50f).u(8, 0); // slice_qp_delta, then "slice data"
        s.nal(if idr { 0x65 } else { 0x61 })
    }

    /// The slice NAL unit `push` sent last in `out`.
    fn last_nal(out: &[u8]) -> &[u8] {
        let at = out.windows(4).rposition(|w| w == START).unwrap();
        &out[at + 4..]
    }

    #[test]
    fn r3trickplay_h264_rewrite_renumbers_only_frame_num_and_poc() {
        for coding in [
            Coding::default(),
            Coding {
                bottom_field_poc: true,
                ..Coding::default()
            },
            Coding {
                poc_type: 1,
                bottom_field_poc: true,
                ..Coding::default()
            },
            Coding {
                poc_type: 2,
                ..Coding::default()
            },
            // Frames in a stream that may hold fields, and redundant_pic_cnt 0
            Coding {
                fields: true,
                redundant: true,
                ..Coding::default()
            },
        ] {
            let mut stream = h264_stream(coding);
            let mut out = Vec::new();
            let plain = (false, 0, false);
            let idr = h264_slice(coding, true, (0, 6), plain);
            stream
                .push(&packet(std::slice::from_ref(&idr)), &mut out)
                .unwrap();
            assert_eq!(last_nal(&out), idr, "an IDR slice goes out as it came");
            // Whatever they held, the next key frames get frame_num 1, 2, 3 and an LSB 4 on
            for k in 1..=3 {
                out.clear();
                let came = h264_slice(coding, false, (9, 13), plain);
                stream.push(&packet(&[came]), &mut out).unwrap();
                let lsb = (6 + 4 * k) % 16;
                let expected = h264_slice(coding, false, (k, lsb), plain);
                assert_eq!(last_nal(&out), expected, "key frame {k}");
            }
        }
    }

    #[test]
    fn r3trickplay_h264_refuses_fields_redundant_pictures_and_mmcos() {
        let fields = Coding {
            fields: true,
            redundant: true,
            ..Coding::default()
        };
        for (what, slice) in [
            (
                "a field",
                h264_slice(fields, false, (1, 2), (true, 0, false)),
            ),
            (
                "a redundant picture",
                h264_slice(fields, false, (1, 2), (false, 1, false)),
            ),
            (
                "an MMCO",
                h264_slice(fields, false, (1, 2), (false, 0, true)),
            ),
        ] {
            let mut stream = h264_stream(fields);
            let e = stream.push(&packet(&[slice]), &mut Vec::new()).unwrap_err();
            assert!(e.starts_with(NOT_ALONE), "{what}: {e}");
        }
        // An MMCO behind POC type 1's deltas (with the bottom field delta) is found too
        let poc1 = Coding {
            poc_type: 1,
            bottom_field_poc: true,
            ..Coding::default()
        };
        let mut stream = h264_stream(poc1);
        let mmco = h264_slice(poc1, false, (1, 0), (false, 0, true));
        let e = stream.push(&packet(&[mmco]), &mut Vec::new()).unwrap_err();
        assert!(e.contains("marks reference frames"), "{e}");
        let plain = h264_slice(poc1, false, (1, 0), (false, 0, false));
        stream.push(&packet(&[plain]), &mut Vec::new()).unwrap();
    }

    /// A Blu-ray encode's avcC: 4-byte lengths, one High profile SPS, one PPS.
    const AVCC: [u8; 41] = [
        1, 0x64, 0, 0x29, 0xff, 0xe1, 0, 0x18, 0x67, 0x64, 0, 0x29, 0xac, 0xd9, 0x40, 0x78, 0x02,
        0x27, 0xe5, 0x84, 0, 0x06, 0x5d, 0x3c, 0x01, 0x31, 0x2d, 0x02, 0x3c, 0x60, 0xc6, 0x58,
        0x01, 0, 0x06, 0x68, 0xe9, 0x38, 0x6c, 0xb2, 0x2c,
    ];

    /// An HEVC NAL unit header of `kind` in `layer`, then `payload`.
    fn hevc_nal(kind: u8, layer: u8, payload: &[u8]) -> Vec<u8> {
        let mut nal = vec![kind << 1 | layer >> 5, (layer & 0x1f) << 3 | 1];
        nal.extend_from_slice(payload);
        nal
    }

    /// `nals` as one packet with 4-byte lengths.
    fn packet(nals: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for nal in nals {
            out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            out.extend_from_slice(nal);
        }
        out
    }

    /// A minimal `hvcC` record with 4-byte lengths and no parameter sets.
    fn empty_hvcc() -> Vec<u8> {
        let mut config = vec![1];
        config.extend([0; 20]);
        config.extend([0xff, 0]);
        config
    }

    #[test]
    fn r3trickplay_hevc_upper_layers_are_left_out() {
        let mut stream = KeyFrameStream::new(Codec::Hevc, &empty_hvcc()).unwrap();
        let mut out = Vec::new();
        // Only a layer-1 picture: no base layer frame, so refused
        let e = stream
            .push(&packet(&[hevc_nal(21, 1, &[0x80])]), &mut out)
            .unwrap_err();
        assert!(e.starts_with(NOT_ALONE), "{e}");
        // A layer-1 SPS and PPS that can't be read, and a layer-1 non-IRAP slice: none is
        // parsed or counted, nor sent on
        out.clear();
        let base = hevc_nal(36, 0, &[]); // an end of sequence, sent on as it is
        let upper = [
            hevc_nal(33, 1, &[0xff]),
            hevc_nal(34, 1, &[]),
            hevc_nal(1, 1, &[0x80]),
        ];
        let mut nals = upper.to_vec();
        nals.push(base.clone());
        let e = stream.push(&packet(&nals), &mut out).unwrap_err();
        assert!(e.contains("a packet without a frame"), "{e}");
        assert_eq!(out, [&[0, 0, 0, 1][..], &base].concat());
        assert_eq!(hevc_layer(&hevc_nal(33, 63, &[])), 63);
        assert_eq!(hevc_layer(&[0x42]), 0);
    }

    #[test]
    fn r3trickplay_garbage_never_panics() {
        // xorshift: the same bytes on every run
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut out = Vec::new();
        for round in 0..20_000 {
            let len = (next() % 48) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            // Parameter sets, IRAP slices and anything else, cut anywhere, in every layer
            let kind = [32, 33, 34, 16, 19, 21, 1, 62][round % 8];
            let layer = if round % 16 == 0 { 1 } else { 0 };
            let nal = hevc_nal(kind, layer, &bytes);
            let cut = &nal[..(next() as usize % (nal.len() + 1))];
            let mut hevc = KeyFrameStream::new(Codec::Hevc, &empty_hvcc()).unwrap();
            for nal in [&nal[..], cut] {
                let _ = hevc.push(&packet(&[nal.to_vec()]), &mut out);
            }
            // Raw bytes as a packet, as a decoder configuration, and as a dump line
            let _ = hevc.push(&bytes, &mut out);
            let mut h264 = KeyFrameStream::new(Codec::H264, &AVCC).unwrap();
            let _ = h264.push(&bytes, &mut out);
            // H.264 slices of every kind, then cut anywhere
            let nal = [
                &[[0x65, 0x61, 0x41, 0x01, 0x67, 0x68][round % 6]][..],
                &bytes,
            ]
            .concat();
            let cut = &nal[..(next() as usize % (nal.len() + 1))];
            for nal in [&nal[..], cut] {
                let _ = h264.push(&packet(&[nal.to_vec()]), &mut out);
            }
            let _ = KeyFrameStream::new(Codec::Hevc, &bytes);
            let _ = KeyFrameStream::new(Codec::H264, &bytes);
            let mut line = b"00000000: ".to_vec();
            line.extend_from_slice(&bytes);
            hex_line(&line, &mut out);
            out.clear();
        }
        // The parsers on their own, past every field's limit
        for _ in 0..20_000 {
            let len = (next() % 40) as usize;
            let rbsp: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let _ = parse_sps(&rbsp);
            let _ = parse_pps(&rbsp);
            let _ = parse_h264_sps(&rbsp);
            let _ = parse_h264_pps(&rbsp);
            let _ = unescape(&rbsp);
        }
        // Exp-Golomb numbers too long to be one; a CTB of 8 pixels
        assert!(parse_sps(&[0; 40]).is_none());
        assert!(parse_sps(&[0xff; 40]).is_none());
    }

    #[test]
    fn r3trickplay_escaping_round_trips() {
        let rbsp = [0, 0, 0, 0, 0, 1, 0, 0, 2, 5, 0, 0];
        let mut out = Vec::new();
        escape(&rbsp, &mut out);
        assert_eq!(out, [0, 0, 3, 0, 0, 3, 0, 1, 0, 0, 3, 2, 5, 0, 0]);
        assert_eq!(unescape(&out), rbsp);
    }

    #[test]
    fn r3trickplay_h264_takes_i_frames_and_renumbers_them() {
        // The start of the AVCC encode's first IDR slice: first_mb 0, slice_type 7 (all I),
        // PPS 0, ...
        let slice = [
            0x88, 0x82, 0x00, 0x7f, 0xf2, 0x9d, 0x41, 0xdd, 0x70, 0x83, 0x19,
        ];
        let with = |header: u8| packet(&[[&[header][..], &slice].concat()]);
        let mut stream = KeyFrameStream::new(Codec::H264, &AVCC).unwrap();
        let mut out = Vec::new();
        // An IDR frame goes out as it came, after the parameter sets
        stream.push(&with(0x65), &mut out).unwrap();
        assert_eq!(&out[..5], &[0, 0, 0, 1, 0x67]);
        assert!(out.ends_with(&[&[0, 0, 0, 1, 0x65][..], &slice].concat()));
        // A non-IDR I frame gets the next frame_num and a POC on, so two in a row differ
        let mut renumbered = Vec::new();
        for _ in 0..2 {
            out.clear();
            stream.push(&with(0x61), &mut out).unwrap();
            assert_eq!(&out[..5], &[0, 0, 0, 1, 0x61]);
            renumbered.push(out[5..].to_vec());
        }
        assert_ne!(renumbered[0], slice);
        assert_ne!(renumbered[0], renumbered[1]);
        // A P slice (slice_type 5), a key frame nothing refers to, data partitions: refused
        for refused in [
            packet(&[vec![0x41, 0x98, 0x80]]),
            with(0x01),
            packet(&[vec![0x42, 0x88]]),
        ] {
            let e = stream.push(&refused, &mut out).unwrap_err();
            assert!(e.starts_with(NOT_ALONE), "{e}");
        }
        assert!(stream.push(&[0, 0, 0, 9, 0x65], &mut out).is_err());
        assert!(KeyFrameStream::new(Codec::H264, &[0, 0, 0, 1, 0x67]).is_none());
    }
}
