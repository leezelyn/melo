use std::io::Cursor;
use symphonia::core::{
    audio::SampleBuffer, codecs::DecoderOptions, errors::Error, formats::FormatOptions,
    io::MediaSourceStream, meta::MetadataOptions, probe::Hint,
};

pub const MAX_SAMPLES: usize = 3_600_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Original,
    Mild,
    Strong,
}
impl Quality {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "original" => Some(Self::Original),
            "mild" => Some(Self::Mild),
            "strong" => Some(Self::Strong),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Mild => "mild",
            Self::Strong => "strong",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "原版",
            Self::Mild => "轻度增强",
            Self::Strong => "明显增强",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Original => "保留原曲动态，峰值 64",
            Self::Mild => "温和提升轻声片段，峰值 64",
            Self::Strong => "更强的响度与动态压缩，峰值 127",
        }
    }
    pub fn peak(self) -> u8 {
        if self == Self::Strong { 127 } else { 64 }
    }
    fn dynamics(self) -> (f64, f64, f64, f64, f64) {
        match self {
            Self::Original => (-12.0, 1.0, 5.0, 120.0, 6.0),
            Self::Mild => (-12.0, 2.0, 5.0, 120.0, 6.0),
            Self::Strong => (-24.0, 4.0, 1.0, 40.0, 18.0),
        }
    }
}
// Same bounded lookahead compressor and preset values as Android Loudness.kt.
// The monotone queue retains only the anticipation window, never a song copy.
fn dynamics(
    samples: &[f32],
    mean: f64,
    input_peak: f64,
    quality: Quality,
    mut emit: impl FnMut(usize, f64),
) {
    let (threshold, ratio, attack, release, _) = quality.dynamics();
    if ratio == 1.0 || input_peak < 1e-8 {
        for (i, &x) in samples.iter().enumerate() {
            emit(i, x as f64 - mean);
        }
        return;
    }
    let ahead = (5.0 * 12.0 * attack).ceil() as usize;
    let mut ring = vec![0.0; ahead + 2];
    let mut queue = std::collections::VecDeque::<(usize, f64)>::new();
    let slope = 20.0 / 10.0_f64.ln() / (12.0 * attack);
    let decay = (-1.0 / (12.0 * release)).exp();
    let mut reduction: f64 = 0.0;
    for i in 0..samples.len() + ahead {
        if i < samples.len() {
            let x = samples[i] as f64 - mean;
            let slot = i % ring.len();
            ring[slot] = x;
            let key = 20.0 * (x.abs() / input_peak).log10() - i as f64 * slope;
            while queue.back().is_some_and(|&(_, k)| k <= key) {
                queue.pop_back();
            }
            queue.push_back((i, key));
        }
        if i >= ahead {
            let output = i - ahead;
            while queue.front().is_some_and(|&(p, _)| p < output) {
                queue.pop_front();
            }
            let level = queue
                .front()
                .map_or(f64::NEG_INFINITY, |&(_, k)| k + output as f64 * slope);
            let target = (level - threshold).max(0.0) * (1.0 - 1.0 / ratio);
            reduction = target.max(reduction * decay);
            emit(
                output,
                ring[output % ring.len()] * 10.0_f64.powf(-reduction / 20.0),
            );
        }
    }
}
pub fn validate_clip(start: f64, end: f64) -> Result<(), String> {
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start || end - start > 300.0
    {
        return Err("请选择有效片段，长度不超过 300 秒".into());
    }
    Ok(())
}

pub fn encode(samples: &[f32]) -> Result<Vec<u8>, String> {
    encode_with_quality(samples, Quality::Original)
}
pub fn encode_with_quality(samples: &[f32], quality: Quality) -> Result<Vec<u8>, String> {
    let n = samples.len();
    if !(2..=MAX_SAMPLES).contains(&n) || samples.iter().any(|x| !x.is_finite()) {
        return Err("音频为空、无效或超过 300 秒".into());
    }
    let mean = samples.iter().map(|&x| x as f64).sum::<f64>() / n as f64;
    let fade = |i: usize| {
        1.0_f64
            .min(if i < 960 {
                0.5 - 0.5 * (std::f64::consts::PI * i as f64 / 959.0).cos()
            } else {
                1.0
            })
            .min(if n - 1 - i < 1440 {
                0.5 - 0.5 * (std::f64::consts::PI * (n - 1 - i) as f64 / 1439.0).cos()
            } else {
                1.0
            })
    };
    let baseline_peak = samples
        .iter()
        .enumerate()
        .map(|(i, &x)| ((x as f64 - mean) * fade(i)).abs())
        .fold(0.0_f64, f64::max);
    let input_peak = samples
        .iter()
        .map(|&x| (x as f64 - mean).abs())
        .fold(0.0_f64, f64::max);
    let mut peak: f64 = 0.0;
    dynamics(samples, mean, input_peak, quality, |i, x| {
        peak = peak.max((x * fade(i)).abs())
    });
    let ceiling = quality.peak() as f64;
    let uncompressed_gain = if baseline_peak < 1e-8 {
        0.0
    } else {
        ceiling / baseline_peak
    };
    let gain = if peak < 1e-8 {
        0.0
    } else {
        (ceiling / peak).min(uncompressed_gain * 10.0_f64.powf(quality.dynamics().4 / 20.0))
    };
    let mut bytes = Vec::with_capacity(n);
    dynamics(samples, mean, input_peak, quality, |i, x| {
        bytes.push((x * fade(i) * gain).round().clamp(-ceiling, ceiling) as i8 as u8)
    });
    Ok(bytes)
}
struct Resampler {
    ring: [f32; 128],
    input: usize,
    output: Vec<f32>,
    rate: u32,
}
impl Resampler {
    fn new(rate: u32) -> Self {
        Self {
            ring: [0.0; 128],
            input: 0,
            output: Vec::new(),
            rate,
        }
    }
    fn push(&mut self, x: f32) {
        self.ring[self.input % 128] = if x.is_finite() {
            x.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.input += 1;
        while (self.output.len() as f64 * self.rate as f64 / 12000.0).floor() as usize + 32
            < self.input
        {
            self.sample()
        }
    }
    fn sample(&mut self) {
        let pos = self.output.len() as f64 * self.rate as f64 / 12000.0;
        let center = pos.floor() as i64;
        let cutoff = (12000.0 / self.rate as f64).min(1.0) * 0.9;
        let (mut sum, mut weight) = (0.0, 0.0);
        for i in center - 31..=center + 32 {
            let d = i as f64 - pos;
            if d.abs() >= 32.0 {
                continue;
            }
            let x = std::f64::consts::PI * d * cutoff;
            let sinc = if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
            let w = cutoff * sinc * (0.5 + 0.5 * (std::f64::consts::PI * d / 32.0).cos());
            weight += w;
            if i >= 0 && (i as usize) < self.input && (i as usize) >= self.input.saturating_sub(128)
            {
                sum += self.ring[i as usize % 128] as f64 * w
            }
        }
        self.output.push((sum / weight) as f32);
    }
    fn finish(mut self) -> Vec<f32> {
        let count = (self.input as f64 * 12000.0 / self.rate as f64).ceil() as usize;
        while self.output.len() < count {
            self.sample()
        }
        self.output
    }
}
pub fn decode_clip(
    data: impl Into<std::sync::Arc<[u8]>>,
    start: f64,
    end: f64,
) -> Result<Vec<u8>, String> {
    decode_clip_with_quality(data, start, end, Quality::Original)
}
pub fn decode_clip_with_quality(
    data: impl Into<std::sync::Arc<[u8]>>,
    start: f64,
    end: f64,
    quality: Quality,
) -> Result<Vec<u8>, String> {
    let data = data.into();
    validate_clip(start, end)?;
    if data.is_empty() || data.len() > 64 * 1024 * 1024 {
        return Err("MP3 为空或超过 64 MiB".into());
    }
    let stream = MediaSourceStream::new(Box::new(Cursor::new(data)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|_| "无法读取 MP3 文件")?;
    let mut format = probed.format;
    let track = format.default_track().ok_or("MP3 没有音轨")?;
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| "无法创建 MP3 解码器")?;
    let mut resampler: Option<Resampler> = None;
    let mut frames = 0usize;
    'packets: loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("MP3 数据读取失败：{e}")),
        };
        if packet.track_id() != id {
            continue;
        }
        let audio = decoder
            .decode(&packet)
            .map_err(|e| format!("MP3 解码失败：{e}"))?;
        let spec = *audio.spec();
        let channels = spec.channels.count();
        if channels == 0 || !(8000..=192000).contains(&spec.rate) {
            return Err("不支持的 MP3 采样率或声道".into());
        }
        if resampler.as_ref().is_some_and(|r| r.rate != spec.rate) {
            return Err("MP3 中途改变采样率".into());
        }
        let r = resampler.get_or_insert_with(|| Resampler::new(spec.rate));
        let mut samples = SampleBuffer::<f32>::new(audio.capacity() as u64, spec);
        samples.copy_interleaved_ref(audio);
        for frame in samples.samples().chunks_exact(channels) {
            let time = frames as f64 / spec.rate as f64;
            frames += 1;
            if time >= end {
                break 'packets;
            }
            if time >= start {
                r.push(frame.iter().sum::<f32>() / channels as f32)
            }
        }
    }
    let mono = resampler.ok_or("MP3 没有有效采样")?.finish();
    encode_with_quality(&mono, quality)
}
pub fn wav_data_uri(bytes: &[u8]) -> String {
    use base64::Engine;
    let mut b = Vec::with_capacity(44 + bytes.len());
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + bytes.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&12000u32.to_le_bytes());
    b.extend_from_slice(&12000u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&8u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    b.extend(bytes.iter().map(|x| x.wrapping_add(128)));
    "data:audio/wav;base64,".to_owned() + &base64::engine::general_purpose::STANDARD.encode(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn three_quality_presets_change_dynamics_without_changing_duration() {
        let samples: Vec<_> = (0..24000)
            .map(|i| {
                let amplitude = if (10000..11000).contains(&i) {
                    0.9
                } else {
                    0.06
                };
                (i as f32 * 0.23).sin() * amplitude
            })
            .collect();
        let original = encode_with_quality(&samples, Quality::Original).unwrap();
        let mild = encode_with_quality(&samples, Quality::Mild).unwrap();
        let strong = encode_with_quality(&samples, Quality::Strong).unwrap();
        let quiet_rms = |b: &[u8]| {
            (b[2000..8000]
                .iter()
                .map(|&x| (x as i8 as f64).powi(2))
                .sum::<f64>()
                / 6000.0)
                .sqrt()
        };
        assert!(quiet_rms(&mild) > quiet_rms(&original) * 1.2);
        assert!(quiet_rms(&strong) > quiet_rms(&mild) * 1.2);
        for (b, quality) in [
            (&original, Quality::Original),
            (&mild, Quality::Mild),
            (&strong, Quality::Strong),
        ] {
            assert_eq!(b.len(), samples.len());
            assert_eq!(b[0], 0);
            assert_eq!(*b.last().unwrap(), 0);
            assert!(
                b.iter()
                    .all(|&x| (x as i8 as i16).abs() <= quality.peak() as i16)
            );
            assert!(
                encode_with_quality(&vec![0.7; 12000], quality)
                    .unwrap()
                    .iter()
                    .all(|&x| x == 0)
            );
        }
        assert_eq!(encode(&samples).unwrap(), original);
    }
    #[test]
    fn dc_and_silence_are_zero() {
        for x in [0.0, 0.7] {
            assert!(encode(&vec![x; 12000]).unwrap().iter().all(|&v| v == 0))
        }
    }
    #[test]
    fn normalized_s8_is_bounded_and_faded() {
        let samples: Vec<_> = (0..12000)
            .map(|i| (i as f32 / 8.0).sin() * 0.2 + 0.3)
            .collect();
        let b = encode(&samples).unwrap();
        assert_eq!(b.len(), 12000);
        assert_eq!(b[0], 0);
        assert_eq!(*b.last().unwrap(), 0);
        assert_eq!(b.iter().map(|&x| (x as i8 as i16).abs()).max(), Some(64))
    }
    #[test]
    fn invalid_input_is_rejected() {
        assert!(encode(&[]).is_err());
        assert!(encode(&[f32::NAN, 1.0]).is_err());
        assert!(validate_clip(0.0, 301.0).is_err());
        assert!(validate_clip(3.0, 2.0).is_err())
    }
    #[test]
    fn invalid_mp3_is_rejected() {
        assert!(decode_clip(vec![0; 128], 0.0, 10.0).is_err())
    }
    #[test]
    fn real_stereo_mp3_is_decoded_and_clipped_at_12khz() {
        let bytes = decode_clip(
            include_bytes!("../tests/fixtures/tone.mp3").to_vec(),
            0.2,
            0.8,
        )
        .unwrap();
        assert!(
            (7199..=7201).contains(&bytes.len()),
            "{} samples",
            bytes.len()
        );
        assert!(bytes.iter().any(|&v| v != 0));
        assert!(bytes.iter().all(|&v| (v as i8 as i16).abs() <= 64));
    }
    #[test]
    fn sinc_preserves_duration() {
        let mut r = Resampler::new(44100);
        for i in 0..44100 {
            r.push((i as f32 / 10.0).sin())
        }
        assert_eq!(r.finish().len(), 12000)
    }
}
