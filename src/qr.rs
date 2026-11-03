//! A small QR code encoder (ISO/IEC 18004) for the TV sign-in screen, which shows the device-flow
//! verification URL as a code to scan with a phone. Byte mode, error correction level M, versions
//! 1–10 (up to 213 bytes), mask chosen by the standard penalty rules.

use std::fmt::Write;

/// Per version at level M: (ECC codewords per block, group-1 blocks, data codewords per group-1
/// block, group-2 blocks). Group-2 blocks hold one data codeword more than group-1 blocks.
#[rustfmt::skip]
const BLOCKS: [(usize, usize, usize, usize); 10] = [
    (10, 1, 16, 0), (16, 1, 28, 0), (26, 1, 44, 0), (18, 2, 32, 0), (24, 2, 43, 0),
    (16, 4, 27, 0), (18, 4, 31, 0), (22, 2, 38, 2), (22, 3, 36, 2), (26, 4, 43, 1),
];

/// Alignment pattern centre coordinates per version.
#[rustfmt::skip]
const ALIGN: [&[usize]; 10] = [
    &[], &[6, 18], &[6, 22], &[6, 26], &[6, 30],
    &[6, 34], &[6, 22, 38], &[6, 24, 42], &[6, 26, 46], &[6, 28, 50],
];

/// A QR code: `size` modules per side, `dark(x, y)`.
pub struct Qr {
    pub size: usize,
    bits: Vec<bool>,
}

impl Qr {
    /// Byte mode, error correction level M, the smallest version 1..=10 that fits, best mask by the
    /// standard penalty rules. None if the data does not fit version 10-M.
    pub fn encode(data: &[u8]) -> Option<Qr> {
        let capacity = |v: usize| {
            let (_, b1, d1, b2) = BLOCKS[v - 1];
            b1 * d1 + b2 * (d1 + 1)
        };
        let count_bits = |v: usize| if v < 10 { 8 } else { 16 };
        let ver = (1..=10).find(|&v| 4 + count_bits(v) + 8 * data.len() <= capacity(v) * 8)?;
        let cap = capacity(ver);

        // Bit stream: mode 0100, character count, data, terminator, zero padding to a byte.
        let mut bits = Vec::with_capacity(cap * 8);
        let mut push =
            |value: usize, n: usize| bits.extend((0..n).rev().map(|i| value >> i & 1 == 1));
        push(0b0100, 4);
        push(data.len(), count_bits(ver));
        data.iter().for_each(|&b| push(b as usize, 8));
        let terminator = (cap * 8 - bits.len()).min(4);
        bits.extend(std::iter::repeat_n(false, terminator));
        bits.resize(bits.len().div_ceil(8) * 8, false);
        let mut codewords: Vec<u8> = bits
            .chunks(8)
            .map(|c| c.iter().fold(0, |a, &b| a << 1 | b as u8))
            .collect();
        let mut pad = 0xEC;
        while codewords.len() < cap {
            codewords.push(pad);
            pad ^= 0xEC ^ 0x11;
        }

        // Split into blocks, add Reed–Solomon ECC, interleave.
        let (ecc, b1, d1, b2) = BLOCKS[ver - 1];
        let generator = rs_generator(ecc);
        let mut rest = &codewords[..];
        let blocks: Vec<(&[u8], Vec<u8>)> = (0..b1 + b2)
            .map(|i| {
                let (block, tail) = rest.split_at(d1 + usize::from(i >= b1));
                rest = tail;
                (block, rs_remainder(block, &generator))
            })
            .collect();
        let mut stream = Vec::new();
        for i in 0..=d1 {
            stream.extend(blocks.iter().filter_map(|(d, _)| d.get(i)));
        }
        for i in 0..ecc {
            stream.extend(blocks.iter().map(|(_, e)| e[i]));
        }

        let mut grid = Grid::new(ver);
        grid.place(&stream);
        let best = (0..8)
            .min_by_key(|&m| {
                let mut g = grid.clone();
                g.mask(m);
                g.penalty()
            })
            .unwrap_or(0);
        grid.mask(best);
        Some(Qr {
            size: grid.size,
            bits: grid.dark,
        })
    }

    pub fn dark(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.bits[y * self.size + x]
    }

    /// One SVG path `d` for all dark modules in module units (each horizontal run is one
    /// "M{x} {y}h{n}v1h-{n}z"), without the quiet zone; draw it in
    /// `viewBox="-4 -4 {size+8} {size+8}"`.
    pub fn svg_path(&self) -> String {
        let mut d = String::new();
        for y in 0..self.size {
            let mut x = 0;
            while x < self.size {
                let start = x;
                while self.dark(x, y) {
                    x += 1;
                }
                if x > start {
                    let n = x - start;
                    let _ = write!(d, "M{start} {y}h{n}v1h-{n}z");
                } else {
                    x += 1;
                }
            }
        }
        d
    }
}

/// The symbol under construction: module colours plus which modules are function patterns.
#[derive(Clone)]
struct Grid {
    size: usize,
    dark: Vec<bool>,
    func: Vec<bool>,
}

impl Grid {
    /// An empty symbol with finder, separator, timing and alignment patterns, the dark module, a
    /// reserved format area and (version ≥ 7) the version information.
    fn new(ver: usize) -> Grid {
        let size = 17 + 4 * ver;
        let blank = vec![false; size * size];
        let mut g = Grid {
            size,
            dark: blank.clone(),
            func: blank,
        };
        for i in 0..size {
            g.set(6, i, i % 2 == 0);
            g.set(i, 6, i % 2 == 0);
        }
        for (cx, cy) in [(3, 3), (size - 4, 3), (3, size - 4)] {
            g.square(cx, cy, 4, |d| d != 2 && d != 4);
        }
        let pos = ALIGN[ver - 1];
        let last = pos.len().saturating_sub(1);
        for (i, &x) in pos.iter().enumerate() {
            for (j, &y) in pos.iter().enumerate() {
                if (i, j) != (0, 0) && (i, j) != (0, last) && (i, j) != (last, 0) {
                    g.square(x, y, 2, |d| d != 1);
                }
            }
        }
        g.format(0);
        if ver >= 7 {
            let bits = ver << 12 | bch(ver, 0x1F25, 12);
            for i in 0..18 {
                let (a, b) = (size - 11 + i % 3, i / 3);
                g.set(a, b, bits >> i & 1 == 1);
                g.set(b, a, bits >> i & 1 == 1);
            }
        }
        g
    }

    fn set(&mut self, x: usize, y: usize, dark: bool) {
        self.dark[y * self.size + x] = dark;
        self.func[y * self.size + x] = true;
    }

    /// A square pattern of radius `r` around (cx, cy), clipped to the symbol; `dark(ring distance)`.
    fn square(&mut self, cx: usize, cy: usize, r: isize, dark: impl Fn(isize) -> bool) {
        for dy in -r..=r {
            for dx in -r..=r {
                let (x, y) = (cx.wrapping_add_signed(dx), cy.wrapping_add_signed(dy));
                if x < self.size && y < self.size {
                    self.set(x, y, dark(dx.abs().max(dy.abs())));
                }
            }
        }
    }

    /// Both copies of the 15-bit format information for level M and `mask`, plus the dark module.
    fn format(&mut self, mask: usize) {
        let bits = format_bits(mask);
        let bit = |i: usize| bits >> i & 1 == 1;
        let n = self.size;
        for i in 0..6 {
            self.set(8, i, bit(i));
        }
        self.set(8, 7, bit(6));
        self.set(8, 8, bit(7));
        self.set(7, 8, bit(8));
        for i in 9..15 {
            self.set(14 - i, 8, bit(i));
        }
        for i in 0..8 {
            self.set(n - 1 - i, 8, bit(i));
        }
        for i in 8..15 {
            self.set(8, n - 15 + i, bit(i));
        }
        self.set(8, n - 8, true);
    }

    /// Codeword bits in the zigzag order (two-column strips, right to left, skipping column 6).
    /// Remainder bits stay light.
    fn place(&mut self, stream: &[u8]) {
        let n = self.size;
        let mut i = 0;
        for strip in 0..(n - 1) / 2 {
            let right = n - 1 - 2 * strip - usize::from(2 * strip >= n - 7);
            let upward = (right + 1) & 2 == 0;
            for vert in 0..n {
                let y = if upward { n - 1 - vert } else { vert };
                for x in [right, right - 1] {
                    if !self.func[y * n + x] && i < stream.len() * 8 {
                        self.dark[y * n + x] = stream[i / 8] >> (7 - i % 8) & 1 == 1;
                        i += 1;
                    }
                }
            }
        }
    }

    /// XOR mask pattern `m` over the data modules and write the matching format information.
    fn mask(&mut self, m: usize) {
        for y in 0..self.size {
            for x in 0..self.size {
                let flip = match m {
                    0 => (x + y) % 2 == 0,
                    1 => y % 2 == 0,
                    2 => x % 3 == 0,
                    3 => (x + y) % 3 == 0,
                    4 => (x / 3 + y / 2) % 2 == 0,
                    5 => x * y % 2 + x * y % 3 == 0,
                    6 => (x * y % 2 + x * y % 3) % 2 == 0,
                    _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
                };
                let k = y * self.size + x;
                if flip && !self.func[k] {
                    self.dark[k] = !self.dark[k];
                }
            }
        }
        self.format(m);
    }

    /// The four penalty rules: runs of ≥ 5, 2×2 blocks, finder-like patterns, dark balance.
    fn penalty(&self) -> usize {
        let finder = [1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0].map(|b| b == 1);
        let n = self.size;
        let at = |x: usize, y: usize| self.dark[y * n + x];
        let mut p = 0;
        for horizontal in [true, false] {
            for a in 0..n {
                let line: Vec<bool> = (0..n)
                    .map(|b| if horizontal { at(b, a) } else { at(a, b) })
                    .collect();
                let mut run = 1;
                for i in 1..=n {
                    if i < n && line[i] == line[i - 1] {
                        run += 1;
                    } else {
                        p += if run >= 5 { run - 2 } else { 0 };
                        run = 1;
                    }
                }
                let like_finder =
                    |w: &&[bool]| w.iter().eq(&finder) || w.iter().eq(finder.iter().rev());
                p += 40 * line.windows(11).filter(like_finder).count();
            }
        }
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let c = at(x, y);
                p += 3 * usize::from(
                    c == at(x + 1, y) && c == at(x, y + 1) && c == at(x + 1, y + 1),
                );
            }
        }
        let dark = self.dark.iter().filter(|&&d| d).count();
        p + (dark * 100 / (n * n)).abs_diff(50) / 5 * 10
    }
}

/// `data` followed by its BCH remainder modulo `poly` (degree `deg`); returns only the remainder.
fn bch(data: usize, poly: usize, deg: u32) -> usize {
    let mut rem = data;
    for _ in 0..deg {
        rem = (rem << 1) ^ ((rem >> (deg - 1) & 1) * poly);
    }
    rem & ((1 << deg) - 1)
}

/// The 15 format bits for level M (bits 00) and `mask`.
fn format_bits(mask: usize) -> usize {
    (mask << 10 | bch(mask, 0x537, 10)) ^ 0x5412
}

/// Multiplication in GF(256) with the reducing polynomial 0x11D.
fn gf_mul(x: u8, y: u8) -> u8 {
    let mut z: u16 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= (y as u16 >> i & 1) * x as u16;
    }
    z as u8
}

/// Coefficients (highest degree first, leading 1 omitted) of ∏ (x − α^i), i < `degree`.
fn rs_generator(degree: usize) -> Vec<u8> {
    let mut g = vec![0; degree];
    g[degree - 1] = 1;
    let mut root = 1;
    for _ in 0..degree {
        for j in 0..degree {
            g[j] = gf_mul(g[j], root);
            if j + 1 < degree {
                g[j] ^= g[j + 1];
            }
        }
        root = gf_mul(root, 2);
    }
    g
}

/// The ECC codewords: `data`·x^n mod generator.
fn rs_remainder(data: &[u8], generator: &[u8]) -> Vec<u8> {
    let mut r = vec![0; generator.len()];
    for &b in data {
        let factor = b ^ r.remove(0);
        r.push(0);
        r.iter_mut()
            .zip(generator)
            .for_each(|(x, &g)| *x ^= gf_mul(g, factor));
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_smallest_version() {
        let size = |n: usize| Qr::encode(&vec![b'a'; n]).map(|q| q.size);
        let caps = [14, 26, 42, 62, 84, 106, 122, 152, 180, 213];
        for (v, &cap) in caps.iter().enumerate() {
            assert_eq!(size(cap), Some(21 + 4 * v), "{cap} bytes");
            assert_eq!(
                size(cap + 1),
                (v < 9).then_some(25 + 4 * v),
                "{} bytes",
                cap + 1
            );
        }
        assert_eq!(size(0), Some(21));
        assert!(Qr::encode(&[0; 214]).is_none());
    }

    #[test]
    fn reed_solomon() {
        // Thonky's 1-M "HELLO WORLD" data codewords and their ten ECC codewords.
        let data = [
            32, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236, 17, 236, 17,
        ];
        assert_eq!(
            rs_remainder(&data, &rs_generator(10)),
            [196, 35, 39, 119, 235, 215, 231, 226, 93, 23]
        );
        // Every codeword polynomial vanishes at the generator's roots α^0..α^25.
        let data: Vec<u8> = (0..44u8).map(|i| i.wrapping_mul(37) ^ 0x5A).collect();
        let word: Vec<u8> = data
            .iter()
            .copied()
            .chain(rs_remainder(&data, &rs_generator(26)))
            .collect();
        let mut root = 1;
        for _ in 0..26 {
            assert_eq!(word.iter().fold(0, |acc, &c| gf_mul(acc, root) ^ c), 0);
            root = gf_mul(root, 2);
        }
    }

    #[test]
    fn format_and_version_bits() {
        assert_eq!(format_bits(0), 0b101010000010010);
        assert_eq!(format_bits(4), 0b100010111111001);
        assert_eq!(format_bits(7), 0b100101010100000);
        assert_eq!(7 << 12 | bch(7, 0x1F25, 12), 0x07C94);
        assert_eq!(10 << 12 | bch(10, 0x1F25, 12), 0x0A4D3);
    }

    #[test]
    fn layout_and_svg() {
        let q = Qr::encode(b"https://www.google.com/device").unwrap();
        assert_eq!(q.size, 29);
        // Finder corners, timing pattern, dark module, nothing outside the symbol.
        assert!(q.dark(0, 0) && q.dark(28, 0) && q.dark(0, 28));
        assert!(
            q.dark(6, 6) && !q.dark(7, 7) && q.dark(8, 6) && !q.dark(9, 6) && q.dark(8, q.size - 8)
        );
        assert!(!q.dark(29, 0) && !q.dark(0, 29));
        let path = q.svg_path();
        assert!(path.starts_with("M0 0h7v1h-7z"));
        let modules: usize = path
            .split('h')
            .skip(1)
            .step_by(2)
            .map(|s| s.split('v').next().unwrap().parse::<usize>().unwrap())
            .sum();
        assert_eq!(modules, q.bits.iter().filter(|&&d| d).count());
    }
}
