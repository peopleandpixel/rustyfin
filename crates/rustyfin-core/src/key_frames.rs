//! Raw H.264 and HEVC streams made of a video's key frames alone, for trickplay's seek mode.
//!
//! ffprobe can seek many times in one opening of a file (`-read_intervals`) and print each packet
//! it lands on (`-show_data`). `KeyFrameStream` turns those packets, which Matroska and MP4 store
//! with length prefixes (`avcC`, `hvcC`), into an Annex B stream that one ffmpeg decodes, a key
//! frame after another, in the order they came.
//!
//! H.264 takes IDR frames only: each starts afresh. An HEVC CRA key frame (x265's open GOP) is
//! decoded as a random access point, but its picture order count (POC) still follows the one
//! before, and the decoder makes up the frames its reference set names. A CRA's references all
//! precede it, so `KeyFrameStream` rewrites each CRA's POC to count up a quarter of the POC range
//! from the last one: no frame then lands on a made-up one, and frames come out in the order they
//! went in.

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

/// Turns key frame packets into a raw Annex B stream; see the module docs.
pub(crate) struct KeyFrameStream {
    codec: Codec,
    /// The bytes of each NAL unit's length prefix.
    length_size: usize,
    /// The parameter sets of the decoder configuration, sent before the first frame.
    header: Option<Vec<u8>>,
    hevc: HevcPoc,
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
                    5 => pictures += 1,
                    1..=4 => return Err(format!("{NOT_ALONE}: an H.264 frame that isn't IDR")),
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
            let nal = self.nal(nal.to_vec())?;
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
            Codec::H264 => Ok(nal),
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
            sets.push(data.get(at + 2..at + 2 + len)?);
            at += 2 + len;
        }
    }
    Some((length_size, sets))
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
        for i in 0..sps.poc_lsb_bits {
            let bit = (self.lsb >> (sps.poc_lsb_bits - 1 - i)) & 1 == 1;
            let p = at + i as usize;
            let mask = 0x80 >> (p % 8);
            if bit {
                rbsp[p / 8] |= mask;
            } else {
                rbsp[p / 8] &= !mask;
            }
        }
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
            let mut h264 = KeyFrameStream::new(
                Codec::H264,
                &[
                    1, 0x64, 0, 0x29, 0xff, 0xe1, 0, 2, 0x67, 0x64, 1, 0, 2, 0x68, 0xe9,
                ],
            )
            .unwrap();
            let _ = h264.push(&bytes, &mut out);
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
    fn r3trickplay_h264_takes_idr_frames_only() {
        // avcC: version 1, 4-byte lengths, one SPS (67 ..), one PPS (68 ..)
        let config = [
            1, 0x64, 0, 0x29, 0xff, 0xe1, 0, 2, 0x67, 0x64, 1, 0, 2, 0x68, 0xe9,
        ];
        let mut stream = KeyFrameStream::new(Codec::H264, &config).unwrap();
        let mut out = Vec::new();
        stream
            .push(&[0, 0, 0, 3, 0x65, 0x88, 0x80], &mut out)
            .unwrap();
        assert_eq!(
            out,
            [
                0, 0, 0, 1, 0x67, 0x64, 0, 0, 0, 1, 0x68, 0xe9, 0, 0, 0, 1, 0x65, 0x88, 0x80
            ]
        );
        out.clear();
        stream.push(&[0, 0, 0, 2, 0x65, 0x88], &mut out).unwrap();
        assert_eq!(out, [0, 0, 0, 1, 0x65, 0x88]);
        let e = stream
            .push(&[0, 0, 0, 2, 0x41, 0x9a], &mut out)
            .unwrap_err();
        assert!(e.starts_with(NOT_ALONE), "{e}");
        assert!(stream.push(&[0, 0, 0, 9, 0x65], &mut out).is_err());
        assert!(KeyFrameStream::new(Codec::H264, &[0, 0, 0, 1, 0x67]).is_none());
    }
}
