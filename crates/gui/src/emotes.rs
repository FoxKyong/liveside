//! Emote pictures: fetched once, kept on disk, decoded here.
//!
//! Decoding is done in Rust rather than by gdk-pixbuf, because 7TV serves
//! WebP and AVIF, which need loaders that are not installed everywhere.
//! This way the client shows the same emotes on any machine.
//!
//! Animated emotes (GIF, animated WebP, APNG) are decoded whole and played
//! on the window's own frame clock, so every copy of an emote shows the
//! same frame and nothing is drawn while it is off screen.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

/// How tall an emote is drawn when the line height is not known.
pub const DEFAULT_EMOTE_HEIGHT: i32 = 24;

/// What to do with a picture once it is here.
type Ready = Box<dyn Fn(&Rc<Animation>)>;

/// Emotes still waiting for their picture, by key.
type Waiting = Rc<RefCell<HashMap<String, Vec<Ready>>>>;

/// A decoded picture on its way to the widgets waiting for it. A still
/// emote is simply an animation of one frame.
struct Loaded {
    key: String,
    frames: Vec<LoadedFrame>,
}

/// One frame: raw RGBA with its size, and how long it is shown.
struct LoadedFrame {
    pixels: Vec<u8>,
    width: i32,
    height: i32,
    delay_ms: u32,
}

/// An emote ready to be drawn: one frame, or many with their timing.
pub struct Animation {
    frames: Vec<gdk::Texture>,
    /// When each frame stops being the one to show, in milliseconds from
    /// the start of the loop. Only used when there is more than one.
    ends: Vec<u32>,
    total: u32,
}

impl Animation {
    pub fn still(&self) -> &gdk::Texture {
        &self.frames[0]
    }

    fn moving(&self) -> bool {
        self.frames.len() > 1
    }

    /// The frame to show `ms` into the loop.
    fn at(&self, ms: u32) -> usize {
        frame_at(&self.ends, ms)
    }
}

/// The frame whose turn it is `ms` into the loop, given when each frame
/// ends. Past the end only by a rounding error between the clock and the
/// sums, and then the last frame is the honest answer.
fn frame_at(ends: &[u32], ms: u32) -> usize {
    ends.iter()
        .position(|end| ms < *end)
        .unwrap_or(ends.len().saturating_sub(1))
}

/// Fetches and remembers emote pictures.
pub struct Emotes {
    requests: tokio::sync::mpsc::UnboundedSender<(String, String)>,
    animations: Rc<RefCell<HashMap<String, Rc<Animation>>>>,
    /// What to do when a picture arrives, for emotes already on screen.
    waiting: Waiting,
    asked: Rc<RefCell<std::collections::HashSet<String>>>,
    /// Whether emotes that move are played, as the setting says.
    animated: Rc<Cell<bool>>,
    /// The emotes that could move, so the setting can reach the pictures
    /// already on screen.
    playing: Rc<RefCell<Vec<Playing>>>,
}

/// One picture of an emote that moves, and its place on the frame clock
/// while it is playing.
struct Playing {
    picture: glib::WeakRef<gtk::Picture>,
    animation: Rc<Animation>,
    running: Option<gtk::TickCallbackId>,
}

impl Playing {
    fn start(&mut self) {
        if self.running.is_some() {
            return;
        }
        if let Some(picture) = self.picture.upgrade() {
            self.running = Some(play(&picture, self.animation.clone()));
        }
    }

    fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.remove();
        }
        if let Some(picture) = self.picture.upgrade() {
            picture.set_paintable(Some(self.animation.still()));
        }
    }
}

impl Emotes {
    /// Starts the fetcher on its own runtime, and hands what it decodes
    /// back to the GTK main loop.
    pub fn start(animated: bool) -> Self {
        let (requests, mut requests_in) =
            tokio::sync::mpsc::unbounded_channel::<(String, String)>();
        let (results_out, results) = async_channel::unbounded::<Loaded>();
        let cache = cache_dir();

        std::thread::Builder::new()
            .name("emotes".to_owned())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                else {
                    return;
                };
                runtime.block_on(async move {
                    let http = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(10))
                        .timeout(std::time::Duration::from_secs(20))
                        .build()
                        .unwrap_or_default();
                    while let Some((key, url)) = requests_in.recv().await {
                        let Some(bytes) = bytes_for(&http, &cache, &key, &url).await else {
                            continue;
                        };
                        // Decoding is slow enough to keep off the main loop.
                        let Ok(loaded) = tokio::task::block_in_place(|| decode(&key, &bytes))
                        else {
                            continue;
                        };
                        if results_out.send(loaded).await.is_err() {
                            return;
                        }
                    }
                });
            })
            .expect("a thread for emote pictures");

        let animations: Rc<RefCell<HashMap<String, Rc<Animation>>>> = Rc::default();
        let waiting: Waiting = Rc::default();
        glib::spawn_future_local({
            let animations = animations.clone();
            let waiting = waiting.clone();
            async move {
                while let Ok(loaded) = results.recv().await {
                    let animation = Rc::new(animation_from(&loaded));
                    for ready in waiting.borrow_mut().remove(&loaded.key).unwrap_or_default() {
                        ready(&animation);
                    }
                    animations.borrow_mut().insert(loaded.key, animation);
                }
            }
        });

        Self {
            requests,
            animations,
            waiting,
            asked: Rc::default(),
            animated: Rc::new(Cell::new(animated)),
            playing: Rc::default(),
        }
    }

    /// A picture of an emote, as tall as a line of chat, keeping its shape.
    /// A wide emote is allowed to be wide: that is what it looks like.
    pub fn picture(&self, animation: &Rc<Animation>, tooltip: &str, height: i32) -> gtk::Picture {
        let texture = animation.still();
        let picture = gtk::Picture::for_paintable(texture);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        let width = texture.width() * height / texture.height().max(1);
        picture.set_size_request(width.clamp(height / 2, height * 6), height);
        picture.set_tooltip_text(Some(tooltip));
        if animation.moving() {
            let mut playing = Playing {
                picture: picture.downgrade(),
                animation: animation.clone(),
                running: None,
            };
            if self.animated.get() {
                playing.start();
            }
            let mut all = self.playing.borrow_mut();
            // Emotes long gone from the buffer are let go of here rather
            // than in a sweep of their own.
            all.retain(|playing| playing.picture.upgrade().is_some());
            all.push(playing);
        }
        picture
    }

    /// Starts or stops every emote that moves, for the setting being
    /// changed while the window is open. Stopping parks each one on its
    /// first frame: a face caught mid-blink is not what the emote is.
    pub fn set_animated(&self, animated: bool) {
        if self.animated.replace(animated) == animated {
            return;
        }
        let mut all = self.playing.borrow_mut();
        all.retain(|playing| playing.picture.upgrade().is_some());
        for playing in all.iter_mut() {
            if animated {
                playing.start();
            } else {
                playing.stop();
            }
        }
    }

    /// Asks for an emote's picture. `ready` runs at once if it is already
    /// known, later when it arrives, and never if there is none to be had —
    /// which is why the caller shows the emote's code until then.
    /// Asks for a picture. `ready` runs at once if it is already known,
    /// later when it arrives, and never if there is none to be had —
    /// which is why the caller writes something readable meanwhile.
    /// `key` names the file in the cache, so it must be usable as one.
    pub fn request_url(&self, key: String, url: String, ready: impl Fn(&Rc<Animation>) + 'static) {
        self.fetch(key, url, ready);
    }

    fn fetch(&self, key: String, url: String, ready: impl Fn(&Rc<Animation>) + 'static) {
        if let Some(animation) = self.animations.borrow().get(&key) {
            ready(animation);
            return;
        }
        self.waiting
            .borrow_mut()
            .entry(key.clone())
            .or_default()
            .push(Box::new(ready));
        // Ask once, however many messages use the emote.
        if self.asked.borrow_mut().insert(key.clone()) {
            let _ = self.requests.send((key, url));
        }
    }
}

/// Runs an animation on the window's frame clock. GTK calls this only
/// while the picture is realized and on screen, so an emote scrolled out
/// of sight costs nothing, and the frame is taken from the clock rather
/// than counted, so every copy of an emote moves together and a slow
/// moment does not leave one behind.
fn play(picture: &gtk::Picture, animation: Rc<Animation>) -> gtk::TickCallbackId {
    let showing = Cell::new(0usize);
    picture.add_tick_callback(move |picture, clock| {
        if !on_screen(picture) {
            return glib::ControlFlow::Continue;
        }
        let ms = (clock.frame_time() / 1000).unsigned_abs() % u64::from(animation.total);
        let at = animation.at(ms as u32);
        if at != showing.get() {
            showing.set(at);
            picture.set_paintable(Some(&animation.frames[at]));
        }
        glib::ControlFlow::Continue
    })
}

/// Whether a picture is where the reader can see it. A text view keeps
/// the widgets of lines scrolled far above, and those must not be given a
/// new frame sixty times a second for nobody.
fn on_screen(picture: &gtk::Picture) -> bool {
    let Some(root) = picture.root() else {
        return false;
    };
    let Some(bounds) = picture.compute_bounds(&root) else {
        return false;
    };
    bounds.y() + bounds.height() > 0.0
        && bounds.y() < root.height() as f32
        && bounds.x() + bounds.width() > 0.0
        && bounds.x() < root.width() as f32
}

fn animation_from(loaded: &Loaded) -> Animation {
    let mut frames = Vec::with_capacity(loaded.frames.len());
    let mut ends = Vec::with_capacity(loaded.frames.len());
    let mut total = 0u32;
    for frame in &loaded.frames {
        let bytes = glib::Bytes::from(&frame.pixels);
        frames.push(
            gdk::MemoryTexture::new(
                frame.width,
                frame.height,
                gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                (frame.width * 4) as usize,
            )
            .upcast(),
        );
        total = total.saturating_add(frame.delay_ms);
        ends.push(total);
    }
    Animation {
        frames,
        ends,
        total: total.max(1),
    }
}

/// The bytes of one emote: from the cache when it is there, from the
/// provider otherwise. A picture never changes, so the cache never expires.
async fn bytes_for(
    http: &reqwest::Client,
    cache: &Option<PathBuf>,
    key: &str,
    url: &str,
) -> Option<Vec<u8>> {
    let path = cache.as_ref().map(|dir| dir.join(key));
    if let Some(path) = &path
        && let Ok(bytes) = std::fs::read(path)
        && !bytes.is_empty()
    {
        return Some(bytes);
    }
    let response = http.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = response.bytes().await.ok()?.to_vec();
    if let Some(path) = &path
        && let Some(dir) = path.parent()
    {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(path, &bytes);
    }
    Some(bytes)
}

/// How many frames of one emote are kept. Past this an animation is so
/// long that it is somebody being clever, not an emote.
const MAX_FRAMES: usize = 240;

/// Decodes a picture to RGBA: every frame of an animated emote, or the one
/// frame of a still one. A file that says it moves but cannot be read
/// frame by frame falls back to a still picture rather than to nothing.
fn decode(key: &str, bytes: &[u8]) -> Result<Loaded, image::ImageError> {
    if let Some(frames) = frames_of(bytes)
        && frames.len() > 1
    {
        return Ok(Loaded {
            key: key.to_owned(),
            frames,
        });
    }
    let image = image::load_from_memory(bytes)?.to_rgba8();
    let (width, height) = image.dimensions();
    Ok(Loaded {
        key: key.to_owned(),
        frames: vec![LoadedFrame {
            pixels: image.into_raw(),
            width: width as i32,
            height: height as i32,
            delay_ms: 0,
        }],
    })
}

/// The frames of an animated emote, if it is one. The decoders compose
/// each frame onto the canvas themselves, so what comes out is a finished
/// picture rather than the patch that changed.
fn frames_of(bytes: &[u8]) -> Option<Vec<LoadedFrame>> {
    use image::AnimationDecoder;
    let cursor = std::io::Cursor::new(bytes);
    let frames = match image::guess_format(bytes).ok()? {
        image::ImageFormat::Gif => image::codecs::gif::GifDecoder::new(cursor)
            .ok()?
            .into_frames(),
        image::ImageFormat::WebP => {
            let decoder = image::codecs::webp::WebPDecoder::new(cursor).ok()?;
            if !decoder.has_animation() {
                return None;
            }
            decoder.into_frames()
        }
        image::ImageFormat::Png => {
            let decoder = image::codecs::png::PngDecoder::new(cursor).ok()?;
            if !decoder.is_apng().ok()? {
                return None;
            }
            decoder.apng().ok()?.into_frames()
        }
        _ => return None,
    };
    let mut out = Vec::new();
    for frame in frames.take(MAX_FRAMES) {
        let frame = frame.ok()?;
        let (numer, denom) = frame.delay().numer_denom_ms();
        // A frame with no timing of its own, or one asking to be shown for
        // a hundredth of a second, is shown for what browsers show it for.
        let delay = numer.checked_div(denom.max(1)).unwrap_or(100);
        let image = frame.into_buffer();
        let (width, height) = image.dimensions();
        out.push(LoadedFrame {
            pixels: image.into_raw(),
            width: width as i32,
            height: height as i32,
            delay_ms: if delay < 20 { 100 } else { delay },
        });
    }
    Some(out)
}

/// `$XDG_CACHE_HOME/liveside/emotes`. Pictures are public and replaceable, so
/// they live in the cache, not beside the chat log.
fn cache_dir() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CACHE_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
    };
    Some(base.join("liveside").join("emotes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-frame GIF, red then blue, each shown for 100 ms.
    fn animated_gif() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
            for colour in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                let image = image::RgbaImage::from_pixel(4, 4, image::Rgba(colour));
                let frame = image::Frame::from_parts(
                    image,
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(100, 1),
                );
                encoder.encode_frame(frame).unwrap();
            }
        }
        out
    }

    fn still_png() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 255, 0, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("a png");
        out.into_inner()
    }

    #[test]
    fn an_animated_emote_keeps_all_its_frames() {
        let loaded = decode("test", &animated_gif()).expect("decoded");
        assert_eq!(loaded.frames.len(), 2);
        assert!(loaded.frames.iter().all(|f| f.delay_ms == 100));
        // Composed onto the canvas, so every frame is a whole picture.
        assert!(loaded.frames.iter().all(|f| f.pixels.len() == 4 * 4 * 4));
    }

    #[test]
    fn a_still_emote_is_one_frame() {
        let loaded = decode("test", &still_png()).expect("decoded");
        assert_eq!(loaded.frames.len(), 1);
    }

    #[test]
    fn rubbish_is_not_a_picture() {
        assert!(decode("test", b"not a picture at all").is_err());
    }

    #[test]
    fn the_clock_picks_the_frame() {
        let ends = [100, 200, 300];
        assert_eq!(frame_at(&ends, 0), 0);
        assert_eq!(frame_at(&ends, 99), 0);
        assert_eq!(frame_at(&ends, 100), 1);
        assert_eq!(frame_at(&ends, 299), 2);
        // Only reachable through rounding; it must not panic or wrap.
        assert_eq!(frame_at(&ends, 300), 2);
    }
}
