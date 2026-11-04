//! Progressive appends (pure). A media segment's bytes are appended to its SourceBuffer while
//! they download: Chromium parses a partial `moof`+`mdat` (or a partial WebM `Cluster`) sample by
//! sample, so the first frame shows once the keyframe is in, not when the whole segment is
//! (measured 3 Nov 2026: `loadeddata` after 12 KB of a 108 KB first segment; YouTube's segments are
//! one fragment with one keyframe each, so appending whole boxes would gain nothing).
//!
//! `Asm` puts one segment's bytes in order when several requests deliver them (the index request
//! that brings the segment's start, the request for the rest, a mirror twin of either, a retry
//! from where a broken one stopped): only bytes past the frontier are appended, once.
//! `mp4_covered` / `webm_covered` say how much media time the bytes so far hold (the instant
//! quality switch seeks once the new quality covers the playhead).

/// One segment's bytes `[start, end]` (file offsets, inclusive), appended in order.
#[derive(Debug, Default)]
pub struct Asm {
    pub start: u64,
    pub end: u64,
    frontier: u64,
    /// Pieces that arrived ahead of the frontier: (offset, bytes), at most the segment's size.
    ahead: Vec<(u64, Vec<u8>)>,
}

impl Asm {
    pub fn new(start: u64, end: u64) -> Asm {
        Asm { start, end, frontier: start, ahead: Vec::new() }
    }

    /// The next byte to append.
    pub fn frontier(&self) -> u64 {
        self.frontier
    }

    pub fn done(&self) -> bool {
        self.frontier > self.end
    }

    /// Bytes `bytes` at file offset `at` arrived: the bytes that are now contiguous with what was
    /// appended before (possibly none; pieces ahead of the frontier wait here).
    pub fn add(&mut self, at: u64, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        self.take(at, bytes, &mut out);
        // Pieces that were ahead and now touch the frontier.
        loop {
            let Some(i) = self.ahead.iter().position(|(a, _)| *a <= self.frontier) else { break };
            let (a, b) = self.ahead.swap_remove(i);
            self.take(a, &b, &mut out);
        }
        out
    }

    fn take(&mut self, at: u64, bytes: &[u8], out: &mut Vec<u8>) {
        if self.done() || bytes.is_empty() {
            return;
        }
        let stop = (at + bytes.len() as u64).min(self.end + 1);
        if stop <= self.frontier {
            return; // all of it is appended already
        }
        if at > self.frontier {
            // Ahead of the frontier: keep it (clipped to the segment) unless a kept piece covers it.
            let b = &bytes[..(stop - at) as usize];
            if !self.ahead.iter().any(|(a, x)| *a <= at && a + x.len() as u64 >= stop) {
                self.ahead.push((at, b.to_vec()));
            }
            return;
        }
        let from = (self.frontier - at) as usize;
        out.extend_from_slice(&bytes[from..(stop - at) as usize]);
        self.frontier = stop;
    }
}

fn be32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
}

/// ISO-BMFF box at `o` within `b[..end]`: (type, header size, total size). The size may exceed
/// what is there yet.
fn mp4_box(b: &[u8], o: usize) -> Option<([u8; 4], usize, usize)> {
    let n = be32(b, o)? as usize;
    let t: [u8; 4] = b.get(o + 4..o + 8)?.try_into().ok()?;
    if n == 1 {
        let x = b.get(o + 8..o + 16)?;
        let n = u64::from_be_bytes(x.try_into().ok()?) as usize;
        return (n >= 16).then_some((t, 16, n));
    }
    (n >= 8).then_some((t, 8, n))
}

fn child(b: &[u8], from: usize, to: usize, ty: &[u8; 4]) -> Option<(usize, usize, usize)> {
    let mut o = from;
    while o + 8 <= to {
        let (t, h, n) = mp4_box(b, o)?;
        if &t == ty {
            return Some((o, h, n));
        }
        o += n;
    }
    None
}

/// The complete `moof`'s sample ends and elapsed durations. Once the header is in, progressive
/// coverage needs only the received byte count; the media payload itself need not be retained.
pub struct Mp4Coverage {
    samples: Vec<(i64, u64)>,
    total: u64,
}

impl Mp4Coverage {
    pub fn parse(b: &[u8]) -> Option<Self> {
        let mut samples = Vec::new();
        let total = mp4_samples(b, |end, duration| samples.push((end, duration)))?;
        Some(Self { samples, total })
    }

    pub fn covered(&self, bytes: usize) -> f64 {
        let end = self.samples.partition_point(|sample| sample.0 <= bytes as i64);
        let have = end.checked_sub(1).map_or(0, |i| self.samples[i].1);
        have as f64 / self.total as f64
    }
}

/// The share (0–1) of an fMP4 media segment's duration (`b` starts at its `moof`) whose samples are
/// all in `b` (decode order). None when `b` does not start with a complete `moof` yet, or for a
/// layout this does not read (an absolute base data offset). The one-shot reader allocates nothing.
pub fn mp4_covered(b: &[u8]) -> Option<f64> {
    let mut have = 0;
    let mut whole = true;
    let total = mp4_samples(b, |end, duration| {
        if whole && end <= b.len() as i64 {
            have = duration;
        } else {
            whole = false;
        }
    })?;
    Some(have as f64 / total as f64)
}

fn mp4_samples(b: &[u8], mut sample: impl FnMut(i64, u64)) -> Option<u64> {
    let mut o = 0;
    // `styp`/`sidx`/`emsg` boxes may come first.
    let (moof, mh, mn) = loop {
        let (t, h, n) = mp4_box(b, o)?;
        if &t == b"moof" {
            break (o, h, n);
        }
        o += n;
    };
    if b.len() < moof + mn {
        return None;
    }
    let (traf, th, tn) = child(b, moof + mh, moof + mn, b"traf")?;
    let (tf, tfh, _) = child(b, traf + th, traf + tn, b"tfhd")?;
    let tflags = be32(b, tf + tfh)? & 0xff_ffff;
    if tflags & 0x1 != 0 {
        return None; // base-data-offset: offsets are absolute
    }
    let mut p = tf + tfh + 8;
    if tflags & 0x2 != 0 {
        p += 4;
    }
    let def_dur = if tflags & 0x8 != 0 { let d = be32(b, p)?; p += 4; d } else { 0 };
    let def_size = if tflags & 0x10 != 0 { be32(b, p)? } else { 0 };
    let (tr, trh, _) = child(b, traf + th, traf + tn, b"trun")?;
    let flags = be32(b, tr + trh)? & 0xff_ffff;
    let count = be32(b, tr + trh + 4)? as usize;
    let mut p = tr + trh + 8;
    let data = if flags & 0x1 != 0 { let d = be32(b, p)? as i32; p += 4; d } else { mn as i32 + 8 };
    if flags & 0x4 != 0 {
        p += 4;
    }
    let mut at = moof as i64 + data as i64;
    let mut total = 0u64;
    for _ in 0..count.min(100_000) {
        let dur = if flags & 0x100 != 0 { let d = be32(b, p)?; p += 4; d } else { def_dur };
        let size = if flags & 0x200 != 0 { let s = be32(b, p)?; p += 4; s } else { def_size };
        if flags & 0x400 != 0 {
            p += 4;
        }
        if flags & 0x800 != 0 {
            p += 4;
        }
        at += size as i64;
        total += dur as u64;
        sample(at, total);
    }
    if total == 0 {
        return None;
    }
    Some(total)
}

/// EBML variable-size integer at `o`: (value, length); `marker` keeps the length bits (element IDs).
fn vint(b: &[u8], o: usize, marker: bool) -> Option<(u64, usize)> {
    let x = *b.get(o)?;
    let len = x.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    let mut v = if marker { x as u64 } else { (x as u64) & ((1u64 << (8 - len)) - 1) };
    for i in 1..len {
        v = (v << 8) | *b.get(o + i)? as u64;
    }
    Some((v, len))
}

/// The media time (seconds, timecodes in ms as YouTube's WebM files have them) of the last
/// complete block in `b`, which starts at a `Cluster`. None before a complete block.
pub fn webm_covered(b: &[u8]) -> Option<f64> {
    let mut o = 0;
    let mut last = None;
    while o < b.len() {
        let (id, il) = vint(b, o, true)?;
        let (size, sl) = vint(b, o + il, false)?;
        let body = o + il + sl;
        if id != 0x1F43_B675 {
            o = body.checked_add(size as usize)?;
            continue;
        }
        // Unknown-size clusters run to the next cluster; this walks their children either way.
        let unknown = size == (1u64 << (7 * sl)) - 1;
        let end = if unknown { b.len() } else { (body + size as usize).min(b.len()) };
        let mut tc = 0u64;
        let mut p = body;
        while p < end {
            let Some((cid, cl)) = vint(b, p, true) else { break };
            let Some((cs, csl)) = vint(b, p + cl, false) else { break };
            let cb = p + cl + csl;
            if cid == 0x1F43_B675 {
                break; // the next cluster (unknown size)
            }
            let Some(ce) = cb.checked_add(cs as usize) else { break };
            if ce > b.len() {
                return last;
            }
            match cid {
                0xE7 => tc = b[cb..ce].iter().fold(0, |a, x| (a << 8) | *x as u64),
                0xA3 | 0xA0 => {
                    // SimpleBlock, or a BlockGroup whose first child is the Block.
                    let blk = if cid == 0xA0 { vint(b, cb, true).and_then(|(_, l)| vint(b, cb + l, false).map(|(_, s)| cb + l + s)) } else { Some(cb) };
                    if let Some(bo) = blk {
                        if let Some((_, tl)) = vint(b, bo, false) {
                            if let Some(r) = b.get(bo + tl..bo + tl + 2) {
                                let rel = i16::from_be_bytes([r[0], r[1]]) as i64;
                                last = Some((tc as i64 + rel).max(0) as f64 / 1000.0);
                            }
                        }
                    }
                }
                _ => {}
            }
            p = ce;
        }
        if unknown {
            o = p;
            if p >= b.len() {
                break;
            }
        } else {
            o = body + size as usize;
        }
    }
    last
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn assembles_in_order_from_several_requests() {
        let seg: Vec<u8> = (0..100u8).collect();
        let mut a = Asm::new(1000, 1099);
        // The index request brings the first 30 bytes (plus bytes before the segment).
        let mut head = vec![9u8; 10];
        head.extend_from_slice(&seg[..30]);
        assert_eq!(a.add(990, &head), seg[..30].to_vec());
        // The rest request started early: its first piece is ahead of the frontier.
        assert!(a.add(1060, &seg[60..80]).is_empty());
        // A retry from 1030 delivers 30..70: the overlap with the kept piece is not repeated.
        let out = a.add(1030, &seg[30..70]);
        assert_eq!(out, seg[30..80].to_vec(), "30..70 and then the kept 70..80");
        assert_eq!(a.frontier(), 1080);
        // A mirror twin repeats bytes already appended, then the end (clipped past the segment).
        assert_eq!(a.add(1070, &seg[70..90]), seg[80..90].to_vec());
        let mut tail = seg[90..].to_vec();
        tail.extend_from_slice(&[7, 7, 7]);
        assert_eq!(a.add(1090, &tail), seg[90..].to_vec());
        assert!(a.done());
        assert!(a.add(1095, &[1, 2, 3]).is_empty());
    }

    #[test]
    fn single_bytes_and_split_points() {
        let seg: Vec<u8> = (0..64u8).collect();
        let mut a = Asm::new(0, 63);
        let mut got = Vec::new();
        for (i, b) in seg.iter().enumerate() {
            got.extend(a.add(i as u64, &[*b]));
        }
        assert_eq!(got, seg);
    }

    fn bx(t: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(t);
        v.extend_from_slice(body);
        v
    }

    /// moof (tfhd default-base-is-moof, trun with data offset + durations + sizes) + mdat.
    pub(crate) fn fragment(samples: &[(u32, u32)]) -> Vec<u8> {
        let tfhd = bx(b"tfhd", &[0, 0x02, 0, 0, 0, 0, 0, 1]);
        let mut trun_body = vec![0, 0, 0x03, 0x01];
        trun_body.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        trun_body.extend_from_slice(&0u32.to_be_bytes()); // data offset, patched below
        for (d, s) in samples {
            trun_body.extend_from_slice(&d.to_be_bytes());
            trun_body.extend_from_slice(&s.to_be_bytes());
        }
        let build = |off: u32| {
            let mut tb = trun_body.clone();
            tb[8..12].copy_from_slice(&off.to_be_bytes());
            let traf = bx(b"traf", &[tfhd.clone(), bx(b"trun", &tb)].concat());
            bx(b"moof", &[bx(b"mfhd", &[0, 0, 0, 0, 0, 0, 0, 1]), traf].concat())
        };
        let moof = build(0);
        let moof = build(moof.len() as u32 + 8);
        let data: Vec<u8> = samples.iter().flat_map(|(_, s)| vec![0xAB; *s as usize]).collect();
        [moof, bx(b"mdat", &data)].concat()
    }

    #[test]
    fn mp4_share_of_complete_samples() {
        let f = fragment(&[(100, 50), (100, 10), (100, 10), (100, 30)]);
        let mdat = f.len() - 100;
        assert_eq!(mp4_covered(&f[..20]), None, "moof incomplete");
        assert_eq!(mp4_covered(&f[..mdat]), Some(0.0), "no sample yet");
        assert_eq!(mp4_covered(&f[..mdat + 49]), Some(0.0));
        assert_eq!(mp4_covered(&f[..mdat + 50]), Some(0.25), "the keyframe");
        assert_eq!(mp4_covered(&f[..mdat + 69]), Some(0.5));
        assert_eq!(mp4_covered(&f), Some(1.0));
        // A styp box before the moof.
        let s = [bx(b"styp", b"msdh0000"), f.clone()].concat();
        assert_eq!(mp4_covered(&s[..s.len() - 30]), Some(0.75));
    }

    fn compare_mp4_prefixes(f: &[u8]) {
        let mut plan = None;
        for end in 0..=f.len() {
            if plan.is_none() {
                plan = Mp4Coverage::parse(&f[..end]);
            }
            assert_eq!(plan.as_ref().map(|p| p.covered(end)), mp4_covered(&f[..end]), "prefix {end}");
        }
    }

    #[test]
    fn mp4_plan_keeps_sample_boundaries_for_every_prefix() {
        let f = fragment(&[(100, 50), (0, 10), (200, 0), (50, 30)]);
        compare_mp4_prefixes(&f);
        compare_mp4_prefixes(&[bx(b"styp", b"msdh0000"), f.clone()].concat());
        compare_mp4_prefixes(&fragment(&[(0, 20), (0, 10)]));
        // The signed data offset and unsupported absolute base have the same interpretation.
        let mut negative = f.clone();
        let tr = negative.windows(4).position(|b| b == b"trun").unwrap();
        negative[tr + 12..tr + 16].copy_from_slice(&(-4i32).to_be_bytes());
        compare_mp4_prefixes(&negative);
        let mut absolute = f;
        let tf = absolute.windows(4).position(|b| b == b"tfhd").unwrap();
        absolute[tf + 7] |= 1;
        assert!(Mp4Coverage::parse(&absolute).is_none());
        compare_mp4_prefixes(&absolute);
    }

    #[test]
    fn mp4_plan_reads_default_sample_fields() {
        // tfhd: sample-description index, default duration and size. trun: first-sample flags,
        // no per-sample fields or data offset (the implicit position follows moof + mdat header).
        let mut tf = vec![0, 2, 0, 0x1a];
        for n in [1u32, 7, 100, 25] {
            tf.extend_from_slice(&n.to_be_bytes());
        }
        let mut tr = vec![0, 0, 0, 4];
        tr.extend_from_slice(&4u32.to_be_bytes());
        tr.extend_from_slice(&0u32.to_be_bytes());
        let moof = bx(b"moof", &bx(b"traf", &[bx(b"tfhd", &tf), bx(b"trun", &tr)].concat()));
        let f = [moof, bx(b"mdat", &[0xAB; 100])].concat();
        let data = f.len() - 100;
        let p = Mp4Coverage::parse(&f).unwrap();
        assert_eq!(p.covered(data + 24), 0.0);
        assert_eq!(p.covered(data + 25), 0.25);
        assert_eq!(p.covered(data + 75), 0.75);
        assert_eq!(p.covered(f.len()), 1.0);
        compare_mp4_prefixes(&f);
    }

    /// An EBML element with an 8-byte size (0x01 + 7 bytes), like YouTube's muxer.
    fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.push(0x01);
        out.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn webm_last_complete_block() {
        let block = |rel: i16, n: usize| el(&[0xA3], &[&[0x81][..], &rel.to_be_bytes(), &[0x80], &vec![0u8; n]].concat());
        let cluster = el(&[0x1F, 0x43, 0xB6, 0x75], &[el(&[0xE7], &[0x13, 0x88]), block(0, 40), block(40, 20), block(80, 20)].concat());
        let full = webm_covered(&cluster);
        assert_eq!(full, Some(5.08));
        let first_end = 12 + 9 + 2 + 9 + 4 + 40;
        assert_eq!(webm_covered(&cluster[..first_end - 1]), None);
        assert_eq!(webm_covered(&cluster[..first_end]), Some(5.0));
        assert_eq!(webm_covered(&cluster[..first_end + 9 + 4 + 20]), Some(5.04));
    }
}
