//! What kind of sound is playing, for video effects that hear the sound: YAMNet through
//! flex-depth, on a thread of its own, fed the samples effects measure. It looks at the last
//! second every half second, which takes it a few milliseconds.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use flex_depth::sound::{KINDS, Listener, kinds};

/// What was heard last.
#[derive(Clone, Default)]
pub struct Heard {
    /// As in `KINDS`, 0 to 1.
    pub kinds: [f32; KINDS.len()],
    /// The likeliest classes, or why there are none, for showing.
    pub described: String,
}

pub struct SoundKind {
    send: Option<Sender<(u32, Vec<f32>)>>,
    heard: Arc<Mutex<Heard>>,
    thread: Option<JoinHandle<()>>,
}

impl SoundKind {
    pub fn start() -> Self {
        let (send, receive) = channel();
        let heard = Arc::new(Mutex::new(Heard { described: "starting…".into(), ..Default::default() }));
        let thread = std::thread::Builder::new()
            .name("sound kind".into())
            .spawn({
                let heard = heard.clone();
                move || listen(&receive, &heard)
            })
            .map_err(|e| log::warn!("video effects: can't tell what kind of sound it is: {e}"))
            .ok();
        Self { send: Some(send), heard, thread }
    }

    /// Some more of the sound effects hear, at `rate`.
    pub fn push(&self, rate: u32, mono: &[f32]) {
        if let Some(send) = &self.send {
            let _ = send.send((rate, mono.to_vec()));
        }
    }

    pub fn heard(&self) -> Heard {
        self.heard.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for SoundKind {
    fn drop(&mut self) {
        // Not waited for: this is dropped on the UI thread, and the thread may still be loading
        // the network. With nothing left to send it, it stops by itself.
        self.send = None;
        drop(self.thread.take());
    }
}

fn listen(receive: &Receiver<(u32, Vec<f32>)>, heard: &Mutex<Heard>) {
    let mut listener = match Listener::new() {
        Ok(listener) => listener,
        Err(e) => {
            log::warn!("video effects: can't tell what kind of sound it is: {e:#}");
            heard.lock().unwrap_or_else(|e| e.into_inner()).described = format!("can't: {e:#}");
            return;
        }
    };
    loop {
        match receive.recv_timeout(Duration::from_millis(500)) {
            Ok((rate, mono)) => listener.push(rate, &mono),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // Everything waiting, before looking.
        while let Ok((rate, mono)) = receive.try_recv() {
            listener.push(rate, &mono);
        }
        while let Some(scores) = listener.listen() {
            let mut best: Vec<usize> = (0..scores.len()).collect();
            best.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]));
            let described = best.iter().take(3).map(|&i| format!("{} {:.0}%", listener.names()[i], scores[i] * 100.0)).collect::<Vec<_>>().join(", ");
            *heard.lock().unwrap_or_else(|e| e.into_inner()) = Heard { kinds: kinds(&scores), described };
        }
    }
}
