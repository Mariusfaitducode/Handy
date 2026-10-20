//! Cuts one meeting track into utterances from per-frame speech probabilities.
//!
//! A meeting is hours of mostly silence on each track: the microphone is quiet
//! while the others talk, and the system track is quiet while you do. Feeding
//! silence to a speech model wastes time and invites hallucinations ("Merci
//! d'avoir regardé"), so only speech is cut out and sent, one utterance at a
//! time. Short utterances also give the transcript conversational granularity:
//! each one carries its own timestamp and speaker.
//!
//! Pure logic over samples and probabilities, so it is tested without a VAD.

/// Timing rules, in frames of the detector that scores the audio.
#[derive(Debug, Clone, Copy)]
pub struct SegmenterConfig {
    pub frame_samples: usize,
    /// A frame at or above this probability is speech.
    pub threshold: f32,
    /// Below this, a frame is silence. In between, it continues the state it
    /// is in (hysteresis), so a soft syllable never splits a word.
    pub neg_threshold: f32,
    /// Utterances with fewer speech frames are dropped (clicks, coughs).
    pub min_speech_frames: usize,
    /// This much silence ends an utterance.
    pub end_silence_frames: usize,
    /// Audio kept before the first speech frame (soft onsets).
    pub pre_pad_frames: usize,
    /// Audio kept after the last speech frame (trailing consonants).
    pub post_pad_frames: usize,
    /// An utterance longer than this is cut, so a monologue still shows up in
    /// the transcript while it goes on and no run exceeds the model's sweet
    /// spot.
    pub max_frames: usize,
    /// A forced cut goes at the quietest frame of this many trailing frames.
    pub cut_search_frames: usize,
}

impl SegmenterConfig {
    /// Defaults for 16 kHz audio scored in `frame_samples` frames.
    pub fn for_frame(frame_samples: usize) -> Self {
        let frames = |ms: usize| (ms * 16).div_ceil(frame_samples).max(1);
        Self {
            frame_samples,
            threshold: 0.5,
            neg_threshold: 0.35,
            min_speech_frames: frames(300),
            end_silence_frames: frames(800),
            pre_pad_frames: frames(250),
            post_pad_frames: frames(300),
            max_frames: frames(25_000),
            cut_search_frames: frames(6_000),
        }
    }
}

/// One stretch of speech, ready to transcribe.
#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    /// First sample, counted from the start of the track.
    pub start_sample: u64,
    pub samples: Vec<f32>,
}

impl Utterance {
    pub fn start_ms(&self) -> u64 {
        self.start_sample * 1000 / 16_000
    }

    pub fn end_ms(&self) -> u64 {
        (self.start_sample + self.samples.len() as u64) * 1000 / 16_000
    }
}

#[derive(Debug, Clone, Copy)]
enum State {
    Idle,
    Speech {
        /// First frame of the utterance (padding not included).
        start: u64,
        /// Whether the utterance gets pre-padding: not after a forced cut,
        /// whose audio before `start` already went out with the previous one.
        pad_start: bool,
        last_voiced: u64,
        speech_frames: usize,
        silence_run: usize,
    },
}

pub struct Segmenter {
    cfg: SegmenterConfig,
    /// Audio from frame `buf_start` on.
    buf: Vec<f32>,
    /// One probability per frame in `buf`.
    probs: Vec<f32>,
    buf_start: u64,
    state: State,
}

impl Segmenter {
    pub fn new(cfg: SegmenterConfig) -> Self {
        assert!(cfg.frame_samples > 0, "frame size must be non-zero");
        Self {
            cfg,
            buf: Vec::new(),
            probs: Vec::new(),
            buf_start: 0,
            state: State::Idle,
        }
    }

    /// Adds whole frames and their probabilities (one per frame) and returns
    /// the utterances they complete.
    pub fn push(&mut self, frames: &[f32], probs: &[f32]) -> Vec<Utterance> {
        let fs = self.cfg.frame_samples;
        debug_assert_eq!(frames.len(), probs.len() * fs);
        let mut out = Vec::new();
        for (i, &prob) in probs.iter().enumerate() {
            self.buf.extend_from_slice(&frames[i * fs..(i + 1) * fs]);
            self.probs.push(prob);
            if let Some(utterance) = self.step(prob) {
                out.push(utterance);
            }
        }
        out
    }

    /// Ends the track (or a pause): the utterance in progress, if it has
    /// enough speech. The next frame pushed continues the same timeline.
    pub fn flush(&mut self) -> Option<Utterance> {
        let result = match self.state {
            State::Speech {
                start,
                pad_start,
                speech_frames,
                ..
            } if speech_frames >= self.cfg.min_speech_frames => {
                let end = self.end_frame();
                Some(self.extract(self.padded_start(start, pad_start), end))
            }
            _ => None,
        };
        self.state = State::Idle;
        let end = self.end_frame();
        self.discard_before(end);
        result
    }

    /// Moves the timeline forward by `frames` frames of silence that were
    /// never pushed (a gap in the audio). Ends the utterance in progress.
    pub fn skip(&mut self, frames: u64) -> Option<Utterance> {
        let utterance = self.flush();
        self.buf_start += frames;
        utterance
    }

    /// Frames pushed so far, including skipped ones.
    #[cfg(test)]
    pub fn position_frames(&self) -> u64 {
        self.end_frame()
    }

    fn end_frame(&self) -> u64 {
        self.buf_start + self.probs.len() as u64
    }

    fn step(&mut self, prob: f32) -> Option<Utterance> {
        let n = self.end_frame() - 1;
        let cfg = self.cfg;
        match self.state {
            State::Idle => {
                if prob >= cfg.threshold {
                    self.state = State::Speech {
                        start: n,
                        pad_start: true,
                        last_voiced: n,
                        speech_frames: 1,
                        silence_run: 0,
                    };
                } else {
                    let keep_from = (n + 1).saturating_sub(cfg.pre_pad_frames as u64);
                    self.discard_before(keep_from);
                }
                None
            }
            State::Speech {
                start,
                pad_start,
                mut last_voiced,
                mut speech_frames,
                mut silence_run,
            } => {
                if prob >= cfg.threshold || (prob >= cfg.neg_threshold && silence_run == 0) {
                    if prob >= cfg.threshold {
                        speech_frames += 1;
                    }
                    last_voiced = n;
                    silence_run = 0;
                } else {
                    silence_run += 1;
                }

                if silence_run >= cfg.end_silence_frames {
                    self.state = State::Idle;
                    let utterance = (speech_frames >= cfg.min_speech_frames).then(|| {
                        let end = (last_voiced + 1 + cfg.post_pad_frames as u64).min(n + 1);
                        self.extract(self.padded_start(start, pad_start), end)
                    });
                    let keep_from = (n + 1).saturating_sub(cfg.pre_pad_frames as u64);
                    self.discard_before(keep_from);
                    return utterance;
                }

                if (n + 1 - start) as usize >= cfg.max_frames {
                    return Some(self.forced_cut(start, pad_start, n));
                }

                self.state = State::Speech {
                    start,
                    pad_start,
                    last_voiced,
                    speech_frames,
                    silence_run,
                };
                None
            }
        }
    }

    /// Cuts a too-long utterance at its quietest recent frame and carries on
    /// with the rest as a new one.
    fn forced_cut(&mut self, start: u64, pad_start: bool, n: u64) -> Utterance {
        let cfg = self.cfg;
        let search_from = (n + 1)
            .saturating_sub(cfg.cut_search_frames as u64)
            .max(start + 1);
        let mut cut = n + 1;
        let mut quietest = f32::INFINITY;
        for frame in search_from..=n {
            let prob = self.probs[(frame - self.buf_start) as usize];
            if prob < quietest {
                quietest = prob;
                cut = frame;
            }
        }
        let utterance = self.extract(self.padded_start(start, pad_start), cut);

        let rest = &self.probs[(cut - self.buf_start) as usize..];
        let speech_frames = rest.iter().filter(|&&p| p >= cfg.threshold).count();
        let silence_run = rest
            .iter()
            .rev()
            .take_while(|&&p| p < cfg.neg_threshold)
            .count();
        let last_voiced = n - silence_run as u64;
        self.state = State::Speech {
            start: cut,
            pad_start: false,
            last_voiced: last_voiced.max(cut),
            speech_frames,
            silence_run,
        };
        self.discard_before(cut);
        utterance
    }

    fn padded_start(&self, start: u64, pad_start: bool) -> u64 {
        if pad_start {
            start
                .saturating_sub(self.cfg.pre_pad_frames as u64)
                .max(self.buf_start)
        } else {
            start.max(self.buf_start)
        }
    }

    fn extract(&self, from: u64, to: u64) -> Utterance {
        let fs = self.cfg.frame_samples;
        let a = (from - self.buf_start) as usize * fs;
        let b = ((to - self.buf_start) as usize * fs).min(self.buf.len());
        Utterance {
            start_sample: from * fs as u64,
            samples: self.buf[a..b].to_vec(),
        }
    }

    fn discard_before(&mut self, frame: u64) {
        if frame <= self.buf_start {
            return;
        }
        let drop = ((frame - self.buf_start) as usize).min(self.probs.len());
        self.buf.drain(..drop * self.cfg.frame_samples);
        self.probs.drain(..drop);
        self.buf_start += drop as u64;
    }
}

/// A crude speech probability from loudness, for when the VAD model can't be
/// loaded: quiet audio scores 0, normal speech levels approach 1.
pub fn energy_probability(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let mean_square = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
    let db = 10.0 * mean_square.max(1e-10).log10();
    ((db + 50.0) / 20.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: usize = 512;

    fn cfg() -> SegmenterConfig {
        SegmenterConfig::for_frame(FS)
    }

    /// Frames of audio whose sample value encodes the frame index, and the
    /// matching probabilities.
    fn track(probs: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut audio = Vec::with_capacity(probs.len() * FS);
        for i in 0..probs.len() {
            audio.extend(std::iter::repeat_n(i as f32, FS));
        }
        (audio, probs.to_vec())
    }

    fn frames_for_ms(ms: usize) -> usize {
        (ms * 16).div_ceil(FS)
    }

    fn run(probs: &[f32]) -> Vec<Utterance> {
        let (audio, probs) = track(probs);
        let mut seg = Segmenter::new(cfg());
        let mut out = seg.push(&audio, &probs);
        out.extend(seg.flush());
        out
    }

    #[test]
    fn silence_yields_nothing() {
        assert!(run(&vec![0.0; 500]).is_empty());
    }

    #[test]
    fn one_burst_is_one_padded_utterance() {
        let mut probs = vec![0.0; 50];
        probs.extend(vec![0.9; 60]); // ~1.9 s of speech starting at frame 50
        probs.extend(vec![0.0; 100]);
        let out = run(&probs);
        assert_eq!(out.len(), 1);
        let u = &out[0];
        let pre = cfg().pre_pad_frames as u64;
        assert_eq!(u.start_sample, (50 - pre) * FS as u64);
        // Ends after the last voiced frame plus post-padding.
        let frames = u.samples.len() / FS;
        assert_eq!(frames as u64, pre + 60 + cfg().post_pad_frames as u64);
        // The audio really is the audio of those frames.
        assert_eq!(u.samples[0], (50 - pre) as f32);
    }

    #[test]
    fn clicks_are_dropped() {
        let mut probs = vec![0.0; 20];
        probs.extend(vec![0.9; 3]); // ~100 ms
        probs.extend(vec![0.0; 100]);
        assert!(run(&probs).is_empty());
    }

    #[test]
    fn short_pause_keeps_one_utterance_long_pause_splits() {
        let short = frames_for_ms(300);
        let long = frames_for_ms(1500);
        let mut probs = vec![0.9; 40];
        probs.extend(vec![0.0; short]);
        probs.extend(vec![0.9; 40]);
        probs.extend(vec![0.0; long]);
        probs.extend(vec![0.9; 40]);
        probs.extend(vec![0.0; long]);
        assert_eq!(run(&probs).len(), 2);
    }

    #[test]
    fn hysteresis_band_does_not_split_words() {
        // Alternating speech / in-between frames never accumulate silence.
        let mut probs = Vec::new();
        for _ in 0..100 {
            probs.push(0.9);
            probs.push(0.4);
        }
        probs.extend(vec![0.0; 100]);
        assert_eq!(run(&probs).len(), 1);
    }

    #[test]
    fn monologue_is_cut_into_contiguous_bounded_pieces() {
        let c = cfg();
        let total = c.max_frames * 3 + 10;
        let mut probs: Vec<f32> = (0..total)
            .map(|i| if i % 97 == 0 { 0.6 } else { 0.95 })
            .collect();
        probs.extend(vec![0.0; 100]);
        let out = run(&probs);
        assert!(out.len() >= 3, "got {} pieces", out.len());
        for pair in out.windows(2) {
            let end = pair[0].start_sample + pair[0].samples.len() as u64;
            assert_eq!(end, pair[1].start_sample, "pieces must be contiguous");
        }
        for u in &out {
            let frames = u.samples.len() / FS;
            assert!(frames <= c.max_frames + c.pre_pad_frames + c.post_pad_frames);
        }
    }

    #[test]
    fn forced_cut_prefers_the_quietest_frame() {
        let c = cfg();
        let mut probs = vec![0.95; c.max_frames + 5];
        let dip = c.max_frames - 20;
        probs[dip] = 0.36;
        probs.extend(vec![0.0; 100]);
        let out = run(&probs);
        let first_end = out[0].start_sample / FS as u64 + (out[0].samples.len() / FS) as u64;
        assert_eq!(first_end, dip as u64);
    }

    #[test]
    fn flush_emits_speech_in_progress() {
        let mut probs = vec![0.0; 10];
        probs.extend(vec![0.9; 40]);
        let (audio, probs) = track(&probs);
        let mut seg = Segmenter::new(cfg());
        assert!(seg.push(&audio, &probs).is_empty());
        let u = seg.flush().expect("speech in progress");
        assert!(u.samples.len() >= 40 * FS);
        assert!(seg.flush().is_none());
    }

    #[test]
    fn skip_advances_the_timeline() {
        let mut seg = Segmenter::new(cfg());
        let (audio, probs) = track(&[0.0; 10]);
        seg.push(&audio, &probs);
        seg.skip(1000);
        let (audio, probs) = track(&[0.9; 40]);
        seg.push(&audio, &probs);
        let u = seg.flush().unwrap();
        assert_eq!(u.start_sample, 1010 * FS as u64);
        assert_eq!(seg.position_frames(), 1050);
    }

    #[test]
    fn pushing_in_pieces_matches_pushing_at_once() {
        let mut probs = vec![0.0; 30];
        probs.extend(vec![0.9; 70]);
        probs.extend(vec![0.0; 60]);
        probs.extend(vec![0.8; 50]);
        probs.extend(vec![0.0; 60]);
        let whole = run(&probs);

        let (audio, probs) = track(&probs);
        let mut seg = Segmenter::new(cfg());
        let mut pieces = Vec::new();
        for i in (0..probs.len()).step_by(7) {
            let end = (i + 7).min(probs.len());
            pieces.extend(seg.push(&audio[i * FS..end * FS], &probs[i..end]));
        }
        pieces.extend(seg.flush());
        assert_eq!(whole, pieces);
    }

    #[test]
    fn energy_probability_tracks_loudness() {
        assert_eq!(energy_probability(&[0.0; 512]), 0.0);
        assert!(energy_probability(&[0.1; 512]) > 0.5);
        assert!(energy_probability(&[0.001; 512]) < 0.2);
    }

    /// Real speech through the real VAD: set HANDY_MEETING_TEST_WAV to a
    /// 16 kHz mono WAV with pauses (e.g. made with macOS `say -o x.wav
    /// --data-format=LEI16@16000 "One. [[slnc 2500]] Two."`) and run with
    /// `--ignored`.
    #[test]
    #[ignore]
    fn real_speech_is_cut_at_pauses() {
        use crate::audio_toolkit::vad::{SpeechScorer, VoiceActivityDetector};
        let Ok(path) = std::env::var("HANDY_MEETING_TEST_WAV") else {
            return;
        };
        let samples = crate::audio_toolkit::read_wav_samples(&path).unwrap();
        let vad_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/models/silero-vad-v6.2-F32.gguf"
        );
        let mut vad = crate::audio_toolkit::SileroVad::new(vad_path, 0.3).unwrap();
        let frame = vad.frame_samples();
        let mut seg = Segmenter::new(SegmenterConfig::for_frame(frame));
        let mut out = Vec::new();
        // Feed it like the recorder does: 30 ms chunks.
        let mut pending: Vec<f32> = Vec::new();
        for chunk in samples.chunks(480) {
            pending.extend_from_slice(chunk);
            let whole = pending.len() / frame * frame;
            if whole == 0 {
                continue;
            }
            let frames: Vec<f32> = pending.drain(..whole).collect();
            let probs = vad.score(&frames).unwrap();
            out.extend(seg.push(&frames, &probs));
        }
        out.extend(seg.flush());
        for u in &out {
            eprintln!("utterance {} ms -> {} ms", u.start_ms(), u.end_ms());
        }
        let expected: usize = std::env::var("HANDY_MEETING_TEST_EXPECTED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if expected > 0 {
            assert_eq!(out.len(), expected);
        }
        assert!(!out.is_empty());
    }

    #[test]
    fn utterance_times_are_in_milliseconds() {
        let u = Utterance {
            start_sample: 16_000,
            samples: vec![0.0; 8_000],
        };
        assert_eq!(u.start_ms(), 1000);
        assert_eq!(u.end_ms(), 1500);
    }
}
