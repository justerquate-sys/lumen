//! Segment indexes: MP4 `sidx` and WebM `Cues` → byte ranges with media
//! times. Input is bytes `0..=indexRange.end` of the file (init + index).

#[derive(Clone, Debug, PartialEq)]
pub struct Seg {
    /// First byte (absolute file offset).
    pub start: u64,
    /// Last byte (inclusive).
    pub end: u64,
    pub t0: f64,
    pub t1: f64,
}

fn be(b: &[u8], p: usize, n: usize) -> Option<u64> {
    let s = b.get(p..p + n)?;
    Some(s.iter().fold(0u64, |v, &x| (v << 8) | x as u64))
}

/// ISO BMFF: finds the top-level `sidx` box and expands its references.
pub fn parse_sidx(b: &[u8]) -> Result<Vec<Seg>, &'static str> {
    let mut p = 0usize;
    while p + 8 <= b.len() {
        let mut size = be(b, p, 4).ok_or("short box")?;
        let typ = &b[p + 4..p + 8];
        let mut hdr = 8;
        if size == 1 {
            size = be(b, p + 8, 8).ok_or("short largesize")?;
            hdr = 16;
        } else if size == 0 {
            size = (b.len() - p) as u64;
        }
        if size < hdr as u64 {
            return Err("bad box size");
        }
        if typ == b"sidx" {
            let q0 = p + hdr;
            let version = *b.get(q0).ok_or("short sidx")?;
            let mut q = q0 + 4 + 4; // version/flags, reference_ID
            let timescale = be(b, q, 4).ok_or("short sidx")? as f64;
            q += 4;
            let (ept, first) = if version == 0 {
                let v = (be(b, q, 4).ok_or("short sidx")?, be(b, q + 4, 4).ok_or("short sidx")?);
                q += 8;
                v
            } else {
                let v = (be(b, q, 8).ok_or("short sidx")?, be(b, q + 8, 8).ok_or("short sidx")?);
                q += 16;
                v
            };
            let count = be(b, q + 2, 2).ok_or("short sidx")? as usize;
            q += 4;
            if timescale <= 0.0 || count == 0 {
                return Err("empty sidx");
            }
            let mut off = p as u64 + size + first;
            let mut t = ept;
            let mut out = Vec::with_capacity(count);
            for _ in 0..count {
                let r = be(b, q, 4).ok_or("truncated sidx")?;
                if r & 0x8000_0000 != 0 {
                    return Err("hierarchical sidx");
                }
                let dur = be(b, q + 4, 4).ok_or("truncated sidx")?;
                let len = r & 0x7fff_ffff;
                if len == 0 {
                    return Err("zero-size reference");
                }
                out.push(Seg { start: off, end: off + len - 1, t0: t as f64 / timescale, t1: (t + dur) as f64 / timescale });
                off += len;
                t += dur;
                q += 12;
            }
            return Ok(out);
        }
        p = p.checked_add(size as usize).ok_or("box overflow")?;
    }
    Err("no sidx")
}

/// EBML variable-length integer at `p`: (value, length). `id` keeps the marker bit.
fn vint(b: &[u8], p: usize, id: bool) -> Option<(u64, usize, bool)> {
    let first = *b.get(p)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    let mut v = if id { first as u64 } else { (first as u64) & (0xff >> len) };
    let mut all_ones = v == (0xff >> len) as u64;
    for i in 1..len {
        let x = *b.get(p + i)?;
        all_ones &= x == 0xff;
        v = (v << 8) | x as u64;
    }
    Some((v, len, !id && all_ones))
}

const SEGMENT: u64 = 0x1853_8067;
const INFO: u64 = 0x1549_A966;
const TIMECODE_SCALE: u64 = 0x2A_D7B1;
const CUES: u64 = 0x1C53_BB6B;
const CUE_POINT: u64 = 0xBB;
const CUE_TIME: u64 = 0xB3;
const CUE_TRACK_POSITIONS: u64 = 0xB7;
const CUE_CLUSTER_POSITION: u64 = 0xF1;
const CLUSTER: u64 = 0x1F43_B675;

#[derive(Default)]
struct Cues {
    seg_start: Option<u64>,
    scale: u64,
    points: Vec<(u64, Option<u64>)>,
    in_cues: bool,
}

fn walk(b: &[u8], from: usize, to: usize, st: &mut Cues, depth: u32) -> Result<(), &'static str> {
    let mut q = from;
    while q < to {
        let Some((id, il, _)) = vint(b, q, true) else { return Ok(()) };
        let Some((size, sl, unknown)) = vint(b, q + il, false) else { return Ok(()) };
        let d = q + il + sl;
        let end = if unknown { to } else { (d as u64).saturating_add(size).min(to as u64) as usize };
        match id {
            SEGMENT if depth == 0 => {
                st.seg_start = Some(d as u64);
                return walk(b, d, end, st, depth + 1);
            }
            CLUSTER => return Ok(()), // index comes before media; nothing useful after
            INFO | CUE_TRACK_POSITIONS => walk(b, d, end, st, depth + 1)?,
            CUES => {
                st.in_cues = true;
                walk(b, d, end, st, depth + 1)?;
            }
            CUE_POINT if st.in_cues => {
                st.points.push((u64::MAX, None));
                walk(b, d, end, st, depth + 1)?;
            }
            TIMECODE_SCALE => st.scale = be(b, d, size as usize).unwrap_or(1_000_000),
            CUE_TIME if !st.points.is_empty() => {
                if let Some(t) = be(b, d, size as usize) {
                    st.points.last_mut().unwrap().0 = t;
                }
            }
            CUE_CLUSTER_POSITION if !st.points.is_empty() => {
                let last = st.points.last_mut().unwrap();
                if last.1.is_none() {
                    last.1 = be(b, d, size as usize);
                }
            }
            _ => {}
        }
        if unknown {
            return Ok(());
        }
        q = d + size as usize;
    }
    Ok(())
}

/// WebM: Segment data offset + TimecodeScale + Cues (CueTime, first
/// CueClusterPosition) → cluster byte ranges. The last cluster ends at
/// `content_len - 1` and at `duration` seconds.
pub fn parse_cues(b: &[u8], content_len: u64, duration: f64) -> Result<Vec<Seg>, &'static str> {
    let mut st = Cues { scale: 1_000_000, ..Default::default() };
    walk(b, 0, b.len(), &mut st, 0)?;
    let seg = st.seg_start.ok_or("no Segment")?;
    let pts: Vec<(f64, u64)> = st
        .points
        .iter()
        .filter_map(|&(t, pos)| (t != u64::MAX).then_some(()).and(pos).map(|p| (t as f64 * st.scale as f64 / 1e9, seg + p)))
        .collect();
    if pts.is_empty() {
        return Err("no Cues");
    }
    let mut out = Vec::with_capacity(pts.len());
    for (i, &(t0, start)) in pts.iter().enumerate() {
        let (t1, end) = match pts.get(i + 1) {
            Some(&(t, s)) => (t, s.checked_sub(1).ok_or("bad cue order")?),
            None => (duration.max(t0), content_len.checked_sub(1).ok_or("no length")?),
        };
        if end < start {
            return Err("bad cue order");
        }
        out.push(Seg { start, end, t0, t1 });
    }
    Ok(out)
}

/// Drops segments that start at or after `duration` (keeps at least one).
pub fn clip(mut segs: Vec<Seg>, duration: f64) -> Vec<Seg> {
    if duration > 0.0 {
        let keep = segs.iter().take_while(|s| s.t0 < duration - 0.001).count().max(1);
        segs.truncate(keep);
    }
    segs
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Minimal EBML writer for tests: id bytes + 8-byte size + payload.
    pub fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.push(0x01);
        v.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
        v.extend_from_slice(body);
        v
    }
    fn uint(id: &[u8], n: u64, width: usize) -> Vec<u8> {
        let mut v = id.to_vec();
        v.push(0x80 | width as u8);
        v.extend_from_slice(&n.to_be_bytes()[8 - width..]);
        v
    }

    fn webm(points: &[(u64, u64)], scale: Option<u64>, unknown_segment: bool) -> (Vec<u8>, usize) {
        let ebml = el(&[0x1A, 0x45, 0xDF, 0xA3], &uint(&[0x42, 0x86], 1, 1));
        let mut info = Vec::new();
        if let Some(s) = scale {
            info.extend(uint(&[0x2A, 0xD7, 0xB1], s, 3));
        }
        let info = el(&[0x15, 0x49, 0xA9, 0x66], &info);
        let void = el(&[0xEC], &[0; 5]);
        let mut cues = Vec::new();
        for &(t, pos) in points {
            let mut tp = uint(&[0xF7], 1, 1);
            tp.extend(uint(&[0xF1], pos, 4));
            let mut cp = uint(&[0xB3], t, 4);
            cp.extend(el(&[0xB7], &tp));
            // A second track position must not override the first.
            let mut tp2 = uint(&[0xF7], 2, 1);
            tp2.extend(uint(&[0xF1], pos + 7, 4));
            cp.extend(el(&[0xB7], &tp2));
            cues.extend(el(&[0xBB], &cp));
        }
        let cues = el(&[0x1C, 0x53, 0xBB, 0x6B], &cues);
        let mut body = info;
        body.extend(void);
        body.extend(cues);
        let mut out = ebml;
        out.extend_from_slice(&[0x18, 0x53, 0x80, 0x67]);
        if unknown_segment {
            out.push(0x01);
            out.extend_from_slice(&[0xff; 7]);
        } else {
            out.push(0x01);
            out.extend_from_slice(&(body.len() as u64 + 1_000_000).to_be_bytes()[1..]);
        }
        let seg_start = out.len();
        out.extend(body);
        (out, seg_start)
    }

    #[test]
    fn webm_cues() {
        let (b, s) = webm(&[(0, 3000), (5005, 90000), (10010, 200000)], Some(1_000_000), false);
        let segs = parse_cues(&b, 1_000_000, 12.5).unwrap();
        let s = s as u64;
        assert_eq!(segs, vec![
            Seg { start: s + 3000, end: s + 89999, t0: 0.0, t1: 5.005 },
            Seg { start: s + 90000, end: s + 199999, t0: 5.005, t1: 10.01 },
            Seg { start: s + 200000, end: 999_999, t0: 10.01, t1: 12.5 },
        ]);
        // Unknown-size Segment, default TimecodeScale, and a truncated buffer.
        let (b, s) = webm(&[(0, 10), (4000, 20)], None, true);
        let segs = parse_cues(&b, 500, 9.0).unwrap();
        assert_eq!(segs[1], Seg { start: s as u64 + 20, end: 499, t0: 4.0, t1: 9.0 });
        assert!(parse_cues(&b[..b.len() - 30], 500, 9.0).is_ok_and(|v| v.len() == 1));
        // Microsecond timecode scale.
        let (b, _) = webm(&[(0, 0), (2_500_000, 50)], Some(1000), false);
        assert_eq!(parse_cues(&b, 100, 5.0).unwrap()[1].t0, 2.5);
        assert!(parse_cues(&[0x1A, 0x45], 10, 1.0).is_err());
        assert!(parse_cues(&[], 10, 1.0).is_err());
        let (b, _) = webm(&[], Some(1_000_000), false);
        assert_eq!(parse_cues(&b, 10, 1.0), Err("no Cues"));
    }

    pub fn sidx_box(version: u8, timescale: u32, first: u64, refs: &[(u32, u32)]) -> Vec<u8> {
        let mut body = vec![version, 0, 0, 0, 0, 0, 0, 1];
        body.extend(timescale.to_be_bytes());
        if version == 0 {
            body.extend(0u32.to_be_bytes());
            body.extend((first as u32).to_be_bytes());
        } else {
            body.extend(0u64.to_be_bytes());
            body.extend(first.to_be_bytes());
        }
        body.extend([0, 0]);
        body.extend((refs.len() as u16).to_be_bytes());
        for &(size, dur) in refs {
            body.extend(size.to_be_bytes());
            body.extend(dur.to_be_bytes());
            body.extend(0x9000_0000u32.to_be_bytes());
        }
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend(b"sidx");
        b.extend(body);
        b
    }

    #[test]
    fn mp4_sidx() {
        // ftyp(24) + moov(700) + sidx: like YouTube's MP4 DASH files.
        let mut b = vec![0, 0, 0, 24];
        b.extend(b"ftypdash");
        b.extend([0; 12]);
        b.extend(700u32.to_be_bytes());
        b.extend(b"moov");
        b.extend(vec![0; 692]);
        let sidx_at = b.len() as u64;
        let sx = sidx_box(0, 90000, 0, &[(1000, 450_000), (2000, 450_000), (500, 90_000)]);
        let sidx_end = sidx_at + sx.len() as u64;
        b.extend(sx);
        let segs = parse_sidx(&b).unwrap();
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0], Seg { start: sidx_end, end: sidx_end + 999, t0: 0.0, t1: 5.0 });
        assert_eq!(segs[2], Seg { start: sidx_end + 3000, end: sidx_end + 3499, t0: 10.0, t1: 11.0 });
        // Version 1 with a first_offset, and a 64-bit largesize box before it.
        let mut b = vec![0, 0, 0, 1];
        b.extend(b"free");
        b.extend(16u64.to_be_bytes());
        let sx = sidx_box(1, 1000, 10, &[(7, 1500)]);
        let end = 16 + sx.len() as u64;
        b.extend(sx);
        assert_eq!(parse_sidx(&b).unwrap(), vec![Seg { start: end + 10, end: end + 16, t0: 0.0, t1: 1.5 }]);
        assert_eq!(parse_sidx(&b[..20]), Err("no sidx"));
        assert!(parse_sidx(&[0, 0, 0, 3, b'a', b'b', b'c', b'd']).is_err());
    }

    #[test]
    fn clipping() {
        let s = |t0: f64| Seg { start: 0, end: 1, t0, t1: t0 + 5.0 };
        let v = clip(vec![s(0.0), s(5.0), s(10.0), s(15.0)], 15.0);
        assert_eq!(v.len(), 3);
        assert_eq!(clip(vec![s(0.0)], 0.0).len(), 1);
    }
}
