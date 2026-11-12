//! Finds black bars that are part of the picture itself, as when a wide film has been encoded
//! in a 16:9 frame, so effects can leave them alone too. mpv only knows about the ones it adds
//! to fit the window. A dark scene can look just like bars, so only frames with enough light in
//! them count, and a bar can only ever get thinner.

use eframe::glow::{self, HasContext};

use crate::effects::{Fence, Target, create_target};

const SIZE: (usize, usize) = (256, 270);
/// Brighter than this (0–255, the highest of the three colours) isn't a bar.
const BLACK: u8 = 16;
/// How bright the picture has to be on average to say anything about the bars.
const ENOUGH_LIGHT: u32 = 24;
/// Frames looked at before believing them.
const FRAMES_NEEDED: u32 = 3;
/// Thinner than this (pixels of `SIZE`) is just the edge of the picture.
const THINNEST: u32 = 4;
/// How often to look (seconds).
const EVERY: f32 = 0.4;

/// How many rows and columns are black on each side: left, right, bottom, top.
fn measure(rgba: &[u8]) -> [u32; 4] {
    let (w, h) = SIZE;
    let black_row = |y: usize| rgba[y * w * 4..][..w * 4].as_chunks::<4>().0.iter().all(|p| p[..3].iter().all(|c| *c <= BLACK));
    let black_column = |x: usize| (0..h).all(|y| rgba[(y * w + x) * 4..][..3].iter().all(|c| *c <= BLACK));
    let count = |n: usize, black: &dyn Fn(usize) -> bool| (0..n).take_while(|i| black(*i)).count() as u32;
    [count(w, &black_column), count(w, &|x| black_column(w - 1 - x)), count(h, &black_row), count(h, &|y| black_row(h - 1 - y))]
}

#[derive(Default)]
struct BarFinder {
    seen: u32,
    bars: [u32; 4],
}

impl BarFinder {
    /// Takes a frame of `SIZE`, RGBA, rows from the bottom.
    fn look(&mut self, rgba: &[u8]) {
        let (w, h) = (SIZE.0 as u32, SIZE.1 as u32);
        let bars = measure(rgba);
        if bars[0] + bars[1] >= w * 3 / 5 || bars[2] + bars[3] >= h * 3 / 5 {
            return;
        }
        // The average brightness of what isn't a bar.
        let mut sum = 0u64;
        let mut count = 0u64;
        for y in bars[2]..h - bars[3] {
            for x in bars[0]..w - bars[1] {
                let p = &rgba[(y * w + x) as usize * 4..][..3];
                sum += u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]);
                count += 3;
            }
        }
        if count == 0 || sum / count < u64::from(ENOUGH_LIGHT) {
            return;
        }
        self.bars = if self.seen == 0 { bars } else { std::array::from_fn(|i| self.bars[i].min(bars[i])) };
        self.seen += 1;
    }

    /// How much of each side is bars, as fractions: left, right, bottom, top.
    fn found(&self) -> [f32; 4] {
        if self.seen < FRAMES_NEEDED {
            return [0.0; 4];
        }
        let side = |bar: u32, of: usize| if bar < THINNEST { 0.0 } else { (bar + 1) as f32 / of as f32 };
        [side(self.bars[0], SIZE.0), side(self.bars[1], SIZE.0), side(self.bars[2], SIZE.1), side(self.bars[3], SIZE.1)]
    }
}

/// Reads back a small copy of the picture now and then (a frame late, so the CPU never waits for
/// the GPU), and says where the bars are.
#[derive(Default)]
pub struct BarGl {
    objects: Option<(Target, glow::Buffer)>,
    failed: bool,
    pending: Option<Fence>,
    last_look: f32,
    finder: BarFinder,
}

impl BarGl {
    /// `rect` narrowed by any bars found inside it.
    pub fn inside(&self, rect: [f32; 4]) -> [f32; 4] {
        let [left, right, bottom, top] = self.finder.found();
        let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
        [rect[0] + left * w, rect[1] + bottom * h, rect[2] - right * w, rect[3] - top * h]
    }

    /// `video` is a framebuffer of `size` with the picture at `rect`; `now` is the time in seconds.
    pub unsafe fn update(&mut self, gl: &glow::Context, video: glow::Framebuffer, size: (i32, i32), rect: [f32; 4], now: f32) {
        let bytes = SIZE.0 * SIZE.1 * 4;
        unsafe {
            if self.failed {
                return;
            }
            if self.objects.is_none() {
                let made = create_target(gl, (SIZE.0 as i32, SIZE.1 as i32), &[(glow::RGBA8, glow::UNSIGNED_BYTE)]).and_then(|small| {
                    let pbo = gl.create_buffer()?;
                    gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(pbo));
                    gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, bytes as i32, glow::STREAM_READ);
                    gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                    Ok((small, pbo))
                });
                match made {
                    Ok(o) => self.objects = Some(o),
                    Err(e) => {
                        log::warn!("black bars: {e}");
                        self.failed = true;
                        return;
                    }
                }
            }
            let Some((small, pbo)) = self.objects.as_ref() else { return };
            if let Some(Fence(fence)) = self.pending {
                match gl.client_wait_sync(fence, 0, 0) {
                    glow::TIMEOUT_EXPIRED => return,
                    status => {
                        gl.delete_sync(fence);
                        self.pending = None;
                        if status != glow::WAIT_FAILED {
                            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(*pbo));
                            let ptr = gl.map_buffer_range(glow::PIXEL_PACK_BUFFER, 0, bytes as i32, glow::MAP_READ_BIT);
                            if !ptr.is_null() {
                                self.finder.look(std::slice::from_raw_parts(ptr, bytes));
                                gl.unmap_buffer(glow::PIXEL_PACK_BUFFER);
                            }
                            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                        }
                    }
                }
            }
            if (now - self.last_look).abs() < EVERY {
                return;
            }
            self.last_look = now;
            let (x0, y0) = ((rect[0] * size.0 as f32) as i32, (rect[1] * size.1 as f32) as i32);
            let (x1, y1) = (((rect[2] * size.0 as f32) as i32).max(x0 + 1), ((rect[3] * size.1 as f32) as i32).max(y0 + 1));
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(video));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(small.fbo));
            gl.blit_framebuffer(x0, y0, x1, y1, 0, 0, SIZE.0 as i32, SIZE.1 as i32, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(small.fbo));
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(*pbo));
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
            gl.read_pixels(0, 0, SIZE.0 as i32, SIZE.1 as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::BufferOffset(0));
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            if let Ok(fence) = gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
                self.pending = Some(Fence(fence));
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }

    pub unsafe fn release(&mut self, gl: &glow::Context) {
        unsafe {
            if let Some(Fence(fence)) = self.pending.take() {
                gl.delete_sync(fence);
            }
            if let Some((small, pbo)) = self.objects.take() {
                crate::signals::delete_target(gl, small);
                gl.delete_buffer(pbo);
            }
        }
        self.finder = BarFinder::default();
        self.last_look = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with bars of these many rows at the bottom and top, and this bright between.
    fn frame(bottom: usize, top: usize, light: u8) -> Vec<u8> {
        let (w, h) = SIZE;
        let mut rgba = vec![0u8; w * h * 4];
        for y in bottom..h - top {
            for x in 0..w {
                rgba[(y * w + x) * 4..][..3].fill(light);
            }
        }
        rgba
    }

    #[test]
    fn bars_are_found_once_there_have_been_a_few_bright_frames() {
        let mut f = BarFinder::default();
        f.look(&frame(30, 34, 120));
        assert_eq!(f.found(), [0.0; 4], "too soon to say");
        f.look(&frame(30, 34, 120));
        f.look(&frame(30, 34, 120));
        let [left, right, bottom, top] = f.found();
        assert_eq!((left, right), (0.0, 0.0));
        assert!((bottom - 31.0 / 270.0).abs() < 1e-5 && (top - 35.0 / 270.0).abs() < 1e-5);
    }

    #[test]
    fn dark_frames_say_nothing() {
        let mut f = BarFinder::default();
        for _ in 0..10 {
            f.look(&frame(60, 60, 10));
        }
        assert_eq!(f.found(), [0.0; 4]);
    }

    #[test]
    fn a_bar_only_gets_thinner() {
        let mut f = BarFinder::default();
        for _ in 0..3 {
            f.look(&frame(30, 30, 120));
        }
        f.look(&frame(0, 30, 120));
        f.look(&frame(60, 60, 120));
        let [_, _, bottom, top] = f.found();
        assert_eq!(bottom, 0.0, "picture reached the bottom");
        assert!((top - 31.0 / 270.0).abs() < 1e-5);
    }

    #[test]
    fn a_picture_that_fills_the_frame_has_no_bars() {
        let mut f = BarFinder::default();
        for _ in 0..4 {
            f.look(&frame(0, 0, 120));
        }
        assert_eq!(f.found(), [0.0; 4]);
    }
}
