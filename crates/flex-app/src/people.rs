//! Who's who, from MoGe-2's scene head: where each person is, their box, head, hands and feet,
//! and which person each pixel belongs to. The head gives a heat map that peaks at each person's
//! middle and, for each person's pixel, the way to their middle: each peak is a person, and each
//! pixel goes with the peak it points nearest. A person keeps their slot while they're in shot,
//! so an effect can give each their own colour or anything else that sticks to them.

/// The most people effects are told about (the biggest, when there are more).
pub const MAX_PEOPLE: usize = 8;
/// How hot the heat map has to be for a person's middle.
const PEAK: f32 = 0.3;
/// How sure of a person a pixel has to be to count as theirs.
const PERSON: f32 = 0.4;
/// Middles closer than this (in picture heights) are the same person.
const APART: f32 = 0.035;
/// The most middles looked at (the hottest), before the biggest MAX_PEOPLE are kept.
const MOST_MIDDLES: usize = 64;
/// The fewest of the picture's pixels (as a share) a person can have.
const SMALLEST: f32 = 0.0015;
/// How sure of a hand, head or foot a pixel has to be to count towards where it is.
const PART: f32 = 0.3;
/// How far (in their own heights, plus a little) a person can move between maps and still be the
/// same person, how many maps they're kept missing, and how much each new map moves them.
const REACH: f32 = 0.6;
const KEPT_MISSING: u32 = 3;
const FOLLOW: f32 = 0.6;

/// Where a part of someone is: its middle (x, y, as fractions of the picture, rows from the top),
/// how big it is (its radius, in picture heights) and how sure that is, from 0 to 1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spot {
    pub at: [f32; 2],
    pub size: f32,
    pub seen: f32,
}

/// One person, in fractions of the picture with rows from the top.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Person {
    /// 1 while they're found, fading over a few maps once they're lost, and 0 for an empty slot.
    pub seen: f32,
    pub centre: [f32; 2],
    /// Left, top, right, bottom.
    pub bounds: [f32; 4],
    pub head: Spot,
    /// Picture-left first.
    pub hands: [Spot; 2],
    pub feet: [Spot; 2],
}

pub type People = [Person; MAX_PEOPLE];

/// The head's planes for people, each `size`, rows from the top: how much of a person each pixel
/// is; the heat map; the way to their middle (x and y, in picture heights); and how much of a
/// head, hand and foot it is.
pub struct Planes<'a> {
    pub person: &'a [f32],
    pub heat: &'a [f32],
    pub to_x: &'a [f32],
    pub to_y: &'a [f32],
    pub head: &'a [f32],
    pub hand: &'a [f32],
    pub foot: &'a [f32],
}

/// The people in one picture, biggest first, and for each pixel which of them it is (1 for the
/// first, 0 for nobody).
pub fn find(planes: &Planes, size: (usize, usize)) -> (Vec<Person>, Vec<u8>) {
    let (w, h) = size;
    let mut middles = peaks(planes.heat, size);
    middles.truncate(MOST_MIDDLES);
    if middles.is_empty() {
        // Someone the heat map missed: one person, wherever the person pixels are.
        let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
        for (i, p) in planes.person.iter().enumerate() {
            if *p >= PERSON {
                sx += (i % w) as f32;
                sy += (i / w) as f32;
                n += 1.0;
            }
        }
        if n > 0.0 {
            middles.push((sx / n, sy / n));
        }
    }
    let mut whose = vec![0u8; w * h];
    let mut count = vec![0usize; middles.len()];
    if !middles.is_empty() {
        for (i, p) in planes.person.iter().enumerate() {
            if *p < PERSON {
                continue;
            }
            let (x, y) = ((i % w) as f32 + planes.to_x[i] * h as f32, (i / w) as f32 + planes.to_y[i] * h as f32);
            let nearest = middles.iter().enumerate().map(|(n, m)| (n, (m.0 - x).powi(2) + (m.1 - y).powi(2))).min_by(|a, b| a.1.total_cmp(&b.1)).map_or(0, |(n, _)| n);
            whose[i] = nearest as u8 + 1;
            count[nearest] += 1;
        }
    }
    // The biggest, renumbered.
    let mut order: Vec<usize> = (0..middles.len()).filter(|n| count[*n] as f32 >= SMALLEST * (w * h) as f32).collect();
    order.sort_by_key(|n| std::cmp::Reverse(count[*n]));
    order.truncate(MAX_PEOPLE);
    let mut renumber = [0u8; 256];
    for (new, old) in order.iter().enumerate() {
        renumber[old + 1] = new as u8 + 1;
    }
    for who in &mut whose {
        *who = renumber[*who as usize];
    }
    let middles: Vec<(f32, f32)> = order.iter().map(|old| middles[*old]).collect();
    (describe(planes, &whose, &middles, size), whose)
}

/// Where the heat map peaks: the hottest point within a few pixels, hotter than PEAK, and not
/// too near a hotter one. In pixels, hottest first.
fn peaks(heat: &[f32], (w, h): (usize, usize)) -> Vec<(f32, f32)> {
    let r = ((APART * h as f32 / 2.0).round() as usize).max(2);
    // The most within r along each row, then down each column of that.
    let mut across = vec![0.0f32; w * h];
    for y in 0..h {
        let row = &heat[y * w..][..w];
        for x in 0..w {
            across[y * w + x] = row[x.saturating_sub(r)..(x + r + 1).min(w)].iter().copied().fold(f32::MIN, f32::max);
        }
    }
    let mut found = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let v = heat[y * w + x];
            if v <= PEAK {
                continue;
            }
            let most = (y.saturating_sub(r)..(y + r + 1).min(h)).map(|yy| across[yy * w + x]).fold(f32::MIN, f32::max);
            if v >= most {
                found.push((v, x as f32, y as f32));
            }
        }
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    let apart = (APART * h as f32).powi(2);
    let mut kept: Vec<(f32, f32)> = Vec::new();
    for (_, x, y) in found {
        if kept.iter().all(|k| (k.0 - x).powi(2) + (k.1 - y).powi(2) > apart) {
            kept.push((x, y));
        }
    }
    kept
}

/// Each person's box and parts, from the pixels marked as theirs (1 for the first of `middles`).
fn describe(planes: &Planes, whose: &[u8], middles: &[(f32, f32)], (w, h): (usize, usize)) -> Vec<Person> {
    #[derive(Default)]
    struct Pixels {
        bounds: Option<(usize, usize, usize, usize)>,
        parts: [Vec<(f32, f32, f32)>; 3],
    }
    let mut each: Vec<Pixels> = middles.iter().map(|_| Pixels::default()).collect();
    for (i, who) in whose.iter().enumerate() {
        let Some(p) = (*who as usize).checked_sub(1).and_then(|n| each.get_mut(n)) else { continue };
        let (x, y) = (i % w, i / w);
        if planes.person[i] >= 0.5 {
            p.bounds = Some(p.bounds.map_or((x, y, x, y), |(x0, y0, x1, y1)| (x0.min(x), y0.min(y), x1.max(x), y1.max(y))));
        }
        for (plane, list) in [planes.head, planes.hand, planes.foot].into_iter().zip(&mut p.parts) {
            if plane[i] >= PART {
                list.push((x as f32, y as f32, plane[i]));
            }
        }
    }
    each.iter().zip(middles).map(|(p, middle)| one(p.bounds, &p.parts, *middle, (w, h))).collect()
}

/// One person's box (in pixels, if any were sure), head, hands and feet.
fn one(bounds: Option<(usize, usize, usize, usize)>, [heads, hands, feet]: &[Vec<(f32, f32, f32)>; 3], middle: (f32, f32), (w, h): (usize, usize)) -> Person {
    let (m0, m1) = (middle.0 as usize, middle.1 as usize);
    let (x0, y0, x1, y1) = bounds.unwrap_or((m0, m1, m0, m1));
    let (fw, fh) = (w as f32, h as f32);
    let spot = |points: &[(f32, f32, f32)]| -> Spot {
        let total: f32 = points.iter().map(|p| p.2).sum();
        if total <= 0.0 {
            return Spot::default();
        }
        let x = points.iter().map(|p| p.0 * p.2).sum::<f32>() / total;
        let y = points.iter().map(|p| p.1 * p.2).sum::<f32>() / total;
        // A head of a hundredth of the picture's height across is all but certain.
        let size = (total / std::f32::consts::PI).sqrt() / fh;
        Spot { at: [(x + 0.5) / fw, (y + 0.5) / fh], size, seen: (size / 0.012).clamp(0.0, 1.0) }
    };
    Person {
        seen: 1.0,
        centre: [(middle.0 + 0.5) / fw, (middle.1 + 0.5) / fh],
        bounds: [x0 as f32 / fw, y0 as f32 / fh, (x1 + 1) as f32 / fw, (y1 + 1) as f32 / fh],
        head: spot(heads),
        hands: pair(hands, fh).map(|p| spot(&p)),
        feet: pair(feet, fh).map(|p| spot(&p)),
    }
}

/// Points split into two groups (picture-left first) by where they are, for two hands or feet;
/// one group, and an empty one, when they're all together.
fn pair(points: &[(f32, f32, f32)], h: f32) -> [Vec<(f32, f32, f32)>; 2] {
    if points.is_empty() {
        return [Vec::new(), Vec::new()];
    }
    let mut a = *points.iter().min_by(|p, q| p.0.total_cmp(&q.0)).unwrap();
    let mut b = *points.iter().max_by(|p, q| p.0.total_cmp(&q.0)).unwrap();
    let mut groups = [Vec::new(), Vec::new()];
    for _ in 0..5 {
        groups = [Vec::new(), Vec::new()];
        for p in points {
            let near_a = (p.0 - a.0).powi(2) + (p.1 - a.1).powi(2) <= (p.0 - b.0).powi(2) + (p.1 - b.1).powi(2);
            groups[usize::from(!near_a)].push(*p);
        }
        let mean = |g: &[(f32, f32, f32)], or: (f32, f32, f32)| {
            let total: f32 = g.iter().map(|p| p.2).sum();
            if total <= 0.0 { or } else { (g.iter().map(|p| p.0 * p.2).sum::<f32>() / total, g.iter().map(|p| p.1 * p.2).sum::<f32>() / total, 1.0) }
        };
        a = mean(&groups[0], a);
        b = mean(&groups[1], b);
    }
    // Two hands touching are one spot.
    if (a.0 - b.0).hypot(a.1 - b.1) < 0.04 * h || groups.iter().any(Vec::is_empty) {
        let all: Vec<_> = points.to_vec();
        return if a.0 <= b.0 { [all, Vec::new()] } else { [Vec::new(), all] };
    }
    if a.0 > b.0 {
        groups.swap(0, 1);
    }
    groups
}

/// Keeps each person in the same slot from one map to the next.
#[derive(Default)]
pub struct Tracker {
    slots: [Option<(Person, u32)>; MAX_PEOPLE],
}

impl Tracker {
    /// A new shot: nobody's the same.
    pub fn cut(&mut self) {
        self.slots = Default::default();
    }

    /// Takes in the people found in a picture and which each pixel is (as `find` gives them),
    /// and returns everyone in their slots, with each pixel's slot (plus 1) in place of its
    /// number in `found`.
    pub fn update(&mut self, found: &[Person], whose: &mut [u8]) -> People {
        let mut pairs = Vec::new();
        for (n, new) in found.iter().enumerate() {
            for (slot, kept) in self.slots.iter().enumerate() {
                if let Some((old, _)) = kept {
                    let tall = (old.bounds[3] - old.bounds[1]).max(0.05);
                    let away = (new.centre[0] - old.centre[0]).hypot(new.centre[1] - old.centre[1]) / tall;
                    if away < REACH {
                        pairs.push((away, n, slot));
                    }
                }
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut slot_of = [None; MAX_PEOPLE];
        let mut taken = [false; MAX_PEOPLE];
        for (_, n, slot) in pairs {
            if slot_of[n].is_none() && !taken[slot] {
                slot_of[n] = Some(slot);
                taken[slot] = true;
            }
        }
        for slot in slot_of.iter_mut().take(found.len()) {
            if slot.is_none()
                && let Some(free) = (0..MAX_PEOPLE).find(|s| !taken[*s] && self.slots[*s].is_none())
            {
                *slot = Some(free);
                taken[free] = true;
            }
        }
        // Whoever wasn't found fades, and goes after a few maps.
        for (slot, kept) in self.slots.iter_mut().enumerate() {
            if taken[slot] {
                continue;
            }
            if let Some((person, missing)) = kept {
                *missing += 1;
                person.seen *= 0.5;
                if *missing > KEPT_MISSING {
                    *kept = None;
                }
            }
        }
        for (n, new) in found.iter().enumerate() {
            let Some(slot) = slot_of[n] else { continue };
            let person = match self.slots[slot] {
                Some((old, _)) => blend(&old, new),
                None => *new,
            };
            self.slots[slot] = Some((person, 0));
        }
        let mut renumber = [0u8; MAX_PEOPLE + 1];
        for (n, slot) in slot_of.iter().enumerate().take(found.len()) {
            renumber[n + 1] = slot.map_or(0, |s| s as u8 + 1);
        }
        for who in whose.iter_mut() {
            *who = renumber.get(*who as usize).copied().unwrap_or(0);
        }
        std::array::from_fn(|slot| self.slots[slot].map(|(p, _)| p).unwrap_or_default())
    }
}

/// Someone moved most of the way from where they were to where they're found now, so their
/// spots don't jitter. A part that's only just been found jumps straight there.
fn blend(old: &Person, new: &Person) -> Person {
    let mix = |a: f32, b: f32| a + (b - a) * FOLLOW;
    let mix2 = |a: [f32; 2], b: [f32; 2]| [mix(a[0], b[0]), mix(a[1], b[1])];
    let spot = |a: Spot, b: Spot| if a.seen < 0.1 || b.seen < 0.1 { b } else { Spot { at: mix2(a.at, b.at), size: mix(a.size, b.size), seen: b.seen } };
    Person {
        seen: new.seen,
        centre: mix2(old.centre, new.centre),
        bounds: std::array::from_fn(|i| mix(old.bounds[i], new.bounds[i])),
        head: spot(old.head, new.head),
        hands: std::array::from_fn(|i| spot(old.hands[i], new.hands[i])),
        feet: std::array::from_fn(|i| spot(old.feet[i], new.feet[i])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 120;
    const H: usize = 80;

    /// Two people side by side, as the head would give them: each a block of person pixels
    /// pointing at its middle, with a head on top, hands at its sides and feet below.
    fn two_people(shift: f32) -> Vec<Vec<f32>> {
        let mut planes = vec![vec![0.0; W * H]; 7];
        for (cx, cy) in [(30.0 + shift, 40.0), (85.0 + shift, 42.0)] {
            for y in 0..H {
                for x in 0..W {
                    let i = y * W + x;
                    let (dx, dy) = (cx - x as f32, cy - y as f32);
                    planes[1][i] = f32::max(planes[1][i], (-(dx * dx + dy * dy) / (2.0 * 3.0 * 3.0)).exp());
                    if dx.abs() < 12.0 && dy.abs() < 30.0 {
                        planes[0][i] = 1.0;
                        planes[2][i] = dx / H as f32;
                        planes[3][i] = dy / H as f32;
                        if dy > 22.0 {
                            planes[4][i] = 1.0;
                        }
                        if dy.abs() < 3.0 && dx.abs() > 9.0 {
                            planes[5][i] = 1.0;
                        }
                        if dy < -26.0 && dx.abs() > 3.0 {
                            planes[6][i] = 1.0;
                        }
                    }
                }
            }
        }
        planes
    }

    fn planes(p: &[Vec<f32>]) -> Planes<'_> {
        Planes { person: &p[0], heat: &p[1], to_x: &p[2], to_y: &p[3], head: &p[4], hand: &p[5], foot: &p[6] }
    }

    #[test]
    fn each_person_gets_their_own_pixels_and_parts() {
        let p = two_people(0.0);
        let (people, whose) = find(&planes(&p), (W, H));
        assert_eq!(people.len(), 2);
        let left = people.iter().position(|p| p.centre[0] < 0.5).unwrap() as u8 + 1;
        assert_eq!(whose[40 * W + 25], left);
        assert_ne!(whose[40 * W + 90], left);
        assert_eq!(whose[40 * W + 60], 0);
        let person = &people[left as usize - 1];
        assert!((person.centre[0] - 30.5 / W as f32).abs() < 0.02, "{:?}", person.centre);
        assert!(person.head.seen > 0.0 && person.head.at[1] < person.centre[1], "{:?}", person.head);
        let [l, r] = person.hands;
        assert!(l.seen > 0.0 && r.seen > 0.0 && l.at[0] < person.centre[0] && r.at[0] > person.centre[0], "{l:?} {r:?}");
        assert!(person.feet.iter().all(|f| f.at[1] > person.centre[1]));
    }

    #[test]
    fn people_keep_their_slots_as_they_move() {
        let mut tracker = Tracker::default();
        let p = two_people(0.0);
        let (found, mut whose) = find(&planes(&p), (W, H));
        let first = tracker.update(&found, &mut whose);
        let left_slot = whose[40 * W + 25];
        let q = two_people(4.0);
        let (found, mut whose) = find(&planes(&q), (W, H));
        let later = tracker.update(&found, &mut whose);
        assert_eq!(whose[40 * W + 29], left_slot);
        let s = left_slot as usize - 1;
        assert!(later[s].centre[0] > first[s].centre[0]);
        // Gone for longer than they're kept: the slot's free.
        for _ in 0..=KEPT_MISSING {
            tracker.update(&[], &mut []);
        }
        assert!(tracker.slots.iter().all(Option::is_none));
    }
}
