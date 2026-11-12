//! How things are moving, for effects that ask (`flow = true`): NeuFlow v2 on the GPU through
//! flex-depth, in a thread of its own. The paint callback hands it small frames and picks up what
//! it found. Until that's going, or if it can't, effects get `signals::FlowGl`'s block matching.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use flex_depth::flow::Flow;

use crate::worker::{Job, Worker, push_capped};

pub use flex_depth::flow::{FLOW_HEIGHT, FLOW_WIDTH, HEIGHT, WIDTH};

/// The least time between frames, and as a multiple of how long one takes, so the GPU has time
/// for the picture too.
const MIN_INTERVAL: Duration = Duration::from_millis(10);
const INTERVAL_PER_RUN: f32 = 1.3;
/// The least time between frames when depth is being carried along the movement: every second
/// frame of a film, leaving the GPU room for depth.
const CARRYING_INTERVAL: Duration = Duration::from_millis(66);
/// Slower than this on average (ms), after the first few runs, and depth isn't carried along the
/// movement: this GPU is better off running depth on every frame by itself.
const TOO_SLOW_TO_CARRY: f32 = 70.0;
const RUNS_BEFORE_JUDGING: u32 = 30;
/// Reports waiting for the player to collect them.
const MOST_WAITING: usize = 16;
/// A gap this long (seconds) between frames isn't movement.
const LONGEST_GAP: f32 = 0.5;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Off,
    Starting,
    Ready { adapter: String, ms: f32 },
    Failed(String),
}

/// A report's movement as GL wants it: RGBA floats, `FLOW_WIDTH` × `FLOW_HEIGHT`, the bottom row
/// first and y upwards, where the worker's rows start at the top and its y points down. Nothing
/// moving if there's no movement.
pub fn texels(flow: Option<&[f32]>) -> Vec<f32> {
    let mut texels = vec![0.0f32; FLOW_WIDTH * FLOW_HEIGHT * 4];
    if let Some(flow) = flow {
        for y in 0..FLOW_HEIGHT {
            for x in 0..FLOW_WIDTH {
                let at = (FLOW_HEIGHT - 1 - y) * FLOW_WIDTH + x;
                let from = y * FLOW_WIDTH + x;
                texels[at * 4] = flow[from];
                texels[at * 4 + 1] = -flow[FLOW_WIDTH * FLOW_HEIGHT + from];
            }
        }
    }
    texels
}

impl Status {
    pub fn describe(&self) -> String {
        match self {
            Status::Off => "off".into(),
            Status::Starting => "starting…".into(),
            Status::Ready { adapter, ms } => format!("{ms:.0} ms a frame on {adapter}"),
            Status::Failed(e) => format!("⚠ {e}"),
        }
    }
}

/// The movement in one video frame.
pub struct Report {
    /// The frame's position in the video (seconds).
    pub time: f32,
    /// Planes of x then y, `FLOW_WIDTH` × `FLOW_HEIGHT`, from the top, in fractions of the picture
    /// a second, y downwards. Nothing if the frame doesn't follow on from one before it.
    pub flow: Option<Vec<f32>>,
}

struct Input {
    /// RGBA, bottom row first (as read back from GL), `WIDTH` × `HEIGHT`.
    rgba: Vec<u8>,
    time: f32,
    /// There's been a scene cut since the last one handed over.
    cut: bool,
}

#[derive(Default)]
pub struct State {
    status: Status,
    input: Option<Input>,
    busy: bool,
    reports: VecDeque<Report>,
    /// The depth worker to tell about the movement too, if it's carrying its maps along it.
    carrying: Option<crate::depth::Link>,
    too_slow_to_carry: bool,
}

impl Job for State {
    type Status = Status;
    fn status(&self) -> &Status {
        &self.status
    }
    fn failed(&self) -> bool {
        matches!(self.status, Status::Failed(_))
    }
    fn starting(&mut self) {
        self.status = Status::Starting;
    }
    fn fail(&mut self, why: String) {
        self.status = Status::Failed(why);
    }
    fn stopped(&mut self) {
        self.reports.clear();
        if matches!(self.status, Status::Ready { .. }) {
            self.status = Status::Off;
        }
    }
}

/// Shared between the UI, the paint callback and the worker.
pub type Link = Worker<State>;

impl Link {
    /// An effect wants movement: starts the worker if it isn't running. `carrying` is the depth
    /// worker, if it should carry its maps along the movement. Call every frame.
    pub fn wanted(&self, carrying: Option<crate::depth::Link>) {
        self.want("flow", |state| state.carrying = carrying, |link| link.work());
    }

    /// Whether depth can be carried along the movement on this GPU.
    pub fn can_carry(&self) -> bool {
        let state = self.lock();
        !state.too_slow_to_carry && !matches!(state.status, Status::Failed(_))
    }

    /// Whether the worker would take a frame now.
    pub fn ready_for_frame(&self) -> bool {
        let state = self.lock();
        matches!(state.status, Status::Ready { .. }) && state.input.is_none() && !state.busy
    }

    /// Hands over a frame: RGBA, bottom row first, `WIDTH` × `HEIGHT`. `time` is its position, and
    /// `cut` says there's been a scene cut since the last frame handed over.
    pub fn submit(&self, rgba: Vec<u8>, time: f32, cut: bool) {
        self.lock().input = Some(Input { rgba, time, cut });
        self.notify();
    }

    /// The reports that have come in since the last call, oldest first.
    pub fn take_reports(&self) -> Vec<Report> {
        self.lock().reports.drain(..).collect()
    }

    fn publish(&self, report: Report) {
        let mut state = self.lock();
        if let Some(depth) = &state.carrying {
            depth.carry(Report { time: report.time, flow: report.flow.clone() });
        }
        push_capped(&mut state.reports, [report], MOST_WAITING);
    }

    fn work(&self) {
        let mut net = match Flow::new() {
            Ok(net) => net,
            Err(e) => {
                log::warn!("flow: {e:#}");
                let mut state = self.lock();
                state.status = Status::Failed(format!("{e:#}"));
                state.running = false;
                return;
            }
        };
        log::info!("flow: running on {}", net.adapter());
        self.lock().status = Status::Ready { adapter: net.adapter().to_owned(), ms: 0.0 };

        let mut rgb = vec![0u8; WIDTH * HEIGHT * 3];
        let mut last_time: Option<f32> = None;
        let mut ms = 0.0f32;
        let mut runs = 0;
        while let Some(input) = self.next(|state| {
            let input = state.input.take()?;
            state.busy = true;
            Some(input)
        }) {
            let started = Instant::now();
            crate::faces::top_down(&input.rgba, WIDTH, HEIGHT, &mut rgb);
            let gap = last_time.map(|t| input.time - t).filter(|gap| (1e-4..LONGEST_GAP).contains(gap));
            let gap = gap.filter(|_| !input.cut);
            if gap.is_none() {
                net.reset();
            }
            let turn = crate::gpu::turn(crate::gpu::Net::Flow);
            let flow = net.run(&rgb);
            let run = turn.ms();
            drop(turn);
            let flow = match flow {
                Ok(flow) => flow,
                Err(e) => {
                    log::warn!("flow: {e:#}");
                    let mut state = self.lock();
                    state.status = Status::Failed(format!("{e:#}"));
                    state.busy = false;
                    state.running = false;
                    return;
                }
            };
            last_time = Some(input.time);
            let flow = flow.zip(gap).map(|(mut flow, gap)| {
                flow.iter_mut().for_each(|v| *v /= gap);
                flow
            });
            self.publish(Report { time: input.time, flow });

            ms = if ms == 0.0 { run } else { ms + (run - ms) * 0.1 };
            runs += 1;
            {
                let mut state = self.lock();
                if let Status::Ready { ms: shown, .. } = &mut state.status {
                    *shown = ms;
                }
                if runs >= RUNS_BEFORE_JUDGING && ms > TOO_SLOW_TO_CARRY && !state.too_slow_to_carry {
                    log::info!("flow: {ms:.0} ms a frame, too slow to carry depth along");
                    state.too_slow_to_carry = true;
                }
            }
            let least = if self.lock().carrying.is_some() { CARRYING_INTERVAL } else { MIN_INTERVAL };
            let own = least.max(Duration::from_secs_f32(ms * INTERVAL_PER_RUN / 1000.0));
            std::thread::sleep(crate::gpu::rest(crate::gpu::Net::Flow, own).saturating_sub(started.elapsed()));
            self.lock().busy = false;
        }
        drop(net);
        log::info!("flow: stopped");
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texels_are_the_right_way_up() {
        let (w, h) = (FLOW_WIDTH, FLOW_HEIGHT);
        let mut flow = vec![0.0f32; w * h * 2];
        // Top left moving right and down.
        flow[0] = 0.25;
        flow[w * h] = 0.5;
        let t = texels(Some(&flow));
        let bottom_row_start = (h - 1) * w * 4;
        assert_eq!(&t[bottom_row_start..bottom_row_start + 2], &[0.25, -0.5]);
        assert!(t[..bottom_row_start].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn no_movement_is_still() {
        assert!(texels(None).iter().all(|v| *v == 0.0));
    }
}
