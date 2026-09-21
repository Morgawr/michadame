use ffmpeg_next::Packet;
use std::{collections::VecDeque, sync::Arc};

pub struct Encoded {
    pub packet: Packet,
    pub video: bool,
    pub start: i64,
    pub end: i64,
    pub key: bool,
    pub bytes: usize,
}
impl Encoded {
    pub fn new(packet: Packet, video: bool) -> Self {
        let start = packet.pts().unwrap_or(0);
        let end = start + packet.duration().max(1);
        let key = video && packet.is_key();
        // Includes a conservative allowance for packet objects, references and side data.
        let bytes = packet.size() + packet.side_data().map(|s| s.data().len()).sum::<usize>() + 256;
        Self {
            packet,
            video,
            start,
            end,
            key,
            bytes,
        }
    }
}
pub struct History {
    packets: VecDeque<Arc<Encoded>>,
    pub bytes: usize,
    seconds: i64,
    video_end: i64,
    audio_end: i64,
    video_count: usize,
    audio_count: usize,
}
impl History {
    pub fn new(seconds: u32) -> Self {
        Self {
            packets: VecDeque::new(),
            bytes: 0,
            seconds: seconds as i64,
            video_end: 0,
            audio_end: 0,
            video_count: 0,
            audio_count: 0,
        }
    }
    pub fn push(&mut self, packet: Encoded, limit: usize) {
        if packet.video {
            self.video_end = self.video_end.max(packet.end);
            self.video_count += 1;
        } else {
            self.audio_end = self.audio_end.max(packet.end);
            self.audio_count += 1;
        }
        self.bytes += packet.bytes;
        self.packets.push_back(Arc::new(packet));
        self.trim(limit);
    }
    pub fn clear(&mut self) {
        self.packets.clear();
        self.bytes = 0;
        self.video_end = 0;
        self.audio_end = 0;
        self.video_count = 0;
        self.audio_count = 0;
    }
    pub fn trim(&mut self, limit: usize) {
        let newest = self.video_end.max(self.audio_end);
        // Drop entire dependency groups; an oversized group leaves no savable history.
        loop {
            let first_key = self.packets.iter().position(|p| p.key);
            let Some(first) = first_key else {
                self.clear();
                break;
            };
            for _ in 0..first {
                self.pop();
            }
            let too_old = newest - self.packets[0].start > self.seconds * 1_000_000;
            if self.bytes <= limit && !too_old {
                break;
            }
            self.pop();
        }
    }
    fn pop(&mut self) {
        if let Some(p) = self.packets.pop_front() {
            self.bytes -= p.bytes;
            if p.video {
                self.video_count -= 1;
            } else {
                self.audio_count -= 1;
            }
        }
    }
    pub fn end(&self) -> Option<i64> {
        if self.video_count == 0 || self.audio_count == 0 {
            return None;
        }
        Some(self.video_end.min(self.audio_end))
    }

    pub fn duration(&self) -> f64 {
        self.end()
            .zip(self.packets.front())
            .map(|(end, p)| (end - p.start).max(0) as f64 / 1e6)
            .unwrap_or(0.)
    }
    pub fn clip(&self, seconds: u32, requested_end: i64) -> Option<Vec<Arc<Encoded>>> {
        let end = self.end()?.min(requested_end);
        let target = end - seconds as i64 * 1_000_000;
        // Round start forward to a keyframe, never extend beyond the requested duration.
        let start = self
            .packets
            .iter()
            .find(|p| p.key && p.start >= target && p.start < end)?
            .start;
        let packets: Vec<_> = self
            .packets
            .iter()
            .filter(|p| p.start >= start && p.end <= end)
            .cloned()
            .collect();
        if !packets.iter().any(|p| !p.video) || !packets.iter().any(|p| p.key) {
            return None;
        }
        Some(packets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(t: i64, key: bool, video: bool, size: usize) -> Encoded {
        let mut p = Packet::copy(&vec![0; size]);
        p.set_pts(Some(t * 1_000_000));
        p.set_dts(p.pts());
        p.set_duration(1_000_000);
        if key {
            p.set_flags(ffmpeg_next::packet::Flags::KEY);
        }
        Encoded::new(p, video)
    }
    #[test]
    fn eviction_never_leaves_dependent_frames_at_start() {
        let mut h = History::new(3);
        for t in 0..8 {
            h.push(packet(t, t % 2 == 0, true, 10), 100000);
            h.push(packet(t, false, false, 10), 100000);
        }
        assert!(h.packets.front().unwrap().key);
        assert!(h.duration() <= 3.);
    }
    #[test]
    fn byte_limit_includes_overhead_and_discards_oversized_gop() {
        let mut h = History::new(600);
        h.push(packet(0, true, true, 1024), 100);
        assert_eq!(h.bytes, 0);
        h.push(packet(1, false, true, 10), 10000);
        assert_eq!(h.bytes, 0);
    }
    #[test]
    fn clip_is_decodable_and_does_not_exceed_requested_interval() {
        let mut h = History::new(600);
        for t in 0..10 {
            h.push(packet(t, t % 2 == 0, true, 10), 100000);
            h.push(packet(t, false, false, 10), 100000);
        }
        let clip = h.clip(5, 10_000_000).unwrap();
        assert!(clip[0].key);
        assert_eq!(clip[0].start, 6_000_000);
        assert!(clip.iter().all(|p| p.end <= 10_000_000));
    }
    #[test]
    fn audio_absence_cannot_produce_successful_clip() {
        let mut h = History::new(10);
        h.push(packet(0, true, true, 10), 10000);
        assert!(h.clip(10, 1_000_000).is_none());
    }
}
