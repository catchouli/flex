//! The networks taking turns on the GPU. Depth, faces, movement, the camera and restyling each
//! have a thread, and left to themselves they'd run at once and each take as long as all of them. Here one runs at a
//! time, in the order they asked, and they're told how long to rest so that together they leave
//! the GPU time to draw the picture.

use std::sync::{Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Net {
    Depth,
    Faces,
    Flow,
    Camera,
    Restyle,
}

const NETS: usize = 5;
const NAMES: [&str; NETS] = ["depth", "faces", "movement", "camera", "restyle"];
/// The most of the GPU's time the networks get between them.
const BUDGET: f32 = 0.8;
/// How quickly the averages follow.
const FOLLOW: f32 = 0.1;
/// A network that hasn't run for this long doesn't count.
const ACTIVE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default)]
struct Seen {
    /// How long a run takes, and how long one waits for its turn (ms), on average.
    ms: f32,
    waited: f32,
    /// The time it would like between runs, if the GPU has room (ms).
    every: f32,
    last: Option<Instant>,
}

#[derive(Default)]
struct State {
    next: u64,
    serving: u64,
    nets: [Seen; NETS],
}

static TURNS: LazyLock<(Mutex<State>, Condvar)> = LazyLock::new(Default::default);

fn state() -> MutexGuard<'static, State> {
    TURNS.0.lock().unwrap_or_else(|e| e.into_inner())
}

fn eased(average: f32, new: f32) -> f32 {
    if average == 0.0 { new } else { average + (new - average) * FOLLOW }
}

/// A network's turn on the GPU, until it's dropped.
pub struct Turn {
    net: Net,
    started: Instant,
}

/// Waits for `net`'s turn.
pub fn turn(net: Net) -> Turn {
    let asked = Instant::now();
    let mut s = state();
    let ticket = s.next;
    s.next += 1;
    while s.serving != ticket {
        s = TURNS.1.wait(s).unwrap_or_else(|e| e.into_inner());
    }
    let seen = &mut s.nets[net as usize];
    seen.waited = eased(seen.waited, asked.elapsed().as_secs_f32() * 1000.0);
    Turn { net, started: Instant::now() }
}

impl Turn {
    /// How long this turn has lasted (ms): the network's own time on the GPU.
    pub fn ms(&self) -> f32 {
        self.started.elapsed().as_secs_f32() * 1000.0
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let ms = self.ms();
        let mut s = state();
        let seen = &mut s.nets[self.net as usize];
        seen.ms = eased(seen.ms, ms);
        seen.last = Some(Instant::now());
        s.serving += 1;
        TURNS.1.notify_all();
    }
}

/// How much longer than they'd like the networks must wait between runs to fit in the budget.
fn stretch(nets: &[Seen; NETS], now: Instant) -> f32 {
    let wanted: f32 = nets
        .iter()
        .filter(|n| n.last.is_some_and(|last| now.duration_since(last) < ACTIVE))
        .map(|n| n.ms / n.every.max(n.ms).max(1.0))
        .sum();
    (wanted / BUDGET).max(1.0)
}

/// How long `net` should leave between the start of one run and the next, given that `own` is
/// what it would like.
pub fn rest(net: Net, own: Duration) -> Duration {
    let mut s = state();
    s.nets[net as usize].every = own.as_secs_f32() * 1000.0;
    own.mul_f32(stretch(&s.nets, Instant::now()))
}

/// How long a network that failed waits before it's tried again: a failure may not last (no
/// network while downloading, say), but trying every frame would hammer whatever's wrong.
pub const RETRY_FAILED: Duration = Duration::from_secs(30);

/// For a worker's `wanted()` when its network has failed: whether to keep waiting before trying
/// again. `since` is when the failure was first seen here, and is cleared when it's time.
pub fn still_failed(since: &mut Option<Instant>) -> bool {
    let first = *since.get_or_insert_with(Instant::now);
    if first.elapsed() < RETRY_FAILED {
        return true;
    }
    *since = None;
    false
}

/// For the F3 window: what each network running costs.
pub fn describe() -> Option<String> {
    let s = state();
    let now = Instant::now();
    let parts: Vec<String> = s
        .nets
        .iter()
        .zip(NAMES)
        .filter(|(n, _)| n.last.is_some_and(|last| now.duration_since(last) < ACTIVE))
        .map(|(n, name)| format!("{name} {:.0} ms (waits {:.0})", n.ms, n.waited))
        .collect();
    (!parts.is_empty()).then(|| format!("GPU: {} · {:.1}× the wait between runs", parts.join(" · "), stretch(&s.nets, now)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(ms: f32, every: f32, last: Option<Instant>) -> Seen {
        Seen { ms, every, last, waited: 0.0 }
    }

    #[test]
    fn room_to_spare_changes_nothing() {
        let now = Instant::now();
        let nets = [net(10.0, 40.0, Some(now)), net(5.0, 40.0, Some(now)), Seen::default(), Seen::default(), Seen::default()];
        assert_eq!(stretch(&nets, now), 1.0);
    }

    #[test]
    fn too_much_work_is_spread_out_to_fit_the_budget() {
        let now = Instant::now();
        let nets = [net(40.0, 40.0, Some(now)), net(40.0, 40.0, Some(now)), Seen::default(), Seen::default(), Seen::default()];
        // Each wants all of the GPU: together that's twice it, over the budget.
        assert!((stretch(&nets, now) - 2.0 / BUDGET).abs() < 1e-4);
    }

    #[test]
    fn a_network_thats_stopped_doesnt_count() {
        let now = Instant::now();
        let long_ago = now.checked_sub(ACTIVE * 2);
        let nets = [net(40.0, 40.0, Some(now)), net(40.0, 40.0, long_ago), Seen::default(), Seen::default(), Seen::default()];
        assert!((stretch(&nets, now) - 1.0 / BUDGET).abs() < 1e-4);
    }

    #[test]
    fn a_failure_is_tried_again_after_a_while() {
        let mut since = None;
        assert!(still_failed(&mut since), "not straight away");
        since = Instant::now().checked_sub(RETRY_FAILED * 2);
        assert!(!still_failed(&mut since));
        assert_eq!(since, None, "the next failure waits from when it's seen");
    }

    #[test]
    fn turns_go_in_order_and_are_timed() {
        let first = turn(Net::Faces);
        let waiting = std::thread::spawn(|| turn(Net::Flow).ms());
        std::thread::sleep(Duration::from_millis(20));
        drop(first);
        assert!(waiting.join().unwrap() < 20.0, "the time waiting isn't part of the turn");
    }
}
